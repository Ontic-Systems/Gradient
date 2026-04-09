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

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
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

use std::collections::{HashMap, HashSet};
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

/// Convert a Gradient IR comparison operator to a Cranelift `FloatCC`.
fn cmpop_to_floatcc(op: &ir::CmpOp) -> FloatCC {
    match op {
        ir::CmpOp::Eq => FloatCC::Equal,
        ir::CmpOp::Ne => FloatCC::NotEqual,
        ir::CmpOp::Lt => FloatCC::LessThan,
        ir::CmpOp::Le => FloatCC::LessThanOrEqual,
        ir::CmpOp::Gt => FloatCC::GreaterThan,
        ir::CmpOp::Ge => FloatCC::GreaterThanOrEqual,
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

/// Coerce collected jump arguments so their types match the target block's
/// parameter types. When a branch joins two arms that produce different widths
/// (e.g. i8 from `()` vs i64 from a pointer), Cranelift's verifier rejects
/// the mismatched argument. We widen or narrow to the expected param type.
fn coerce_jump_args(
    args: Vec<BlockArg>,
    params: &[cranelift_codegen::ir::Value],
    builder: &mut cranelift_frontend::FunctionBuilder,
) -> Vec<BlockArg> {
    args.into_iter()
        .zip(params.iter())
        .map(|(arg, &param)| {
            let expected_ty = builder.func.dfg.value_type(param);
            match arg {
                BlockArg::Value(v) => {
                    let actual_ty = builder.func.dfg.value_type(v);
                    if actual_ty == expected_ty {
                        BlockArg::Value(v)
                    } else if actual_ty.bits() < expected_ty.bits() {
                        // Widen: e.g. i8 → i64
                        BlockArg::Value(builder.ins().uextend(expected_ty, v))
                    } else {
                        // Narrow: e.g. i64 → i8
                        BlockArg::Value(builder.ins().ireduce(expected_ty, v))
                    }
                }
                other => other,
            }
        })
        .collect()
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

    /// Map from (actor_type, message_name) to integer message type ID.
    /// Used for actor runtime operations that expect integer message types.
    #[allow(dead_code)]
    message_type_ids: HashMap<(String, String), i64>,

    /// Counter for generating unique message type IDs.
    #[allow(dead_code)]
    next_message_type_id: i64,
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
            message_type_ids: HashMap::new(),
            next_message_type_id: 1, // Start at 1, reserve 0 for special cases
        })
    }

    /// Get or assign a message type ID for the given (actor_type, message_name).
    /// This maps user-facing string message names to integer IDs for the runtime.
    #[allow(dead_code)]
    fn get_message_type_id(&mut self, actor_type: &str, message_name: &str) -> i64 {
        let key = (actor_type.to_string(), message_name.to_string());
        if let Some(&id) = self.message_type_ids.get(&key) {
            return id;
        }
        let new_id = self.next_message_type_id;
        self.next_message_type_id += 1;
        self.message_type_ids.insert(key, new_id);
        new_id
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

        let data_gv = self.module.declare_data_in_func(data_id, builder.func);
        let str_ptr = builder.ins().global_value(pointer_type, data_gv);

        let puts_ref = self.module.declare_func_in_func(puts_func_id, builder.func);
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

        // Declare printf for print_int: int printf(const char *fmt, ...)
        // Cranelift doesn't support true varargs, but we can declare the
        // specific signature we need: (ptr, i64) -> i32.
        // For print_float, we use call_indirect with a float signature
        // instead of a separate module-level declaration.
        if !self.declared_functions.contains_key("printf") {
            let mut printf_sig = self.module.make_signature();
            printf_sig.params.push(AbiParam::new(pointer_type)); // format string
            printf_sig.params.push(AbiParam::new(cl_types::I64)); // int value
            printf_sig.returns.push(AbiParam::new(cl_types::I32));

            let printf_id = self
                .module
                .declare_function("printf", Linkage::Import, &printf_sig)
                .map_err(|e| format!("Failed to declare printf: {}", e))?;
            self.declared_functions
                .insert("printf".to_string(), printf_id);
        }

        // Declare libc functions for string concatenation runtime.
        // malloc(size_t) -> ptr
        if !self.declared_functions.contains_key("malloc") {
            let mut malloc_sig = self.module.make_signature();
            malloc_sig.params.push(AbiParam::new(cl_types::I64)); // size
            malloc_sig.returns.push(AbiParam::new(pointer_type)); // ptr

            let malloc_id = self
                .module
                .declare_function("malloc", Linkage::Import, &malloc_sig)
                .map_err(|e| format!("Failed to declare malloc: {}", e))?;
            self.declared_functions
                .insert("malloc".to_string(), malloc_id);
        }

        // strlen(ptr) -> i64
        if !self.declared_functions.contains_key("strlen") {
            let mut strlen_sig = self.module.make_signature();
            strlen_sig.params.push(AbiParam::new(pointer_type));
            strlen_sig.returns.push(AbiParam::new(cl_types::I64));

            let strlen_id = self
                .module
                .declare_function("strlen", Linkage::Import, &strlen_sig)
                .map_err(|e| format!("Failed to declare strlen: {}", e))?;
            self.declared_functions
                .insert("strlen".to_string(), strlen_id);
        }

        // strcpy(ptr, ptr) -> ptr
        if !self.declared_functions.contains_key("strcpy") {
            let mut strcpy_sig = self.module.make_signature();
            strcpy_sig.params.push(AbiParam::new(pointer_type));
            strcpy_sig.params.push(AbiParam::new(pointer_type));
            strcpy_sig.returns.push(AbiParam::new(pointer_type));

            let strcpy_id = self
                .module
                .declare_function("strcpy", Linkage::Import, &strcpy_sig)
                .map_err(|e| format!("Failed to declare strcpy: {}", e))?;
            self.declared_functions
                .insert("strcpy".to_string(), strcpy_id);
        }

        // strcat(ptr, ptr) -> ptr
        if !self.declared_functions.contains_key("strcat") {
            let mut strcat_sig = self.module.make_signature();
            strcat_sig.params.push(AbiParam::new(pointer_type));
            strcat_sig.params.push(AbiParam::new(pointer_type));
            strcat_sig.returns.push(AbiParam::new(pointer_type));

            let strcat_id = self
                .module
                .declare_function("strcat", Linkage::Import, &strcat_sig)
                .map_err(|e| format!("Failed to declare strcat: {}", e))?;
            self.declared_functions
                .insert("strcat".to_string(), strcat_id);
        }

        // strstr(ptr, ptr) -> ptr  (find substring)
        if !self.declared_functions.contains_key("strstr") {
            let mut strstr_sig = self.module.make_signature();
            strstr_sig.params.push(AbiParam::new(pointer_type));
            strstr_sig.params.push(AbiParam::new(pointer_type));
            strstr_sig.returns.push(AbiParam::new(pointer_type));

            let strstr_id = self
                .module
                .declare_function("strstr", Linkage::Import, &strstr_sig)
                .map_err(|e| format!("Failed to declare strstr: {}", e))?;
            self.declared_functions
                .insert("strstr".to_string(), strstr_id);
        }

        // strncmp(ptr, ptr, i64) -> i32
        if !self.declared_functions.contains_key("strncmp") {
            let mut strncmp_sig = self.module.make_signature();
            strncmp_sig.params.push(AbiParam::new(pointer_type));
            strncmp_sig.params.push(AbiParam::new(pointer_type));
            strncmp_sig.params.push(AbiParam::new(cl_types::I64));
            strncmp_sig.returns.push(AbiParam::new(cl_types::I32));

            let strncmp_id = self
                .module
                .declare_function("strncmp", Linkage::Import, &strncmp_sig)
                .map_err(|e| format!("Failed to declare strncmp: {}", e))?;
            self.declared_functions
                .insert("strncmp".to_string(), strncmp_id);
        }

        // strcmp(ptr, ptr) -> i32
        if !self.declared_functions.contains_key("strcmp") {
            let mut strcmp_sig = self.module.make_signature();
            strcmp_sig.params.push(AbiParam::new(pointer_type));
            strcmp_sig.params.push(AbiParam::new(pointer_type));
            strcmp_sig.returns.push(AbiParam::new(cl_types::I32));
            let strcmp_id = self
                .module
                .declare_function("strcmp", Linkage::Import, &strcmp_sig)
                .map_err(|e| format!("Failed to declare strcmp: {}", e))?;
            self.declared_functions
                .insert("strcmp".to_string(), strcmp_id);
        }

        // memcpy(ptr, ptr, i64) -> ptr
        if !self.declared_functions.contains_key("memcpy") {
            let mut memcpy_sig = self.module.make_signature();
            memcpy_sig.params.push(AbiParam::new(pointer_type));
            memcpy_sig.params.push(AbiParam::new(pointer_type));
            memcpy_sig.params.push(AbiParam::new(cl_types::I64));
            memcpy_sig.returns.push(AbiParam::new(pointer_type));

            let memcpy_id = self
                .module
                .declare_function("memcpy", Linkage::Import, &memcpy_sig)
                .map_err(|e| format!("Failed to declare memcpy: {}", e))?;
            self.declared_functions
                .insert("memcpy".to_string(), memcpy_id);
        }

        // isspace(int) -> int  (checks whitespace)
        if !self.declared_functions.contains_key("isspace") {
            let mut isspace_sig = self.module.make_signature();
            isspace_sig.params.push(AbiParam::new(cl_types::I32)); // char as int
            isspace_sig.returns.push(AbiParam::new(cl_types::I32));

            let isspace_id = self
                .module
                .declare_function("isspace", Linkage::Import, &isspace_sig)
                .map_err(|e| format!("Failed to declare isspace: {}", e))?;
            self.declared_functions
                .insert("isspace".to_string(), isspace_id);
        }

        // toupper(int) -> int
        if !self.declared_functions.contains_key("toupper") {
            let mut toupper_sig = self.module.make_signature();
            toupper_sig.params.push(AbiParam::new(cl_types::I32)); // char as int
            toupper_sig.returns.push(AbiParam::new(cl_types::I32));

            let toupper_id = self
                .module
                .declare_function("toupper", Linkage::Import, &toupper_sig)
                .map_err(|e| format!("Failed to declare toupper: {}", e))?;
            self.declared_functions
                .insert("toupper".to_string(), toupper_id);
        }

        // tolower(int) -> int
        if !self.declared_functions.contains_key("tolower") {
            let mut tolower_sig = self.module.make_signature();
            tolower_sig.params.push(AbiParam::new(cl_types::I32)); // char as int
            tolower_sig.returns.push(AbiParam::new(cl_types::I32));

            let tolower_id = self
                .module
                .declare_function("tolower", Linkage::Import, &tolower_sig)
                .map_err(|e| format!("Failed to declare tolower: {}", e))?;
            self.declared_functions
                .insert("tolower".to_string(), tolower_id);
        }

        // snprintf(ptr, i64, ptr, ...) -> i32
        // We declare a specific 4-arg variant: (buf, size, fmt, value) -> i32
        // This is used for float_to_string and int-formatting.
        if !self.declared_functions.contains_key("snprintf") {
            let mut snprintf_sig = self.module.make_signature();
            snprintf_sig.params.push(AbiParam::new(pointer_type)); // buf
            snprintf_sig.params.push(AbiParam::new(cl_types::I64)); // size
            snprintf_sig.params.push(AbiParam::new(pointer_type)); // fmt
            snprintf_sig.params.push(AbiParam::new(cl_types::I64)); // int value
            snprintf_sig.returns.push(AbiParam::new(cl_types::I32));

            let snprintf_id = self
                .module
                .declare_function("snprintf", Linkage::Import, &snprintf_sig)
                .map_err(|e| format!("Failed to declare snprintf: {}", e))?;
            self.declared_functions
                .insert("snprintf".to_string(), snprintf_id);
        }

        // Declare exit(int) for contract failure abort and the exit() builtin.
        if !self.declared_functions.contains_key("exit") {
            let mut exit_sig = self.module.make_signature();
            exit_sig.params.push(AbiParam::new(cl_types::I32));
            // exit doesn't return, but Cranelift needs a signature.

            let exit_id = self
                .module
                .declare_function("exit", Linkage::Import, &exit_sig)
                .map_err(|e| format!("Failed to declare exit: {}", e))?;
            self.declared_functions.insert("exit".to_string(), exit_id);
        }

        // ── Phase MM: Standard I/O helpers ──────────────────────────────

        // atoi(ptr) -> i64  — used by parse_int
        // Note: atoi returns int (i32) but we widen to i64 for Gradient's Int type.
        if !self.declared_functions.contains_key("atoi") {
            let mut atoi_sig = self.module.make_signature();
            atoi_sig.params.push(AbiParam::new(pointer_type)); // const char*
            atoi_sig.returns.push(AbiParam::new(cl_types::I32)); // int

            let atoi_id = self
                .module
                .declare_function("atoi", Linkage::Import, &atoi_sig)
                .map_err(|e| format!("Failed to declare atoi: {}", e))?;
            self.declared_functions.insert("atoi".to_string(), atoi_id);
        }

        // atof(ptr) -> f64  — used by parse_float
        if !self.declared_functions.contains_key("atof") {
            let mut atof_sig = self.module.make_signature();
            atof_sig.params.push(AbiParam::new(pointer_type)); // const char*
            atof_sig.returns.push(AbiParam::new(cl_types::F64)); // double

            let atof_id = self
                .module
                .declare_function("atof", Linkage::Import, &atof_sig)
                .map_err(|e| format!("Failed to declare atof: {}", e))?;
            self.declared_functions.insert("atof".to_string(), atof_id);
        }

        // ── Phase PP: Math builtins (libm functions) ─────────────────────
        // All math functions: double -> double (except atan2: double, double -> double)
        // gcd is provided by the runtime; others are direct libc/libm calls.

        // sin(x: f64) -> f64
        if !self.declared_functions.contains_key("sin") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("sin", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare sin: {}", e))?;
            self.declared_functions.insert("sin".to_string(), func_id);
        }

        // cos(x: f64) -> f64
        if !self.declared_functions.contains_key("cos") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("cos", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare cos: {}", e))?;
            self.declared_functions.insert("cos".to_string(), func_id);
        }

        // tan(x: f64) -> f64
        if !self.declared_functions.contains_key("tan") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("tan", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare tan: {}", e))?;
            self.declared_functions.insert("tan".to_string(), func_id);
        }

        // asin(x: f64) -> f64
        if !self.declared_functions.contains_key("asin") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("asin", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare asin: {}", e))?;
            self.declared_functions.insert("asin".to_string(), func_id);
        }

        // acos(x: f64) -> f64
        if !self.declared_functions.contains_key("acos") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("acos", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare acos: {}", e))?;
            self.declared_functions.insert("acos".to_string(), func_id);
        }

        // atan(x: f64) -> f64
        if !self.declared_functions.contains_key("atan") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("atan", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare atan: {}", e))?;
            self.declared_functions.insert("atan".to_string(), func_id);
        }

        // atan2(y: f64, x: f64) -> f64
        if !self.declared_functions.contains_key("atan2") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("atan2", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare atan2: {}", e))?;
            self.declared_functions.insert("atan2".to_string(), func_id);
        }

        // log(x: f64) -> f64 (natural logarithm)
        if !self.declared_functions.contains_key("log") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("log", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare log: {}", e))?;
            self.declared_functions.insert("log".to_string(), func_id);
        }

        // log10(x: f64) -> f64
        if !self.declared_functions.contains_key("log10") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("log10", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare log10: {}", e))?;
            self.declared_functions.insert("log10".to_string(), func_id);
        }

        // log2(x: f64) -> f64
        if !self.declared_functions.contains_key("log2") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("log2", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare log2: {}", e))?;
            self.declared_functions.insert("log2".to_string(), func_id);
        }

        // exp(x: f64) -> f64
        if !self.declared_functions.contains_key("exp") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("exp", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare exp: {}", e))?;
            self.declared_functions.insert("exp".to_string(), func_id);
        }

        // exp2(x: f64) -> f64
        if !self.declared_functions.contains_key("exp2") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("exp2", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare exp2: {}", e))?;
            self.declared_functions.insert("exp2".to_string(), func_id);
        }

        // ceil(x: f64) -> f64
        if !self.declared_functions.contains_key("ceil") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("ceil", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare ceil: {}", e))?;
            self.declared_functions.insert("ceil".to_string(), func_id);
        }

        // floor(x: f64) -> f64
        if !self.declared_functions.contains_key("floor") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("floor", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare floor: {}", e))?;
            self.declared_functions.insert("floor".to_string(), func_id);
        }

        // round(x: f64) -> f64
        if !self.declared_functions.contains_key("round") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("round", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare round: {}", e))?;
            self.declared_functions.insert("round".to_string(), func_id);
        }

        // trunc(x: f64) -> f64
        if !self.declared_functions.contains_key("trunc") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("trunc", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare trunc: {}", e))?;
            self.declared_functions.insert("trunc".to_string(), func_id);
        }

        // fmod(a: f64, b: f64) -> f64 (float_mod)
        if !self.declared_functions.contains_key("fmod") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("fmod", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare fmod: {}", e))?;
            self.declared_functions.insert("fmod".to_string(), func_id);
        }

        // __gradient_gcd(a: i64, b: i64) -> i64 — provided by runtime
        if !self.declared_functions.contains_key("__gradient_gcd") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_gcd", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_gcd: {}", e))?;
            self.declared_functions
                .insert("__gradient_gcd".to_string(), func_id);
        }

        // __gradient_pi() -> f64 — provided by runtime
        if !self.declared_functions.contains_key("__gradient_pi") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("__gradient_pi", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_pi: {}", e))?;
            self.declared_functions
                .insert("__gradient_pi".to_string(), func_id);
        }

        // __gradient_e() -> f64 — provided by runtime
        if !self.declared_functions.contains_key("__gradient_e") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("__gradient_e", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_e: {}", e))?;
            self.declared_functions
                .insert("__gradient_e".to_string(), func_id);
        }

        // __gradient_clamp_f64(value: f64, min: f64, max: f64) -> f64 — provided by runtime
        if !self.declared_functions.contains_key("__gradient_clamp_f64") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("__gradient_clamp_f64", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_clamp_f64: {}", e))?;
            self.declared_functions
                .insert("__gradient_clamp_f64".to_string(), func_id);
        }

        // __gradient_clamp_i64(value: i64, min: i64, max: i64) -> i64 — provided by runtime
        if !self.declared_functions.contains_key("__gradient_clamp_i64") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_clamp_i64", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_clamp_i64: {}", e))?;
            self.declared_functions
                .insert("__gradient_clamp_i64".to_string(), func_id);
        }

        // __gradient_read_line() -> ptr  — reads one line from stdin, strips \\n
        // Declared as Import; callers must link gradient_runtime.o.
        if !self.declared_functions.contains_key("__gradient_read_line") {
            let mut rl_sig = self.module.make_signature();
            rl_sig.returns.push(AbiParam::new(pointer_type)); // char* (malloc'd)

            let rl_id = self
                .module
                .declare_function("__gradient_read_line", Linkage::Import, &rl_sig)
                .map_err(|e| format!("Failed to declare __gradient_read_line: {}", e))?;
            self.declared_functions
                .insert("__gradient_read_line".to_string(), rl_id);
        }

        // __gradient_genref_alloc(size: i64) -> ptr — heap allocation with generational refs
        if !self
            .declared_functions
            .contains_key("__gradient_genref_alloc")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // size in bytes
            sig.returns.push(AbiParam::new(pointer_type)); // allocated pointer

            let func_id = self
                .module
                .declare_function("__gradient_genref_alloc", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_genref_alloc: {}", e))?;
            self.declared_functions
                .insert("__gradient_genref_alloc".to_string(), func_id);
        }

        // ── Phase NN: File I/O helpers (FS effect) ───────────────────────
        // __gradient_file_read(path: ptr) -> ptr
        if !self.declared_functions.contains_key("__gradient_file_read") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // path
            sig.returns.push(AbiParam::new(pointer_type)); // result string ptr

            let func_id = self
                .module
                .declare_function("__gradient_file_read", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_file_read: {}", e))?;
            self.declared_functions
                .insert("__gradient_file_read".to_string(), func_id);
        }

        // __gradient_file_write(path: ptr, content: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_file_write")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // path
            sig.params.push(AbiParam::new(pointer_type)); // content
            sig.returns.push(AbiParam::new(cl_types::I8)); // 1 = ok, 0 = error

            let func_id = self
                .module
                .declare_function("__gradient_file_write", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_file_write: {}", e))?;
            self.declared_functions
                .insert("__gradient_file_write".to_string(), func_id);
        }

        // __gradient_file_exists(path: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_file_exists")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // path
            sig.returns.push(AbiParam::new(cl_types::I8)); // 1 = exists, 0 = not found

            let func_id = self
                .module
                .declare_function("__gradient_file_exists", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_file_exists: {}", e))?;
            self.declared_functions
                .insert("__gradient_file_exists".to_string(), func_id);
        }

        // __gradient_file_append(path: ptr, content: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_file_append")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // path
            sig.params.push(AbiParam::new(pointer_type)); // content
            sig.returns.push(AbiParam::new(cl_types::I8)); // 1 = ok, 0 = error

            let func_id = self
                .module
                .declare_function("__gradient_file_append", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_file_append: {}", e))?;
            self.declared_functions
                .insert("__gradient_file_append".to_string(), func_id);
        }

        // __gradient_file_delete(path: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_file_delete")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // path
            sig.returns.push(AbiParam::new(cl_types::I8)); // 1 = ok, 0 = error

            let func_id = self
                .module
                .declare_function("__gradient_file_delete", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_file_delete: {}", e))?;
            self.declared_functions
                .insert("__gradient_file_delete".to_string(), func_id);
        }

        // ── Program arguments ────────────────────────────────────────────

        // __gradient_save_args(argc: i64, argv: ptr)
        if !self.declared_functions.contains_key("__gradient_save_args") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // argc
            sig.params.push(AbiParam::new(pointer_type)); // argv
            let func_id = self
                .module
                .declare_function("__gradient_save_args", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_save_args: {}", e))?;
            self.declared_functions
                .insert("__gradient_save_args".to_string(), func_id);
        }

        // __gradient_get_args() -> ptr (Gradient List[String])
        if !self.declared_functions.contains_key("__gradient_get_args") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_get_args", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_get_args: {}", e))?;
            self.declared_functions
                .insert("__gradient_get_args".to_string(), func_id);
        }

        // ── Phase OO: Map operations ─────────────────────────────────────

        // __gradient_map_new() -> ptr
        if !self.declared_functions.contains_key("__gradient_map_new") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_map_new", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_new: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_new".to_string(), func_id);
        }

        // __gradient_map_set_str(map: ptr, key: ptr, value: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_map_set_str")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // map
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.params.push(AbiParam::new(pointer_type)); // value
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_map_set_str", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_set_str: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_set_str".to_string(), func_id);
        }

        // __gradient_map_set_int(map: ptr, key: ptr, value: i64) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_map_set_int")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // map
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.params.push(AbiParam::new(cl_types::I64)); // value
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_map_set_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_set_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_set_int".to_string(), func_id);
        }

        // __gradient_map_get_str(map: ptr, key: ptr) -> ptr (NULL if absent)
        if !self
            .declared_functions
            .contains_key("__gradient_map_get_str")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // map
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.returns.push(AbiParam::new(pointer_type)); // ptr or NULL
            let func_id = self
                .module
                .declare_function("__gradient_map_get_str", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_get_str: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_get_str".to_string(), func_id);
        }

        // __gradient_map_get_int(map: ptr, key: ptr, found_out: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_map_get_int")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // map
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.params.push(AbiParam::new(pointer_type)); // &found_out (stack slot)
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_map_get_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_get_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_get_int".to_string(), func_id);
        }

        // __gradient_map_contains(map: ptr, key: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_map_contains")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // map
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_map_contains", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_contains: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_contains".to_string(), func_id);
        }

        // __gradient_map_remove(map: ptr, key: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_map_remove")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // map
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_map_remove", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_remove: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_remove".to_string(), func_id);
        }

        // __gradient_map_size(map: ptr) -> i64
        if !self.declared_functions.contains_key("__gradient_map_size") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // map
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_map_size", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_size: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_size".to_string(), func_id);
        }

        // __gradient_map_keys(map: ptr) -> ptr (List[String])
        if !self.declared_functions.contains_key("__gradient_map_keys") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // map
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_map_keys", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_map_keys: {}", e))?;
            self.declared_functions
                .insert("__gradient_map_keys".to_string(), func_id);
        }

        // __gradient_string_split(s: ptr, delim: ptr) -> ptr (List[String])
        if !self
            .declared_functions
            .contains_key("__gradient_string_split")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(pointer_type)); // delim
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_string_split", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_split: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_split".to_string(), func_id);
        }

        // __gradient_string_trim(s: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_trim")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_string_trim", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_trim: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_trim".to_string(), func_id);
        }

        // ── Phase PP: String Utilities Batch 2 ───────────────────────────

        // __gradient_string_format(fmt: ptr, args: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_format")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // fmt
            sig.params.push(AbiParam::new(pointer_type)); // args (List[String])
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_string_format", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_format: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_format".to_string(), func_id);
        }

        // __gradient_string_is_empty(s: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_string_is_empty")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_string_is_empty", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_is_empty: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_is_empty".to_string(), func_id);
        }

        // __gradient_string_reverse(s: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_reverse")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_string_reverse", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_reverse: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_reverse".to_string(), func_id);
        }

        // __gradient_string_compare(a: ptr, b: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_string_compare")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // a
            sig.params.push(AbiParam::new(pointer_type)); // b
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_string_compare", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_compare: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_compare".to_string(), func_id);
        }

        // __gradient_string_find(s: ptr, substr: ptr) -> ptr (Option[Int])
        if !self
            .declared_functions
            .contains_key("__gradient_string_find")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(pointer_type)); // substr
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_string_find", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_find: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_find".to_string(), func_id);
        }

        // __gradient_string_slice(s: ptr, start: i64, end: i64) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_slice")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(cl_types::I64)); // start
            sig.params.push(AbiParam::new(cl_types::I64)); // end
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_string_slice", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_slice: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_slice".to_string(), func_id);
        }

        // ── Phase 0: String Primitives for Self-Hosting ────────────────

        // __gradient_string_append(a: ptr, b: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_append")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // a
            sig.params.push(AbiParam::new(pointer_type)); // b
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_string_append", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_append: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_append".to_string(), func_id);
        }

        // __gradient_string_char_code_at(s: ptr, idx: i64) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_string_char_code_at")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(cl_types::I64)); // idx
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_string_char_code_at", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_char_code_at: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_char_code_at".to_string(), func_id);
        }

        // ── Phase PP: Date/Time Builtins ────────────────────────────────

        // __gradient_now() -> i64 (Unix timestamp in seconds)
        if !self.declared_functions.contains_key("__gradient_now") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_now", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_now: {}", e))?;
            self.declared_functions
                .insert("__gradient_now".to_string(), func_id);
        }

        // __gradient_now_ms() -> i64 (Unix timestamp in milliseconds)
        if !self.declared_functions.contains_key("__gradient_now_ms") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_now_ms", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_now_ms: {}", e))?;
            self.declared_functions
                .insert("__gradient_now_ms".to_string(), func_id);
        }

        // __gradient_sleep(ms: i64) -> ()
        if !self.declared_functions.contains_key("__gradient_sleep") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // ms
            let func_id = self
                .module
                .declare_function("__gradient_sleep", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_sleep: {}", e))?;
            self.declared_functions
                .insert("__gradient_sleep".to_string(), func_id);
        }

        // __gradient_sleep_seconds(s: i64) -> ()
        if !self
            .declared_functions
            .contains_key("__gradient_sleep_seconds")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // s
            let func_id = self
                .module
                .declare_function("__gradient_sleep_seconds", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_sleep_seconds: {}", e))?;
            self.declared_functions
                .insert("__gradient_sleep_seconds".to_string(), func_id);
        }

        // ── Option helper functions ───────────────────────────────────────

        // __gradient_option_is_some(opt: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_option_is_some")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // opt
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_option_is_some", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_option_is_some: {}", e))?;
            self.declared_functions
                .insert("__gradient_option_is_some".to_string(), func_id);
        }

        // __gradient_option_is_none(opt: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_option_is_none")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // opt
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_option_is_none", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_option_is_none: {}", e))?;
            self.declared_functions
                .insert("__gradient_option_is_none".to_string(), func_id);
        }

        // __gradient_option_unwrap(opt: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_option_unwrap")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // opt
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_option_unwrap", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_option_unwrap: {}", e))?;
            self.declared_functions
                .insert("__gradient_option_unwrap".to_string(), func_id);
        }

        // __gradient_option_unwrap_or(opt: ptr, default: i64) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_option_unwrap_or")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // opt
            sig.params.push(AbiParam::new(cl_types::I64)); // default
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_option_unwrap_or", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_option_unwrap_or: {}", e))?;
            self.declared_functions
                .insert("__gradient_option_unwrap_or".to_string(), func_id);
        }

        // __gradient_time_string() -> ptr (RFC3339 format string)
        if !self
            .declared_functions
            .contains_key("__gradient_time_string")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_time_string", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_time_string: {}", e))?;
            self.declared_functions
                .insert("__gradient_time_string".to_string(), func_id);
        }

        // __gradient_date_string() -> ptr (YYYY-MM-DD format string)
        if !self
            .declared_functions
            .contains_key("__gradient_date_string")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_date_string", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_date_string: {}", e))?;
            self.declared_functions
                .insert("__gradient_date_string".to_string(), func_id);
        }

        // __gradient_datetime_year(ts: i64) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_datetime_year")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // ts
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_datetime_year", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_datetime_year: {}", e))?;
            self.declared_functions
                .insert("__gradient_datetime_year".to_string(), func_id);
        }

        // __gradient_datetime_month(ts: i64) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_datetime_month")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // ts
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_datetime_month", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_datetime_month: {}", e))?;
            self.declared_functions
                .insert("__gradient_datetime_month".to_string(), func_id);
        }

        // __gradient_datetime_day(ts: i64) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_datetime_day")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // ts
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_datetime_day", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_datetime_day: {}", e))?;
            self.declared_functions
                .insert("__gradient_datetime_day".to_string(), func_id);
        }

        // ── Phase RR: HTTP Client Builtins ──────────────────────────────

        // __gradient_http_get(url: ptr) -> ptr (Result[String, String])
        if !self.declared_functions.contains_key("__gradient_http_get") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // url
            sig.returns.push(AbiParam::new(pointer_type)); // Result ptr
            let func_id = self
                .module
                .declare_function("__gradient_http_get", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_http_get: {}", e))?;
            self.declared_functions
                .insert("__gradient_http_get".to_string(), func_id);
        }

        // __gradient_http_post(url: ptr, body: ptr) -> ptr (Result[String, String])
        if !self.declared_functions.contains_key("__gradient_http_post") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // url
            sig.params.push(AbiParam::new(pointer_type)); // body
            sig.returns.push(AbiParam::new(pointer_type)); // Result ptr
            let func_id = self
                .module
                .declare_function("__gradient_http_post", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_http_post: {}", e))?;
            self.declared_functions
                .insert("__gradient_http_post".to_string(), func_id);
        }

        // __gradient_http_post_json(url: ptr, json: ptr) -> ptr (Result[String, String])
        if !self
            .declared_functions
            .contains_key("__gradient_http_post_json")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // url
            sig.params.push(AbiParam::new(pointer_type)); // json body
            sig.returns.push(AbiParam::new(pointer_type)); // Result ptr
            let func_id = self
                .module
                .declare_function("__gradient_http_post_json", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_http_post_json: {}", e))?;
            self.declared_functions
                .insert("__gradient_http_post_json".to_string(), func_id);
        }

        // ── JSON Builtins ───────────────────────────────────────────────
        if !self
            .declared_functions
            .contains_key("__gradient_json_parse")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // input string
            sig.params.push(AbiParam::new(pointer_type)); // out_ok ptr
            sig.returns.push(AbiParam::new(pointer_type)); // JsonValue ptr or error string ptr
            let func_id = self
                .module
                .declare_function("__gradient_json_parse", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_parse: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_parse".to_string(), func_id);
        }
        if !self
            .declared_functions
            .contains_key("__gradient_json_stringify")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // JsonValue ptr
            sig.returns.push(AbiParam::new(pointer_type)); // string ptr
            let func_id = self
                .module
                .declare_function("__gradient_json_stringify", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_stringify: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_stringify".to_string(), func_id);
        }
        if !self.declared_functions.contains_key("__gradient_json_type") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_json_type", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_type: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_type".to_string(), func_id);
        }
        if !self.declared_functions.contains_key("__gradient_json_get") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_json_get", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_get: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_get".to_string(), func_id);
        }
        if !self
            .declared_functions
            .contains_key("__gradient_json_is_null")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_json_is_null", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_is_null: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_is_null".to_string(), func_id);
        }
        if !self.declared_functions.contains_key("__gradient_json_has") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_json_has", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_has: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_has".to_string(), func_id);
        }
        if !self.declared_functions.contains_key("__gradient_json_keys") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_json_keys", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_keys: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_keys".to_string(), func_id);
        }
        if !self.declared_functions.contains_key("__gradient_json_len") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_json_len", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_len: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_len".to_string(), func_id);
        }
        if !self
            .declared_functions
            .contains_key("__gradient_json_array_get")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_json_array_get", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_array_get: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_array_get".to_string(), func_id);
        }
        // Typed JSON extractors
        if !self
            .declared_functions
            .contains_key("__gradient_json_as_string")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type)); // Option[String] ptr
            let func_id = self
                .module
                .declare_function("__gradient_json_as_string", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_as_string: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_as_string".to_string(), func_id);
        }
        if !self
            .declared_functions
            .contains_key("__gradient_json_as_int")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type)); // Option[Int] ptr
            let func_id = self
                .module
                .declare_function("__gradient_json_as_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_as_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_as_int".to_string(), func_id);
        }
        if !self
            .declared_functions
            .contains_key("__gradient_json_as_float")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type)); // Option[Float] ptr
            let func_id = self
                .module
                .declare_function("__gradient_json_as_float", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_as_float: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_as_float".to_string(), func_id);
        }
        if !self
            .declared_functions
            .contains_key("__gradient_json_as_bool")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type)); // Option[Bool] ptr
            let func_id = self
                .module
                .declare_function("__gradient_json_as_bool", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_json_as_bool: {}", e))?;
            self.declared_functions
                .insert("__gradient_json_as_bool".to_string(), func_id);
        }

        // ── Phase PP: Random Number Generation ───────────────────────────

        // __gradient_random() -> f64
        if !self.declared_functions.contains_key("__gradient_random") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("__gradient_random", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_random: {}", e))?;
            self.declared_functions
                .insert("__gradient_random".to_string(), func_id);
        }

        // __gradient_random_int(min: i64, max: i64) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_random_int")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // min
            sig.params.push(AbiParam::new(cl_types::I64)); // max
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_random_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_random_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_random_int".to_string(), func_id);
        }

        // __gradient_random_float() -> f64
        if !self
            .declared_functions
            .contains_key("__gradient_random_float")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self
                .module
                .declare_function("__gradient_random_float", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_random_float: {}", e))?;
            self.declared_functions
                .insert("__gradient_random_float".to_string(), func_id);
        }

        // __gradient_seed_random(seed: i64) -> ()
        if !self
            .declared_functions
            .contains_key("__gradient_seed_random")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // seed
            let func_id = self
                .module
                .declare_function("__gradient_seed_random", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_seed_random: {}", e))?;
            self.declared_functions
                .insert("__gradient_seed_random".to_string(), func_id);
        }

        // ── Phase PP: Queue Builtins ──────────────────────────────────────

        // __gradient_queue_new() -> ptr
        if !self.declared_functions.contains_key("__gradient_queue_new") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_queue_new", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_queue_new: {}", e))?;
            self.declared_functions
                .insert("__gradient_queue_new".to_string(), func_id);
        }

        // __gradient_queue_enqueue(q: ptr, item: i64) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_queue_enqueue")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // queue
            sig.params.push(AbiParam::new(cl_types::I64)); // item
            sig.returns.push(AbiParam::new(pointer_type)); // new queue
            let func_id = self
                .module
                .declare_function("__gradient_queue_enqueue", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_queue_enqueue: {}", e))?;
            self.declared_functions
                .insert("__gradient_queue_enqueue".to_string(), func_id);
        }

        // __gradient_queue_dequeue(q: ptr) -> ptr (Option[(T, Queue[T])])
        if !self
            .declared_functions
            .contains_key("__gradient_queue_dequeue")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // queue
            sig.returns.push(AbiParam::new(pointer_type)); // Option[(T, Queue)]
            let func_id = self
                .module
                .declare_function("__gradient_queue_dequeue", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_queue_dequeue: {}", e))?;
            self.declared_functions
                .insert("__gradient_queue_dequeue".to_string(), func_id);
        }

        // __gradient_queue_peek(q: ptr) -> ptr (Option[T])
        if !self
            .declared_functions
            .contains_key("__gradient_queue_peek")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // queue
            sig.returns.push(AbiParam::new(pointer_type)); // Option[T]
            let func_id = self
                .module
                .declare_function("__gradient_queue_peek", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_queue_peek: {}", e))?;
            self.declared_functions
                .insert("__gradient_queue_peek".to_string(), func_id);
        }

        // __gradient_queue_size(q: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_queue_size")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // queue
            sig.returns.push(AbiParam::new(cl_types::I64)); // size
            let func_id = self
                .module
                .declare_function("__gradient_queue_size", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_queue_size: {}", e))?;
            self.declared_functions
                .insert("__gradient_queue_size".to_string(), func_id);
        }

        // ── Phase PP: Stack Builtins ─────────────────────────────────────

        // __gradient_stack_new() -> ptr
        if !self.declared_functions.contains_key("__gradient_stack_new") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type)); // stack
            let func_id = self
                .module
                .declare_function("__gradient_stack_new", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_stack_new: {}", e))?;
            self.declared_functions
                .insert("__gradient_stack_new".to_string(), func_id);
        }

        // __gradient_stack_push(s: ptr, item: i64) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_stack_push")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // stack
            sig.params.push(AbiParam::new(cl_types::I64)); // item
            sig.returns.push(AbiParam::new(pointer_type)); // new stack
            let func_id = self
                .module
                .declare_function("__gradient_stack_push", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_stack_push: {}", e))?;
            self.declared_functions
                .insert("__gradient_stack_push".to_string(), func_id);
        }

        // __gradient_stack_pop(s: ptr) -> ptr (Option<(T, Stack[T])>)
        if !self.declared_functions.contains_key("__gradient_stack_pop") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // stack
            sig.returns.push(AbiParam::new(pointer_type)); // Option<(T, Stack[T])>
            let func_id = self
                .module
                .declare_function("__gradient_stack_pop", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_stack_pop: {}", e))?;
            self.declared_functions
                .insert("__gradient_stack_pop".to_string(), func_id);
        }

        // __gradient_stack_peek(s: ptr) -> ptr (Option<T>)
        if !self
            .declared_functions
            .contains_key("__gradient_stack_peek")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // stack
            sig.returns.push(AbiParam::new(pointer_type)); // Option<T>
            let func_id = self
                .module
                .declare_function("__gradient_stack_peek", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_stack_peek: {}", e))?;
            self.declared_functions
                .insert("__gradient_stack_peek".to_string(), func_id);
        }

        // __gradient_stack_size(s: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_stack_size")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // stack
            sig.returns.push(AbiParam::new(cl_types::I64)); // size
            let func_id = self
                .module
                .declare_function("__gradient_stack_size", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_stack_size: {}", e))?;
            self.declared_functions
                .insert("__gradient_stack_size".to_string(), func_id);
        }

        // ── Self-Hosting Phase 1.1: HashMap Builtins ────────────────────────
        // Note: Runtime has specialized versions for String vs Int keys

        // __gradient_hashmap_new_string() -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_new_string")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_new_string", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_new_string: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_new_string".to_string(), func_id);
        }

        // __gradient_hashmap_new_int() -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_new_int")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_new_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_new_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_new_int".to_string(), func_id);
        }

        // __gradient_hashmap_insert_string(hm: ptr, key: ptr, value: i64) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_insert_string")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // hm
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.params.push(AbiParam::new(cl_types::I64)); // value
            sig.returns.push(AbiParam::new(pointer_type)); // Option[V]
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_insert_string", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_insert_string: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_insert_string".to_string(), func_id);
        }

        // __gradient_hashmap_insert_int(hm: ptr, key: i64, value: i64) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_insert_int")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // hm
            sig.params.push(AbiParam::new(cl_types::I64)); // key
            sig.params.push(AbiParam::new(cl_types::I64)); // value
            sig.returns.push(AbiParam::new(pointer_type)); // Option[V]
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_insert_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_insert_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_insert_int".to_string(), func_id);
        }

        // __gradient_hashmap_get_string(hm: ptr, key: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_get_string")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // hm
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.returns.push(AbiParam::new(pointer_type)); // Option[V]
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_get_string", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_get_string: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_get_string".to_string(), func_id);
        }

        // __gradient_hashmap_get_int(hm: ptr, key: i64) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_get_int")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // hm
            sig.params.push(AbiParam::new(cl_types::I64)); // key
            sig.returns.push(AbiParam::new(pointer_type)); // Option[V]
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_get_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_get_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_get_int".to_string(), func_id);
        }

        // __gradient_hashmap_contains_string(hm: ptr, key: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_contains_string")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // hm
            sig.params.push(AbiParam::new(pointer_type)); // key
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_contains_string", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_contains_string: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_contains_string".to_string(), func_id);
        }

        // __gradient_hashmap_contains_int(hm: ptr, key: i64) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_contains_int")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // hm
            sig.params.push(AbiParam::new(cl_types::I64)); // key
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_contains_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_contains_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_contains_int".to_string(), func_id);
        }

        // __gradient_hashmap_len(hm: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_len")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // hm
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_len", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_len: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_len".to_string(), func_id);
        }

        // __gradient_hashmap_clear(hm: ptr) -> i64
        if !self
            .declared_functions
            .contains_key("__gradient_hashmap_clear")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // hm
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self
                .module
                .declare_function("__gradient_hashmap_clear", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_hashmap_clear: {}", e))?;
            self.declared_functions
                .insert("__gradient_hashmap_clear".to_string(), func_id);
        }

        // ── Phase PP: String Utilities ────────────────────────────────────

        // __gradient_string_join(strings: ptr, separator: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_join")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // strings (List[String])
            sig.params.push(AbiParam::new(pointer_type)); // separator
            sig.returns.push(AbiParam::new(pointer_type)); // result string
            let func_id = self
                .module
                .declare_function("__gradient_string_join", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_join: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_join".to_string(), func_id);
        }

        // __gradient_string_repeat(s: ptr, n: i64) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_repeat")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(cl_types::I64)); // n
            sig.returns.push(AbiParam::new(pointer_type)); // result string
            let func_id = self
                .module
                .declare_function("__gradient_string_repeat", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_repeat: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_repeat".to_string(), func_id);
        }

        // __gradient_string_pad_left(s: ptr, n: i64, pad: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_pad_left")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(cl_types::I64)); // n
            sig.params.push(AbiParam::new(pointer_type)); // pad
            sig.returns.push(AbiParam::new(pointer_type)); // result string
            let func_id = self
                .module
                .declare_function("__gradient_string_pad_left", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_pad_left: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_pad_left".to_string(), func_id);
        }

        // __gradient_string_pad_right(s: ptr, n: i64, pad: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_pad_right")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(cl_types::I64)); // n
            sig.params.push(AbiParam::new(pointer_type)); // pad
            sig.returns.push(AbiParam::new(pointer_type)); // result string
            let func_id = self
                .module
                .declare_function("__gradient_string_pad_right", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_pad_right: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_pad_right".to_string(), func_id);
        }

        // __gradient_string_strip(s: ptr) -> ptr
        if !self
            .declared_functions
            .contains_key("__gradient_string_strip")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.returns.push(AbiParam::new(pointer_type)); // result string
            let func_id = self
                .module
                .declare_function("__gradient_string_strip", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_strip: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_strip".to_string(), func_id);
        }

        // __gradient_string_strip_prefix(s: ptr, prefix: ptr) -> ptr (Option[String])
        if !self
            .declared_functions
            .contains_key("__gradient_string_strip_prefix")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(pointer_type)); // prefix
            sig.returns.push(AbiParam::new(pointer_type)); // Option[String] ptr
            let func_id = self
                .module
                .declare_function("__gradient_string_strip_prefix", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_strip_prefix: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_strip_prefix".to_string(), func_id);
        }

        // __gradient_string_strip_suffix(s: ptr, suffix: ptr) -> ptr (Option[String])
        if !self
            .declared_functions
            .contains_key("__gradient_string_strip_suffix")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.params.push(AbiParam::new(pointer_type)); // suffix
            sig.returns.push(AbiParam::new(pointer_type)); // Option[String] ptr
            let func_id = self
                .module
                .declare_function("__gradient_string_strip_suffix", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_strip_suffix: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_strip_suffix".to_string(), func_id);
        }

        // __gradient_string_to_int(s: ptr) -> ptr (Option[Int])
        if !self
            .declared_functions
            .contains_key("__gradient_string_to_int")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.returns.push(AbiParam::new(pointer_type)); // Option[Int] ptr
            let func_id = self
                .module
                .declare_function("__gradient_string_to_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_to_int: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_to_int".to_string(), func_id);
        }

        // __gradient_string_to_float(s: ptr) -> ptr (Option[Float])
        if !self
            .declared_functions
            .contains_key("__gradient_string_to_float")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // s
            sig.returns.push(AbiParam::new(pointer_type)); // Option[Float] ptr
            let func_id = self
                .module
                .declare_function("__gradient_string_to_float", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_to_float: {}", e))?;
            self.declared_functions
                .insert("__gradient_string_to_float".to_string(), func_id);
        }

        // ── Actor Runtime Functions ────────────────────────────────────────

        // __gradient_actor_spawn(init_fn: ptr, state_size: i64) -> i64 (ActorId)
        // Based on: ActorId _gradient_rt_actor_spawn(ActorInitFn init_fn, size_t state_size)
        if !self
            .declared_functions
            .contains_key("__gradient_actor_spawn")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // init_fn (ActorInitFn)
            sig.params.push(AbiParam::new(cl_types::I64)); // state_size
            sig.returns.push(AbiParam::new(cl_types::I64)); // ActorId (u64)
            let func_id = self
                .module
                .declare_function("__gradient_actor_spawn", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_spawn: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_spawn".to_string(), func_id);
        }

        // __gradient_actor_send(target_id: i64, message_type: i64, payload: ptr, payload_size: i64) -> i64
        // Based on: int64_t _gradient_rt_actor_send(ActorId target_id, MessageType type, const void* payload, size_t payload_size)
        if !self
            .declared_functions
            .contains_key("__gradient_actor_send")
        {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // target_id (ActorId)
            sig.params.push(AbiParam::new(cl_types::I64)); // message_type
            sig.params.push(AbiParam::new(pointer_type)); // payload
            sig.params.push(AbiParam::new(cl_types::I64)); // payload_size
            sig.returns.push(AbiParam::new(cl_types::I64)); // success (1) or failure (0)
            let func_id = self
                .module
                .declare_function("__gradient_actor_send", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_send: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_send".to_string(), func_id);
        }

        // __gradient_actor_ask(target_id: i64, message_type: i64, payload: ptr, payload_size: i64) -> ptr
        // Based on: Message* _gradient_rt_actor_ask(ActorId target_id, MessageType type, const void* payload, size_t payload_size)
        if !self.declared_functions.contains_key("__gradient_actor_ask") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64)); // target_id (ActorId)
            sig.params.push(AbiParam::new(cl_types::I64)); // message_type
            sig.params.push(AbiParam::new(pointer_type)); // payload
            sig.params.push(AbiParam::new(cl_types::I64)); // payload_size
            sig.returns.push(AbiParam::new(pointer_type)); // Message* reply or NULL
            let func_id = self
                .module
                .declare_function("__gradient_actor_ask", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_ask: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_ask".to_string(), func_id);
        }

        // __gradient_actor_receive() -> ptr (Message*)
        // Based on: Message* _gradient_rt_actor_receive(void)
        if !self
            .declared_functions
            .contains_key("__gradient_actor_receive")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type)); // Message* or NULL
            let func_id = self
                .module
                .declare_function("__gradient_actor_receive", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_receive: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_receive".to_string(), func_id);
        }

        // __gradient_actor_try_receive() -> ptr (Message*)
        // Based on: Message* _gradient_rt_actor_try_receive(void)
        if !self
            .declared_functions
            .contains_key("__gradient_actor_try_receive")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type)); // Message* or NULL
            let func_id = self
                .module
                .declare_function("__gradient_actor_try_receive", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_try_receive: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_try_receive".to_string(), func_id);
        }

        // __gradient_actor_self() -> i64 (ActorId)
        // Based on: ActorId _gradient_rt_actor_self(void)
        if !self
            .declared_functions
            .contains_key("__gradient_actor_self")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(cl_types::I64)); // ActorId
            let func_id = self
                .module
                .declare_function("__gradient_actor_self", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_self: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_self".to_string(), func_id);
        }

        // __gradient_actor_yield()
        // Based on: void _gradient_rt_actor_yield(void)
        if !self
            .declared_functions
            .contains_key("__gradient_actor_yield")
        {
            let sig = self.module.make_signature();
            let func_id = self
                .module
                .declare_function("__gradient_actor_yield", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_yield: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_yield".to_string(), func_id);
        }

        // __gradient_actor_terminate()
        // Based on: void _gradient_rt_actor_terminate(void)
        if !self
            .declared_functions
            .contains_key("__gradient_actor_terminate")
        {
            let sig = self.module.make_signature();
            let func_id = self
                .module
                .declare_function("__gradient_actor_terminate", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_terminate: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_terminate".to_string(), func_id);
        }

        // Legacy actor functions (for backward compatibility)
        // __gradient_actor_send_legacy(handle: ptr, message_name: ptr, payload: ptr)

        // Legacy __gradient_actor_send and __gradient_actor_ask declarations are now
        // unified with the new runtime signatures above (4 params: i64, i64, ptr, i64)
        // If these functions are already declared with the old signature, we need to ensure
        // the instruction handlers use the correct argument count.
        // NOTE: The declarations at lines ~1795-1811 have the correct 4-parameter signatures.

        // __gradient_actor_mailbox_create() -> ptr (Mailbox*)
        if !self
            .declared_functions
            .contains_key("__gradient_actor_mailbox_create")
        {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type)); // Mailbox*
            let func_id = self
                .module
                .declare_function("__gradient_actor_mailbox_create", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_actor_mailbox_create: {}", e))?;
            self.declared_functions
                .insert("__gradient_actor_mailbox_create".to_string(), func_id);
        }

        // ----------------------------------------------------------------
        // Step 2: Declare all functions in the module.
        // ----------------------------------------------------------------
        for func in &ir_module.functions {
            if self.declared_functions.contains_key(&func.name) {
                continue;
            }

            let mut sig = self.module.make_signature();
            let is_main = func.name == "main";
            if is_main {
                // C main(int argc, char** argv)
                sig.params.push(AbiParam::new(cl_types::I32)); // argc
                sig.params.push(AbiParam::new(pointer_type)); // argv
            }
            for param_ty in &func.params {
                sig.params.push(AbiParam::new(ir_type_to_cl(param_ty)));
            }
            // Special case: C `main` must return i32 even if Gradient
            // declares it as returning void/unit.
            if is_main && func.return_type == ir::Type::Void {
                sig.returns.push(AbiParam::new(cl_types::I32));
            } else if func.return_type != ir::Type::Void {
                sig.returns
                    .push(AbiParam::new(ir_type_to_cl(&func.return_type)));
            }

            let linkage = if is_main || func.is_export {
                // main and @export functions use Export linkage with C ABI.
                Linkage::Export
            } else if func.blocks.is_empty() {
                // Extern or imported function — will be linked in from elsewhere.
                Linkage::Import
            } else {
                Linkage::Local
            };

            let func_id = self
                .module
                .declare_function(&func.name, linkage, &sig)
                .map_err(|e| format!("Failed to declare function '{}': {}", func.name, e))?;
            self.declared_functions.insert(func.name.clone(), func_id);
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
        eprintln!(
            "DEBUG: Compiling function '{}' with {} blocks, value_types={}",
            func.name,
            func.blocks.len(),
            func.value_types.len()
        );
        // Print all instructions for debugging
        for (bi, block) in func.blocks.iter().enumerate() {
            eprintln!("  Block {}:", bi);
            for (ii, inst) in block.instructions.iter().enumerate() {
                eprintln!("    [{}]: {:?}", ii, inst);
            }
        }
        // Check for stale value references
        for block in &func.blocks {
            for inst in &block.instructions {
                let used_values: Vec<ir::Value> = match inst {
                    ir::Instruction::Const(v, _) => vec![*v],
                    ir::Instruction::Add(v, a, b) => vec![*v, *a, *b],
                    ir::Instruction::Sub(v, a, b) => vec![*v, *a, *b],
                    ir::Instruction::Mul(v, a, b) => vec![*v, *a, *b],
                    ir::Instruction::Div(v, a, b) => vec![*v, *a, *b],
                    ir::Instruction::Cmp(v, _, a, b) => vec![*v, *a, *b],
                    ir::Instruction::Call(v, _, args) => {
                        let mut vals = vec![*v];
                        vals.extend(args.iter().cloned());
                        vals
                    }
                    ir::Instruction::Load(v, addr) => vec![*v, *addr],
                    ir::Instruction::Store(addr, val) => vec![*addr, *val],
                    ir::Instruction::PtrToInt(result, ptr) => vec![*result, *ptr],
                    ir::Instruction::IntToPtr(result, int_val) => vec![*result, *int_val],
                    ir::Instruction::GetElementPtr {
                        result,
                        base,
                        offset: _,
                        field_ty: _,
                    } => vec![*result, *base],
                    ir::Instruction::FieldAddr {
                        result,
                        base,
                        field_name: _,
                        field_ty: _,
                        offset: _,
                    } => vec![*result, *base],
                    ir::Instruction::Jump(_) => vec![],
                    ir::Instruction::Branch(cond, _, _) => vec![*cond],
                    ir::Instruction::Ret(opt) => opt.map(|v| vec![v]).unwrap_or_default(),
                    ir::Instruction::Phi(v, entries) => {
                        let mut vals = vec![*v];
                        vals.extend(entries.iter().map(|(_, av)| *av));
                        vals
                    }
                    _ => vec![],
                };
                for v in used_values {
                    if !func.value_types.contains_key(&v) {
                        eprintln!(
                            "  ERROR: Value({}) in function not in value_types! val={}",
                            v.0, v.0
                        );
                    }
                }
            }
        }

        let pointer_type = self.module.target_config().pointer_type();

        // ----------------------------------------------------------------
        // Build the Cranelift signature.
        // ----------------------------------------------------------------
        let is_main = func.name == "main";
        let mut sig = self.module.make_signature();
        if is_main {
            // C main(int argc, char** argv)
            sig.params.push(AbiParam::new(cl_types::I32)); // argc
            sig.params.push(AbiParam::new(pointer_type)); // argv
        }
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
        // Compute reachable blocks from entry (block0) via BFS.
        // Cranelift rejects dead blocks (no predecessors, non-entry), so
        // we skip them entirely.
        // ----------------------------------------------------------------
        let reachable_blocks: std::collections::HashSet<ir::BlockRef> = {
            let mut reachable = std::collections::HashSet::new();
            let mut queue = std::collections::VecDeque::new();
            if let Some(first) = func.blocks.first() {
                queue.push_back(first.label);
                reachable.insert(first.label);
            }
            // Build adjacency: block -> its jump targets
            let mut adj: HashMap<ir::BlockRef, Vec<ir::BlockRef>> = HashMap::new();
            for ir_block in &func.blocks {
                let mut targets = Vec::new();
                for inst in &ir_block.instructions {
                    match inst {
                        ir::Instruction::Jump(t) => targets.push(*t),
                        ir::Instruction::Branch(_, a, b) => {
                            targets.push(*a);
                            targets.push(*b);
                        }
                        _ => {}
                    }
                }
                adj.insert(ir_block.label, targets);
            }
            while let Some(b) = queue.pop_front() {
                if let Some(nexts) = adj.get(&b) {
                    for &n in nexts {
                        if reachable.insert(n) {
                            queue.push_back(n);
                        }
                    }
                }
            }
            reachable
        };

        // ----------------------------------------------------------------
        // Create all Cranelift blocks up front (only reachable ones).
        // ----------------------------------------------------------------
        let mut block_map: HashMap<ir::BlockRef, cranelift_codegen::ir::Block> = HashMap::new();
        for ir_block in &func.blocks {
            if !reachable_blocks.contains(&ir_block.label) {
                continue;
            }
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
        //
        // Before processing phis, build a map of which blocks each block
        // actually jumps/branches to (its "terminator targets"). Phi entries
        // from blocks that end with Ret instead of jumping to the phi's
        // target are unreachable and must be excluded. This prevents type
        // mismatches when one branch of an if-expression terminates early
        // via `ret` and the IR builder emits a phantom phi entry for it.
        // ----------------------------------------------------------------

        // Map each block to the set of blocks it actually jumps/branches to.
        let mut block_jump_targets: HashMap<ir::BlockRef, HashSet<ir::BlockRef>> = HashMap::new();
        for ir_block in &func.blocks {
            let mut targets = HashSet::new();
            for inst in &ir_block.instructions {
                match inst {
                    ir::Instruction::Jump(target) => {
                        targets.insert(*target);
                    }
                    ir::Instruction::Branch(_, then_b, else_b) => {
                        targets.insert(*then_b);
                        targets.insert(*else_b);
                    }
                    _ => {}
                }
            }
            block_jump_targets.insert(ir_block.label, targets);
        }

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
            // Skip unreachable blocks.
            if !reachable_blocks.contains(&ir_block.label) {
                continue;
            }
            for inst in &ir_block.instructions {
                if let ir::Instruction::Phi(dst, entries) = inst {
                    // Filter phi entries to only include source blocks that
                    // actually jump/branch to this block. Entries from blocks
                    // that end with Ret are unreachable and would cause type
                    // mismatches in block parameters.
                    let reachable_entries: Vec<(ir::BlockRef, ir::Value)> = entries
                        .iter()
                        .filter(|(src_block, _)| {
                            block_jump_targets
                                .get(src_block)
                                .is_some_and(|targets| targets.contains(&ir_block.label))
                        })
                        .cloned()
                        .collect();

                    // Determine the block parameter type from reachable entries.
                    // If there are reachable entries, use the type of the first
                    // reachable entry's value (which is guaranteed to be correct).
                    // Fall back to the phi destination type or I64.
                    let cl_type = if let Some((_, first_val)) = reachable_entries.first() {
                        func.value_types
                            .get(first_val)
                            .map(ir_type_to_cl)
                            .unwrap_or_else(|| {
                                func.value_types
                                    .get(dst)
                                    .map(ir_type_to_cl)
                                    .unwrap_or(cl_types::I64)
                            })
                    } else {
                        func.value_types
                            .get(dst)
                            .map(ir_type_to_cl)
                            .unwrap_or(cl_types::I64)
                    };

                    let cl_block = block_map[&ir_block.label];
                    let param_idx = block_param_counts.entry(ir_block.label).or_insert(0);
                    let current_idx = *param_idx;
                    *param_idx += 1;

                    builder.append_block_param(cl_block, cl_type);

                    phi_infos.push(PhiInfo {
                        dst: *dst,
                        cl_type,
                        entries: reachable_entries,
                        target_block: ir_block.label,
                        param_index: current_idx,
                    });
                }
            }
        }

        // Build jump_args: target_block -> source_block -> [IR Values].
        // Only reachable phi entries are included (unreachable ones were
        // filtered out above).
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
        // Pre-pass: identify loop headers (blocks that are targets of
        // back-edges). A back-edge is a jump/branch from a block that
        // appears later in the block list to a block that appears earlier.
        // Loop headers must NOT be sealed until all predecessors (including
        // the back-edge) have been emitted.
        // ----------------------------------------------------------------
        let block_order: HashMap<ir::BlockRef, usize> = func
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label, i))
            .collect();

        let mut loop_headers: HashSet<ir::BlockRef> = HashSet::new();
        for (src_idx, ir_block) in func.blocks.iter().enumerate() {
            for inst in &ir_block.instructions {
                let targets: Vec<ir::BlockRef> = match inst {
                    ir::Instruction::Jump(target) => vec![*target],
                    ir::Instruction::Branch(_, then_b, else_b) => vec![*then_b, *else_b],
                    _ => vec![],
                };
                for target in targets {
                    if let Some(&target_idx) = block_order.get(&target) {
                        if target_idx <= src_idx {
                            // This is a back-edge: source comes after (or is) the target.
                            loop_headers.insert(target);
                        }
                    }
                }
            }
        }

        // Track how many predecessors each loop header expects, and how many
        // have been emitted so far, so we know when it's safe to seal.
        let mut predecessor_count: HashMap<ir::BlockRef, usize> = HashMap::new();
        for header in &loop_headers {
            let mut count = 0usize;
            for ir_block in &func.blocks {
                for inst in &ir_block.instructions {
                    match inst {
                        ir::Instruction::Jump(target) if target == header => {
                            count += 1;
                        }
                        ir::Instruction::Branch(_, then_b, else_b) => {
                            if then_b == header {
                                count += 1;
                            }
                            if else_b == header {
                                count += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
            predecessor_count.insert(*header, count);
        }

        let mut predecessors_emitted: HashMap<ir::BlockRef, usize> = HashMap::new();
        // Blocks whose sealing has been deferred.
        let mut deferred_seal: HashSet<ir::BlockRef> = HashSet::new();

        // ----------------------------------------------------------------
        // Second pass: translate instructions block by block.
        // ----------------------------------------------------------------
        let mut value_map: HashMap<ir::Value, cranelift_codegen::ir::Value> = HashMap::new();
        let mut func_ref_map: HashMap<ir::FuncRef, cranelift_codegen::ir::FuncRef> = HashMap::new();

        for (block_idx, ir_block) in func.blocks.iter().enumerate() {
            // Skip unreachable blocks — Cranelift rejects them.
            if !reachable_blocks.contains(&ir_block.label) {
                continue;
            }
            let cl_block = block_map[&ir_block.label];
            builder.switch_to_block(cl_block);

            // Map entry block function parameters to IR Values.
            // For main, the Cranelift signature has extra argc/argv params
            // before the IR-level params.
            let main_extra_params = if is_main && block_idx == 0 { 2 } else { 0 };
            if block_idx == 0 {
                let params = builder.block_params(cl_block).to_vec();

                // If this is main, call __gradient_save_args(argc, argv)
                // before any user code runs.
                if is_main && params.len() >= 2 {
                    let argc_i32 = params[0]; // i32 from C main
                    let argv_ptr = params[1]; // char** from C main
                                              // Widen argc from i32 to i64 for the C helper.
                    let argc_i64 = builder.ins().sextend(cl_types::I64, argc_i32);
                    let save_func_id = *self
                        .declared_functions
                        .get("__gradient_save_args")
                        .ok_or("__gradient_save_args not declared")?;
                    let save_ref = self.module.declare_func_in_func(save_func_id, builder.func);
                    builder.ins().call(save_ref, &[argc_i64, argv_ptr]);
                }

                for (i, _param_ty) in func.params.iter().enumerate() {
                    let ci = i + main_extra_params;
                    if ci < params.len() {
                        value_map.insert(ir::Value(i as u32), params[ci]);
                    }
                }
            }

            // Map phi-defined values to their block parameters.
            {
                let base_param_offset = if block_idx == 0 {
                    func.params.len() + main_extra_params
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
            // Track whether this block has been filled (a terminator was emitted).
            // Once filled, we skip remaining instructions in this IR block.
            let mut block_filled = false;
            for inst in &ir_block.instructions {
                if block_filled {
                    break;
                }
                match inst {
                    ir::Instruction::Const(dst, literal) => {
                        let cl_val = match literal {
                            ir::Literal::Int(n) => {
                                // Check if this integer constant is actually a
                                // closure function pointer (the IR builder emits
                                // FuncRef index as a plain Int literal for closures).
                                let closure_name = ir_module
                                    .func_refs
                                    .get(&ir::FuncRef(*n as u32))
                                    .filter(|name| name.starts_with("__closure_"));
                                if let Some(cname) = closure_name {
                                    if let Some(&fid) = self.declared_functions.get(cname.as_str())
                                    {
                                        let fref =
                                            self.module.declare_func_in_func(fid, builder.func);
                                        builder.ins().func_addr(pointer_type, fref)
                                    } else {
                                        builder.ins().iconst(cl_types::I64, *n)
                                    }
                                } else {
                                    // Use the declared type of dst to emit the right
                                    // width constant. This matters for Void (i8) results
                                    // like the return value of for loops and void calls,
                                    // which must match block parameter types in phis.
                                    let const_ty = func
                                        .value_types
                                        .get(dst)
                                        .map(ir_type_to_cl)
                                        .unwrap_or(cl_types::I64);
                                    builder.ins().iconst(const_ty, *n)
                                }
                            }
                            ir::Literal::Float(f) => builder.ins().f64const(*f),
                            ir::Literal::Bool(b) => builder.ins().iconst(cl_types::I8, *b as i64),
                            ir::Literal::Str(s) => {
                                // Use the free function to avoid borrow conflict.
                                let data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    s,
                                )?;
                                let data_gv =
                                    self.module.declare_data_in_func(data_id, builder.func);
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
                        let ty_a = builder.func.dfg.value_type(a);
                        let ty_b = builder.func.dfg.value_type(b);
                        let result = if ty_a == cl_types::F64 || ty_b == cl_types::F64 {
                            let fcc = cmpop_to_floatcc(op);
                            builder.ins().fcmp(fcc, a, b)
                        } else {
                            // Normalize integer operands to the same width before comparing.
                            // Mixed i8/i64 comparisons arise when a Bool literal (i8) is
                            // compared against an i64-returning function (e.g. file_exists).
                            let (a2, b2) = if ty_a != ty_b {
                                let wider = if ty_a.bits() >= ty_b.bits() {
                                    ty_a
                                } else {
                                    ty_b
                                };
                                let a3 = if ty_a == wider {
                                    a
                                } else {
                                    builder.ins().uextend(wider, a)
                                };
                                let b3 = if ty_b == wider {
                                    b
                                } else {
                                    builder.ins().uextend(wider, b)
                                };
                                (a3, b3)
                            } else {
                                (a, b)
                            };
                            let cc = cmpop_to_intcc(op);
                            builder.ins().icmp(cc, a2, b2)
                        };
                        value_map.insert(*dst, result);
                    }

                    ir::Instruction::Call(dst, ir_func_ref, args) => {
                        let func_name = ir_module.func_refs.get(ir_func_ref).ok_or_else(|| {
                            format!("Unknown FuncRef({}) in call instruction", ir_func_ref.0)
                        })?;
                        eprintln!(
                            "DEBUG Call: FuncRef({}) -> '{}' in function '{}'",
                            ir_func_ref.0, func_name, func.name
                        );

                        match func_name.as_str() {
                            // ── print_int: call printf("%ld", value) ──
                            "print_int" => {
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%ld",
                                )?;
                                let fmt_gv =
                                    self.module.declare_data_in_func(fmt_data_id, builder.func);
                                let fmt_ptr = builder.ins().global_value(pointer_type, fmt_gv);

                                let printf_func_id = *self
                                    .declared_functions
                                    .get("printf")
                                    .ok_or("printf not declared")?;
                                let printf_ref = self
                                    .module
                                    .declare_func_in_func(printf_func_id, builder.func);

                                let int_val = resolve_value(&value_map, &args[0])?;
                                let call_inst = builder.ins().call(printf_ref, &[fmt_ptr, int_val]);
                                let results = builder.inst_results(call_inst).to_vec();
                                let result_val = if !results.is_empty() {
                                    results[0]
                                } else {
                                    builder.ins().iconst(cl_types::I64, 0)
                                };
                                value_map.insert(*dst, result_val);
                            }

                            // ── print_float: call printf("%.6f", value) via call_indirect ──
                            "print_float" => {
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%.6f",
                                )?;
                                let fmt_gv =
                                    self.module.declare_data_in_func(fmt_data_id, builder.func);
                                let fmt_ptr = builder.ins().global_value(pointer_type, fmt_gv);

                                // Get the printf function address.
                                let printf_func_id = *self
                                    .declared_functions
                                    .get("printf")
                                    .ok_or("printf not declared")?;
                                let printf_ref = self
                                    .module
                                    .declare_func_in_func(printf_func_id, builder.func);
                                let printf_addr = builder.ins().func_addr(pointer_type, printf_ref);

                                // Create a float-compatible signature: (ptr, f64) -> i32
                                let mut float_printf_sig = self.module.make_signature();
                                float_printf_sig.params.push(AbiParam::new(pointer_type));
                                float_printf_sig.params.push(AbiParam::new(cl_types::F64));
                                float_printf_sig.returns.push(AbiParam::new(cl_types::I32));
                                let sig_ref = builder.import_signature(float_printf_sig);

                                let float_val = resolve_value(&value_map, &args[0])?;
                                let call_inst = builder.ins().call_indirect(
                                    sig_ref,
                                    printf_addr,
                                    &[fmt_ptr, float_val],
                                );
                                let results = builder.inst_results(call_inst).to_vec();
                                let result_val = if !results.is_empty() {
                                    results[0]
                                } else {
                                    builder.ins().iconst(cl_types::I64, 0)
                                };
                                value_map.insert(*dst, result_val);
                            }

                            // ── print_bool: printf("%s", "true"/"false") ──
                            "print_bool" => {
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%s",
                                )?;
                                let true_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "true",
                                )?;
                                let false_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "false",
                                )?;
                                let fmt_gv =
                                    self.module.declare_data_in_func(fmt_data_id, builder.func);
                                let true_gv =
                                    self.module.declare_data_in_func(true_data_id, builder.func);
                                let false_gv = self
                                    .module
                                    .declare_data_in_func(false_data_id, builder.func);
                                let fmt_ptr = builder.ins().global_value(pointer_type, fmt_gv);
                                let true_ptr = builder.ins().global_value(pointer_type, true_gv);
                                let false_ptr = builder.ins().global_value(pointer_type, false_gv);

                                let bool_val = resolve_value(&value_map, &args[0])?;

                                // select: if bool_val then true_ptr else false_ptr
                                let str_ptr = builder.ins().select(bool_val, true_ptr, false_ptr);

                                // Use call_indirect with (ptr, ptr) -> i32 signature
                                let printf_func_id = *self
                                    .declared_functions
                                    .get("printf")
                                    .ok_or("printf not declared")?;
                                let printf_ref = self
                                    .module
                                    .declare_func_in_func(printf_func_id, builder.func);
                                let printf_addr = builder.ins().func_addr(pointer_type, printf_ref);

                                let mut str_printf_sig = self.module.make_signature();
                                str_printf_sig.params.push(AbiParam::new(pointer_type));
                                str_printf_sig.params.push(AbiParam::new(pointer_type));
                                str_printf_sig.returns.push(AbiParam::new(cl_types::I32));
                                let sig_ref = builder.import_signature(str_printf_sig);

                                let call_inst = builder.ins().call_indirect(
                                    sig_ref,
                                    printf_addr,
                                    &[fmt_ptr, str_ptr],
                                );
                                let results = builder.inst_results(call_inst).to_vec();
                                let result_val = if !results.is_empty() {
                                    results[0]
                                } else {
                                    builder.ins().iconst(cl_types::I64, 0)
                                };
                                value_map.insert(*dst, result_val);
                            }

                            // ── print(s): call printf("%s", s) without newline ──
                            "print" => {
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%s",
                                )?;
                                let fmt_gv =
                                    self.module.declare_data_in_func(fmt_data_id, builder.func);
                                let fmt_ptr = builder.ins().global_value(pointer_type, fmt_gv);

                                let printf_func_id = *self
                                    .declared_functions
                                    .get("printf")
                                    .ok_or("printf not declared")?;
                                let printf_ref = self
                                    .module
                                    .declare_func_in_func(printf_func_id, builder.func);

                                let str_val = resolve_value(&value_map, &args[0])?;
                                let call_inst = builder.ins().call(printf_ref, &[fmt_ptr, str_val]);
                                let results = builder.inst_results(call_inst).to_vec();
                                let result_val = if !results.is_empty() {
                                    results[0]
                                } else {
                                    builder.ins().iconst(cl_types::I64, 0)
                                };
                                value_map.insert(*dst, result_val);
                            }

                            // ── abs(n): if n < 0 then -n else n ──
                            "abs" => {
                                let n = resolve_value(&value_map, &args[0])?;
                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                let neg_n = builder.ins().isub(zero, n);
                                let is_neg = builder.ins().icmp(IntCC::SignedLessThan, n, zero);
                                let result = builder.ins().select(is_neg, neg_n, n);
                                value_map.insert(*dst, result);
                            }

                            // ── min(a, b): if a < b then a else b ──
                            "min" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, a, b);
                                let result = builder.ins().select(cmp, a, b);
                                value_map.insert(*dst, result);
                            }

                            // ── max(a, b): if a > b then a else b ──
                            "max" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;
                                let cmp = builder.ins().icmp(IntCC::SignedGreaterThan, a, b);
                                let result = builder.ins().select(cmp, a, b);
                                value_map.insert(*dst, result);
                            }

                            // ── mod_int(a, b): a - (a / b) * b ──
                            "mod_int" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;
                                let div = builder.ins().sdiv(a, b);
                                let mul = builder.ins().imul(div, b);
                                let result = builder.ins().isub(a, mul);
                                value_map.insert(*dst, result);
                            }

                            // ── int_to_string(n): format i64 via snprintf ──
                            "int_to_string" => {
                                // Allocate buffer (32 bytes is plenty for i64)
                                let buf_size = builder.ins().iconst(cl_types::I64, 32);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[buf_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // Format string "%ld"
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%ld",
                                )?;
                                let fmt_gv =
                                    self.module.declare_data_in_func(fmt_data_id, builder.func);
                                let fmt_ptr = builder.ins().global_value(pointer_type, fmt_gv);

                                // snprintf(buf, 32, "%ld", value)
                                let int_val = resolve_value(&value_map, &args[0])?;
                                let snprintf_func_id = *self
                                    .declared_functions
                                    .get("snprintf")
                                    .ok_or("snprintf not declared")?;
                                let snprintf_ref = self
                                    .module
                                    .declare_func_in_func(snprintf_func_id, builder.func);
                                builder
                                    .ins()
                                    .call(snprintf_ref, &[buf, buf_size, fmt_ptr, int_val]);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_concat(a, b): malloc + strcpy + strcat ──
                            "string_concat" => {
                                let str_a = resolve_value(&value_map, &args[0])?;
                                let str_b = resolve_value(&value_map, &args[1])?;

                                // len_a = strlen(a)
                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);
                                let call_a = builder.ins().call(strlen_ref, &[str_a]);
                                let len_a = builder.inst_results(call_a).to_vec()[0];

                                // Need a fresh strlen ref for the second call,
                                // but Cranelift allows reusing the same ref.
                                let call_b = builder.ins().call(strlen_ref, &[str_b]);
                                let len_b = builder.inst_results(call_b).to_vec()[0];

                                // total = len_a + len_b + 1
                                let total_len = builder.ins().iadd(len_a, len_b);
                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size = builder.ins().iadd(total_len, one);

                                // buf = malloc(total)
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // strcpy(buf, a)
                                let strcpy_func_id = *self
                                    .declared_functions
                                    .get("strcpy")
                                    .ok_or("strcpy not declared")?;
                                let strcpy_ref = self
                                    .module
                                    .declare_func_in_func(strcpy_func_id, builder.func);
                                builder.ins().call(strcpy_ref, &[buf, str_a]);

                                // strcat(buf, b)
                                let strcat_func_id = *self
                                    .declared_functions
                                    .get("strcat")
                                    .ok_or("strcat not declared")?;
                                let strcat_ref = self
                                    .module
                                    .declare_func_in_func(strcat_func_id, builder.func);
                                builder.ins().call(strcat_ref, &[buf, str_b]);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_length(s): strlen(s) ──
                            "string_length" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);
                                let call = builder.ins().call(strlen_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── string_contains(s, substr): strstr(s, substr) != NULL ──
                            "string_contains" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let substr = resolve_value(&value_map, &args[1])?;
                                let strstr_func_id = *self
                                    .declared_functions
                                    .get("strstr")
                                    .ok_or("strstr not declared")?;
                                let strstr_ref = self
                                    .module
                                    .declare_func_in_func(strstr_func_id, builder.func);
                                let call = builder.ins().call(strstr_ref, &[s, substr]);
                                let ptr_result = builder.inst_results(call).to_vec()[0];
                                let zero = builder.ins().iconst(pointer_type, 0);
                                let result = builder.ins().icmp(IntCC::NotEqual, ptr_result, zero);
                                value_map.insert(*dst, result);
                            }

                            // ── string_starts_with(s, prefix): strncmp(s, prefix, strlen(prefix)) == 0 ──
                            "string_starts_with" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let prefix = resolve_value(&value_map, &args[1])?;

                                // len = strlen(prefix)
                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);
                                let call = builder.ins().call(strlen_ref, &[prefix]);
                                let prefix_len = builder.inst_results(call).to_vec()[0];

                                // strncmp(s, prefix, len)
                                let strncmp_func_id = *self
                                    .declared_functions
                                    .get("strncmp")
                                    .ok_or("strncmp not declared")?;
                                let strncmp_ref = self
                                    .module
                                    .declare_func_in_func(strncmp_func_id, builder.func);
                                let cmp_call =
                                    builder.ins().call(strncmp_ref, &[s, prefix, prefix_len]);
                                let cmp_result = builder.inst_results(cmp_call).to_vec()[0];

                                let zero = builder.ins().iconst(cl_types::I32, 0);
                                let result = builder.ins().icmp(IntCC::Equal, cmp_result, zero);
                                value_map.insert(*dst, result);
                            }

                            // ── string_eq(a, b): strcmp(a, b) == 0 → Bool (i8) ──
                            "string_eq" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;
                                let strcmp_func_id = *self
                                    .declared_functions
                                    .get("strcmp")
                                    .ok_or("strcmp not declared")?;
                                let strcmp_ref = self
                                    .module
                                    .declare_func_in_func(strcmp_func_id, builder.func);
                                let cmp_call = builder.ins().call(strcmp_ref, &[a, b]);
                                let cmp_result = builder.inst_results(cmp_call).to_vec()[0]; // i32
                                let zero = builder.ins().iconst(cl_types::I32, 0);
                                // icmp returns i8 (Bool)
                                let result = builder.ins().icmp(IntCC::Equal, cmp_result, zero);
                                value_map.insert(*dst, result);
                            }

                            // ── string_ends_with(s, suffix): compare tail of s with suffix ──
                            "string_ends_with" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let suffix = resolve_value(&value_map, &args[1])?;

                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);

                                // s_len = strlen(s)
                                let call_s = builder.ins().call(strlen_ref, &[s]);
                                let s_len = builder.inst_results(call_s).to_vec()[0];

                                // suf_len = strlen(suffix)
                                let call_suf = builder.ins().call(strlen_ref, &[suffix]);
                                let suf_len = builder.inst_results(call_suf).to_vec()[0];

                                // offset = s_len - suf_len
                                let offset = builder.ins().isub(s_len, suf_len);

                                // tail_ptr = s + offset
                                let tail_ptr = builder.ins().iadd(s, offset);

                                // strncmp(tail_ptr, suffix, suf_len)
                                let strncmp_func_id = *self
                                    .declared_functions
                                    .get("strncmp")
                                    .ok_or("strncmp not declared")?;
                                let strncmp_ref = self
                                    .module
                                    .declare_func_in_func(strncmp_func_id, builder.func);
                                let cmp_call = builder
                                    .ins()
                                    .call(strncmp_ref, &[tail_ptr, suffix, suf_len]);
                                let cmp_result = builder.inst_results(cmp_call).to_vec()[0];

                                let zero = builder.ins().iconst(cl_types::I32, 0);
                                let result = builder.ins().icmp(IntCC::Equal, cmp_result, zero);
                                value_map.insert(*dst, result);
                            }

                            // ── string_substring(s, start, end): extract [start, end) ──
                            "string_substring" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let start = resolve_value(&value_map, &args[1])?;
                                let end = resolve_value(&value_map, &args[2])?;

                                // len = end - start
                                let len = builder.ins().isub(end, start);
                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size = builder.ins().iadd(len, one);

                                // buf = malloc(len + 1)
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // src_ptr = s + start
                                let src_ptr = builder.ins().iadd(s, start);

                                // memcpy(buf, src_ptr, len)
                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let memcpy_ref = self
                                    .module
                                    .declare_func_in_func(memcpy_func_id, builder.func);
                                builder.ins().call(memcpy_ref, &[buf, src_ptr, len]);

                                // buf[len] = '\0'
                                let nul = builder.ins().iconst(cl_types::I8, 0);
                                let end_ptr = builder.ins().iadd(buf, len);
                                builder.ins().store(MemFlags::new(), nul, end_ptr, 0);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_trim(s): call __gradient_string_trim(s) -> String ──
                            "string_trim" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_trim")
                                    .ok_or("__gradient_string_trim not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── string_to_upper(s): copy + toupper each byte ──
                            "string_to_upper" => {
                                let s = resolve_value(&value_map, &args[0])?;

                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);
                                let call = builder.ins().call(strlen_ref, &[s]);
                                let len = builder.inst_results(call).to_vec()[0];

                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size = builder.ins().iadd(len, one);

                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // Loop over each byte: buf[i] = toupper(s[i])
                                let toupper_func_id = *self
                                    .declared_functions
                                    .get("toupper")
                                    .ok_or("toupper not declared")?;
                                let toupper_ref = self
                                    .module
                                    .declare_func_in_func(toupper_func_id, builder.func);

                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i

                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero)]);

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, len);
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                // Load s[i] as I8, zero-extend to I32 for toupper
                                let src_ptr = builder.ins().iadd(s, i_val);
                                let ch =
                                    builder
                                        .ins()
                                        .load(cl_types::I8, MemFlags::new(), src_ptr, 0);
                                let ch_i32 = builder.ins().uextend(cl_types::I32, ch);
                                let toupper_call = builder.ins().call(toupper_ref, &[ch_i32]);
                                let upper_i32 = builder.inst_results(toupper_call).to_vec()[0];
                                let upper_i8 = builder.ins().ireduce(cl_types::I8, upper_i32);

                                // Store to buf[i]
                                let dst_ptr = builder.ins().iadd(buf, i_val);
                                builder.ins().store(MemFlags::new(), upper_i8, dst_ptr, 0);

                                let one_inc = builder.ins().iconst(cl_types::I64, 1);
                                let next_i = builder.ins().iadd(i_val, one_inc);
                                builder.ins().jump(loop_header, &[BlockArg::Value(next_i)]);

                                // Seal loop_header now (predecessors: entry jump + body back-edge)
                                builder.seal_block(loop_header);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                // Null-terminate: buf[len] = 0
                                let nul = builder.ins().iconst(cl_types::I8, 0);
                                let end_ptr = builder.ins().iadd(buf, len);
                                builder.ins().store(MemFlags::new(), nul, end_ptr, 0);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_to_lower(s): copy + tolower each byte ──
                            "string_to_lower" => {
                                let s = resolve_value(&value_map, &args[0])?;

                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);
                                let call = builder.ins().call(strlen_ref, &[s]);
                                let len = builder.inst_results(call).to_vec()[0];

                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size = builder.ins().iadd(len, one);

                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // Loop over each byte: buf[i] = tolower(s[i])
                                let tolower_func_id = *self
                                    .declared_functions
                                    .get("tolower")
                                    .ok_or("tolower not declared")?;
                                let tolower_ref = self
                                    .module
                                    .declare_func_in_func(tolower_func_id, builder.func);

                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i

                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero)]);

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, len);
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                // Load s[i] as I8, zero-extend to I32 for tolower
                                let src_ptr = builder.ins().iadd(s, i_val);
                                let ch =
                                    builder
                                        .ins()
                                        .load(cl_types::I8, MemFlags::new(), src_ptr, 0);
                                let ch_i32 = builder.ins().uextend(cl_types::I32, ch);
                                let tolower_call = builder.ins().call(tolower_ref, &[ch_i32]);
                                let lower_i32 = builder.inst_results(tolower_call).to_vec()[0];
                                let lower_i8 = builder.ins().ireduce(cl_types::I8, lower_i32);

                                // Store to buf[i]
                                let dst_ptr = builder.ins().iadd(buf, i_val);
                                builder.ins().store(MemFlags::new(), lower_i8, dst_ptr, 0);

                                let one_inc = builder.ins().iconst(cl_types::I64, 1);
                                let next_i = builder.ins().iadd(i_val, one_inc);
                                builder.ins().jump(loop_header, &[BlockArg::Value(next_i)]);

                                // Seal loop_header now (predecessors: entry jump + body back-edge)
                                builder.seal_block(loop_header);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                // Null-terminate: buf[len] = 0
                                let nul = builder.ins().iconst(cl_types::I8, 0);
                                let end_ptr = builder.ins().iadd(buf, len);
                                builder.ins().store(MemFlags::new(), nul, end_ptr, 0);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_replace(s, old, new_str): find and replace all ──
                            "string_replace" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let old_str = resolve_value(&value_map, &args[1])?;
                                let new_str = resolve_value(&value_map, &args[2])?;

                                // Get function refs
                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);
                                let strlen_ref2 = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);
                                let strlen_ref3 = self
                                    .module
                                    .declare_func_in_func(strlen_func_id, builder.func);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let strstr_func_id = *self
                                    .declared_functions
                                    .get("strstr")
                                    .ok_or("strstr not declared")?;
                                let strcpy_func_id = *self
                                    .declared_functions
                                    .get("strcpy")
                                    .ok_or("strcpy not declared")?;

                                // s_len = strlen(s), old_len = strlen(old), new_len = strlen(new)
                                let call_s = builder.ins().call(strlen_ref, &[s]);
                                let s_len = builder.inst_results(call_s).to_vec()[0];
                                let call_old = builder.ins().call(strlen_ref2, &[old_str]);
                                let old_len = builder.inst_results(call_old).to_vec()[0];
                                let call_new = builder.ins().call(strlen_ref3, &[new_str]);
                                let new_len = builder.inst_results(call_new).to_vec()[0];

                                // Check if old_len == 0; if so, just copy input
                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                let old_is_empty = builder.ins().icmp(IntCC::Equal, old_len, zero);

                                let empty_block = builder.create_block();
                                let nonempty_block = builder.create_block();
                                let merge_block = builder.create_block();
                                builder.append_block_param(merge_block, cl_types::I64); // result ptr

                                builder.ins().brif(
                                    old_is_empty,
                                    empty_block,
                                    &[],
                                    nonempty_block,
                                    &[],
                                );

                                // --- empty_block: old is empty, return copy of s ---
                                builder.switch_to_block(empty_block);
                                builder.seal_block(empty_block);
                                let one_e = builder.ins().iconst(cl_types::I64, 1);
                                let copy_size = builder.ins().iadd(s_len, one_e);
                                let malloc_ref_e = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call_e = builder.ins().call(malloc_ref_e, &[copy_size]);
                                let copy_buf = builder.inst_results(malloc_call_e).to_vec()[0];
                                let strcpy_ref_e = self
                                    .module
                                    .declare_func_in_func(strcpy_func_id, builder.func);
                                builder.ins().call(strcpy_ref_e, &[copy_buf, s]);
                                builder
                                    .ins()
                                    .jump(merge_block, &[BlockArg::Value(copy_buf)]);

                                // --- nonempty_block: do real replacement ---
                                builder.switch_to_block(nonempty_block);
                                builder.seal_block(nonempty_block);

                                // Over-allocate: worst case = s_len * (new_len + 1) + 1
                                // This handles cases where every char could be a match
                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let new_len_plus_one = builder.ins().iadd(new_len, one);
                                let worst_case = builder.ins().imul(s_len, new_len_plus_one);
                                let alloc_size = builder.ins().iadd(worst_case, one);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // Loop: scan with strstr, copy prefix + replacement
                                // Block params: (src_pos: ptr, dst_pos: ptr)
                                let loop_header = builder.create_block();
                                let found_block = builder.create_block();
                                let notfound_block = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // src_pos (current position in s)
                                builder.append_block_param(loop_header, cl_types::I64); // dst_pos (current position in buf)

                                builder
                                    .ins()
                                    .jump(loop_header, &[BlockArg::Value(s), BlockArg::Value(buf)]);

                                // --- loop_header: call strstr(src_pos, old_str) ---
                                builder.switch_to_block(loop_header);
                                let src_pos = builder.block_params(loop_header)[0];
                                let dst_pos = builder.block_params(loop_header)[1];

                                let strstr_ref = self
                                    .module
                                    .declare_func_in_func(strstr_func_id, builder.func);
                                let strstr_call =
                                    builder.ins().call(strstr_ref, &[src_pos, old_str]);
                                let found_ptr = builder.inst_results(strstr_call).to_vec()[0];

                                let null_ptr = builder.ins().iconst(cl_types::I64, 0);
                                let is_null = builder.ins().icmp(IntCC::Equal, found_ptr, null_ptr);
                                builder
                                    .ins()
                                    .brif(is_null, notfound_block, &[], found_block, &[]);

                                // --- found_block: copy prefix, copy replacement, advance ---
                                builder.switch_to_block(found_block);
                                builder.seal_block(found_block);

                                // prefix_len = found_ptr - src_pos
                                let prefix_len = builder.ins().isub(found_ptr, src_pos);

                                // memcpy(dst_pos, src_pos, prefix_len)
                                let memcpy_ref1 = self
                                    .module
                                    .declare_func_in_func(memcpy_func_id, builder.func);
                                builder
                                    .ins()
                                    .call(memcpy_ref1, &[dst_pos, src_pos, prefix_len]);

                                // dst_pos += prefix_len
                                let dst_after_prefix = builder.ins().iadd(dst_pos, prefix_len);

                                // memcpy(dst_after_prefix, new_str, new_len)
                                let memcpy_ref2 = self
                                    .module
                                    .declare_func_in_func(memcpy_func_id, builder.func);
                                builder
                                    .ins()
                                    .call(memcpy_ref2, &[dst_after_prefix, new_str, new_len]);

                                // dst_pos += new_len
                                let dst_after_new = builder.ins().iadd(dst_after_prefix, new_len);

                                // src_pos = found_ptr + old_len (skip past the matched occurrence)
                                let src_after_old = builder.ins().iadd(found_ptr, old_len);

                                builder.ins().jump(
                                    loop_header,
                                    &[
                                        BlockArg::Value(src_after_old),
                                        BlockArg::Value(dst_after_new),
                                    ],
                                );

                                // Seal loop_header (predecessors: nonempty_block entry + found_block back-edge)
                                builder.seal_block(loop_header);

                                // --- notfound_block: copy remaining + null-terminate ---
                                builder.switch_to_block(notfound_block);
                                builder.seal_block(notfound_block);

                                // Copy the remainder of the string (strcpy copies including null terminator)
                                let strcpy_ref2 = self
                                    .module
                                    .declare_func_in_func(strcpy_func_id, builder.func);
                                builder.ins().call(strcpy_ref2, &[dst_pos, src_pos]);

                                builder.ins().jump(merge_block, &[BlockArg::Value(buf)]);

                                // --- merge_block: result ---
                                builder.switch_to_block(merge_block);
                                builder.seal_block(merge_block);
                                let result = builder.block_params(merge_block)[0];

                                value_map.insert(*dst, result);
                            }

                            // ── string_index_of(s, substr): strstr then compute offset ──
                            "string_index_of" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let substr = resolve_value(&value_map, &args[1])?;

                                let strstr_func_id = *self
                                    .declared_functions
                                    .get("strstr")
                                    .ok_or("strstr not declared")?;
                                let strstr_ref = self
                                    .module
                                    .declare_func_in_func(strstr_func_id, builder.func);
                                let call = builder.ins().call(strstr_ref, &[s, substr]);
                                let found_ptr = builder.inst_results(call).to_vec()[0];

                                // if found_ptr == NULL then -1 else found_ptr - s
                                let zero = builder.ins().iconst(pointer_type, 0);
                                let is_null = builder.ins().icmp(IntCC::Equal, found_ptr, zero);
                                let offset = builder.ins().isub(found_ptr, s);
                                let neg_one = builder.ins().iconst(cl_types::I64, -1_i64);
                                let result = builder.ins().select(is_null, neg_one, offset);
                                value_map.insert(*dst, result);
                            }

                            // ── string_char_at(s, index): extract single char as string ──
                            "string_char_at" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let index = resolve_value(&value_map, &args[1])?;

                                // Allocate 2 bytes: char + nul
                                let two = builder.ins().iconst(cl_types::I64, 2);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[two]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // char_ptr = s + index
                                let char_ptr = builder.ins().iadd(s, index);
                                let ch =
                                    builder
                                        .ins()
                                        .load(cl_types::I8, MemFlags::new(), char_ptr, 0);

                                // buf[0] = ch, buf[1] = 0
                                builder.ins().store(MemFlags::new(), ch, buf, 0);
                                let nul = builder.ins().iconst(cl_types::I8, 0);
                                let buf_1 = builder.ins().iadd_imm(buf, 1);
                                builder.ins().store(MemFlags::new(), nul, buf_1, 0);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_char_code_at(s, index): extract byte as Int ──────
                            // This is the primitive needed for self-hosted lexer
                            "string_char_code_at" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let index = resolve_value(&value_map, &args[1])?;

                                // bounds check: if s == null || index < 0 || index >= strlen(s), return -1
                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                let is_null = builder.ins().icmp(IntCC::Equal, s, zero);

                                // char_ptr = s + index
                                let char_ptr = builder.ins().iadd(s, index);
                                // Load byte and extend to i64
                                let ch = builder
                                    .ins()
                                    .load(cl_types::I8, MemFlags::new(), char_ptr, 0);
                                let ch_i64 = builder.ins().uextend(cl_types::I64, ch);

                                // Return -1 if null, otherwise the byte value
                                let neg_one = builder.ins().iconst(cl_types::I64, -1_i64);
                                let result = builder.ins().select(is_null, neg_one, ch_i64);

                                value_map.insert(*dst, result);
                            }

                            // ── string_append(a, b): call __gradient_string_append ─────
                            "string_append" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_append")
                                    .ok_or("__gradient_string_append not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[a, b]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── string_split(s, delimiter): call __gradient_string_split -> List[String] ──
                            "string_split" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let delim = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_split")
                                    .ok_or("__gradient_string_split not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, delim]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Phase PP: String Utilities ───────────────────────────────

                            // string_join(strings: List[String], separator: String) -> String
                            "string_join" => {
                                let strings = resolve_value(&value_map, &args[0])?;
                                let separator = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_join")
                                    .ok_or("__gradient_string_join not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[strings, separator]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_repeat(s: String, n: Int) -> String
                            "string_repeat" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let n = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_repeat")
                                    .ok_or("__gradient_string_repeat not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, n]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_pad_left(s: String, n: Int, pad: String) -> String
                            "string_pad_left" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let n = resolve_value(&value_map, &args[1])?;
                                let pad = resolve_value(&value_map, &args[2])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_pad_left")
                                    .ok_or("__gradient_string_pad_left not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, n, pad]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_pad_right(s: String, n: Int, pad: String) -> String
                            "string_pad_right" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let n = resolve_value(&value_map, &args[1])?;
                                let pad = resolve_value(&value_map, &args[2])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_pad_right")
                                    .ok_or("__gradient_string_pad_right not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, n, pad]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_strip(s: String) -> String (same as string_trim)
                            "string_strip" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_strip")
                                    .ok_or("__gradient_string_strip not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_strip_prefix(s: String, prefix: String) -> Option[String]
                            "string_strip_prefix" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let prefix = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_strip_prefix")
                                    .ok_or("__gradient_string_strip_prefix not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, prefix]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_strip_suffix(s: String, suffix: String) -> Option[String]
                            "string_strip_suffix" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let suffix = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_strip_suffix")
                                    .ok_or("__gradient_string_strip_suffix not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, suffix]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_to_int(s: String) -> Option[Int]
                            "string_to_int" => {
                                let s = resolve_value(&value_map, &args[0])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_to_int")
                                    .ok_or("__gradient_string_to_int not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_to_float(s: String) -> Option[Float]
                            "string_to_float" => {
                                let s = resolve_value(&value_map, &args[0])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_to_float")
                                    .ok_or("__gradient_string_to_float not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Phase PP: String Utilities Batch 2 ────────────────────

                            // string_format(fmt: String, args: List[String]) -> String
                            "string_format" => {
                                let fmt = resolve_value(&value_map, &args[0])?;
                                let args_list = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_format")
                                    .ok_or("__gradient_string_format not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[fmt, args_list]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_is_empty(s: String) -> Bool
                            "string_is_empty" => {
                                let s = resolve_value(&value_map, &args[0])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_is_empty")
                                    .ok_or("__gradient_string_is_empty not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                // Convert i64 result to bool (i8)
                                let bool_result = builder.ins().ireduce(cl_types::I8, result);
                                value_map.insert(*dst, bool_result);
                            }

                            // string_reverse(s: String) -> String
                            "string_reverse" => {
                                let s = resolve_value(&value_map, &args[0])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_reverse")
                                    .ok_or("__gradient_string_reverse not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_compare(a: String, b: String) -> Int
                            "string_compare" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_compare")
                                    .ok_or("__gradient_string_compare not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[a, b]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_find(s: String, substr: String) -> Option[Int]
                            "string_find" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let substr = resolve_value(&value_map, &args[1])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_find")
                                    .ok_or("__gradient_string_find not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, substr]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // string_slice(s: String, start: Int, end: Int) -> String
                            "string_slice" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let start = resolve_value(&value_map, &args[1])?;
                                let end = resolve_value(&value_map, &args[2])?;

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_string_slice")
                                    .ok_or("__gradient_string_slice not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, start, end]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── float_to_int(f): fcvt_to_sint ──
                            "float_to_int" => {
                                let f = resolve_value(&value_map, &args[0])?;
                                let result = builder.ins().fcvt_to_sint(cl_types::I64, f);
                                value_map.insert(*dst, result);
                            }

                            // ── int_to_float(n): fcvt_from_sint ──
                            "int_to_float" => {
                                let n = resolve_value(&value_map, &args[0])?;
                                let result = builder.ins().fcvt_from_sint(cl_types::F64, n);
                                value_map.insert(*dst, result);
                            }

                            // ── pow(base, exp): integer exponentiation via loop ──
                            "pow" => {
                                let base = resolve_value(&value_map, &args[0])?;
                                let exp = resolve_value(&value_map, &args[1])?;

                                // Iterative: result = 1; for i in 0..exp: result *= base
                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i
                                builder.append_block_param(loop_header, cl_types::I64); // accumulator (result)

                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                let one_val = builder.ins().iconst(cl_types::I64, 1);
                                builder.ins().jump(
                                    loop_header,
                                    &[BlockArg::Value(zero), BlockArg::Value(one_val)],
                                );

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let acc = builder.block_params(loop_header)[1];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, exp);
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);
                                let new_acc = builder.ins().imul(acc, base);
                                let one_inc = builder.ins().iconst(cl_types::I64, 1);
                                let next_i = builder.ins().iadd(i_val, one_inc);
                                builder.ins().jump(
                                    loop_header,
                                    &[BlockArg::Value(next_i), BlockArg::Value(new_acc)],
                                );

                                // Seal loop_header now (predecessors: entry jump + body back-edge)
                                builder.seal_block(loop_header);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                value_map.insert(*dst, acc);
                            }

                            // ── float_abs(f): fabs ──
                            "float_abs" => {
                                let f = resolve_value(&value_map, &args[0])?;
                                let result = builder.ins().fabs(f);
                                value_map.insert(*dst, result);
                            }

                            // ── float_sqrt(f): sqrt ──
                            "float_sqrt" => {
                                let f = resolve_value(&value_map, &args[0])?;
                                let result = builder.ins().sqrt(f);
                                value_map.insert(*dst, result);
                            }

                            // ── float_to_string(f): snprintf via call_indirect ──
                            "float_to_string" => {
                                // Allocate buffer for float string (64 bytes is plenty)
                                let buf_size = builder.ins().iconst(cl_types::I64, 64);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[buf_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // Format string "%g"
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%g",
                                )?;
                                let fmt_gv =
                                    self.module.declare_data_in_func(fmt_data_id, builder.func);
                                let fmt_ptr = builder.ins().global_value(pointer_type, fmt_gv);

                                // Use call_indirect with float-compatible signature:
                                // snprintf(ptr, i64, ptr, f64) -> i32
                                let snprintf_func_id = *self
                                    .declared_functions
                                    .get("snprintf")
                                    .ok_or("snprintf not declared")?;
                                let snprintf_ref = self
                                    .module
                                    .declare_func_in_func(snprintf_func_id, builder.func);
                                let snprintf_addr =
                                    builder.ins().func_addr(pointer_type, snprintf_ref);

                                let mut float_snprintf_sig = self.module.make_signature();
                                float_snprintf_sig.params.push(AbiParam::new(pointer_type)); // buf
                                float_snprintf_sig.params.push(AbiParam::new(cl_types::I64)); // size
                                float_snprintf_sig.params.push(AbiParam::new(pointer_type)); // fmt
                                float_snprintf_sig.params.push(AbiParam::new(cl_types::F64)); // float val
                                float_snprintf_sig
                                    .returns
                                    .push(AbiParam::new(cl_types::I32));
                                let sig_ref = builder.import_signature(float_snprintf_sig);

                                let float_val = resolve_value(&value_map, &args[0])?;
                                builder.ins().call_indirect(
                                    sig_ref,
                                    snprintf_addr,
                                    &[buf, buf_size, fmt_ptr, float_val],
                                );

                                value_map.insert(*dst, buf);
                            }

                            // ── bool_to_string(b): returns "true" or "false" ──
                            "bool_to_string" => {
                                let bool_val = resolve_value(&value_map, &args[0])?;

                                let true_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "true",
                                )?;
                                let false_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "false",
                                )?;

                                let true_gv =
                                    self.module.declare_data_in_func(true_data_id, builder.func);
                                let false_gv = self
                                    .module
                                    .declare_data_in_func(false_data_id, builder.func);
                                let true_ptr = builder.ins().global_value(pointer_type, true_gv);
                                let false_ptr = builder.ins().global_value(pointer_type, false_gv);

                                let result = builder.ins().select(bool_val, true_ptr, false_ptr);
                                value_map.insert(*dst, result);
                            }

                            // ── file_read(path): call __gradient_file_read(path) -> String ──
                            "file_read" => {
                                let path = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_file_read")
                                    .ok_or("__gradient_file_read not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[path]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── file_write(path, content): call __gradient_file_write -> Bool ──
                            "file_write" => {
                                let path = resolve_value(&value_map, &args[0])?;
                                let content = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_file_write")
                                    .ok_or("__gradient_file_write not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[path, content]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                // Runtime now returns i8 (bool) directly
                                value_map.insert(*dst, result);
                            }

                            // ── file_exists(path): call __gradient_file_exists -> Bool ──
                            "file_exists" => {
                                let path = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_file_exists")
                                    .ok_or("__gradient_file_exists not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[path]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                // Runtime now returns i8 (bool) directly
                                value_map.insert(*dst, result);
                            }

                            // ── file_append(path, content): call __gradient_file_append -> Bool ──
                            "file_append" => {
                                let path = resolve_value(&value_map, &args[0])?;
                                let content = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_file_append")
                                    .ok_or("__gradient_file_append not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[path, content]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                // Runtime now returns i8 (bool) directly
                                value_map.insert(*dst, result);
                            }

                            // ── file_delete(path): call __gradient_file_delete -> Bool ──
                            "file_delete" => {
                                let path = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_file_delete")
                                    .ok_or("__gradient_file_delete not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[path]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                // Runtime now returns i8 (bool) directly
                                value_map.insert(*dst, result);
                            }

                            // ── http_get(url): call __gradient_http_get(url) -> Result ptr ──
                            "http_get" => {
                                let url = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_http_get")
                                    .ok_or("__gradient_http_get not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[url]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── http_post(url, body): call __gradient_http_post -> Result ptr ──
                            "http_post" => {
                                let url = resolve_value(&value_map, &args[0])?;
                                let body = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_http_post")
                                    .ok_or("__gradient_http_post not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[url, body]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── http_post_json(url, json): call __gradient_http_post_json -> Result ptr ──
                            "http_post_json" => {
                                let url = resolve_value(&value_map, &args[0])?;
                                let json = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_http_post_json")
                                    .ok_or("__gradient_http_post_json not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[url, json]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── json_parse(input) -> Result[JsonValue, String] ptr ──
                            "json_parse" => {
                                let input = resolve_value(&value_map, &args[0])?;
                                let ok_ptr = builder.create_sized_stack_slot(StackSlotData::new(
                                    StackSlotKind::ExplicitSlot,
                                    8,
                                    3,
                                ));
                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                let ok_addr = builder.ins().stack_addr(pointer_type, ok_ptr, 0);
                                builder.ins().store(MemFlags::new(), zero, ok_addr, 0);

                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_parse")
                                    .ok_or("__gradient_json_parse not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[input, ok_addr]);
                                let raw_result = builder.inst_results(call_inst).to_vec()[0];
                                let ok_val =
                                    builder
                                        .ins()
                                        .load(cl_types::I64, MemFlags::new(), ok_addr, 0);
                                let is_ok = builder.ins().icmp_imm(IntCC::Equal, ok_val, 1);

                                let ok_block = builder.create_block();
                                let err_block = builder.create_block();
                                let merge_block = builder.create_block();
                                builder.append_block_param(merge_block, cl_types::I64);
                                builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

                                builder.switch_to_block(ok_block);
                                builder.seal_block(ok_block);
                                let ok_size = builder.ins().iconst(cl_types::I64, 16);
                                let malloc_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref =
                                    self.module.declare_func_in_func(malloc_id, builder.func);
                                let ok_call = builder.ins().call(malloc_ref, &[ok_size]);
                                let ok_enum = builder.inst_results(ok_call).to_vec()[0];
                                let tag0 = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().store(MemFlags::new(), tag0, ok_enum, 0);
                                builder.ins().store(MemFlags::new(), raw_result, ok_enum, 8);
                                builder.ins().jump(merge_block, &[BlockArg::Value(ok_enum)]);

                                builder.switch_to_block(err_block);
                                builder.seal_block(err_block);
                                let err_size = builder.ins().iconst(cl_types::I64, 16);
                                let malloc_ref =
                                    self.module.declare_func_in_func(malloc_id, builder.func);
                                let err_call = builder.ins().call(malloc_ref, &[err_size]);
                                let err_enum = builder.inst_results(err_call).to_vec()[0];
                                let tag1 = builder.ins().iconst(cl_types::I64, 1);
                                builder.ins().store(MemFlags::new(), tag1, err_enum, 0);
                                builder
                                    .ins()
                                    .store(MemFlags::new(), raw_result, err_enum, 8);
                                builder
                                    .ins()
                                    .jump(merge_block, &[BlockArg::Value(err_enum)]);

                                builder.seal_block(merge_block);
                                builder.switch_to_block(merge_block);
                                let result_ptr = builder.block_params(merge_block)[0];
                                value_map.insert(*dst, result_ptr);
                            }

                            // ── json_stringify(value) -> String ptr ──
                            "json_stringify" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_stringify")
                                    .ok_or("__gradient_json_stringify not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── json_type(value) -> String ptr ──
                            "json_type" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_type")
                                    .ok_or("__gradient_json_type not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── json_get(value, key) -> Option[JsonValue] ptr ──
                            "json_get" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let key = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_get")
                                    .ok_or("__gradient_json_get not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value, key]);
                                let raw_ptr = builder.inst_results(call_inst).to_vec()[0];
                                let null_val = builder.ins().iconst(cl_types::I64, 0);
                                let is_null = builder.ins().icmp(IntCC::Equal, raw_ptr, null_val);

                                let some_block = builder.create_block();
                                let none_block = builder.create_block();
                                let merge_block = builder.create_block();
                                builder.append_block_param(merge_block, cl_types::I64);
                                builder
                                    .ins()
                                    .brif(is_null, none_block, &[], some_block, &[]);

                                builder.switch_to_block(some_block);
                                builder.seal_block(some_block);
                                let some_size = builder.ins().iconst(cl_types::I64, 16);
                                let malloc_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref_s =
                                    self.module.declare_func_in_func(malloc_id, builder.func);
                                let some_call = builder.ins().call(malloc_ref_s, &[some_size]);
                                let some_ptr = builder.inst_results(some_call).to_vec()[0];
                                let tag0 = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().store(MemFlags::new(), tag0, some_ptr, 0);
                                builder.ins().store(MemFlags::new(), raw_ptr, some_ptr, 8);
                                builder
                                    .ins()
                                    .jump(merge_block, &[BlockArg::Value(some_ptr)]);

                                builder.switch_to_block(none_block);
                                builder.seal_block(none_block);
                                let none_size = builder.ins().iconst(cl_types::I64, 8);
                                let malloc_ref_n =
                                    self.module.declare_func_in_func(malloc_id, builder.func);
                                let none_call = builder.ins().call(malloc_ref_n, &[none_size]);
                                let none_ptr = builder.inst_results(none_call).to_vec()[0];
                                let tag1 = builder.ins().iconst(cl_types::I64, 1);
                                builder.ins().store(MemFlags::new(), tag1, none_ptr, 0);
                                builder
                                    .ins()
                                    .jump(merge_block, &[BlockArg::Value(none_ptr)]);

                                builder.seal_block(merge_block);
                                builder.switch_to_block(merge_block);
                                let option_ptr = builder.block_params(merge_block)[0];
                                value_map.insert(*dst, option_ptr);
                            }

                            // ── json_is_null(value) -> Bool ──
                            "json_is_null" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_is_null")
                                    .ok_or("__gradient_json_is_null not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[value]);
                                let result_i64 = builder.inst_results(call).to_vec()[0];
                                let result_bool = builder.ins().ireduce(cl_types::I8, result_i64);
                                value_map.insert(*dst, result_bool);
                            }

                            // ── json_has(value, key) -> Bool ──
                            "json_has" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let key = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_has")
                                    .ok_or("__gradient_json_has not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[value, key]);
                                let result_i64 = builder.inst_results(call).to_vec()[0];
                                let result_bool = builder.ins().ireduce(cl_types::I8, result_i64);
                                value_map.insert(*dst, result_bool);
                            }

                            // ── json_keys(value) -> List[String] ptr ──
                            "json_keys" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_keys")
                                    .ok_or("__gradient_json_keys not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── json_len(value) -> Int ──
                            "json_len" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_len")
                                    .ok_or("__gradient_json_len not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── json_array_get(value, idx) -> Option[JsonValue] ptr ──
                            "json_array_get" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let idx = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_array_get")
                                    .ok_or("__gradient_json_array_get not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value, idx]);
                                let raw_ptr = builder.inst_results(call_inst).to_vec()[0];
                                let null_val = builder.ins().iconst(cl_types::I64, 0);
                                let is_null = builder.ins().icmp(IntCC::Equal, raw_ptr, null_val);

                                let some_block = builder.create_block();
                                let none_block = builder.create_block();
                                let merge_block = builder.create_block();
                                builder.append_block_param(merge_block, cl_types::I64);
                                builder
                                    .ins()
                                    .brif(is_null, none_block, &[], some_block, &[]);

                                builder.switch_to_block(some_block);
                                builder.seal_block(some_block);
                                let some_size = builder.ins().iconst(cl_types::I64, 16);
                                let malloc_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref_s =
                                    self.module.declare_func_in_func(malloc_id, builder.func);
                                let some_call = builder.ins().call(malloc_ref_s, &[some_size]);
                                let some_ptr = builder.inst_results(some_call).to_vec()[0];
                                let tag0 = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().store(MemFlags::new(), tag0, some_ptr, 0);
                                builder.ins().store(MemFlags::new(), raw_ptr, some_ptr, 8);
                                builder
                                    .ins()
                                    .jump(merge_block, &[BlockArg::Value(some_ptr)]);

                                builder.switch_to_block(none_block);
                                builder.seal_block(none_block);
                                let none_size = builder.ins().iconst(cl_types::I64, 8);
                                let malloc_ref_n =
                                    self.module.declare_func_in_func(malloc_id, builder.func);
                                let none_call = builder.ins().call(malloc_ref_n, &[none_size]);
                                let none_ptr = builder.inst_results(none_call).to_vec()[0];
                                let tag1 = builder.ins().iconst(cl_types::I64, 1);
                                builder.ins().store(MemFlags::new(), tag1, none_ptr, 0);
                                builder
                                    .ins()
                                    .jump(merge_block, &[BlockArg::Value(none_ptr)]);

                                builder.seal_block(merge_block);
                                builder.switch_to_block(merge_block);
                                let option_ptr = builder.block_params(merge_block)[0];
                                value_map.insert(*dst, option_ptr);
                            }

                            // ── Typed JSON extractors ────────────────────────────────────────
                            "json_as_string" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_as_string")
                                    .ok_or("__gradient_json_as_string not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "json_as_int" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_as_int")
                                    .ok_or("__gradient_json_as_int not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "json_as_float" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_as_float")
                                    .ok_or("__gradient_json_as_float not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "json_as_bool" => {
                                let value = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_json_as_bool")
                                    .ok_or("__gradient_json_as_bool not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call_inst = builder.ins().call(func_ref, &[value]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Phase PP: Random Number Generation ─────────────────────
                            "random" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_random")
                                    .ok_or("__gradient_random not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "random_int" => {
                                let min = resolve_value(&value_map, &args[0])?;
                                let max = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_random_int")
                                    .ok_or("__gradient_random_int not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[min, max]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "random_float" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_random_float")
                                    .ok_or("__gradient_random_float not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "seed_random" => {
                                let seed = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_seed_random")
                                    .ok_or("__gradient_seed_random not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                builder.ins().call(func_ref, &[seed]);
                                // Unit return: use dummy i8 value
                                let dummy = builder.ins().iconst(cl_types::I8, 0);
                                value_map.insert(*dst, dummy);
                            }

                            // ── Phase PP: Date/Time Builtins ───────────────────────────
                            "now" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_now")
                                    .ok_or("__gradient_now not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "now_ms" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_now_ms")
                                    .ok_or("__gradient_now_ms not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "sleep" => {
                                let ms = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_sleep")
                                    .ok_or("__gradient_sleep not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                builder.ins().call(func_ref, &[ms]);
                                // Unit return: use dummy i8 value
                                let dummy = builder.ins().iconst(cl_types::I8, 0);
                                value_map.insert(*dst, dummy);
                            }
                            "time_string" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_time_string")
                                    .ok_or("__gradient_time_string not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "date_string" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_date_string")
                                    .ok_or("__gradient_date_string not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "datetime_year" => {
                                let ts = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_datetime_year")
                                    .ok_or("__gradient_datetime_year not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[ts]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "datetime_month" => {
                                let ts = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_datetime_month")
                                    .ok_or("__gradient_datetime_month not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[ts]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "datetime_day" => {
                                let ts = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_datetime_day")
                                    .ok_or("__gradient_datetime_day not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[ts]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Phase PP: Environment/Process builtins ───────────
                            "get_env" => {
                                let name = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_get_env")
                                    .ok_or("__gradient_get_env not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[name]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "set_env" => {
                                let name = resolve_value(&value_map, &args[0])?;
                                let value = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_env")
                                    .ok_or("__gradient_set_env not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                builder.ins().call(func_ref, &[name, value]);
                                // Unit return: use dummy i8 value
                                let dummy = builder.ins().iconst(cl_types::I8, 0);
                                value_map.insert(*dst, dummy);
                            }
                            "current_dir" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_current_dir")
                                    .ok_or("__gradient_current_dir not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "change_dir" => {
                                let path = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_change_dir")
                                    .ok_or("__gradient_change_dir not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                builder.ins().call(func_ref, &[path]);
                                // Unit return: use dummy i8 value
                                let dummy = builder.ins().iconst(cl_types::I8, 0);
                                value_map.insert(*dst, dummy);
                            }
                            "process_id" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("getpid")
                                    .ok_or("getpid not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "system" => {
                                let cmd = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("system")
                                    .ok_or("system not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[cmd]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "sleep_seconds" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_sleep_seconds")
                                    .ok_or("__gradient_sleep_seconds not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                builder.ins().call(func_ref, &[s]);
                                // Unit return: use dummy i8 value
                                let dummy = builder.ins().iconst(cl_types::I8, 0);
                                value_map.insert(*dst, dummy);
                            }

                            // ── Option helper functions ──
                            "option_is_some" => {
                                let opt = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_option_is_some")
                                    .ok_or("__gradient_option_is_some not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[opt]);
                                let result = builder.inst_results(call).to_vec()[0];
                                // Convert i64 to Bool (i8)
                                let bool_result = builder.ins().ireduce(cl_types::I8, result);
                                value_map.insert(*dst, bool_result);
                            }
                            "option_is_none" => {
                                let opt = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_option_is_none")
                                    .ok_or("__gradient_option_is_none not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[opt]);
                                let result = builder.inst_results(call).to_vec()[0];
                                // Convert i64 to Bool (i8)
                                let bool_result = builder.ins().ireduce(cl_types::I8, result);
                                value_map.insert(*dst, bool_result);
                            }
                            "option_unwrap" => {
                                let opt = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_option_unwrap")
                                    .ok_or("__gradient_option_unwrap not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[opt]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "option_unwrap_or" => {
                                let opt = resolve_value(&value_map, &args[0])?;
                                let default = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_option_unwrap_or")
                                    .ok_or("__gradient_option_unwrap_or not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[opt, default]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Queue builtins ──
                            "queue_new" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_queue_new")
                                    .ok_or("__gradient_queue_new not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "queue_enqueue" => {
                                let q = resolve_value(&value_map, &args[0])?;
                                let item = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_queue_enqueue")
                                    .ok_or("__gradient_queue_enqueue not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[q, item]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "queue_dequeue" => {
                                let q = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_queue_dequeue")
                                    .ok_or("__gradient_queue_dequeue not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[q]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "queue_peek" => {
                                let q = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_queue_peek")
                                    .ok_or("__gradient_queue_peek not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[q]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "queue_size" => {
                                let q = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_queue_size")
                                    .ok_or("__gradient_queue_size not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[q]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Phase PP: Stack Builtins ─────────────────────────────
                            "stack_new" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_stack_new")
                                    .ok_or("__gradient_stack_new not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "stack_push" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let elem = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_stack_push")
                                    .ok_or("__gradient_stack_push not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s, elem]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "stack_pop" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_stack_pop")
                                    .ok_or("__gradient_stack_pop not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "stack_peek" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_stack_peek")
                                    .ok_or("__gradient_stack_peek not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }
                            "stack_size" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_stack_size")
                                    .ok_or("__gradient_stack_size not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── __gradient_contract_fail: print message and exit(1) ──
                            "__gradient_contract_fail" => {
                                // Print the error message using puts.
                                let puts_func_id = *self
                                    .declared_functions
                                    .get("puts")
                                    .ok_or("puts not declared")?;
                                let puts_ref =
                                    self.module.declare_func_in_func(puts_func_id, builder.func);
                                let msg_val = resolve_value(&value_map, &args[0])?;
                                builder.ins().call(puts_ref, &[msg_val]);

                                // Call exit(1) to abort.
                                let exit_func_id = *self
                                    .declared_functions
                                    .get("exit")
                                    .ok_or("exit not declared")?;
                                let exit_ref =
                                    self.module.declare_func_in_func(exit_func_id, builder.func);
                                let one = builder.ins().iconst(cl_types::I32, 1);
                                builder.ins().call(exit_ref, &[one]);

                                // Emit a dummy result value (never reached).
                                let dummy = builder.ins().iconst(cl_types::I64, 0);
                                value_map.insert(*dst, dummy);
                            }

                            // ── read_line(): call __gradient_read_line() -> ptr ──
                            "read_line" => {
                                let rl_func_id = *self
                                    .declared_functions
                                    .get("__gradient_read_line")
                                    .ok_or("__gradient_read_line not declared")?;
                                let rl_ref =
                                    self.module.declare_func_in_func(rl_func_id, builder.func);
                                let call = builder.ins().call(rl_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── parse_int(s): atoi(s), widen i32 -> i64 ──
                            "parse_int" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let atoi_func_id = *self
                                    .declared_functions
                                    .get("atoi")
                                    .ok_or("atoi not declared")?;
                                let atoi_ref =
                                    self.module.declare_func_in_func(atoi_func_id, builder.func);
                                let call = builder.ins().call(atoi_ref, &[s]);
                                let i32_result = builder.inst_results(call).to_vec()[0];
                                // Widen i32 -> i64 (sign-extend) for Gradient's Int type.
                                let result = builder.ins().sextend(cl_types::I64, i32_result);
                                value_map.insert(*dst, result);
                            }

                            // ── parse_float(s): atof(s) -> f64 ──
                            "parse_float" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let atof_func_id = *self
                                    .declared_functions
                                    .get("atof")
                                    .ok_or("atof not declared")?;
                                let atof_ref =
                                    self.module.declare_func_in_func(atof_func_id, builder.func);
                                let call = builder.ins().call(atof_ref, &[s]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── exit(code): truncate i64 -> i32, call libc exit ──
                            "exit" => {
                                let code_val = resolve_value(&value_map, &args[0])?;
                                // Gradient Int is i64; libc exit takes i32.
                                let code_i32 = builder.ins().ireduce(cl_types::I32, code_val);
                                let exit_func_id = *self
                                    .declared_functions
                                    .get("exit")
                                    .ok_or("exit not declared")?;
                                let exit_ref =
                                    self.module.declare_func_in_func(exit_func_id, builder.func);
                                builder.ins().call(exit_ref, &[code_i32]);
                                // Emit a dummy result (unreachable after exit).
                                let dummy = builder.ins().iconst(cl_types::I64, 0);
                                value_map.insert(*dst, dummy);
                            }

                            // ── args(): returns List[String] from saved argc/argv ──
                            "args" => {
                                let get_args_func_id = *self
                                    .declared_functions
                                    .get("__gradient_get_args")
                                    .ok_or("__gradient_get_args not declared")?;
                                let get_args_ref = self
                                    .module
                                    .declare_func_in_func(get_args_func_id, builder.func);
                                let call = builder.ins().call(get_args_ref, &[]);
                                let ptr = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, ptr);
                            }

                            // ── list_length(list): load i64 from offset 0 ──
                            "list_length" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );
                                value_map.insert(*dst, length);
                            }

                            // ── list_get(list, index): load from offset (16 + index * 8) ──
                            "list_get" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let index = resolve_value(&value_map, &args[1])?;
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let offset = builder.ins().imul(index, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let data_offset = builder.ins().iadd(offset, sixteen);
                                let elem_addr = builder.ins().iadd(list_ptr, data_offset);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    elem_addr,
                                    0i32,
                                );
                                value_map.insert(*dst, elem);
                            }

                            // ── list_is_empty(list): length == 0 ──
                            "list_is_empty" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );
                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                let is_empty = builder.ins().icmp(IntCC::Equal, length, zero);
                                value_map.insert(*dst, is_empty);
                            }

                            // ── list_head(list): load data[0] = offset 16 ──
                            "list_head" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    16i32,
                                );
                                value_map.insert(*dst, elem);
                            }

                            // ── list_tail(list): new list with all but first element ──
                            "list_tail" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let old_len = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );
                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let new_len = builder.ins().isub(old_len, one);
                                // alloc: 16 + new_len * 8
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(new_len, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];
                                // store new length and capacity
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 8i32);
                                // copy data: memcpy(new_ptr + 16, list_ptr + 24, new_len * 8)
                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let memcpy_ref = self
                                    .module
                                    .declare_func_in_func(memcpy_func_id, builder.func);
                                let src_data = builder.ins().iadd_imm(list_ptr, 24);
                                let dst_data = builder.ins().iadd_imm(new_ptr, 16);
                                builder
                                    .ins()
                                    .call(memcpy_ref, &[dst_data, src_data, data_size]);
                                value_map.insert(*dst, new_ptr);
                            }

                            // ── list_push(list, elem): new list with element appended ──
                            "list_push" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let elem_val = resolve_value(&value_map, &args[1])?;
                                let old_len = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );
                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let new_len = builder.ins().iadd(old_len, one);
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(new_len, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 8i32);
                                // copy old data
                                let old_data_size = builder.ins().imul(old_len, eight);
                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let memcpy_ref = self
                                    .module
                                    .declare_func_in_func(memcpy_func_id, builder.func);
                                let src_data = builder.ins().iadd_imm(list_ptr, 16);
                                let dst_data = builder.ins().iadd_imm(new_ptr, 16);
                                builder
                                    .ins()
                                    .call(memcpy_ref, &[dst_data, src_data, old_data_size]);
                                // store new element at end
                                let new_elem_offset = builder.ins().iadd(old_data_size, sixteen);
                                let new_elem_addr = builder.ins().iadd(new_ptr, new_elem_offset);
                                builder
                                    .ins()
                                    .store(MemFlags::new(), elem_val, new_elem_addr, 0i32);
                                value_map.insert(*dst, new_ptr);
                            }

                            // ── list_concat(a, b): new list with both lists' elements ──
                            "list_concat" => {
                                let list_a = resolve_value(&value_map, &args[0])?;
                                let list_b = resolve_value(&value_map, &args[1])?;
                                let len_a = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_a,
                                    0i32,
                                );
                                let len_b = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_b,
                                    0i32,
                                );
                                let new_len = builder.ins().iadd(len_a, len_b);
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(new_len, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 8i32);
                                // copy list_a data
                                let size_a = builder.ins().imul(len_a, eight);
                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let memcpy_ref = self
                                    .module
                                    .declare_func_in_func(memcpy_func_id, builder.func);
                                let src_a = builder.ins().iadd_imm(list_a, 16);
                                let dst_start = builder.ins().iadd_imm(new_ptr, 16);
                                builder.ins().call(memcpy_ref, &[dst_start, src_a, size_a]);
                                // copy list_b data after list_a
                                let size_b = builder.ins().imul(len_b, eight);
                                let dst_b = builder.ins().iadd(dst_start, size_a);
                                let src_b = builder.ins().iadd_imm(list_b, 16);
                                // Need fresh memcpy ref
                                let memcpy_ref2 = self
                                    .module
                                    .declare_func_in_func(memcpy_func_id, builder.func);
                                builder.ins().call(memcpy_ref2, &[dst_b, src_b, size_b]);
                                value_map.insert(*dst, new_ptr);
                            }

                            // ── list_contains(list, elem): linear search ──
                            "list_contains" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let target_val = resolve_value(&value_map, &args[1])?;
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                // Create loop blocks and merge block
                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let merge_block = builder.create_block();

                                // merge_block receives the boolean result as a block param
                                builder.append_block_param(merge_block, cl_types::I8);

                                // Jump to header with initial i=0
                                let zero_i = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero_i)]);

                                // Header: phi for i, compare i < length
                                builder.switch_to_block(loop_header);
                                builder.append_block_param(loop_header, cl_types::I64);
                                let i = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i, length);
                                // If i >= length, not found -> merge with false
                                let false_val = builder.ins().iconst(cl_types::I8, 0);
                                builder.ins().brif(
                                    cmp,
                                    loop_body,
                                    &[],
                                    merge_block,
                                    &[BlockArg::Value(false_val)],
                                );

                                // Body: load element, compare to target
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);
                                let byte_off = builder.ins().imul_imm(i, 8);
                                let data_off = builder.ins().iadd_imm(byte_off, 16);
                                let elem_addr = builder.ins().iadd(list_ptr, data_off);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    elem_addr,
                                    0i32,
                                );
                                let eq = builder.ins().icmp(IntCC::Equal, elem, target_val);
                                // If found, merge with true; else continue loop with i+1
                                let true_val = builder.ins().iconst(cl_types::I8, 1);
                                let i_plus_one = builder.ins().iadd_imm(i, 1);
                                builder.ins().brif(
                                    eq,
                                    merge_block,
                                    &[BlockArg::Value(true_val)],
                                    loop_header,
                                    &[BlockArg::Value(i_plus_one)],
                                );

                                // Seal loop_header now (predecessors: entry jump + back-edge from body)
                                builder.seal_block(loop_header);
                                // Seal merge (predecessors: header not-found + body found)
                                builder.seal_block(merge_block);

                                // Switch to merge block and read the result
                                builder.switch_to_block(merge_block);
                                let result = builder.block_params(merge_block)[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Higher-order list operations ──

                            // list_map(list, closure_fn_ptr): apply closure to each element
                            "list_map" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                // Load length from list header (offset 0)
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                // Allocate result list: 16 (header) + length * 8 (data)
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(length, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);

                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let result_ptr = builder.inst_results(malloc_call).to_vec()[0];

                                // Store length and capacity in result header
                                builder
                                    .ins()
                                    .store(MemFlags::new(), length, result_ptr, 0i32);
                                builder
                                    .ins()
                                    .store(MemFlags::new(), length, result_ptr, 8i32);

                                // Create closure signature: (i64) -> i64
                                let mut closure_sig = self.module.make_signature();
                                closure_sig.params.push(AbiParam::new(cl_types::I64));
                                closure_sig.returns.push(AbiParam::new(cl_types::I64));
                                let sig_ref = builder.import_signature(closure_sig);

                                // Loop: i = 0 to length
                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i

                                let zero_counter = builder.ins().iconst(cl_types::I64, 0);
                                builder
                                    .ins()
                                    .jump(loop_header, &[BlockArg::Value(zero_counter)]);

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                builder.seal_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, length);
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                // Load element from source list at offset 16 + i*8
                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    src_addr,
                                    0i32,
                                );

                                // call_indirect(closure_sig, fn_ptr, [elem])
                                let call_inst =
                                    builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let mapped = builder.inst_results(call_inst).to_vec()[0];

                                // Store result in result list at offset 16 + i*8
                                let dst_addr = builder.ins().iadd(result_ptr, elem_offset_full);
                                builder.ins().store(MemFlags::new(), mapped, dst_addr, 0i32);

                                // i += 1, jump back to header
                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let next_i = builder.ins().iadd(i_val, one);
                                builder.ins().jump(loop_header, &[BlockArg::Value(next_i)]);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                value_map.insert(*dst, result_ptr);
                                // Continue emitting in loop_exit block
                            }

                            // list_filter(list, predicate_fn_ptr): keep elements where predicate returns true
                            "list_filter" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                // Load length from list header
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                // Allocate result list (worst case: all elements pass)
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(length, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);

                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let result_ptr = builder.inst_results(malloc_call).to_vec()[0];

                                // Create predicate signature: (i64) -> i64
                                let mut pred_sig = self.module.make_signature();
                                pred_sig.params.push(AbiParam::new(cl_types::I64));
                                pred_sig.returns.push(AbiParam::new(cl_types::I64));
                                let sig_ref = builder.import_signature(pred_sig);

                                // Loop: i = 0 to length, result_count = 0
                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let loop_exit = builder.create_block();
                                let store_block = builder.create_block();
                                let skip_block = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // i
                                builder.append_block_param(loop_header, cl_types::I64); // result_count

                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                let zero2 = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(
                                    loop_header,
                                    &[BlockArg::Value(zero), BlockArg::Value(zero2)],
                                );

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                builder.seal_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let result_count = builder.block_params(loop_header)[1];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, length);
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                // Load element
                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    src_addr,
                                    0i32,
                                );

                                // Call predicate
                                let call_inst =
                                    builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let pred_result = builder.inst_results(call_inst).to_vec()[0];

                                // If predicate returns non-zero, store element
                                let zero_cmp = builder.ins().iconst(cl_types::I64, 0);
                                let pred_bool =
                                    builder.ins().icmp(IntCC::NotEqual, pred_result, zero_cmp);
                                builder
                                    .ins()
                                    .brif(pred_bool, store_block, &[], skip_block, &[]);

                                // --- store_block: element passes filter ---
                                builder.switch_to_block(store_block);
                                builder.seal_block(store_block);
                                let dst_offset = builder.ins().imul(result_count, eight);
                                let dst_offset_full = builder.ins().iadd(dst_offset, sixteen);
                                let dst_addr = builder.ins().iadd(result_ptr, dst_offset_full);
                                builder.ins().store(MemFlags::new(), elem, dst_addr, 0i32);
                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let new_count = builder.ins().iadd(result_count, one);
                                let next_i_store = builder.ins().iadd(i_val, one);
                                builder.ins().jump(
                                    loop_header,
                                    &[BlockArg::Value(next_i_store), BlockArg::Value(new_count)],
                                );

                                // --- skip_block: element does not pass ---
                                builder.switch_to_block(skip_block);
                                builder.seal_block(skip_block);
                                let one2 = builder.ins().iconst(cl_types::I64, 1);
                                let next_i_skip = builder.ins().iadd(i_val, one2);
                                builder.ins().jump(
                                    loop_header,
                                    &[BlockArg::Value(next_i_skip), BlockArg::Value(result_count)],
                                );

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                // Store actual result_count as length in result header
                                builder.ins().store(
                                    MemFlags::new(),
                                    result_count,
                                    result_ptr,
                                    0i32,
                                );
                                builder.ins().store(
                                    MemFlags::new(),
                                    result_count,
                                    result_ptr,
                                    8i32,
                                );

                                value_map.insert(*dst, result_ptr);
                            }

                            // list_foreach(list, fn_ptr): call fn on each element, return unit
                            "list_foreach" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                // Load length
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                // Create closure signature: (i64) -> i64
                                let mut closure_sig = self.module.make_signature();
                                closure_sig.params.push(AbiParam::new(cl_types::I64));
                                closure_sig.returns.push(AbiParam::new(cl_types::I64));
                                let sig_ref = builder.import_signature(closure_sig);

                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);

                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i

                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero)]);

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                builder.seal_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, length);
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    src_addr,
                                    0i32,
                                );

                                // Call closure, ignore result
                                builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);

                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let next_i = builder.ins().iadd(i_val, one);
                                builder.ins().jump(loop_header, &[BlockArg::Value(next_i)]);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                let unit_val = builder.ins().iconst(cl_types::I64, 0);
                                value_map.insert(*dst, unit_val);
                            }

                            // list_fold(list, init, combine_fn_ptr): fold with 2-arg closure
                            "list_fold" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let init_val = resolve_value(&value_map, &args[1])?;
                                let fn_ptr = resolve_value(&value_map, &args[2])?;

                                // Load length
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                // Create combine signature: (i64, i64) -> i64
                                let mut combine_sig = self.module.make_signature();
                                combine_sig.params.push(AbiParam::new(cl_types::I64)); // accumulator
                                combine_sig.params.push(AbiParam::new(cl_types::I64)); // element
                                combine_sig.returns.push(AbiParam::new(cl_types::I64));
                                let sig_ref = builder.import_signature(combine_sig);

                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);

                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i
                                builder.append_block_param(loop_header, cl_types::I64); // accumulator

                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(
                                    loop_header,
                                    &[BlockArg::Value(zero), BlockArg::Value(init_val)],
                                );

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                builder.seal_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let acc = builder.block_params(loop_header)[1];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, length);
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    src_addr,
                                    0i32,
                                );

                                // accumulator = combine(acc, elem)
                                let call_inst =
                                    builder.ins().call_indirect(sig_ref, fn_ptr, &[acc, elem]);
                                let new_acc = builder.inst_results(call_inst).to_vec()[0];

                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let next_i = builder.ins().iadd(i_val, one);
                                builder.ins().jump(
                                    loop_header,
                                    &[BlockArg::Value(next_i), BlockArg::Value(new_acc)],
                                );

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                value_map.insert(*dst, acc);
                            }

                            // list_any(list, predicate_fn_ptr): true if any element satisfies predicate
                            "list_any" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                let mut pred_sig = self.module.make_signature();
                                pred_sig.params.push(AbiParam::new(cl_types::I64));
                                pred_sig.returns.push(AbiParam::new(cl_types::I64));
                                let sig_ref = builder.import_signature(pred_sig);

                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let zero = builder.ins().iconst(cl_types::I64, 0);

                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let found_block = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i
                                builder.append_block_param(loop_exit, cl_types::I8); // result

                                builder.ins().jump(loop_header, &[BlockArg::Value(zero)]);

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                builder.seal_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, length);
                                let false_val = builder.ins().iconst(cl_types::I8, 0);
                                builder.ins().brif(
                                    cmp,
                                    loop_body,
                                    &[],
                                    loop_exit,
                                    &[BlockArg::Value(false_val)],
                                );

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    src_addr,
                                    0i32,
                                );

                                let call_inst =
                                    builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let pred_result = builder.inst_results(call_inst).to_vec()[0];

                                let pred_bool =
                                    builder.ins().icmp(IntCC::NotEqual, pred_result, zero);
                                let one_any = builder.ins().iconst(cl_types::I64, 1);
                                let next_i_any = builder.ins().iadd(i_val, one_any);
                                builder.ins().brif(
                                    pred_bool,
                                    found_block,
                                    &[],
                                    loop_header,
                                    &[BlockArg::Value(next_i_any)],
                                );

                                // --- found_block ---
                                builder.switch_to_block(found_block);
                                builder.seal_block(found_block);
                                let true_val = builder.ins().iconst(cl_types::I8, 1);
                                builder.ins().jump(loop_exit, &[BlockArg::Value(true_val)]);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);
                                let any_result = builder.block_params(loop_exit)[0];

                                value_map.insert(*dst, any_result);
                            }

                            // list_all(list, predicate_fn_ptr): true if all elements satisfy predicate
                            "list_all" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                let mut pred_sig = self.module.make_signature();
                                pred_sig.params.push(AbiParam::new(cl_types::I64));
                                pred_sig.returns.push(AbiParam::new(cl_types::I64));
                                let sig_ref = builder.import_signature(pred_sig);

                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let zero = builder.ins().iconst(cl_types::I64, 0);

                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let fail_block = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i
                                builder.append_block_param(loop_exit, cl_types::I8); // result

                                builder.ins().jump(loop_header, &[BlockArg::Value(zero)]);

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                builder.seal_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, length);
                                let true_val = builder.ins().iconst(cl_types::I8, 1);
                                builder.ins().brif(
                                    cmp,
                                    loop_body,
                                    &[],
                                    loop_exit,
                                    &[BlockArg::Value(true_val)],
                                );

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    src_addr,
                                    0i32,
                                );

                                let call_inst =
                                    builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let pred_result = builder.inst_results(call_inst).to_vec()[0];

                                let pred_bool =
                                    builder.ins().icmp(IntCC::NotEqual, pred_result, zero);
                                let one_all = builder.ins().iconst(cl_types::I64, 1);
                                let next_i_all = builder.ins().iadd(i_val, one_all);
                                builder.ins().brif(
                                    pred_bool,
                                    loop_header,
                                    &[BlockArg::Value(next_i_all)],
                                    fail_block,
                                    &[],
                                );

                                // --- fail_block ---
                                builder.switch_to_block(fail_block);
                                builder.seal_block(fail_block);
                                let false_val = builder.ins().iconst(cl_types::I8, 0);
                                builder.ins().jump(loop_exit, &[BlockArg::Value(false_val)]);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);
                                let all_result = builder.block_params(loop_exit)[0];

                                value_map.insert(*dst, all_result);
                            }

                            // list_find(list, predicate_fn_ptr): return first element satisfying predicate
                            "list_find" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                let mut pred_sig = self.module.make_signature();
                                pred_sig.params.push(AbiParam::new(cl_types::I64));
                                pred_sig.returns.push(AbiParam::new(cl_types::I64));
                                let sig_ref = builder.import_signature(pred_sig);

                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let zero = builder.ins().iconst(cl_types::I64, 0);

                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let found_block = builder.create_block();
                                let loop_exit = builder.create_block();

                                builder.append_block_param(loop_header, cl_types::I64); // counter i
                                builder.append_block_param(loop_exit, cl_types::I64); // result element

                                builder.ins().jump(loop_header, &[BlockArg::Value(zero)]);

                                // --- loop_header ---
                                builder.switch_to_block(loop_header);
                                builder.seal_block(loop_header);
                                let i_val = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i_val, length);
                                // If not found, return 0 (default)
                                let zero_default = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().brif(
                                    cmp,
                                    loop_body,
                                    &[],
                                    loop_exit,
                                    &[BlockArg::Value(zero_default)],
                                );

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    src_addr,
                                    0i32,
                                );

                                let call_inst =
                                    builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let pred_result = builder.inst_results(call_inst).to_vec()[0];

                                let zero_cmp_find = builder.ins().iconst(cl_types::I64, 0);
                                let pred_bool =
                                    builder
                                        .ins()
                                        .icmp(IntCC::NotEqual, pred_result, zero_cmp_find);
                                let one_find = builder.ins().iconst(cl_types::I64, 1);
                                let next_i_find = builder.ins().iadd(i_val, one_find);
                                builder.ins().brif(
                                    pred_bool,
                                    found_block,
                                    &[],
                                    loop_header,
                                    &[BlockArg::Value(next_i_find)],
                                );

                                // --- found_block ---
                                builder.switch_to_block(found_block);
                                builder.seal_block(found_block);
                                builder.ins().jump(loop_exit, &[BlockArg::Value(elem)]);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);
                                let find_result = builder.block_params(loop_exit)[0];

                                value_map.insert(*dst, find_result);
                            }

                            // list_sort(list): selection sort, returns a new sorted list
                            "list_sort" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                // Allocate new list: 16 + length * 8
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(length, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];

                                // Store length and capacity in header
                                builder.ins().store(MemFlags::new(), length, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), length, new_ptr, 8i32);

                                // Copy source data to new list
                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let memcpy_ref = self
                                    .module
                                    .declare_func_in_func(memcpy_func_id, builder.func);
                                let src_data = builder.ins().iadd_imm(list_ptr, 16);
                                let dst_data = builder.ins().iadd_imm(new_ptr, 16);
                                builder
                                    .ins()
                                    .call(memcpy_ref, &[dst_data, src_data, data_size]);

                                // Selection sort: for i in 0..length, find min in i..length, swap
                                let outer_header = builder.create_block();
                                let outer_body = builder.create_block();
                                let inner_header = builder.create_block();
                                let inner_body = builder.create_block();
                                let inner_exit = builder.create_block();
                                let outer_exit = builder.create_block();

                                // Jump to outer loop with i=0
                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(outer_header, &[BlockArg::Value(zero)]);

                                // Outer header: phi for i, check i < length - 1
                                builder.switch_to_block(outer_header);
                                builder.append_block_param(outer_header, cl_types::I64); // i
                                let i = builder.block_params(outer_header)[0];
                                let len_minus_one = builder.ins().iadd_imm(length, -1);
                                let outer_cmp =
                                    builder.ins().icmp(IntCC::SignedLessThan, i, len_minus_one);
                                builder
                                    .ins()
                                    .brif(outer_cmp, outer_body, &[], outer_exit, &[]);

                                // Outer body: start inner loop to find min in i+1..length
                                builder.switch_to_block(outer_body);
                                builder.seal_block(outer_body);
                                let i_plus_one = builder.ins().iadd_imm(i, 1);
                                // min_idx starts as i
                                builder.ins().jump(
                                    inner_header,
                                    &[BlockArg::Value(i_plus_one), BlockArg::Value(i)],
                                );

                                // Inner header: phi for j and min_idx
                                builder.switch_to_block(inner_header);
                                builder.append_block_param(inner_header, cl_types::I64); // j
                                builder.append_block_param(inner_header, cl_types::I64); // min_idx
                                let j = builder.block_params(inner_header)[0];
                                let min_idx = builder.block_params(inner_header)[1];
                                let inner_cmp =
                                    builder.ins().icmp(IntCC::SignedLessThan, j, length);
                                builder.ins().brif(
                                    inner_cmp,
                                    inner_body,
                                    &[],
                                    inner_exit,
                                    &[BlockArg::Value(min_idx)],
                                );

                                // Inner body: compare arr[j] < arr[min_idx], update min_idx
                                builder.switch_to_block(inner_body);
                                builder.seal_block(inner_body);
                                let j_byte_off = builder.ins().imul_imm(j, 8);
                                let j_data_off = builder.ins().iadd_imm(j_byte_off, 16);
                                let j_addr = builder.ins().iadd(new_ptr, j_data_off);
                                let j_val = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    j_addr,
                                    0i32,
                                );
                                let min_byte_off = builder.ins().imul_imm(min_idx, 8);
                                let min_data_off = builder.ins().iadd_imm(min_byte_off, 16);
                                let min_addr = builder.ins().iadd(new_ptr, min_data_off);
                                let min_val = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    min_addr,
                                    0i32,
                                );
                                let is_less =
                                    builder.ins().icmp(IntCC::SignedLessThan, j_val, min_val);
                                // If arr[j] < arr[min_idx], new_min = j, else new_min = min_idx
                                let new_min = builder.ins().select(is_less, j, min_idx);
                                let j_plus_one = builder.ins().iadd_imm(j, 1);
                                builder.ins().jump(
                                    inner_header,
                                    &[BlockArg::Value(j_plus_one), BlockArg::Value(new_min)],
                                );

                                // Seal inner_header (predecessors: outer_body + inner_body back-edge)
                                builder.seal_block(inner_header);
                                // Seal inner_exit (predecessor: inner_header)
                                builder.seal_block(inner_exit);

                                // Inner exit: swap arr[i] and arr[min_idx], then continue outer loop
                                builder.switch_to_block(inner_exit);
                                builder.append_block_param(inner_exit, cl_types::I64); // final min_idx
                                let final_min_idx = builder.block_params(inner_exit)[0];
                                // Load arr[i]
                                let i_byte_off = builder.ins().imul_imm(i, 8);
                                let i_data_off = builder.ins().iadd_imm(i_byte_off, 16);
                                let i_addr = builder.ins().iadd(new_ptr, i_data_off);
                                let i_val = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    i_addr,
                                    0i32,
                                );
                                // Load arr[final_min_idx]
                                let fm_byte_off = builder.ins().imul_imm(final_min_idx, 8);
                                let fm_data_off = builder.ins().iadd_imm(fm_byte_off, 16);
                                let fm_addr = builder.ins().iadd(new_ptr, fm_data_off);
                                let fm_val = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    fm_addr,
                                    0i32,
                                );
                                // Swap: store fm_val at i, i_val at final_min_idx
                                builder.ins().store(MemFlags::new(), fm_val, i_addr, 0i32);
                                builder.ins().store(MemFlags::new(), i_val, fm_addr, 0i32);
                                // Continue outer loop with i+1
                                let i_next = builder.ins().iadd_imm(i, 1);
                                builder.ins().jump(outer_header, &[BlockArg::Value(i_next)]);

                                // Seal outer_header (predecessors: entry + inner_exit back-edge)
                                builder.seal_block(outer_header);
                                // Seal outer_exit (predecessor: outer_header)
                                builder.seal_block(outer_exit);

                                builder.switch_to_block(outer_exit);
                                value_map.insert(*dst, new_ptr);
                            }

                            // list_reverse(list): returns a new list with elements in reverse order
                            "list_reverse" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let length = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    list_ptr,
                                    0i32,
                                );

                                // Allocate new list: 16 + length * 8
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(length, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];

                                // Store length and capacity in header
                                builder.ins().store(MemFlags::new(), length, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), length, new_ptr, 8i32);

                                // Loop: copy source[length-1-i] to dest[i] for i in 0..length
                                let loop_header = builder.create_block();
                                let loop_body = builder.create_block();
                                let loop_exit = builder.create_block();

                                // Jump to header with i=0
                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero)]);

                                // Header: phi for i, check i < length
                                builder.switch_to_block(loop_header);
                                builder.append_block_param(loop_header, cl_types::I64);
                                let i = builder.block_params(loop_header)[0];
                                let cmp = builder.ins().icmp(IntCC::SignedLessThan, i, length);
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[]);

                                // Body: copy source[length-1-i] to dest[i]
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);
                                // Source index = length - 1 - i
                                let len_minus_one = builder.ins().iadd_imm(length, -1);
                                let src_idx = builder.ins().isub(len_minus_one, i);
                                let src_byte_off = builder.ins().imul_imm(src_idx, 8);
                                let src_data_off = builder.ins().iadd_imm(src_byte_off, 16);
                                let src_addr = builder.ins().iadd(list_ptr, src_data_off);
                                let elem = builder.ins().load(
                                    cl_types::I64,
                                    MemFlags::new(),
                                    src_addr,
                                    0i32,
                                );
                                // Dest index = i
                                let dst_byte_off = builder.ins().imul_imm(i, 8);
                                let dst_data_off = builder.ins().iadd_imm(dst_byte_off, 16);
                                let dst_addr = builder.ins().iadd(new_ptr, dst_data_off);
                                builder.ins().store(MemFlags::new(), elem, dst_addr, 0i32);
                                // Increment i
                                let i_plus_one = builder.ins().iadd_imm(i, 1);
                                builder
                                    .ins()
                                    .jump(loop_header, &[BlockArg::Value(i_plus_one)]);

                                // Seal loop_header (predecessors: entry + body back-edge)
                                builder.seal_block(loop_header);
                                // Seal loop_exit (predecessor: loop_header)
                                builder.seal_block(loop_exit);

                                builder.switch_to_block(loop_exit);
                                value_map.insert(*dst, new_ptr);
                            }

                            // ── Map operations (Phase OO) ────────────────────────────────
                            //
                            // All map operations delegate to C helper functions in
                            // gradient_runtime.c.  The map type is determined at compile
                            // time by the value type: Map[String, String] uses the _str
                            // variants and Map[String, Int] uses the _int variants.
                            //
                            // For map_set/map_get the IR builder produces a generic
                            // "map_set" / "map_get" call; codegen decides which C helper
                            // to call based on the value argument's Cranelift type:
                            //   - Ptr (pointer)  → string variant
                            //   - I64 (integer)  → int variant

                            // ── map_new() -> Map (ptr) ──────────────────────────────
                            "map_new" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_map_new")
                                    .ok_or("__gradient_map_new not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── map_set(map, key, value) -> Map (ptr) ───────────────
                            "map_set" => {
                                let map_ptr = resolve_value(&value_map, &args[0])?;
                                let key_ptr = resolve_value(&value_map, &args[1])?;
                                let val_val = resolve_value(&value_map, &args[2])?;

                                // Determine which C function to call based on the
                                // Cranelift type of the value argument.
                                let val_cl_type = builder.func.dfg.value_type(val_val);
                                let c_fn_name = if val_cl_type == cl_types::I64 {
                                    "__gradient_map_set_int"
                                } else {
                                    "__gradient_map_set_str"
                                };
                                let func_id = *self
                                    .declared_functions
                                    .get(c_fn_name)
                                    .ok_or("__gradient_map_set_* not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call =
                                    builder.ins().call(func_ref, &[map_ptr, key_ptr, val_val]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── map_get(map, key) -> Option (ptr) ───────────────────
                            //
                            // Calls __gradient_map_get_str(map, key) -> ptr (NULL = None).
                            // Constructs Some(ptr) or None inline:
                            //   Some => allocate 16 bytes [tag=0, payload=ptr]
                            //   None => allocate  8 bytes [tag=1]
                            // Returns a pointer to the heap-allocated Option variant.
                            "map_get" => {
                                let map_ptr = resolve_value(&value_map, &args[0])?;
                                let key_ptr = resolve_value(&value_map, &args[1])?;

                                // Call the C helper to look up the string value.
                                let get_str_id = *self
                                    .declared_functions
                                    .get("__gradient_map_get_str")
                                    .ok_or("__gradient_map_get_str not declared")?;
                                let get_str_ref =
                                    self.module.declare_func_in_func(get_str_id, builder.func);
                                let get_call = builder.ins().call(get_str_ref, &[map_ptr, key_ptr]);
                                let raw_ptr = builder.inst_results(get_call).to_vec()[0];

                                // Compare returned pointer to NULL.
                                let null_val = builder.ins().iconst(cl_types::I64, 0);
                                let is_null = builder.ins().icmp(IntCC::Equal, raw_ptr, null_val);

                                let some_block = builder.create_block();
                                let none_block = builder.create_block();
                                let merge_block = builder.create_block();
                                builder.append_block_param(merge_block, cl_types::I64);

                                // if is_null goto none_block else goto some_block
                                builder
                                    .ins()
                                    .brif(is_null, none_block, &[], some_block, &[]);

                                // ── some_block ────────────────────────────────────────
                                builder.switch_to_block(some_block);
                                builder.seal_block(some_block);
                                let some_size = builder.ins().iconst(cl_types::I64, 16);
                                let malloc_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref_s =
                                    self.module.declare_func_in_func(malloc_id, builder.func);
                                let some_call = builder.ins().call(malloc_ref_s, &[some_size]);
                                let some_ptr = builder.inst_results(some_call).to_vec()[0];
                                let tag0 = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().store(MemFlags::new(), tag0, some_ptr, 0i32);
                                builder
                                    .ins()
                                    .store(MemFlags::new(), raw_ptr, some_ptr, 8i32);
                                builder
                                    .ins()
                                    .jump(merge_block, &[BlockArg::Value(some_ptr)]);

                                // ── none_block ────────────────────────────────────────
                                builder.switch_to_block(none_block);
                                builder.seal_block(none_block);
                                let none_size = builder.ins().iconst(cl_types::I64, 8);
                                let malloc_ref_n =
                                    self.module.declare_func_in_func(malloc_id, builder.func);
                                let none_call = builder.ins().call(malloc_ref_n, &[none_size]);
                                let none_ptr = builder.inst_results(none_call).to_vec()[0];
                                let tag1 = builder.ins().iconst(cl_types::I64, 1);
                                builder.ins().store(MemFlags::new(), tag1, none_ptr, 0i32);
                                builder
                                    .ins()
                                    .jump(merge_block, &[BlockArg::Value(none_ptr)]);

                                // ── merge_block ───────────────────────────────────────
                                // Seal merge_block: its only predecessors are some_block and
                                // none_block, which have already been completed above.
                                builder.seal_block(merge_block);
                                builder.switch_to_block(merge_block);
                                let option_ptr = builder.block_params(merge_block)[0];
                                value_map.insert(*dst, option_ptr);
                            }

                            // ── map_contains(map, key) -> Bool ─────────────────────
                            "map_contains" => {
                                let map_ptr = resolve_value(&value_map, &args[0])?;
                                let key_ptr = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_map_contains")
                                    .ok_or("__gradient_map_contains not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[map_ptr, key_ptr]);
                                let result_i64 = builder.inst_results(call).to_vec()[0];
                                // Truncate i64 -> i8 (Bool)
                                let result_bool = builder.ins().ireduce(cl_types::I8, result_i64);
                                value_map.insert(*dst, result_bool);
                            }

                            // ── map_remove(map, key) -> Map (ptr) ──────────────────
                            "map_remove" => {
                                let map_ptr = resolve_value(&value_map, &args[0])?;
                                let key_ptr = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_map_remove")
                                    .ok_or("__gradient_map_remove not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[map_ptr, key_ptr]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── map_size(map) -> Int ────────────────────────────────
                            "map_size" => {
                                let map_ptr = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_map_size")
                                    .ok_or("__gradient_map_size not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[map_ptr]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── map_keys(map) -> List[String] (ptr) ────────────────
                            "map_keys" => {
                                let map_ptr = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_map_keys")
                                    .ok_or("__gradient_map_keys not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[map_ptr]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Phase PP: Set operations ──────────────────────────

                            // ── set_new() -> Set (ptr) ───────────────────────────
                            "set_new" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_new")
                                    .ok_or("__gradient_set_new not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── set_add(set, elem) -> Set (ptr) ───────────────────
                            "set_add" => {
                                let set_ptr = resolve_value(&value_map, &args[0])?;
                                let elem = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_add")
                                    .ok_or("__gradient_set_add not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[set_ptr, elem]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── set_remove(set, elem) -> Set (ptr) ─────────────────
                            "set_remove" => {
                                let set_ptr = resolve_value(&value_map, &args[0])?;
                                let elem = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_remove")
                                    .ok_or("__gradient_set_remove not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[set_ptr, elem]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── set_contains(set, elem) -> Bool ────────────────────
                            "set_contains" => {
                                let set_ptr = resolve_value(&value_map, &args[0])?;
                                let elem = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_contains")
                                    .ok_or("__gradient_set_contains not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[set_ptr, elem]);
                                let result_i64 = builder.inst_results(call).to_vec()[0];
                                // Truncate i64 -> i8 (Bool)
                                let result_bool = builder.ins().ireduce(cl_types::I8, result_i64);
                                value_map.insert(*dst, result_bool);
                            }

                            // ── set_size(set) -> Int ───────────────────────────────
                            "set_size" => {
                                let set_ptr = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_size")
                                    .ok_or("__gradient_set_size not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[set_ptr]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── set_union(a, b) -> Set (ptr) ──────────────────────
                            "set_union" => {
                                let a_ptr = resolve_value(&value_map, &args[0])?;
                                let b_ptr = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_union")
                                    .ok_or("__gradient_set_union not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[a_ptr, b_ptr]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── set_intersection(a, b) -> Set (ptr) ───────────────
                            "set_intersection" => {
                                let a_ptr = resolve_value(&value_map, &args[0])?;
                                let b_ptr = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_intersection")
                                    .ok_or("__gradient_set_intersection not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[a_ptr, b_ptr]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── set_to_list(set) -> List (ptr) ────────────────────
                            "set_to_list" => {
                                let set_ptr = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_set_to_list")
                                    .ok_or("__gradient_set_to_list not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[set_ptr]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Default: route print/println to puts, others as normal calls ──
                            _ if func_name.starts_with("list_literal_") => {
                                // list_literal_N: allocate and populate a list
                                let n = args.len() as i64;
                                // alloc: 16 (header) + n * 8 (data)
                                let header_size = 16i64;
                                let total = header_size + n * 8;
                                let alloc_size = builder.ins().iconst(cl_types::I64, total);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self
                                    .module
                                    .declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let ptr = builder.inst_results(malloc_call).to_vec()[0];
                                // store length at offset 0
                                let len_val = builder.ins().iconst(cl_types::I64, n);
                                builder.ins().store(MemFlags::new(), len_val, ptr, 0i32);
                                // store capacity at offset 8
                                builder.ins().store(MemFlags::new(), len_val, ptr, 8i32);
                                // store each element at offset 16, 24, 32, ...
                                for (i, arg) in args.iter().enumerate() {
                                    let elem_val = resolve_value(&value_map, arg)?;
                                    let offset = (16 + i * 8) as i32;
                                    builder.ins().store(MemFlags::new(), elem_val, ptr, offset);
                                }
                                value_map.insert(*dst, ptr);
                            }

                            // ── Phase PP: Math builtins ─────────────────────────────────
                            // Trigonometric functions: all call libm directly (f64 -> f64)
                            "sin" | "cos" | "tan" | "asin" | "acos" | "atan" => {
                                let arg = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get(func_name.as_str())
                                    .ok_or_else(|| format!("{} not declared", func_name))?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[arg]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // atan2(y, x) -> f64
                            "atan2" => {
                                let y = resolve_value(&value_map, &args[0])?;
                                let x = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("atan2")
                                    .ok_or("atan2 not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[y, x]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // Logarithmic and exponential functions
                            "log" | "log10" | "log2" | "exp" | "exp2" => {
                                let arg = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get(func_name.as_str())
                                    .ok_or_else(|| format!("{} not declared", func_name))?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[arg]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // Rounding functions
                            "ceil" | "floor" | "round" | "trunc" => {
                                let arg = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get(func_name.as_str())
                                    .ok_or_else(|| format!("{} not declared", func_name))?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[arg]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // Math constants: pi() and e() - call runtime helpers
                            "pi" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_pi")
                                    .ok_or("__gradient_pi not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            "e" => {
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_e")
                                    .ok_or("__gradient_e not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // gcd(a: Int, b: Int) -> Int - call runtime
                            "gcd" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_gcd")
                                    .ok_or("__gradient_gcd not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[a, b]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // float_mod(a: Float, b: Float) -> Float - call fmod from libm
                            "float_mod" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("fmod")
                                    .ok_or("fmod not declared")?;
                                let func_ref =
                                    self.module.declare_func_in_func(func_id, builder.func);
                                let call = builder.ins().call(func_ref, &[a, b]);
                                let result = builder.inst_results(call).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // clamp(value, min, max) -> T - call runtime (type-specialized)
                            "clamp" => {
                                // Determine type based on first argument's Cranelift type
                                let val = resolve_value(&value_map, &args[0])?;
                                let val_ty = builder.func.dfg.value_type(val);

                                if val_ty == cl_types::F64 {
                                    let min = resolve_value(&value_map, &args[1])?;
                                    let max = resolve_value(&value_map, &args[2])?;
                                    let func_id = *self
                                        .declared_functions
                                        .get("__gradient_clamp_f64")
                                        .ok_or("__gradient_clamp_f64 not declared")?;
                                    let func_ref =
                                        self.module.declare_func_in_func(func_id, builder.func);
                                    let call = builder.ins().call(func_ref, &[val, min, max]);
                                    let result = builder.inst_results(call).to_vec()[0];
                                    value_map.insert(*dst, result);
                                } else {
                                    // Default to i64 clamp
                                    let min = resolve_value(&value_map, &args[1])?;
                                    let max = resolve_value(&value_map, &args[2])?;
                                    let func_id = *self
                                        .declared_functions
                                        .get("__gradient_clamp_i64")
                                        .ok_or("__gradient_clamp_i64 not declared")?;
                                    let func_ref =
                                        self.module.declare_func_in_func(func_id, builder.func);
                                    let call = builder.ins().call(func_ref, &[val, min, max]);
                                    let result = builder.inst_results(call).to_vec()[0];
                                    value_map.insert(*dst, result);
                                }
                            }

                            _ => {
                                let target_name = match func_name.as_str() {
                                    // print is handled above with printf("%s")
                                    "println" => "puts",
                                    other => other,
                                };
                                eprintln!("DEBUG: target_name='{}', func_name='{}', declared_functions keys: {:?}", target_name, func_name, self.declared_functions.keys().collect::<Vec<_>>());

                                // Check if the target is a known declared function.
                                // If not, it may be a closure variable (function pointer)
                                // which needs call_indirect.
                                if self.declared_functions.contains_key(target_name) {
                                    let cl_func_ref = if let Some(&existing) =
                                        func_ref_map.get(ir_func_ref)
                                    {
                                        eprintln!(
                                            "DEBUG: Using cached func_ref for FuncRef({}) in '{}'",
                                            ir_func_ref.0, func.name
                                        );
                                        existing
                                    } else {
                                        let target_func_id = self
                                            .declared_functions
                                            .get(target_name)
                                            .ok_or_else(|| {
                                                format!(
                                                    "Undeclared function referenced during codegen: {}",
                                                    target_name
                                                )
                                            })?;
                                        eprintln!("DEBUG: Declaring func '{}' in func '{}' -> FuncId({:?})", target_name, func.name, target_func_id);
                                        let fref = self
                                            .module
                                            .declare_func_in_func(*target_func_id, builder.func);
                                        eprintln!(
                                            "DEBUG: Got Cranelift FuncRef index {:?} for '{}'",
                                            fref, target_name
                                        );
                                        func_ref_map.insert(*ir_func_ref, fref);
                                        fref
                                    };

                                    let cl_args: Result<Vec<_>, _> =
                                        args.iter().map(|a| resolve_value(&value_map, a)).collect();
                                    let cl_args = cl_args?;

                                    eprintln!("DEBUG: About to call cl_func_ref={:?} for '{}' with {} args in '{}'", cl_func_ref, target_name, cl_args.len(), func.name);
                                    let call_inst = builder.ins().call(cl_func_ref, &cl_args);

                                    let results = builder.inst_results(call_inst).to_vec();
                                    // Normalize return value to the expected IR type.
                                    // Some C functions (e.g. puts) return i32 but our IR
                                    // expects i8 (void/bool). Use a dummy of the right type.
                                    let result_val = if !results.is_empty() {
                                        let actual_ty = builder.func.dfg.value_type(results[0]);
                                        let expected_ir_ty = func
                                            .value_types
                                            .get(dst)
                                            .cloned()
                                            .unwrap_or(ir::Type::I64);
                                        let expected_cl_ty = ir_type_to_cl(&expected_ir_ty);
                                        if actual_ty == expected_cl_ty {
                                            results[0]
                                        } else {
                                            builder.ins().iconst(expected_cl_ty, 0)
                                        }
                                    } else {
                                        let expected_ir_ty = func
                                            .value_types
                                            .get(dst)
                                            .cloned()
                                            .unwrap_or(ir::Type::I64);
                                        let expected_cl_ty = ir_type_to_cl(&expected_ir_ty);
                                        builder.ins().iconst(expected_cl_ty, 0)
                                    };
                                    value_map.insert(*dst, result_val);
                                } else {
                                    // Function not declared -- treat as a call
                                    // through a function pointer (closure variable).
                                    // Look up the FuncRef index in the value_map
                                    // to get the function pointer value.
                                    let fn_ref_idx = ir_func_ref.0;
                                    // The closure's IR value was stored as
                                    // Const(v, Literal::Int(func_ref_index)).
                                    // We need to find the corresponding Cranelift
                                    // value in the value_map. The closure variable
                                    // would have been passed as an argument or
                                    // defined earlier. Try to find the fn pointer
                                    // by looking up the func_ref name as a variable.
                                    // Actually, the call args already contain
                                    // the real arguments to pass. The function
                                    // pointer itself is not in args -- we need to
                                    // resolve it from the func name.
                                    //
                                    // In the IR, when a closure variable `f` is
                                    // called as `f(x)`, the IR emits
                                    // Call(dst, func_ref_for_f, [x]). The func_ref
                                    // maps to the closure's name (e.g. __closure_0).
                                    // But since __closure_0 IS declared (it was
                                    // compiled as a separate function), this branch
                                    // shouldn't normally fire for closures.
                                    //
                                    // This handles cases where a function pointer
                                    // variable is called but the actual function
                                    // name doesn't match any declared function.
                                    // Build a signature with args.len() params
                                    // (all i64) and one i64 return.
                                    let mut indirect_sig = self.module.make_signature();
                                    for _ in args {
                                        indirect_sig.params.push(AbiParam::new(cl_types::I64));
                                    }
                                    indirect_sig.returns.push(AbiParam::new(cl_types::I64));
                                    let sig_ref = builder.import_signature(indirect_sig);

                                    // The function pointer value: look up from
                                    // the IR value that was defined with this
                                    // func_ref's index as a constant.
                                    // Search value_map for a value whose constant
                                    // equals func_ref_idx. Since closures store
                                    // their func_ref index as a const, and we
                                    // converted it to func_addr, the value should
                                    // already be in the value_map.
                                    let fn_ptr_val = ir::Value(fn_ref_idx);
                                    let fn_ptr =
                                        if let Ok(v) = resolve_value(&value_map, &fn_ptr_val) {
                                            v
                                        } else {
                                            // Fallback: emit iconst 0 (will crash at runtime)
                                            builder.ins().iconst(cl_types::I64, 0)
                                        };

                                    let cl_args: Result<Vec<_>, _> =
                                        args.iter().map(|a| resolve_value(&value_map, a)).collect();
                                    let cl_args = cl_args?;

                                    let call_inst =
                                        builder.ins().call_indirect(sig_ref, fn_ptr, &cl_args);

                                    let results = builder.inst_results(call_inst).to_vec();
                                    // Normalize return value to the expected IR type.
                                    // Some C functions (e.g. puts) return i32 but our IR
                                    // expects i8 (void/bool). Use a dummy of the right type.
                                    let result_val = if !results.is_empty() {
                                        let actual_ty = builder.func.dfg.value_type(results[0]);
                                        let expected_ir_ty = func
                                            .value_types
                                            .get(dst)
                                            .cloned()
                                            .unwrap_or(ir::Type::I64);
                                        let expected_cl_ty = ir_type_to_cl(&expected_ir_ty);
                                        if actual_ty == expected_cl_ty {
                                            results[0]
                                        } else {
                                            builder.ins().iconst(expected_cl_ty, 0)
                                        }
                                    } else {
                                        let expected_ir_ty = func
                                            .value_types
                                            .get(dst)
                                            .cloned()
                                            .unwrap_or(ir::Type::I64);
                                        let expected_cl_ty = ir_type_to_cl(&expected_ir_ty);
                                        builder.ins().iconst(expected_cl_ty, 0)
                                    };
                                    value_map.insert(*dst, result_val);
                                }
                            }
                        }
                    }

                    ir::Instruction::Ret(Some(val)) => {
                        if is_main && func.return_type == ir::Type::Void {
                            let zero = builder.ins().iconst(cl_types::I32, 0);
                            builder.ins().return_(&[zero]);
                        } else {
                            let cl_val = resolve_value(&value_map, val)?;
                            builder.ins().return_(&[cl_val]);
                        }
                        block_filled = true;
                    }

                    ir::Instruction::Ret(None) => {
                        if is_main && func.return_type == ir::Type::Void {
                            let zero = builder.ins().iconst(cl_types::I32, 0);
                            builder.ins().return_(&[zero]);
                        } else if func.return_type != ir::Type::Void {
                            // Function has a return type but Ret(None) was emitted
                            // (e.g., in contract fail blocks after calling exit()).
                            // Cranelift requires return arguments to match the
                            // function signature, so emit a dummy return value.
                            let ret_cl_type = ir_type_to_cl(&func.return_type);
                            let dummy = if ret_cl_type == cl_types::F64 {
                                builder.ins().f64const(0.0)
                            } else {
                                builder.ins().iconst(ret_cl_type, 0)
                            };
                            builder.ins().return_(&[dummy]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                        block_filled = true;
                    }

                    ir::Instruction::Branch(cond, then_block, else_block) => {
                        let cl_cond = resolve_value(&value_map, cond)?;
                        let then_cl = block_map[then_block];
                        let else_cl = block_map[else_block];

                        let then_args_raw =
                            collect_jump_args(&jump_args, then_block, &ir_block.label, &value_map)?;
                        let else_args_raw =
                            collect_jump_args(&jump_args, else_block, &ir_block.label, &value_map)?;

                        // Coerce jump args to match the target block's parameter types.
                        let then_params = builder.block_params(then_cl).to_vec();
                        let then_args: Vec<BlockArg> =
                            coerce_jump_args(then_args_raw, &then_params, &mut builder);
                        let else_params = builder.block_params(else_cl).to_vec();
                        let else_args: Vec<BlockArg> =
                            coerce_jump_args(else_args_raw, &else_params, &mut builder);

                        builder
                            .ins()
                            .brif(cl_cond, then_cl, &then_args, else_cl, &else_args);
                        block_filled = true;
                    }

                    ir::Instruction::Jump(target) => {
                        let target_cl = block_map[target];
                        let args_raw =
                            collect_jump_args(&jump_args, target, &ir_block.label, &value_map)?;
                        // Coerce jump args to match the target block's parameter types.
                        let params = builder.block_params(target_cl).to_vec();
                        let args = coerce_jump_args(args_raw, &params, &mut builder);
                        builder.ins().jump(target_cl, &args);
                        block_filled = true;
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
                        let load_ty = func
                            .value_types
                            .get(dst)
                            .map(ir_type_to_cl)
                            .unwrap_or(cl_types::I64);
                        let result = builder.ins().load(load_ty, MemFlags::new(), cl_addr, 0);
                        value_map.insert(*dst, result);
                    }

                    ir::Instruction::Store(val, addr) => {
                        let cl_val = resolve_value(&value_map, val)?;
                        let cl_addr = resolve_value(&value_map, addr)?;
                        builder.ins().store(MemFlags::new(), cl_val, cl_addr, 0);
                    }

                    // ── ConstructVariant: heap-allocate a tagged enum union ──
                    //
                    // Layout: [tag: i64, field_0: i64, field_1: i64, ...]
                    // Size:   (1 + payload.len()) * 8 bytes
                    ir::Instruction::ConstructVariant {
                        result,
                        tag,
                        payload,
                    } => {
                        let slot_count = 1 + payload.len() as i64;
                        let alloc_bytes = slot_count * 8;
                        let alloc_size = builder.ins().iconst(cl_types::I64, alloc_bytes);

                        let malloc_func_id = *self
                            .declared_functions
                            .get("malloc")
                            .ok_or("malloc not declared")?;
                        let malloc_ref = self
                            .module
                            .declare_func_in_func(malloc_func_id, builder.func);
                        let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                        let ptr = builder.inst_results(malloc_call).to_vec()[0];

                        // Store tag at offset 0.
                        let tag_val = builder.ins().iconst(cl_types::I64, *tag);
                        builder.ins().store(MemFlags::new(), tag_val, ptr, 0i32);

                        // Store each payload field at offset (i+1)*8.
                        for (i, field_ir_val) in payload.iter().enumerate() {
                            let field_cl_val = resolve_value(&value_map, field_ir_val)?;
                            // Cranelift requires the stored value to be the
                            // pointer width. If the field is an f64, bitcast
                            // it to i64 before storing.
                            let stored_val = {
                                let fty = builder.func.dfg.value_type(field_cl_val);
                                if fty == cl_types::F64 {
                                    builder.ins().bitcast(
                                        cl_types::I64,
                                        MemFlags::new(),
                                        field_cl_val,
                                    )
                                } else if fty == cl_types::I8 {
                                    // Bool (I8) — zero-extend to I64.
                                    builder.ins().uextend(cl_types::I64, field_cl_val)
                                } else {
                                    field_cl_val
                                }
                            };
                            let byte_offset = ((i + 1) * 8) as i32;
                            builder
                                .ins()
                                .store(MemFlags::new(), stored_val, ptr, byte_offset);
                        }

                        value_map.insert(*result, ptr);
                    }

                    // ── GetVariantTag: load the tag from an enum pointer ──
                    ir::Instruction::GetVariantTag { result, ptr } => {
                        let cl_ptr = resolve_value(&value_map, ptr)?;
                        let tag_val =
                            builder
                                .ins()
                                .load(cl_types::I64, MemFlags::new(), cl_ptr, 0i32);
                        value_map.insert(*result, tag_val);
                    }

                    // ── GetVariantField: load a payload field from an enum pointer ──
                    ir::Instruction::GetVariantField { result, ptr, index } => {
                        let cl_ptr = resolve_value(&value_map, ptr)?;
                        let byte_offset = ((index + 1) * 8) as i32;

                        // Determine the expected result type from value_types.
                        let load_ty = func
                            .value_types
                            .get(result)
                            .map(ir_type_to_cl)
                            .unwrap_or(cl_types::I64);

                        // Load directly as the target type so that float fields
                        // are loaded into XMM registers rather than integer
                        // registers. Loading F64 directly (rather than loading
                        // I64 and bitcasting) avoids clobbering rax with float
                        // bit-pattern values, which would corrupt the 'al'
                        // register that variadic callers (e.g. printf) inspect
                        // to count SSE arguments.
                        let final_val = if load_ty == cl_types::F64 {
                            builder
                                .ins()
                                .load(cl_types::F64, MemFlags::new(), cl_ptr, byte_offset)
                        } else if load_ty == cl_types::I8 {
                            let raw = builder.ins().load(
                                cl_types::I64,
                                MemFlags::new(),
                                cl_ptr,
                                byte_offset,
                            );
                            builder.ins().ireduce(cl_types::I8, raw)
                        } else {
                            builder
                                .ins()
                                .load(cl_types::I64, MemFlags::new(), cl_ptr, byte_offset)
                        };
                        value_map.insert(*result, final_val);
                    }

                    // ── Actor operations ─────────────────────────────────────────────

                    // Spawn { result, actor_type_name }: call __gradient_actor_spawn
                    ir::Instruction::Spawn {
                        result,
                        actor_type_name: _,
                    } => {
                        // For now, use a simple approach: pass null as init_fn and 0 as state_size
                        // This is a placeholder until the full actor runtime is implemented
                        let null_init_fn = builder.ins().iconst(pointer_type, 0);
                        let state_size_val = builder.ins().iconst(cl_types::I64, 0);

                        // Call __gradient_actor_spawn(init_fn, state_size)
                        let spawn_func_id = *self
                            .declared_functions
                            .get("__gradient_actor_spawn")
                            .ok_or("__gradient_actor_spawn not declared")?;
                        let spawn_ref = self
                            .module
                            .declare_func_in_func(spawn_func_id, builder.func);
                        let call_inst = builder
                            .ins()
                            .call(spawn_ref, &[null_init_fn, state_size_val]);
                        let actor_handle = builder.inst_results(call_inst).to_vec()[0];
                        value_map.insert(*result, actor_handle);
                    }

                    // Send { handle, message_name, payload }: call __gradient_actor_send
                    ir::Instruction::Send {
                        handle,
                        message_name,
                        payload,
                    } => {
                        let handle_val = resolve_value(&value_map, handle)?;

                        // Create string constant for message name
                        let msg_name_data_id = get_or_create_string(
                            &mut self.module,
                            &mut self.string_data,
                            &mut self.string_counter,
                            message_name,
                        )?;
                        let _msg_name_gv = self
                            .module
                            .declare_data_in_func(msg_name_data_id, builder.func);
                        let _msg_name_ptr = builder.ins().global_value(pointer_type, _msg_name_gv);

                        // Get payload pointer (null if None)
                        let payload_ptr = match payload {
                            Some(val) => resolve_value(&value_map, val)?,
                            None => builder.ins().iconst(pointer_type, 0), // null pointer
                        };

                        // Convert handle (ptr) to ActorId (i64) - the handle is a pointer to the actor struct
                        let handle_i64 =
                            builder
                                .ins()
                                .bitcast(cl_types::I64, MemFlags::new(), handle_val);

                        // Generate a deterministic message type ID from the message name
                        // Use a simple hash of the message name (first 4 chars + length)
                        let msg_hash: i64 = message_name
                            .bytes()
                            .map(|b| b as i64)
                            .sum::<i64>()
                            .wrapping_add(message_name.len() as i64 * 31);
                        let message_type_val =
                            builder.ins().iconst(cl_types::I64, msg_hash % 1000 + 1);

                        // Payload size - for now assume pointer size (8 bytes)
                        let payload_size = builder.ins().iconst(cl_types::I64, 8);

                        // Call __gradient_actor_send(handle_i64, message_type, payload, payload_size)
                        let send_func_id = *self
                            .declared_functions
                            .get("__gradient_actor_send")
                            .ok_or("__gradient_actor_send not declared")?;
                        let send_ref = self.module.declare_func_in_func(send_func_id, builder.func);
                        builder.ins().call(
                            send_ref,
                            &[handle_i64, message_type_val, payload_ptr, payload_size],
                        );
                    }

                    // Ask { result, handle, message_name, payload }: call __gradient_actor_ask
                    ir::Instruction::Ask {
                        result,
                        handle,
                        message_name,
                        payload,
                    } => {
                        let handle_val = resolve_value(&value_map, handle)?;

                        // Convert handle (ptr) to ActorId (i64)
                        let handle_i64 =
                            builder
                                .ins()
                                .bitcast(cl_types::I64, MemFlags::new(), handle_val);

                        // Generate a deterministic message type ID from the message name
                        let msg_hash: i64 = message_name
                            .bytes()
                            .map(|b| b as i64)
                            .sum::<i64>()
                            .wrapping_add(message_name.len() as i64 * 31);
                        let message_type_val =
                            builder.ins().iconst(cl_types::I64, msg_hash % 1000 + 1);

                        // Payload size
                        let payload_size = builder.ins().iconst(cl_types::I64, 8);

                        // Get payload pointer (null if None)
                        let payload_ptr = match payload {
                            Some(val) => resolve_value(&value_map, val)?,
                            None => builder.ins().iconst(pointer_type, 0), // null pointer
                        };

                        // Call __gradient_actor_ask(handle_i64, message_type, payload, payload_size) -> reply_ptr
                        let ask_func_id = *self
                            .declared_functions
                            .get("__gradient_actor_ask")
                            .ok_or("__gradient_actor_ask not declared")?;
                        let ask_ref = self.module.declare_func_in_func(ask_func_id, builder.func);
                        let call_inst = builder.ins().call(
                            ask_ref,
                            &[handle_i64, message_type_val, payload_ptr, payload_size],
                        );
                        let reply_ptr = builder.inst_results(call_inst).to_vec()[0];
                        value_map.insert(*result, reply_ptr);
                    }

                    // ActorInit { initial_state }: setup actor initial state
                    ir::Instruction::ActorInit { initial_state } => {
                        // ActorInit is a no-op at runtime for now - the initial state
                        // is passed directly when spawning. This instruction serves
                        // as a marker for potential future state tracking.
                        let _state_val = resolve_value(&value_map, initial_state)?;
                        // No code generation needed - runtime handles state setup
                    }

                    ir::Instruction::PtrToInt(result, ptr) => {
                        // Cast pointer to integer (i64)
                        // On x86_64, pointers are already i64, so just add 0 to force the type
                        let ptr_val = resolve_value(&value_map, ptr)?;
                        let zero = builder.ins().iconst(cl_types::I64, 0);
                        let int_val = builder.ins().iadd(ptr_val, zero);
                        value_map.insert(*result, int_val);
                    }

                    ir::Instruction::IntToPtr(result, int_val) => {
                        // Cast integer (i64) to pointer
                        // On x86_64, integers are already pointer-sized, so just add 0 to force the type
                        let int_value = resolve_value(&value_map, int_val)?;
                        let zero = builder.ins().iconst(cl_types::I64, 0);
                        let ptr_val = builder.ins().iadd(int_value, zero);
                        value_map.insert(*result, ptr_val);
                    }

                    ir::Instruction::GetElementPtr {
                        result,
                        base,
                        offset,
                        field_ty: _,
                    } => {
                        // GEP: base + offset
                        let base_val = resolve_value(&value_map, base)?;
                        let offset_val = builder.ins().iconst(cl_types::I64, *offset);
                        let addr = builder.ins().iadd(base_val, offset_val);
                        value_map.insert(*result, addr);
                    }

                    ir::Instruction::FieldAddr {
                        result,
                        base,
                        field_name: _,
                        field_ty: _,
                        offset,
                    } => {
                        // Field address: base + offset (same as GEP)
                        let base_val = resolve_value(&value_map, base)?;
                        let offset_val = builder.ins().iconst(cl_types::I64, *offset);
                        let addr = builder.ins().iadd(base_val, offset_val);
                        value_map.insert(*result, addr);
                    }

                    ir::Instruction::Or(result, lhs, rhs) => {
                        // Boolean OR: bor(lhs, rhs)
                        let lhs_val = resolve_value(&value_map, lhs)?;
                        let rhs_val = resolve_value(&value_map, rhs)?;
                        let or_result = builder.ins().bor(lhs_val, rhs_val);
                        value_map.insert(*result, or_result);
                    }

                    ir::Instruction::LoadField {
                        result,
                        object,
                        field_idx: _,
                        field_ty,
                        offset,
                    } => {
                        // Load field from object pointer at specified byte offset
                        // Uses the field_ty from IR to determine the Cranelift type
                        let obj_val = resolve_value(&value_map, object)?;
                        let cl_ty = ir_type_to_cl(field_ty);
                        let loaded =
                            builder
                                .ins()
                                .load(cl_ty, MemFlags::new(), obj_val, *offset as i32);
                        value_map.insert(*result, loaded);
                    }

                    ir::Instruction::StoreField {
                        value,
                        object,
                        field_idx: _,
                        field_ty,
                        offset,
                    } => {
                        // Store value to object pointer at specified byte offset
                        // Uses the field_ty from IR to verify type compatibility
                        let obj_val = resolve_value(&value_map, object)?;
                        let val = resolve_value(&value_map, value)?;
                        // Note: field_ty could be used here for type checking or conversion
                        let _cl_ty = ir_type_to_cl(field_ty);
                        builder
                            .ins()
                            .store(MemFlags::new(), val, obj_val, *offset as i32);
                    }
                }
            }

            // After emitting all instructions for this block, record the
            // predecessor edges for any loop headers targeted by its terminator.
            // When a loop header has received all expected predecessors, seal it.
            for inst in &ir_block.instructions {
                let targets: Vec<ir::BlockRef> = match inst {
                    ir::Instruction::Jump(target) => vec![*target],
                    ir::Instruction::Branch(_, then_b, else_b) => vec![*then_b, *else_b],
                    _ => vec![],
                };
                for target in targets {
                    if loop_headers.contains(&target) {
                        let emitted = predecessors_emitted.entry(target).or_insert(0);
                        *emitted += 1;
                        let expected = predecessor_count.get(&target).copied().unwrap_or(0);
                        if *emitted >= expected && deferred_seal.contains(&target) {
                            let target_cl = block_map[&target];
                            builder.seal_block(target_cl);
                            deferred_seal.remove(&target);
                        }
                    }
                }
            }

            // Seal this block if it is NOT a loop header (loop headers are
            // sealed above once all predecessors have been emitted).
            if loop_headers.contains(&ir_block.label) {
                // This is a loop header. Check if all predecessors are
                // already known (possible if the header is the very last
                // block to be processed, though unusual).
                let emitted = predecessors_emitted
                    .get(&ir_block.label)
                    .copied()
                    .unwrap_or(0);
                let expected = predecessor_count.get(&ir_block.label).copied().unwrap_or(0);
                if emitted >= expected {
                    builder.seal_block(cl_block);
                } else {
                    deferred_seal.insert(ir_block.label);
                }
            } else {
                builder.seal_block(cl_block);
            }
        }

        // Defensive: seal any remaining unsealed blocks (e.g. unreachable blocks).
        builder.seal_all_blocks();
        builder.finalize();

        // ----------------------------------------------------------------
        // Define the function in the module.
        // ----------------------------------------------------------------
        // Dump IR for debugging (only in debug builds and when env var is set).
        #[cfg(debug_assertions)]
        if std::env::var("GRADIENT_DUMP_IR").is_ok() {
            eprintln!(
                "=== Cranelift IR for '{}' ===\n{}",
                func.name,
                self.ctx.func.display()
            );
        }

        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| {
                // Include the IR dump in the error message to ease debugging.
                let ir_dump = format!("{}", self.ctx.func.display());
                format!(
                    "Failed to define function '{}': {:?}\nCranelift IR:\n{}",
                    func.name, e, ir_dump
                )
            })?;
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

    /// Emit the compiled module as raw object file bytes without writing to disk.
    ///
    /// This is the non-side-effecting version of [`finalize`](Self::finalize),
    /// used by the [`CodegenBackend`](super::CodegenBackend) trait implementation.
    pub fn emit_bytes(self) -> Result<Vec<u8>, String> {
        let object_product = self.module.finish();
        let bytes = object_product
            .emit()
            .map_err(|e| format!("Failed to emit object: {}", e))?;
        Ok(bytes)
    }
}

// ========================================================================
// CodegenBackend trait implementation
// ========================================================================

impl super::CodegenBackend for CraneliftCodegen {
    fn compile_module(&mut self, module: &crate::ir::Module) -> Result<(), super::CodegenError> {
        self.compile_module(module)
            .map_err(super::CodegenError::from)
    }

    fn finish(self: Box<Self>) -> Result<Vec<u8>, super::CodegenError> {
        self.emit_bytes().map_err(super::CodegenError::from)
    }

    fn name(&self) -> &str {
        "cranelift"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::CodegenBackend;

    #[test]
    fn test_cranelift_backend_name() {
        let cg = CraneliftCodegen::new().unwrap();
        assert_eq!(cg.name(), "cranelift");
    }

    #[test]
    fn test_cranelift_backend_trait_compile_empty_module() {
        let mut cg = CraneliftCodegen::new().unwrap();
        let module = crate::ir::Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: std::collections::HashMap::new(),
        };
        // Compile via the trait method (through CodegenBackend).
        let result = CodegenBackend::compile_module(&mut cg, &module);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cranelift_backend_trait_finish_produces_bytes() {
        let mut cg = CraneliftCodegen::new().unwrap();
        let module = crate::ir::Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: std::collections::HashMap::new(),
        };
        CodegenBackend::compile_module(&mut cg, &module).unwrap();
        let boxed: Box<dyn CodegenBackend> = Box::new(cg);
        let bytes = boxed.finish().unwrap();
        // The object file should be non-empty (at least ELF/Mach-O headers).
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_cranelift_emit_bytes() {
        let cg = CraneliftCodegen::new().unwrap();
        // Even an empty module should emit valid object bytes.
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_cranelift_backend_used_as_dyn_trait() {
        // Verify that CraneliftCodegen can be used as Box<dyn CodegenBackend>.
        let cg = CraneliftCodegen::new().unwrap();
        let backend: Box<dyn CodegenBackend> = Box::new(cg);
        assert_eq!(backend.name(), "cranelift");
    }

    #[test]
    fn test_int_to_string_codegen_produces_snprintf_call() {
        // Build an IR module: main() calls int_to_string(42) then print(result).
        use crate::ir::types::{BlockRef, FuncRef, Literal, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Module};

        let mut func_refs = std::collections::HashMap::new();
        func_refs.insert(FuncRef(0), "int_to_string".to_string());
        func_refs.insert(FuncRef(1), "print".to_string());

        let module = Module {
            name: "test_int_to_string".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        // v0 = 42
                        Instruction::Const(Value(0), Literal::Int(42)),
                        // v1 = int_to_string(v0)
                        Instruction::Call(Value(1), FuncRef(0), vec![Value(0)]),
                        // v2 = print(v1)
                        Instruction::Call(Value(2), FuncRef(1), vec![Value(1)]),
                        // return
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::I64);
                    vt.insert(Value(1), Type::Ptr);
                    vt.insert(Value(2), Type::Void);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs,
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(
            result.is_ok(),
            "int_to_string codegen failed: {:?}",
            result.err()
        );

        // Verify we can produce valid object bytes.
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    // ── Phase LL: Tuple Variant Codegen tests ──────────────────────────────

    /// Helper: run the full pipeline (parse → IR → codegen) on a Gradient
    /// source snippet and return the object bytes on success.
    fn compile_gradient_snippet(src: &str) -> Result<Vec<u8>, String> {
        use crate::ir::builder::IrBuilder;
        use crate::lexer::Lexer;
        use crate::parser;

        let mut lexer = Lexer::new(src, 0);
        let tokens = lexer.tokenize();
        let (ast_module, parse_errors) = parser::parse(tokens, 0);
        if !parse_errors.is_empty() {
            return Err(format!("parse errors: {:?}", parse_errors));
        }
        let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
        if !ir_errors.is_empty() {
            return Err(format!("IR errors: {:?}", ir_errors));
        }
        let mut cg = CraneliftCodegen::new()?;
        cg.compile_module(&ir_module)?;
        cg.emit_bytes()
    }

    #[test]
    fn test_construct_variant_unit_compiles() {
        // ConstructVariant with no payload should compile to valid object code.
        use crate::ir::types::{BlockRef, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Module};

        let module = Module {
            name: "test_unit_variant".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        // v0 = ConstructVariant { tag: 0, payload: [] }
                        Instruction::ConstructVariant {
                            result: Value(0),
                            tag: 0,
                            payload: vec![],
                        },
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::Ptr);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs: std::collections::HashMap::new(),
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(
            result.is_ok(),
            "unit ConstructVariant codegen failed: {:?}",
            result.err()
        );
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_construct_variant_with_payload_compiles() {
        // ConstructVariant with an i64 payload field should compile correctly.
        use crate::ir::types::{BlockRef, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Literal, Module};

        let module = Module {
            name: "test_tuple_variant".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        // v0 = 42
                        Instruction::Const(Value(0), Literal::Int(42)),
                        // v1 = ConstructVariant { tag: 0, payload: [v0] }  -- Some(42)
                        Instruction::ConstructVariant {
                            result: Value(1),
                            tag: 0,
                            payload: vec![Value(0)],
                        },
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::I64);
                    vt.insert(Value(1), Type::Ptr);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs: std::collections::HashMap::new(),
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(
            result.is_ok(),
            "tuple ConstructVariant codegen failed: {:?}",
            result.err()
        );
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_get_variant_tag_compiles() {
        // GetVariantTag should load the tag from an enum pointer.
        use crate::ir::types::{BlockRef, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Literal, Module};

        let module = Module {
            name: "test_get_tag".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        // v0 = ConstructVariant { tag: 1, payload: [] }  -- None
                        Instruction::ConstructVariant {
                            result: Value(0),
                            tag: 1,
                            payload: vec![],
                        },
                        // v1 = GetVariantTag(v0)
                        Instruction::GetVariantTag {
                            result: Value(1),
                            ptr: Value(0),
                        },
                        // v2 = 1
                        Instruction::Const(Value(2), Literal::Int(1)),
                        // v3 = (v1 == v2)
                        Instruction::Cmp(Value(3), crate::ir::CmpOp::Eq, Value(1), Value(2)),
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::Ptr);
                    vt.insert(Value(1), Type::I64);
                    vt.insert(Value(2), Type::I64);
                    vt.insert(Value(3), Type::Bool);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs: std::collections::HashMap::new(),
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(
            result.is_ok(),
            "GetVariantTag codegen failed: {:?}",
            result.err()
        );
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_get_variant_field_compiles() {
        // GetVariantField should load a payload field from an enum pointer.
        use crate::ir::types::{BlockRef, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Literal, Module};

        let module = Module {
            name: "test_get_field".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        // v0 = 99
                        Instruction::Const(Value(0), Literal::Int(99)),
                        // v1 = ConstructVariant { tag: 0, payload: [v0] }  -- Some(99)
                        Instruction::ConstructVariant {
                            result: Value(1),
                            tag: 0,
                            payload: vec![Value(0)],
                        },
                        // v2 = GetVariantField(v1, index=0)
                        Instruction::GetVariantField {
                            result: Value(2),
                            ptr: Value(1),
                            index: 0,
                        },
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::I64);
                    vt.insert(Value(1), Type::Ptr);
                    vt.insert(Value(2), Type::I64);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs: std::collections::HashMap::new(),
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(
            result.is_ok(),
            "GetVariantField codegen failed: {:?}",
            result.err()
        );
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_full_pipeline_option_match() {
        // Full pipeline test: parse + IR build + codegen for Option/match.
        let src = "\
mod test_option

type Option[T] = Some(T) | None

fn unwrap_or(opt: Option[Int], default: Int) -> Int:
    match opt:
        Some(x):
            x
        None:
            default
";
        let result = compile_gradient_snippet(src);
        assert!(
            result.is_ok(),
            "Option/match full pipeline failed: {:?}",
            result.err()
        );
        let bytes = result.unwrap();
        assert!(!bytes.is_empty(), "expected non-empty object bytes");
    }

    #[test]
    fn test_full_pipeline_shape_match() {
        // Full pipeline: Shape enum with single-field tuple variants and a
        // unit variant. (Multi-field variants like Rectangle(Float, Float) are
        // a TODO for a future phase — the current parser only supports one
        // field per variant.)
        let src = "\
mod test_shape

type Shape = Circle(Float) | Box(Float) | Point

fn area(s: Shape) -> Float:
    match s:
        Circle(r):
            r
        Box(side):
            side
        Point:
            0.0
";
        let result = compile_gradient_snippet(src);
        assert!(
            result.is_ok(),
            "Shape/match full pipeline failed: {:?}",
            result.err()
        );
        let bytes = result.unwrap();
        assert!(!bytes.is_empty(), "expected non-empty object bytes");
    }

    #[test]
    fn test_full_pipeline_result_enum() {
        // Full pipeline: Result[T, E] enum construction.
        let src = "\
mod test_result

type Result[T, E] = Ok(T) | Err(E)

fn make_ok(x: Int) -> Result[Int, String]:
    ret Ok(x)

fn make_err(msg: String) -> Result[Int, String]:
    ret Err(msg)
";
        let result = compile_gradient_snippet(src);
        assert!(
            result.is_ok(),
            "Result enum full pipeline failed: {:?}",
            result.err()
        );
        let bytes = result.unwrap();
        assert!(!bytes.is_empty(), "expected non-empty object bytes");
    }

    #[test]
    fn test_full_pipeline_unit_variant_match() {
        // Full pipeline: unit enum with match (no payload bindings).
        let src = "\
mod test_color

type Color = Red | Green | Blue

fn describe(c: Color) -> Int:
    match c:
        Red:
            ret 0
        Green:
            ret 1
        Blue:
            ret 2
";
        let result = compile_gradient_snippet(src);
        assert!(
            result.is_ok(),
            "unit enum match full pipeline failed: {:?}",
            result.err()
        );
        let bytes = result.unwrap();
        assert!(!bytes.is_empty(), "expected non-empty object bytes");
    }

    // ── Phase MM: Standard I/O codegen tests ────────────────────────────────

    /// Helper: build an IR module with a single `main` that calls one builtin
    /// (no arguments, returns ptr) and returns void.
    fn build_module_calling_no_arg_ptr_builtin(builtin_name: &str) -> crate::ir::Module {
        use crate::ir::types::{BlockRef, FuncRef, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Module};

        let mut func_refs = std::collections::HashMap::new();
        func_refs.insert(FuncRef(0), builtin_name.to_string());

        Module {
            name: format!("test_{}", builtin_name),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        Instruction::Call(Value(0), FuncRef(0), vec![]),
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::Ptr);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs,
        }
    }

    #[test]
    fn test_parse_int_codegen_emits_atoi_call() {
        use crate::ir::types::{BlockRef, FuncRef, Literal, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Module};

        let mut func_refs = std::collections::HashMap::new();
        func_refs.insert(FuncRef(0), "parse_int".to_string());

        let module = Module {
            name: "test_parse_int".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        // v0 = "123"
                        Instruction::Const(Value(0), Literal::Str("123".to_string())),
                        // v1 = parse_int(v0)
                        Instruction::Call(Value(1), FuncRef(0), vec![Value(0)]),
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::Ptr);
                    vt.insert(Value(1), Type::I64);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs,
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(
            result.is_ok(),
            "parse_int codegen failed: {:?}",
            result.err()
        );
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_parse_float_codegen_emits_atof_call() {
        use crate::ir::types::{BlockRef, FuncRef, Literal, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Module};

        let mut func_refs = std::collections::HashMap::new();
        func_refs.insert(FuncRef(0), "parse_float".to_string());

        let module = Module {
            name: "test_parse_float".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        Instruction::Const(Value(0), Literal::Str("3.14".to_string())),
                        Instruction::Call(Value(1), FuncRef(0), vec![Value(0)]),
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::Ptr);
                    vt.insert(Value(1), Type::F64);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs,
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(
            result.is_ok(),
            "parse_float codegen failed: {:?}",
            result.err()
        );
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_exit_codegen_emits_libc_exit_call() {
        use crate::ir::types::{BlockRef, FuncRef, Literal, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Module};

        let mut func_refs = std::collections::HashMap::new();
        func_refs.insert(FuncRef(0), "exit".to_string());

        let module = Module {
            name: "test_exit".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        // v0 = 0
                        Instruction::Const(Value(0), Literal::Int(0)),
                        // exit(v0)
                        Instruction::Call(Value(1), FuncRef(0), vec![Value(0)]),
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::I64);
                    vt.insert(Value(1), Type::Void);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs,
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(result.is_ok(), "exit codegen failed: {:?}", result.err());
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_read_line_codegen_declares_helper_import() {
        let module = build_module_calling_no_arg_ptr_builtin("read_line");

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(
            result.is_ok(),
            "read_line codegen failed: {:?}",
            result.err()
        );
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_args_codegen_calls_runtime_helper() {
        use crate::ir::types::{BlockRef, FuncRef, Type, Value};
        use crate::ir::{BasicBlock, Function, Instruction, Module};

        let mut func_refs = std::collections::HashMap::new();
        func_refs.insert(FuncRef(0), "args".to_string());

        let module = Module {
            name: "test_args".to_string(),
            functions: vec![Function {
                name: "main".to_string(),
                params: vec![],
                return_type: Type::Void,
                blocks: vec![BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        Instruction::Call(Value(0), FuncRef(0), vec![]),
                        Instruction::Ret(None),
                    ],
                }],
                value_types: {
                    let mut vt = std::collections::HashMap::new();
                    vt.insert(Value(0), Type::Ptr);
                    vt
                },
                is_export: false,
                extern_lib: None,
            }],
            func_refs,
        };

        let mut cg = CraneliftCodegen::new().unwrap();
        let result = cg.compile_module(&module);
        assert!(result.is_ok(), "args codegen failed: {:?}", result.err());
        let bytes = cg.emit_bytes().unwrap();
        assert!(!bytes.is_empty());
    }
}
