//! Cranelift code generator for the Gradient compiler.
//!
//! This module translates Gradient IR into native machine code using the
//! Cranelift code generation framework. The pipeline is:
//!
//!   Gradient IR  -->  Cranelift IR  -->  Machine Code  -->  Object File (.o)
//!
//! The object file can then be linked with libc using any system linker
//! (typically `cc`) to produce a native executable.
//!
//! # Architecture
//!
//! - [`CraneliftCodegen`] holds the Cranelift module and shared compilation context.
//! - `emit_hello_world()` is the PoC entry point (hardcoded, bypasses our IR).
//! - `compile_module()` / `compile_function()` are the real entry points that
//!   translate Gradient IR into Cranelift IR.
//!
//! # How Cranelift works (brief overview)
//!
//! 1. Create an `ObjectModule` targeting the host (or cross-compile target).
//! 2. Declare functions and data objects in the module.
//! 3. For each function, use `FunctionBuilder` to emit Cranelift IR instructions.
//! 4. Define the function in the module (this triggers compilation to machine code).
//! 5. Call `module.finish()` to get the serialized object file bytes.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types as cl_types;
use cranelift_codegen::ir::{
    AbiParam, BlockArg, InstBuilder, MemFlags, StackSlotData, StackSlotKind, UserFuncName,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use target_lexicon::Triple;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::ir;

// ========================================================================
// Free helper functions (avoid borrow conflicts with self + builder)
// ========================================================================

/// Convert a Gradient IR type to the corresponding Cranelift type.
fn ir_type_to_cl(ty: &ir::Type) -> cranelift_codegen::ir::Type {
    match ty {
        ir::Type::I32 => cl_types::I32,
        ir::Type::I64 => cl_types::I64,
        ir::Type::Ptr => cl_types::I64,
        ir::Type::Bool => cl_types::I8,
        ir::Type::F64 => cl_types::F64,
        ir::Type::Void => cl_types::I8,
    }
}

/// Convert a Gradient IR comparison operator to a Cranelift `IntCC`.
fn cmpop_to_intcc(op: &ir::CmpOp) -> IntCC {
    match op {
        ir::CmpOp::Eq => IntCC::Equal,
        ir::CmpOp::Ne => IntCC::NotEqual,
        ir::CmpOp::Lt => IntCC::SignedLessThan,
        ir::CmpOp::Le => IntCC::SignedLessThanOrEqual,
        ir::CmpOp::Gt => IntCC::SignedGreaterThan,
        ir::CmpOp::Ge => IntCC::SignedGreaterThanOrEqual,
    }
}

/// Look up a Cranelift value for an IR value.
fn resolve_value(
    value_map: &HashMap<ir::Value, cranelift_codegen::ir::Value>,
    val: &ir::Value,
) -> Result<cranelift_codegen::ir::Value, String> {
    value_map
        .get(val)
        .copied()
        .ok_or_else(|| format!("IR Value({}) not found in value map", val.0))
}

/// Collect the Cranelift block arguments that should be passed when
/// jumping from `source_block` to `target_block`.
fn collect_jump_args(
    jump_args: &HashMap<ir::BlockRef, HashMap<ir::BlockRef, Vec<ir::Value>>>,
    target_block: &ir::BlockRef,
    source_block: &ir::BlockRef,
    value_map: &HashMap<ir::Value, cranelift_codegen::ir::Value>,
) -> Result<Vec<BlockArg>, String> {
    if let Some(target_map) = jump_args.get(target_block) {
        if let Some(ir_vals) = target_map.get(source_block) {
            let mut result = Vec::with_capacity(ir_vals.len());
            for ir_val in ir_vals {
                let cl_val = resolve_value(value_map, ir_val)?;
                result.push(BlockArg::Value(cl_val));
            }
            return Ok(result);
        }
    }
    Ok(Vec::new())
}

/// Get or create a null-terminated string data section in the module.
///
/// This is a free function so it can borrow `module`, `string_data`, and
/// `string_counter` independently of `self.ctx` (which is borrowed by the
/// `FunctionBuilder`).
fn get_or_create_string(
    module: &mut ObjectModule,
    string_data: &mut HashMap<String, cranelift_module::DataId>,
    string_counter: &mut u32,
    s: &str,
) -> Result<cranelift_module::DataId, String> {
    if let Some(&data_id) = string_data.get(s) {
        return Ok(data_id);
    }

    let name = format!(".str.{}", *string_counter);
    *string_counter += 1;

    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0); // Null-terminate for C compatibility.

    let mut data_desc = DataDescription::new();
    data_desc.define(bytes.into_boxed_slice());

    let data_id = module
        .declare_data(&name, Linkage::Local, true, false)
        .map_err(|e| format!("Failed to declare string data '{}': {}", name, e))?;

    module
        .define_data(data_id, &data_desc)
        .map_err(|e| format!("Failed to define string data '{}': {}", name, e))?;

    string_data.insert(s.to_string(), data_id);
    Ok(data_id)
}

// ========================================================================
// CraneliftCodegen
// ========================================================================

/// The Cranelift-based code generator for Gradient.
///
/// Holds the compilation state needed to translate one or more functions
/// and produce a native object file.
///
/// # Lifecycle
///
/// ```text
/// let mut cg = CraneliftCodegen::new()?;
/// cg.compile_module(&ir_module)?;    // Real pipeline
/// // or cg.emit_hello_world()?;      // PoC fallback
/// cg.finalize("output.o")?;
/// ```
pub struct CraneliftCodegen {
    /// The Cranelift object module — accumulates compiled functions and data,
    /// then serializes to an object file.
    module: ObjectModule,

    /// Shared compilation context — reused across function compilations to
    /// avoid repeated allocation.
    ctx: Context,

    /// Counter for generating unique names for string data sections.
    string_counter: u32,

    /// Map from string contents to their `DataId`, so identical strings
    /// share the same data section entry.
    string_data: HashMap<String, cranelift_module::DataId>,

    /// Map from function name to Cranelift `FuncId`. Populated during
    /// `compile_module()` when all functions (and externals like `puts`)
    /// are declared.
    declared_functions: HashMap<String, FuncId>,
}

impl CraneliftCodegen {
    /// Create a new code generator targeting the host platform.
    pub fn new() -> Result<Self, String> {
        let mut settings_builder = settings::builder();
        settings_builder
            .set("opt_level", "speed")
            .map_err(|e| format!("Failed to set opt_level: {}", e))?;
        settings_builder
            .set("is_pic", "true")
            .map_err(|e| format!("Failed to set is_pic: {}", e))?;

        let flags = settings::Flags::new(settings_builder);

        let triple = Triple::host();
        let isa = cranelift_codegen::isa::lookup(triple.clone())
            .map_err(|e| format!("Failed to look up ISA for {}: {}", triple, e))?
            .finish(flags)
            .map_err(|e| format!("Failed to finish ISA: {}", e))?;

        let obj_builder = ObjectBuilder::new(
            isa,
            "gradient_module",
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| format!("Failed to create ObjectBuilder: {}", e))?;

        let module = ObjectModule::new(obj_builder);
        let ctx = module.make_context();

        Ok(Self {
            module,
            ctx,
            string_counter: 0,
            string_data: HashMap::new(),
            declared_functions: HashMap::new(),
        })
    }

    // ====================================================================
    // Proof-of-concept (backward-compatible)
    // ====================================================================

    /// Proof-of-concept: emit a hardcoded "Hello from Gradient!" program.
    ///
    /// This bypasses the Gradient IR entirely and directly constructs Cranelift
    /// IR for a `main` function that calls `puts`.
    pub fn emit_hello_world(&mut self) -> Result<(), String> {
        // Create the string constant.
        let mut data_desc = DataDescription::new();
        let hello_str = b"Hello from Gradient!\0";
        data_desc.define(hello_str.to_vec().into_boxed_slice());

        let data_id = self
            .module
            .declare_data("hello_str", Linkage::Local, true, false)
            .map_err(|e| format!("Failed to declare data: {}", e))?;
        self.module
            .define_data(data_id, &data_desc)
            .map_err(|e| format!("Failed to define data: {}", e))?;

        // Declare puts.
        let pointer_type = self.module.target_config().pointer_type();

        let mut puts_sig = self.module.make_signature();
        puts_sig.params.push(AbiParam::new(pointer_type));
        puts_sig.returns.push(AbiParam::new(cl_types::I32));

        let puts_func_id = self
            .module
            .declare_function("puts", Linkage::Import, &puts_sig)
            .map_err(|e| format!("Failed to declare puts: {}", e))?;

        // Define main.
        let mut main_sig = self.module.make_signature();
        main_sig.returns.push(AbiParam::new(cl_types::I32));

        let main_func_id = self
            .module
            .declare_function("main", Linkage::Export, &main_sig)
            .map_err(|e| format!("Failed to declare main: {}", e))?;

        self.ctx.func.signature = main_sig;
        self.ctx.func.name = UserFuncName::user(0, 0);

        let mut fb_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut fb_ctx);

        let entry_block = builder.create_block();
        builder.seal_block(entry_block);
        builder.switch_to_block(entry_block);

        let data_gv = self
            .module
            .declare_data_in_func(data_id, builder.func);
        let str_ptr = builder.ins().global_value(pointer_type, data_gv);

        let puts_ref = self
            .module
            .declare_func_in_func(puts_func_id, builder.func);
        builder.ins().call(puts_ref, &[str_ptr]);

        let zero = builder.ins().iconst(cl_types::I32, 0);
        builder.ins().return_(&[zero]);

        builder.finalize();

        self.module
            .define_function(main_func_id, &mut self.ctx)
            .map_err(|e| format!("Failed to define main function: {}", e))?;
        self.module.clear_context(&mut self.ctx);

        Ok(())
    }

    // ====================================================================
    // Real compilation pipeline
    // ====================================================================

    /// Compile an entire IR module.
    ///
    /// This is the main entry point for the real compilation pipeline. It:
    /// 1. Declares external functions needed by the module (e.g. `puts` for
    ///    the `print` built-in).
    /// 2. Declares all user-defined functions in the module.
    /// 3. Compiles each function that has a body (non-extern).
    pub fn compile_module(&mut self, ir_module: &ir::Module) -> Result<(), String> {
        let pointer_type = self.module.target_config().pointer_type();

        // ----------------------------------------------------------------
        // Step 1: Declare external functions used by built-in operations.
        // ----------------------------------------------------------------
        if !self.declared_functions.contains_key("puts") {
            let mut puts_sig = self.module.make_signature();
            puts_sig.params.push(AbiParam::new(pointer_type));
            puts_sig.returns.push(AbiParam::new(cl_types::I32));

            let puts_id = self
                .module
                .declare_function("puts", Linkage::Import, &puts_sig)
                .map_err(|e| format!("Failed to declare puts: {}", e))?;
            self.declared_functions.insert("puts".to_string(), puts_id);
        }

        // ----------------------------------------------------------------
        // Step 2: Declare all functions in the module.
        // ----------------------------------------------------------------
        for func in &ir_module.functions {
            if self.declared_functions.contains_key(&func.name) {
                continue;
            }

            let mut sig = self.module.make_signature();
            for param_ty in &func.params {
                sig.params.push(AbiParam::new(ir_type_to_cl(param_ty)));
            }
            // Special case: C `main` must return i32 even if Gradient
            // declares it as returning void/unit.
            let is_main = func.name == "main";
            if is_main && func.return_type == ir::Type::Void {
                sig.returns.push(AbiParam::new(cl_types::I32));
            } else if func.return_type != ir::Type::Void {
                sig.returns
                    .push(AbiParam::new(ir_type_to_cl(&func.return_type)));
            }

            let linkage = if is_main {
                Linkage::Export
            } else {
                Linkage::Local
            };

            let func_id = self
                .module
                .declare_function(&func.name, linkage, &sig)
                .map_err(|e| format!("Failed to declare function '{}': {}", func.name, e))?;
            self.declared_functions
                .insert(func.name.clone(), func_id);
        }

        // ----------------------------------------------------------------
        // Step 3: Compile each function that has a body.
        // ----------------------------------------------------------------
        for func in &ir_module.functions {
            if func.blocks.is_empty() {
                continue; // Extern function — no body.
            }
            self.compile_function(func, ir_module)?;
        }

        Ok(())
    }

    /// Compile a single Gradient IR function to Cranelift IR and define it
    /// in the module.
    ///
    /// The `ir_module` parameter is needed to resolve `FuncRef` names
    /// via the module's `func_refs` map.
    pub fn compile_function(
        &mut self,
        func: &ir::Function,
        ir_module: &ir::Module,
    ) -> Result<(), String> {
        let pointer_type = self.module.target_config().pointer_type();

        // ----------------------------------------------------------------
        // Build the Cranelift signature.
        // ----------------------------------------------------------------
        let is_main = func.name == "main";
        let mut sig = self.module.make_signature();
        for param_ty in &func.params {
            sig.params.push(AbiParam::new(ir_type_to_cl(param_ty)));
        }
        // C `main` must return i32 even if Gradient declares void/unit.
        if is_main && func.return_type == ir::Type::Void {
            sig.returns.push(AbiParam::new(cl_types::I32));
        } else if func.return_type != ir::Type::Void {
            sig.returns
                .push(AbiParam::new(ir_type_to_cl(&func.return_type)));
        }

        let func_id = *self
            .declared_functions
            .get(&func.name)
            .ok_or_else(|| format!("Function '{}' was not declared", func.name))?;

        self.ctx.func.signature = sig;
        self.ctx.func.name = UserFuncName::user(0, func_id.as_u32());

        let mut fb_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut fb_ctx);

        // ----------------------------------------------------------------
        // Create all Cranelift blocks up front.
        // ----------------------------------------------------------------
        let mut block_map: HashMap<ir::BlockRef, cranelift_codegen::ir::Block> = HashMap::new();
        for ir_block in &func.blocks {
            let cl_block = builder.create_block();
            block_map.insert(ir_block.label, cl_block);
        }

        // The entry block gets function parameters appended.
        if let Some(first_block) = func.blocks.first() {
            let entry_cl_block = block_map[&first_block.label];
            builder.append_block_params_for_function_params(entry_cl_block);
        }

        // ----------------------------------------------------------------
        // First pass: identify Phi instructions and add block parameters.
        // ----------------------------------------------------------------
        struct PhiInfo {
            dst: ir::Value,
            #[allow(dead_code)]
            cl_type: cranelift_codegen::ir::Type,
            entries: Vec<(ir::BlockRef, ir::Value)>,
            target_block: ir::BlockRef,
            param_index: usize,
        }

        let mut phi_infos: Vec<PhiInfo> = Vec::new();
        let mut block_param_counts: HashMap<ir::BlockRef, usize> = HashMap::new();

        for ir_block in &func.blocks {
            for inst in &ir_block.instructions {
                if let ir::Instruction::Phi(dst, entries) = inst {
                    let cl_type = cl_types::I64;
                    let cl_block = block_map[&ir_block.label];
                    let param_idx = block_param_counts
                        .entry(ir_block.label)
                        .or_insert(0);
                    let current_idx = *param_idx;
                    *param_idx += 1;

                    builder.append_block_param(cl_block, cl_type);

                    phi_infos.push(PhiInfo {
                        dst: *dst,
                        cl_type,
                        entries: entries.clone(),
                        target_block: ir_block.label,
                        param_index: current_idx,
                    });
                }
            }
        }

        // Build jump_args: target_block -> source_block -> [IR Values].
        let mut jump_args: HashMap<ir::BlockRef, HashMap<ir::BlockRef, Vec<ir::Value>>> =
            HashMap::new();
        for phi in &phi_infos {
            for (src_block, src_val) in &phi.entries {
                jump_args
                    .entry(phi.target_block)
                    .or_default()
                    .entry(*src_block)
                    .or_default()
                    .push(*src_val);
            }
        }

        // ----------------------------------------------------------------
        // Second pass: translate instructions block by block.
        // ----------------------------------------------------------------
        let mut value_map: HashMap<ir::Value, cranelift_codegen::ir::Value> = HashMap::new();
        let mut func_ref_map: HashMap<ir::FuncRef, cranelift_codegen::ir::FuncRef> =
            HashMap::new();

        for (block_idx, ir_block) in func.blocks.iter().enumerate() {
            let cl_block = block_map[&ir_block.label];
            builder.switch_to_block(cl_block);

            // Map entry block function parameters to IR Values.
            if block_idx == 0 {
                let params = builder.block_params(cl_block).to_vec();
                for (i, _param_ty) in func.params.iter().enumerate() {
                    if i < params.len() {
                        value_map.insert(ir::Value(i as u32), params[i]);
                    }
                }
            }

            // Map phi-defined values to their block parameters.
            {
                let base_param_offset = if block_idx == 0 {
                    func.params.len()
                } else {
                    0
                };
                let params = builder.block_params(cl_block).to_vec();
                for phi in &phi_infos {
                    if phi.target_block == ir_block.label {
                        let param_idx = base_param_offset + phi.param_index;
                        if param_idx < params.len() {
                            value_map.insert(phi.dst, params[param_idx]);
                        }
                    }
                }
            }

            // Translate each instruction.
            for inst in &ir_block.instructions {
                match inst {
                    ir::Instruction::Const(dst, literal) => {
                        let cl_val = match literal {
                            ir::Literal::Int(n) => {
                                builder.ins().iconst(cl_types::I64, *n)
                            }
                            ir::Literal::Float(f) => {
                                builder.ins().f64const(*f)
                            }
                            ir::Literal::Bool(b) => {
                                builder.ins().iconst(cl_types::I8, *b as i64)
                            }
                            ir::Literal::Str(s) => {
                                // Use the free function to avoid borrow conflict.
                                let data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    s,
                                )?;
                                let data_gv = self
                                    .module
                                    .declare_data_in_func(data_id, builder.func);
                                builder.ins().global_value(pointer_type, data_gv)
                            }
                        };
                        value_map.insert(*dst, cl_val);
                    }

                    ir::Instruction::Add(dst, lhs, rhs) => {
                        let a = resolve_value(&value_map, lhs)?;
                        let b = resolve_value(&value_map, rhs)?;
                        let ty = builder.func.dfg.value_type(a);
                        let result = if ty == cl_types::F64 {
                            builder.ins().fadd(a, b)
                        } else {
                            builder.ins().iadd(a, b)
                        };
                        value_map.insert(*dst, result);
                    }

                    ir::Instruction::Sub(dst, lhs, rhs) => {
                        let a = resolve_value(&value_map, lhs)?;
                        let b = resolve_value(&value_map, rhs)?;
                        let ty = builder.func.dfg.value_type(a);
                        let result = if ty == cl_types::F64 {
                            builder.ins().fsub(a, b)
                        } else {
                            builder.ins().isub(a, b)
                        };
                        value_map.insert(*dst, result);
                    }

                    ir::Instruction::Mul(dst, lhs, rhs) => {
                        let a = resolve_value(&value_map, lhs)?;
                        let b = resolve_value(&value_map, rhs)?;
                        let ty = builder.func.dfg.value_type(a);
                        let result = if ty == cl_types::F64 {
                            builder.ins().fmul(a, b)
                        } else {
                            builder.ins().imul(a, b)
                        };
                        value_map.insert(*dst, result);
                    }

                    ir::Instruction::Div(dst, lhs, rhs) => {
                        let a = resolve_value(&value_map, lhs)?;
                        let b = resolve_value(&value_map, rhs)?;
                        let ty = builder.func.dfg.value_type(a);
                        let result = if ty == cl_types::F64 {
                            builder.ins().fdiv(a, b)
                        } else {
                            builder.ins().sdiv(a, b)
                        };
                        value_map.insert(*dst, result);
                    }

                    ir::Instruction::Cmp(dst, op, lhs, rhs) => {
                        let a = resolve_value(&value_map, lhs)?;
                        let b = resolve_value(&value_map, rhs)?;
                        let cc = cmpop_to_intcc(op);
                        let result = builder.ins().icmp(cc, a, b);
                        value_map.insert(*dst, result);
                    }

                    ir::Instruction::Call(dst, ir_func_ref, args) => {
                        let func_name = ir_module
                            .func_refs
                            .get(ir_func_ref)
                            .ok_or_else(|| {
                                format!(
                                    "Unknown FuncRef({}) in call instruction",
                                    ir_func_ref.0
                                )
                            })?;

                        // Route print/println to puts.
                        let target_name = match func_name.as_str() {
                            "print" | "println" => "puts",
                            other => other,
                        };

                        let cl_func_ref =
                            if let Some(&existing) = func_ref_map.get(ir_func_ref) {
                                existing
                            } else {
                                let target_func_id = self
                                    .declared_functions
                                    .get(target_name)
                                    .ok_or_else(|| {
                                        format!(
                                            "Function '{}' not declared in module",
                                            target_name
                                        )
                                    })?;
                                let fref = self.module.declare_func_in_func(
                                    *target_func_id,
                                    builder.func,
                                );
                                func_ref_map.insert(*ir_func_ref, fref);
                                fref
                            };

                        let cl_args: Result<Vec<_>, _> = args
                            .iter()
                            .map(|a| resolve_value(&value_map, a))
                            .collect();
                        let cl_args = cl_args?;

                        let call_inst = builder.ins().call(cl_func_ref, &cl_args);

                        let results = builder.inst_results(call_inst).to_vec();
                        if !results.is_empty() {
                            value_map.insert(*dst, results[0]);
                        } else {
                            // Void function: insert a dummy value.
                            let dummy = builder.ins().iconst(cl_types::I64, 0);
                            value_map.insert(*dst, dummy);
                        }
                    }

                    ir::Instruction::Ret(Some(val)) => {
                        if is_main && func.return_type == ir::Type::Void {
                            // Void main returns i32 0 to the C runtime;
                            // discard the Gradient expression value.
                            let zero = builder.ins().iconst(cl_types::I32, 0);
                            builder.ins().return_(&[zero]);
                        } else {
                            let cl_val = resolve_value(&value_map, val)?;
                            builder.ins().return_(&[cl_val]);
                        }
                    }

                    ir::Instruction::Ret(None) => {
                        if is_main && func.return_type == ir::Type::Void {
                            // Void main must return i32 0 for success.
                            let zero = builder.ins().iconst(cl_types::I32, 0);
                            builder.ins().return_(&[zero]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }

                    ir::Instruction::Branch(cond, then_block, else_block) => {
                        let cl_cond = resolve_value(&value_map, cond)?;
                        let then_cl = block_map[then_block];
                        let else_cl = block_map[else_block];

                        let then_args = collect_jump_args(
                            &jump_args,
                            then_block,
                            &ir_block.label,
                            &value_map,
                        )?;
                        let else_args = collect_jump_args(
                            &jump_args,
                            else_block,
                            &ir_block.label,
                            &value_map,
                        )?;

                        builder
                            .ins()
                            .brif(cl_cond, then_cl, &then_args, else_cl, &else_args);
                    }

                    ir::Instruction::Jump(target) => {
                        let target_cl = block_map[target];
                        let args = collect_jump_args(
                            &jump_args,
                            target,
                            &ir_block.label,
                            &value_map,
                        )?;
                        builder.ins().jump(target_cl, &args);
                    }

                    ir::Instruction::Phi(_dst, _entries) => {
                        // Handled via block parameters in the first pass
                        // and via jump/branch arguments. Nothing to emit.
                    }

                    ir::Instruction::Alloca(dst, ty) => {
                        let cl_ty = ir_type_to_cl(ty);
                        let size = cl_ty.bytes();
                        let align_shift = match size {
                            8 => 3,
                            4 => 2,
                            _ => 0,
                        };
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            size,
                            align_shift,
                        ));
                        let addr = builder.ins().stack_addr(pointer_type, slot, 0);
                        value_map.insert(*dst, addr);
                    }

                    ir::Instruction::Load(dst, addr) => {
                        let cl_addr = resolve_value(&value_map, addr)?;
                        // TODO: Track IR value types to use the correct load type.
                        let result = builder.ins().load(
                            cl_types::I64,
                            MemFlags::new(),
                            cl_addr,
                            0,
                        );
                        value_map.insert(*dst, result);
                    }

                    ir::Instruction::Store(val, addr) => {
                        let cl_val = resolve_value(&value_map, val)?;
                        let cl_addr = resolve_value(&value_map, addr)?;
                        builder.ins().store(MemFlags::new(), cl_val, cl_addr, 0);
                    }
                }
            }

            // Seal the block after all instructions are emitted.
            builder.seal_block(cl_block);
        }

        // Defensive: seal any remaining unsealed blocks.
        builder.seal_all_blocks();
        builder.finalize();

        // ----------------------------------------------------------------
        // Define the function in the module.
        // ----------------------------------------------------------------
        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| format!("Failed to define function '{}': {}", func.name, e))?;
        self.module.clear_context(&mut self.ctx);

        Ok(())
    }

    /// Write the compiled module to an object file on disk.
    ///
    /// After all functions and data have been compiled and added to the module,
    /// call this to serialize everything into a native object file (.o / .obj)
    /// that can be linked with `cc`.
    pub fn finalize(self, path: &str) -> Result<(), String> {
        let object_product = self.module.finish();
        let bytes = object_product
            .emit()
            .map_err(|e| format!("Failed to emit object: {}", e))?;

        fs::write(Path::new(path), &bytes)
            .map_err(|e| format!("Failed to write object file '{}': {}", path, e))?;

        println!("Wrote object file: {}", path);
        Ok(())
    }
}
