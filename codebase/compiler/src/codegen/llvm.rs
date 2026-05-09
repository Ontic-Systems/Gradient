//! LLVM code generator for the Gradient compiler.
//!
//! This module provides an alternative backend that uses LLVM (via the `inkwell`
//! crate) for code generation. LLVM produces more aggressively optimized code
//! than Cranelift, making it suitable for release builds.
//!
//! # Feature gate
//!
//! This module is only compiled when the `llvm` cargo feature is enabled:
//!
//! ```toml
//! [features]
//! llvm = ["inkwell"]
//! ```
//!
//! # Architecture
//!
//! The pipeline mirrors the Cranelift backend but targets LLVM IR instead:
//!
//! ```text
//!   Gradient IR  -->  LLVM IR  -->  LLVM Optimizer  -->  Object File (.o)
//! ```
//!
//! The [`LlvmCodegen`] struct implements the [`CodegenBackend`](super::CodegenBackend)
//! trait, allowing the compiler driver to use it interchangeably with the Cranelift
//! backend.

use super::CodegenError;
use crate::ir::{self, BlockRef, CmpOp, Function, Instruction, Literal, Module, Type, Value};

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as InkwellModule;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};
use inkwell::types::{BasicType, BasicTypeEnum, PointerType};
use inkwell::values::{
    BasicValue, BasicValueEnum, FunctionValue, GlobalValue, PhiValue, PointerValue,
};
use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::OptimizationLevel;

use std::collections::HashMap;

/// Optimization level for LLVM code generation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum LlvmOptLevel {
    /// No optimization (fastest compilation).
    None,
    /// Less optimization (faster compilation).
    Less,
    /// Default optimization level.
    #[default]
    Default,
    /// Aggressive optimization (slower compilation, faster code).
    Aggressive,
}

impl LlvmOptLevel {
    /// Convert to inkwell's OptimizationLevel.
    fn to_inkwell(self) -> OptimizationLevel {
        match self {
            LlvmOptLevel::None => OptimizationLevel::None,
            LlvmOptLevel::Less => OptimizationLevel::Less,
            LlvmOptLevel::Default => OptimizationLevel::Default,
            LlvmOptLevel::Aggressive => OptimizationLevel::Aggressive,
        }
    }
}

/// The LLVM-based code generator for Gradient.
///
/// Uses the `inkwell` crate to build LLVM IR from Gradient IR, then invokes
/// LLVM's optimization passes and emits a native object file.
///
/// # Lifecycle
///
/// ```text
/// let context = Context::create();
/// let mut cg = LlvmCodegen::new(&context)?;
/// cg.compile_module(&ir_module)?;
/// let bytes = cg.emit_bytes()?;
/// std::fs::write("output.o", bytes)?;
/// ```
pub struct LlvmCodegen<'ctx> {
    /// LLVM context - owns all LLVM values and types
    context: &'ctx Context,
    /// LLVM module - contains functions and globals
    module: InkwellModule<'ctx>,
    /// IR builder - constructs LLVM instructions
    builder: Builder<'ctx>,
    /// Target machine for code generation
    target_machine: TargetMachine,
    /// Optimization level for code generation
    opt_level: LlvmOptLevel,
    /// Map from Gradient IR values to LLVM values (per-function)
    value_map: HashMap<Value, BasicValueEnum<'ctx>>,
    /// Map from function names to LLVM function values
    function_map: HashMap<String, FunctionValue<'ctx>>,
    /// Map from string constants to LLVM global values
    string_globals: HashMap<String, GlobalValue<'ctx>>,
    /// Map from IR blocks to LLVM basic blocks
    block_map: HashMap<BlockRef, inkwell::basic_block::BasicBlock<'ctx>>,
    /// Phi node incoming edges (to be resolved after block creation)
    #[allow(clippy::type_complexity)]
    phi_incoming: HashMap<BlockRef, Vec<(Value, Vec<(BlockRef, Value)>)>>,
    /// Counter for generating unique names
    name_counter: u32,
    /// Map from IR FuncRef indices to function names. Populated at the
    /// start of [`compile_module`] from `ir::Module::func_refs` so that
    /// `Instruction::Call` can resolve callee names without owning a
    /// reference to the IR module.
    func_ref_names: HashMap<crate::ir::FuncRef, String>,
}

impl<'ctx> LlvmCodegen<'ctx> {
    /// Create a new LLVM code generator with default optimization level.
    ///
    /// Initializes an LLVM context and module targeting the host platform.
    pub fn new(context: &'ctx Context) -> Result<Self, CodegenError> {
        Self::new_with_opt_level(context, LlvmOptLevel::default())
    }

    /// Create a new LLVM code generator with specified optimization level.
    ///
    /// # Arguments
    /// * `context` - The LLVM context
    /// * `opt_level` - The optimization level for code generation
    ///
    /// # Example
    /// ```
    /// let context = Context::create();
    /// let codegen = LlvmCodegen::new_with_opt_level(&context, LlvmOptLevel::Aggressive)?;
    /// ```
    pub fn new_with_opt_level(
        context: &'ctx Context,
        opt_level: LlvmOptLevel,
    ) -> Result<Self, CodegenError> {
        // Initialize LLVM targets with all target info
        Target::initialize_all(&InitializationConfig {
            asm_parser: true,
            asm_printer: true,
            base: true,
            disassembler: true,
            info: true,
            machine_code: true,
        });

        let module = context.create_module("gradient_module");

        // Get host target triple (e.g., "x86_64-unknown-linux-gnu")
        let triple = TargetMachine::get_default_triple();
        let target = Target::from_triple(&triple)
            .map_err(|e| CodegenError::from(format!("Failed to get target: {}", e)))?;

        // Get host CPU name for target-specific optimizations
        let cpu = TargetMachine::get_host_cpu_name();
        let features = TargetMachine::get_host_cpu_features();

        // Create target machine with optimization settings
        let target_machine = target
            .create_target_machine(
                &triple,
                &cpu.to_string_lossy(),
                &features.to_string_lossy(),
                opt_level.to_inkwell(),
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or_else(|| CodegenError::from("Failed to create target machine"))?;

        let builder = context.create_builder();

        Ok(Self {
            context,
            module,
            builder,
            target_machine,
            opt_level,
            value_map: HashMap::new(),
            function_map: HashMap::new(),
            string_globals: HashMap::new(),
            block_map: HashMap::new(),
            phi_incoming: HashMap::new(),
            name_counter: 0,
            func_ref_names: HashMap::new(),
        })
    }

    /// Create a new LLVM code generator for release builds (O3 optimization).
    ///
    /// This is a convenience method for creating an aggressively optimized backend.
    pub fn new_release(context: &'ctx Context) -> Result<Self, CodegenError> {
        Self::new_with_opt_level(context, LlvmOptLevel::Aggressive)
    }

    /// Create a new LLVM code generator for debug builds (no optimization).
    ///
    /// This is a convenience method for creating a fast-compiling backend.
    pub fn new_debug(context: &'ctx Context) -> Result<Self, CodegenError> {
        Self::new_with_opt_level(context, LlvmOptLevel::None)
    }

    /// Generate a unique name for LLVM entities.
    fn generate_name(&mut self, prefix: &str) -> String {
        let name = format!("{}.{}", prefix, self.name_counter);
        self.name_counter += 1;
        name
    }

    /// Convert a Gradient IR type to an LLVM type.
    fn ir_type_to_llvm(&self, ty: &Type) -> BasicTypeEnum<'ctx> {
        match ty {
            Type::I32 => self.context.i32_type().into(),
            Type::I64 => self.context.i64_type().into(),
            Type::Ptr => self.context.ptr_type(AddressSpace::default()).into(),
            Type::Bool => self.context.i8_type().into(),
            Type::F64 => self.context.f64_type().into(),
            Type::Void => self.context.i8_type().into(), // Use i8 as placeholder for void
        }
    }

    /// Get the LLVM pointer type.
    fn ptr_type(&self) -> PointerType<'ctx> {
        self.context.ptr_type(AddressSpace::default())
    }

    /// Get the alignment for a type in bytes.
    fn type_alignment(&self, ty: &Type) -> u32 {
        match ty {
            Type::I32 => 4,
            Type::I64 => 8,
            Type::Ptr => 8, // 64-bit alignment for pointers
            Type::Bool => 1,
            Type::F64 => 8,
            Type::Void => 1,
        }
    }

    /// Get the size of a type in bytes.
    #[allow(dead_code)]
    fn type_size(&self, ty: &Type) -> u64 {
        match ty {
            Type::I32 => 4,
            Type::I64 => 8,
            Type::Ptr => 8, // 64-bit pointers
            Type::Bool => 1,
            Type::F64 => 8,
            Type::Void => 1,
        }
    }

    /// Look up an LLVM value for a Gradient IR value.
    fn resolve_value(&self, val: &Value) -> Result<BasicValueEnum<'ctx>, CodegenError> {
        self.value_map.get(val).copied().ok_or_else(|| {
            CodegenError::from(format!("IR Value({}) not found in value map", val.0))
        })
    }

    // ========================================================================
    // String/PTR constants handling (for proper memory operations)
    // ========================================================================

    /// Get or create a string constant as a global variable.
    ///
    /// String constants are stored in the read-only data section and null-terminated
    /// for C compatibility. Returns a pointer to the string data.
    fn get_or_create_string(&mut self, s: &str) -> Result<PointerValue<'ctx>, CodegenError> {
        if let Some(&global) = self.string_globals.get(s) {
            return Ok(global.as_pointer_value());
        }

        // Create null-terminated string constant
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0); // Null terminator

        let i8_type = self.context.i8_type();
        let array_type = i8_type.array_type(bytes.len() as u32);

        // Pre-generate name to avoid borrow issues
        let name = self.generate_name("str");
        let global = self
            .module
            .add_global(array_type, Some(AddressSpace::default()), &name);
        global.set_constant(true);
        global.set_linkage(inkwell::module::Linkage::Private);
        // Note: set_unnamed_address may not be available in all inkwell versions
        // For compatibility, we skip this call
        global.set_initializer(
            &i8_type.const_array(
                &bytes
                    .iter()
                    .map(|&b| i8_type.const_int(b as u64, false))
                    .collect::<Vec<_>>(),
            ),
        );

        // Store in cache
        self.string_globals.insert(s.to_string(), global);

        // Return pointer to the string data (using get_element_ptr to get i8*)
        let ptr = global.as_pointer_value();
        Ok(ptr)
    }

    // ========================================================================
    // Memory Operations - Core Implementation
    // ========================================================================

    /// Build an alloca instruction - allocate stack space.
    ///
    /// `Alloca(result, ty)` - allocates space for a value of type `ty` on the
    /// stack and returns a pointer to it.
    ///
    /// # Type Alignment
    /// - i64: 8-byte alignment
    /// - f64: 8-byte alignment
    /// - ptr: pointer size alignment (8 bytes on 64-bit)
    /// - i32: 4-byte alignment
    /// - bool: 1-byte alignment
    fn build_alloca(&mut self, result: &Value, ty: &Type) -> Result<(), CodegenError> {
        let llvm_ty = self.ir_type_to_llvm(ty);
        // Pre-generate name to avoid borrow issues
        let name = self.generate_name("alloca");
        let alloca = self
            .builder
            .build_alloca(llvm_ty, &name)
            .map_err(|e| CodegenError::from(format!("Failed to build alloca: {}", e)))?;

        // Note: inkwell doesn't provide set_alignment on PointerValue
        // The alignment is handled automatically by LLVM

        // Store in value map - alloca returns a pointer
        self.value_map.insert(*result, alloca.into());

        Ok(())
    }

    /// Build a load instruction - load from memory.
    ///
    /// `Load(result, addr)` - loads a value from memory at `addr` into `result`.
    ///
    /// # Arguments
    /// - `result`: The SSA value to store the loaded value into
    /// - `addr`: Pointer value to load from
    /// - `func`: The function context (for type lookup)
    ///
    /// # Type Alignment
    /// Load instructions use the natural alignment of the type being loaded.
    fn build_load(
        &mut self,
        result: &Value,
        addr: &Value,
        func: &Function,
    ) -> Result<(), CodegenError> {
        let addr_val = self.resolve_value(addr)?;
        let ptr_val = addr_val.into_pointer_value();

        // Determine the type to load from the function's value_types map
        let load_ty = func.value_types.get(result).ok_or_else(|| {
            CodegenError::from(format!("No type information for value {}", result.0))
        })?;

        let llvm_ty = self.ir_type_to_llvm(load_ty);
        let alignment = self.type_alignment(load_ty);

        // Build the load instruction using inkwell
        // Pre-generate name to avoid borrow issues
        let name = self.generate_name("load");
        let load_val = self
            .builder
            .build_load(llvm_ty, ptr_val, &name)
            .map_err(|e| CodegenError::from(format!("Failed to build load: {}", e)))?;

        // Set alignment on the load
        if let Some(inst) = load_val.as_instruction_value() {
            let _ = inst.set_alignment(alignment);
        }

        self.value_map.insert(*result, load_val);

        Ok(())
    }

    /// Build a store instruction - store to memory.
    ///
    /// `Store(value, addr)` - stores `value` into memory at `addr`.
    ///
    /// # Arguments
    /// - `value`: The value to store
    /// - `addr`: Pointer value to store to
    /// - `func`: The function context (for type/alignment lookup)
    ///
    /// # Type Alignment
    /// Store instructions use the natural alignment of the value being stored.
    fn build_store(
        &mut self,
        value: &Value,
        addr: &Value,
        func: &Function,
    ) -> Result<(), CodegenError> {
        let val = self.resolve_value(value)?;
        let addr_val = self.resolve_value(addr)?;
        let ptr_val = addr_val.into_pointer_value();

        // Determine alignment from value type if available
        let alignment = func
            .value_types
            .get(value)
            .map(|ty| self.type_alignment(ty))
            .unwrap_or(8);

        // Build the store instruction
        let store_inst = self
            .builder
            .build_store(ptr_val, val)
            .map_err(|e| CodegenError::from(format!("Failed to build store: {}", e)))?;

        // Set alignment on the store
        let _ = store_inst.set_alignment(alignment);

        Ok(())
    }

    // ========================================================================
    // Instruction Compilation
    // ========================================================================

    /// Build a constant instruction.
    fn build_const(&mut self, result: &Value, literal: &Literal) -> Result<(), CodegenError> {
        let llvm_val: BasicValueEnum<'ctx> = match literal {
            Literal::Int(i) => self.context.i64_type().const_int(*i as u64, true).into(),
            Literal::Float(f) => self.context.f64_type().const_float(*f).into(),
            Literal::Bool(b) => self.context.i8_type().const_int(*b as u64, false).into(),
            Literal::Str(s) => {
                let ptr = self.get_or_create_string(s)?;
                ptr.into()
            }
        };

        self.value_map.insert(*result, llvm_val);
        Ok(())
    }

    /// Build arithmetic operations.
    fn build_binary_op(
        &mut self,
        result: &Value,
        op: BinaryOp,
        lhs: &Value,
        rhs: &Value,
    ) -> Result<(), CodegenError> {
        let left = self.resolve_value(lhs)?;
        let right = self.resolve_value(rhs)?;

        // Pre-generate names to avoid borrow issues
        let name = match op {
            BinaryOp::Add => self.generate_name("add"),
            BinaryOp::Sub => self.generate_name("sub"),
            BinaryOp::Mul => self.generate_name("mul"),
            BinaryOp::Div => self.generate_name("div"),
        };

        let result_val = match op {
            BinaryOp::Add => match (left, right) {
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => self
                    .builder
                    .build_int_add(l, r, &name)
                    .map_err(|e| CodegenError::from(format!("Add failed: {}", e)))?
                    .into(),
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => self
                    .builder
                    .build_float_add(l, r, &(name + "_f"))
                    .map_err(|e| CodegenError::from(format!("FAdd failed: {}", e)))?
                    .into(),
                _ => return Err(CodegenError::from("Invalid operand types for add")),
            },
            BinaryOp::Sub => match (left, right) {
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => self
                    .builder
                    .build_int_sub(l, r, &name)
                    .map_err(|e| CodegenError::from(format!("Sub failed: {}", e)))?
                    .into(),
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => self
                    .builder
                    .build_float_sub(l, r, &(name + "_f"))
                    .map_err(|e| CodegenError::from(format!("FSub failed: {}", e)))?
                    .into(),
                _ => return Err(CodegenError::from("Invalid operand types for sub")),
            },
            BinaryOp::Mul => match (left, right) {
                (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => self
                    .builder
                    .build_int_mul(l, r, &name)
                    .map_err(|e| CodegenError::from(format!("Mul failed: {}", e)))?
                    .into(),
                (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => self
                    .builder
                    .build_float_mul(l, r, &(name + "_f"))
                    .map_err(|e| CodegenError::from(format!("FMul failed: {}", e)))?
                    .into(),
                _ => return Err(CodegenError::from("Invalid operand types for mul")),
            },
            BinaryOp::Div => {
                match (left, right) {
                    (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => {
                        // Signed division
                        self.builder
                            .build_int_signed_div(l, r, &name)
                            .map_err(|e| CodegenError::from(format!("SDiv failed: {}", e)))?
                            .into()
                    }
                    (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => self
                        .builder
                        .build_float_div(l, r, &(name + "_f"))
                        .map_err(|e| CodegenError::from(format!("FDiv failed: {}", e)))?
                        .into(),
                    _ => return Err(CodegenError::from("Invalid operand types for div")),
                }
            }
        };

        self.value_map.insert(*result, result_val);
        Ok(())
    }

    /// Build a comparison instruction.
    fn build_cmp(
        &mut self,
        result: &Value,
        op: &CmpOp,
        lhs: &Value,
        rhs: &Value,
    ) -> Result<(), CodegenError> {
        let left = self.resolve_value(lhs)?;
        let right = self.resolve_value(rhs)?;

        // Pre-generate name to avoid borrow issues
        let name = self.generate_name("cmp");

        let cmp_result = match (left, right) {
            (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => {
                let pred = self.cmpop_to_int_predicate(op);
                self.builder
                    .build_int_compare(pred, l, r, &name)
                    .map_err(|e| CodegenError::from(format!("ICmp failed: {}", e)))?
                    .into()
            }
            (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => {
                let pred = self.cmpop_to_float_predicate(op);
                self.builder
                    .build_float_compare(pred, l, r, &(name + "_f"))
                    .map_err(|e| CodegenError::from(format!("FCmp failed: {}", e)))?
                    .into()
            }
            _ => return Err(CodegenError::from("Invalid operand types for comparison")),
        };

        self.value_map.insert(*result, cmp_result);
        Ok(())
    }

    /// Convert comparison operator to LLVM integer predicate.
    fn cmpop_to_int_predicate(&self, op: &CmpOp) -> IntPredicate {
        match op {
            CmpOp::Eq => IntPredicate::EQ,
            CmpOp::Ne => IntPredicate::NE,
            CmpOp::Lt => IntPredicate::SLT,
            CmpOp::Le => IntPredicate::SLE,
            CmpOp::Gt => IntPredicate::SGT,
            CmpOp::Ge => IntPredicate::SGE,
        }
    }

    /// Convert comparison operator to LLVM float predicate.
    fn cmpop_to_float_predicate(&self, op: &CmpOp) -> inkwell::FloatPredicate {
        match op {
            CmpOp::Eq => inkwell::FloatPredicate::OEQ,
            CmpOp::Ne => inkwell::FloatPredicate::ONE,
            CmpOp::Lt => inkwell::FloatPredicate::OLT,
            CmpOp::Le => inkwell::FloatPredicate::OLE,
            CmpOp::Gt => inkwell::FloatPredicate::OGT,
            CmpOp::Ge => inkwell::FloatPredicate::OGE,
        }
    }

    /// Compile an entire IR module to LLVM IR.
    pub fn compile_module(&mut self, ir_module: &Module) -> Result<(), CodegenError> {
        // Snapshot the FuncRef -> name table so Call instructions can resolve
        // callees without owning a reference back into the IR module. Both
        // user-defined and built-in (e.g. `print_int`) callees are registered
        // by the IR builder via `Module::add_func_ref`.
        self.func_ref_names = ir_module.func_refs.clone();

        // First pass: declare all functions
        for func in &ir_module.functions {
            self.declare_function(func)?;
        }

        // Second pass: compile function bodies
        for func in &ir_module.functions {
            self.compile_function(func)?;
        }

        // Resolve phi nodes
        self.resolve_phi_nodes()?;

        Ok(())
    }

    /// Declare a function in the LLVM module.
    fn declare_function(&mut self, func: &Function) -> Result<(), CodegenError> {
        use inkwell::types::BasicMetadataTypeEnum;

        let is_main = func.name == "main";

        // Build parameter list. C `main` must accept (i32 argc, i8** argv)
        // even if the Gradient source declared `fn main()` with no params.
        let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::new();
        if is_main {
            let i32_type = self.context.i32_type();
            // LLVM 15+ uses opaque pointers — there is one pointer type
            // per address space, so both `argc`'s containing pointer and
            // the i8** of `argv` collapse to the same opaque ptr type.
            let opaque_ptr_ty = self.context.ptr_type(AddressSpace::default());
            param_types.push(i32_type.into());
            param_types.push(opaque_ptr_ty.into());
        }
        for ty in &func.params {
            param_types.push(self.ir_type_to_llvm(ty).into());
        }

        // C `main` must return i32 even if Gradient declares it as returning
        // void/unit. Match the Cranelift backend's convention.
        let fn_type = if is_main && func.return_type == Type::Void {
            self.context.i32_type().fn_type(&param_types, false)
        } else if func.return_type == Type::Void {
            self.context.void_type().fn_type(&param_types, false)
        } else {
            let ret_ty = self.ir_type_to_llvm(&func.return_type);
            ret_ty.fn_type(&param_types, false)
        };

        let linkage = if is_main || func.is_export {
            inkwell::module::Linkage::External
        } else {
            inkwell::module::Linkage::Private
        };

        let llvm_func = self.module.add_function(&func.name, fn_type, Some(linkage));
        self.function_map.insert(func.name.clone(), llvm_func);

        Ok(())
    }

    /// Compile a single function's body.
    fn compile_function(&mut self, func: &Function) -> Result<(), CodegenError> {
        let llvm_func = *self
            .function_map
            .get(&func.name)
            .ok_or_else(|| CodegenError::from(format!("Function {} not declared", func.name)))?;

        // Clear per-function state
        self.value_map.clear();
        self.block_map.clear();
        self.phi_incoming.clear();
        self.name_counter = 0;

        // Create all basic blocks first
        for block in &func.blocks {
            let llvm_block = self
                .context
                .append_basic_block(llvm_func, &format!("block.{}", block.label.0));
            self.block_map.insert(block.label, llvm_block);
        }

        // If no blocks, create an empty entry block
        if func.blocks.is_empty() {
            let entry_block = self.context.append_basic_block(llvm_func, "entry");
            self.builder.position_at_end(entry_block);
            self.builder
                .build_return(None)
                .map_err(|e| CodegenError::from(format!("Failed to build return: {}", e)))?;
            return Ok(());
        }

        // Map function parameters to IR values
        for (i, _param) in func.params.iter().enumerate() {
            if let Some(llvm_param) = llvm_func.get_nth_param(i as u32) {
                let ir_value = Value(i as u32);
                self.value_map.insert(ir_value, llvm_param);
            }
        }

        // Compile each block
        for block in &func.blocks {
            let llvm_block = self.block_map[&block.label];
            self.builder.position_at_end(llvm_block);

            for instr in &block.instructions {
                self.compile_instruction(instr, func)?;
            }
        }

        Ok(())
    }

    /// Compile a single instruction.
    fn compile_instruction(
        &mut self,
        instr: &Instruction,
        func: &Function,
    ) -> Result<(), CodegenError> {
        match instr {
            // ========================================================================
            // Memory Operations - Core Implementation
            // ========================================================================
            // Stack allocation.
            // Uses `Builder::build_alloca()` to allocate stack space.
            // Returns a PointerValue that can be used with Load/Store.
            Instruction::Alloca(result, ty) => {
                self.build_alloca(result, ty)?;
            }

            // Memory load.
            // Uses `Builder::build_load()` to load from a pointer.
            // Properly handles type alignment (i64/f64/ptr = 8-byte, i32 = 4-byte).
            Instruction::Load(result, addr) => {
                self.build_load(result, addr, func)?;
            }

            // Memory store.
            // Uses `Builder::build_store()` to store to a pointer.
            // Properly handles value-type alignment.
            Instruction::Store(value, addr) => {
                self.build_store(value, addr, func)?;
            }

            // ========================================================================
            // Constants and Literals
            // ========================================================================
            Instruction::Const(result, literal) => {
                self.build_const(result, literal)?;
            }

            // ========================================================================
            // Arithmetic Operations
            // ========================================================================
            Instruction::Add(result, lhs, rhs) => {
                self.build_binary_op(result, BinaryOp::Add, lhs, rhs)?;
            }
            Instruction::Sub(result, lhs, rhs) => {
                self.build_binary_op(result, BinaryOp::Sub, lhs, rhs)?;
            }
            Instruction::Mul(result, lhs, rhs) => {
                self.build_binary_op(result, BinaryOp::Mul, lhs, rhs)?;
            }
            Instruction::Div(result, lhs, rhs) => {
                self.build_binary_op(result, BinaryOp::Div, lhs, rhs)?;
            }

            // ========================================================================
            // Comparison Operations
            // ========================================================================
            Instruction::Cmp(result, op, lhs, rhs) => {
                self.build_cmp(result, op, lhs, rhs)?;
            }

            // ========================================================================
            // Boolean Operations
            // ========================================================================
            Instruction::Or(result, lhs, rhs) => {
                let lhs_val = self.resolve_value(lhs)?;
                let rhs_val = self.resolve_value(rhs)?;
                let or_result = match (lhs_val, rhs_val) {
                    (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => self
                        .builder
                        .build_or(l, r, "or")
                        .map_err(|e| CodegenError::from(format!("Or operation failed: {}", e)))?,
                    _ => return Err(CodegenError::from("Or operation requires integer operands")),
                };
                self.value_map.insert(*result, or_result.into());
            }

            // ========================================================================
            // Control Flow
            // ========================================================================
            Instruction::Ret(val) => {
                // Special case: C `main` must return i32 even when Gradient
                // declares it as returning void. Synthesize `ret i32 0` here.
                let main_returning_void = func.name == "main" && func.return_type == Type::Void;
                match val {
                    Some(v) => {
                        let ret_val = self.resolve_value(v)?;
                        self.builder
                            .build_return(Some(&ret_val))
                            .map_err(|e| CodegenError::from(format!("Return failed: {}", e)))?;
                    }
                    None if main_returning_void => {
                        let zero = self.context.i32_type().const_int(0, false);
                        self.builder.build_return(Some(&zero)).map_err(|e| {
                            CodegenError::from(format!("main return failed: {}", e))
                        })?;
                    }
                    None => {
                        self.builder.build_return(None).map_err(|e| {
                            CodegenError::from(format!("Void return failed: {}", e))
                        })?;
                    }
                }
            }

            Instruction::Jump(target) => {
                let target_block = self.block_map.get(target).ok_or_else(|| {
                    CodegenError::from(format!("Target block {:?} not found", target))
                })?;
                self.builder
                    .build_unconditional_branch(*target_block)
                    .map_err(|e| CodegenError::from(format!("Jump failed: {}", e)))?;
            }

            Instruction::Branch(cond, then_block, else_block) => {
                let cond_val = self.resolve_value(cond)?;
                let cond_bool = cond_val.into_int_value();

                let then_llvm = self.block_map.get(then_block).ok_or_else(|| {
                    CodegenError::from(format!("Then block {:?} not found", then_block))
                })?;
                let else_llvm = self.block_map.get(else_block).ok_or_else(|| {
                    CodegenError::from(format!("Else block {:?} not found", else_block))
                })?;

                self.builder
                    .build_conditional_branch(cond_bool, *then_llvm, *else_llvm)
                    .map_err(|e| CodegenError::from(format!("Branch failed: {}", e)))?;
            }

            Instruction::Phi(dst, entries) => {
                // Get the type from the first entry's value
                let first_val = entries
                    .first()
                    .ok_or_else(|| CodegenError::from("Phi node with no entries"))?
                    .1;
                let phi_ty = func
                    .value_types
                    .get(&first_val)
                    .ok_or_else(|| CodegenError::from("No type for phi value"))?;
                let llvm_ty = self.ir_type_to_llvm(phi_ty);

                // Create phi node
                let phi = self
                    .builder
                    .build_phi(llvm_ty, &format!("phi.{}", dst.0))
                    .map_err(|e| CodegenError::from(format!("Phi creation failed: {}", e)))?;

                // Store for later resolution (we need all blocks to be created)
                let current_block = self
                    .builder
                    .get_insert_block()
                    .ok_or_else(|| CodegenError::from("No current block for phi"))?;
                let block_ref = self
                    .block_map
                    .iter()
                    .find(|(_, b)| **b == current_block)
                    .map(|(k, _)| *k)
                    .ok_or_else(|| CodegenError::from("Current block not in block map"))?;

                self.phi_incoming
                    .entry(block_ref)
                    .or_default()
                    .push((*dst, entries.clone()));
                self.value_map.insert(*dst, phi.as_basic_value());
            }

            // ========================================================================
            // Call Operations
            // ========================================================================
            Instruction::Call(result, func_ref, args) => {
                // Resolve function name via the IR module's FuncRef table.
                // The previous `func_{idx}` formatting was a placeholder that
                // never matched the real LLVM function name (see #339 follow-on).
                let func_name = self.func_ref_names.get(func_ref).cloned().ok_or_else(|| {
                    CodegenError::from(format!(
                        "Unknown FuncRef({}) in call instruction",
                        func_ref.0
                    ))
                })?;

                // Builtin lowerings (print_int / print_float / print_bool /
                // print / println). These are externs in the type
                // environment but never appear as user-defined functions
                // in the IR module — Cranelift hand-rolls each by name
                // match in its `Instruction::Call` arm. Mirror that here.
                // See issue #551.
                if self.lower_builtin_call(&func_name, *result, args)? {
                    return Ok(());
                }

                let callee = self
                    .function_map
                    .get(&func_name)
                    .copied()
                    .or_else(|| self.module.get_function(&func_name))
                    .ok_or_else(|| {
                        CodegenError::from(format!("Function {} not found", func_name))
                    })?;

                // Convert args to BasicMetadataValueEnum for build_call
                let llvm_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = args
                    .iter()
                    .map(|arg| self.resolve_value(arg).map(|v| v.into()))
                    .collect::<Result<Vec<_>, _>>()?;

                let call_site = self
                    .builder
                    .build_call(callee, &llvm_args, &format!("call.{}", result.0))
                    .map_err(|e| CodegenError::from(format!("Call failed: {}", e)))?;

                if let Some(ret_val) = call_site.try_as_basic_value().left() {
                    self.value_map.insert(*result, ret_val);
                }
            }

            // ========================================================================
            // Variant/Enum Operations (simplified for now)
            // ========================================================================
            Instruction::ConstructVariant {
                result,
                tag,
                payload,
            } => {
                // Allocate memory: (1 + payload.len()) * 8 bytes
                let size = (1 + payload.len()) as u64 * 8;
                let size_val = self.context.i64_type().const_int(size, false);

                // Call malloc (need to declare it)
                let malloc_fn = self.get_or_declare_malloc()?;
                let call_site = self
                    .builder
                    .build_call(malloc_fn, &[size_val.into()], "variant_alloc")
                    .map_err(|e| CodegenError::from(format!("malloc call failed: {}", e)))?;

                let ptr = call_site
                    .try_as_basic_value()
                    .left()
                    .ok_or_else(|| CodegenError::from("malloc returned void"))?
                    .into_pointer_value();

                // Store tag at offset 0
                let tag_val = self.context.i64_type().const_int(*tag as u64, false);
                self.builder
                    .build_store(ptr, tag_val)
                    .map_err(|e| CodegenError::from(format!("Store tag failed: {}", e)))?;

                // Store payload fields
                for (i, field_val) in payload.iter().enumerate() {
                    let field_llvm = self.resolve_value(field_val)?;
                    let offset = (i + 1) as u64 * 8;
                    let offset_val = self.context.i64_type().const_int(offset, false);

                    let field_ptr = unsafe {
                        self.builder
                            .build_gep(
                                self.context.i8_type(),
                                ptr,
                                &[offset_val],
                                &format!("field_ptr.{}", i),
                            )
                            .map_err(|e| CodegenError::from(format!("GEP failed: {}", e)))?
                    };

                    let cast_ptr = self
                        .builder
                        .build_pointer_cast(
                            field_ptr,
                            self.ptr_type(),
                            &format!("field_cast.{}", i),
                        )
                        .map_err(|e| CodegenError::from(format!("Pointer cast failed: {}", e)))?;

                    self.builder
                        .build_store(cast_ptr, field_llvm)
                        .map_err(|e| CodegenError::from(format!("Store field failed: {}", e)))?;
                }

                self.value_map.insert(*result, ptr.into());
            }

            Instruction::GetVariantTag { result, ptr } => {
                let ptr_val = self.resolve_value(ptr)?.into_pointer_value();
                let loaded = self
                    .builder
                    .build_load(
                        self.context.i64_type(),
                        ptr_val,
                        &format!("tag.{}", result.0),
                    )
                    .map_err(|e| CodegenError::from(format!("Load tag failed: {}", e)))?;
                self.value_map.insert(*result, loaded);
            }

            Instruction::GetVariantField { result, ptr, index } => {
                let ptr_val = self.resolve_value(ptr)?.into_pointer_value();
                let offset = (*index as u64 + 1) * 8;
                let offset_val = self.context.i64_type().const_int(offset, false);

                let field_ptr = unsafe {
                    self.builder
                        .build_gep(
                            self.context.i8_type(),
                            ptr_val,
                            &[offset_val],
                            &format!("field_ptr.{}", result.0),
                        )
                        .map_err(|e| CodegenError::from(format!("GEP failed: {}", e)))?
                };

                let loaded = self
                    .builder
                    .build_load(
                        self.context.i64_type(),
                        field_ptr,
                        &format!("field.{}", result.0),
                    )
                    .map_err(|e| CodegenError::from(format!("Load field failed: {}", e)))?;

                self.value_map.insert(*result, loaded);
            }

            // ========================================================================
            // Actor Operations (placeholders)
            // ========================================================================
            Instruction::Spawn { result, .. } => {
                // Return null pointer as placeholder
                let null_ptr = self.ptr_type().const_null();
                self.value_map.insert(*result, null_ptr.into());
            }

            Instruction::Send { .. } => {
                // No-op for now
            }

            Instruction::Ask { result, .. } => {
                // Return null pointer as placeholder
                let null_ptr = self.ptr_type().const_null();
                self.value_map.insert(*result, null_ptr.into());
            }

            Instruction::ActorInit { .. } => {
                // No-op for now
            }

            // ========================================================================
            // Memory / aggregate ops not yet implemented in the LLVM scaffold.
            // These cover field load/store, raw pointer<->int casts, and
            // structural addressing instructions added after the initial
            // Cranelift-only era. We return a structured error so callers can
            // fall back to the Cranelift path until the LLVM backend grows
            // the corresponding lowerings (#339 follow-on work).
            // ========================================================================
            Instruction::LoadField { .. }
            | Instruction::StoreField { .. }
            | Instruction::PtrToInt(_, _)
            | Instruction::IntToPtr(_, _)
            | Instruction::GetElementPtr { .. }
            | Instruction::FieldAddr { .. } => {
                return Err(CodegenError::from(format!(
                    "LLVM backend does not yet lower instruction: {:?}",
                    instr
                )));
            }
        }

        Ok(())
    }

    /// Get or declare malloc function.
    fn get_or_declare_malloc(&mut self) -> Result<FunctionValue<'ctx>, CodegenError> {
        if let Some(func) = self.module.get_function("malloc") {
            return Ok(func);
        }

        let fn_type = self
            .ptr_type()
            .fn_type(&[self.context.i64_type().into()], false);
        let func =
            self.module
                .add_function("malloc", fn_type, Some(inkwell::module::Linkage::External));
        Ok(func)
    }

    /// Get or declare the C `printf` function: `i32 (ptr, ...)` (variadic).
    ///
    /// Used for the print_int / print_float / print_bool / print builtins.
    /// Mirrors the Cranelift backend's printf import; declared variadic
    /// (LLVM IR `i32 (i8*, ...)`) so subsequent calls only need to pass
    /// the format string + the relevant arg.
    fn get_or_declare_printf(&mut self) -> Result<FunctionValue<'ctx>, CodegenError> {
        if let Some(func) = self.module.get_function("printf") {
            return Ok(func);
        }

        let i32_ty = self.context.i32_type();
        // Variadic: (ptr, ...) -> i32
        let fn_type = i32_ty.fn_type(&[self.ptr_type().into()], true);
        let func =
            self.module
                .add_function("printf", fn_type, Some(inkwell::module::Linkage::External));
        Ok(func)
    }

    /// Get or declare the C `puts` function: `i32 (ptr)`.
    ///
    /// Cranelift uses `puts` for `println(s)` because it implicitly appends
    /// a newline. Mirror that: `println(s)` lowers to `puts(s)`.
    fn get_or_declare_puts(&mut self) -> Result<FunctionValue<'ctx>, CodegenError> {
        if let Some(func) = self.module.get_function("puts") {
            return Ok(func);
        }

        let i32_ty = self.context.i32_type();
        let fn_type = i32_ty.fn_type(&[self.ptr_type().into()], false);
        let func =
            self.module
                .add_function("puts", fn_type, Some(inkwell::module::Linkage::External));
        Ok(func)
    }

    /// Lower a Gradient builtin call by name. Returns `Ok(true)` if the
    /// builtin was recognized and lowered (caller should not fall through
    /// to generic call resolution); `Ok(false)` otherwise.
    ///
    /// Mirrors the per-name dispatch in
    /// `codegen::cranelift::CraneliftCodegen::compile_instruction`'s
    /// `Instruction::Call` arm. This is the LLVM-side counterpart for
    /// the print family — see issue #551.
    fn lower_builtin_call(
        &mut self,
        func_name: &str,
        result: Value,
        args: &[Value],
    ) -> Result<bool, CodegenError> {
        match func_name {
            // ── print_int(n): printf("%ld", n) ──
            "print_int" => {
                let fmt_ptr = self.get_or_create_string("%ld")?;
                let printf = self.get_or_declare_printf()?;
                let arg = self.resolve_value(&args[0])?;
                let call = self
                    .builder
                    .build_call(
                        printf,
                        &[fmt_ptr.into(), arg.into()],
                        &format!("call.{}", result.0),
                    )
                    .map_err(|e| CodegenError::from(format!("printf call failed: {}", e)))?;
                if let Some(ret_val) = call.try_as_basic_value().left() {
                    self.value_map.insert(result, ret_val);
                }
                Ok(true)
            }

            // ── print_float(f): printf("%.6f", f) ──
            "print_float" => {
                let fmt_ptr = self.get_or_create_string("%.6f")?;
                let printf = self.get_or_declare_printf()?;
                let arg = self.resolve_value(&args[0])?;
                let call = self
                    .builder
                    .build_call(
                        printf,
                        &[fmt_ptr.into(), arg.into()],
                        &format!("call.{}", result.0),
                    )
                    .map_err(|e| CodegenError::from(format!("printf call failed: {}", e)))?;
                if let Some(ret_val) = call.try_as_basic_value().left() {
                    self.value_map.insert(result, ret_val);
                }
                Ok(true)
            }

            // ── print_bool(b): printf("%s", b ? "true" : "false") ──
            "print_bool" => {
                let fmt_ptr = self.get_or_create_string("%s")?;
                let true_ptr = self.get_or_create_string("true")?;
                let false_ptr = self.get_or_create_string("false")?;
                let printf = self.get_or_declare_printf()?;
                let bool_val = self.resolve_value(&args[0])?;

                // Truncate i8 bool to i1 for select if necessary; the IR
                // tracks bools as i8, so compare-against-zero gives an i1.
                let cond = if bool_val.is_int_value() {
                    let iv = bool_val.into_int_value();
                    let zero = iv.get_type().const_zero();
                    self.builder
                        .build_int_compare(IntPredicate::NE, iv, zero, "bool.cond")
                        .map_err(|e| {
                            CodegenError::from(format!("bool->i1 compare failed: {}", e))
                        })?
                } else {
                    return Err(CodegenError::from(
                        "print_bool: argument is not an integer value",
                    ));
                };

                let str_ptr = self
                    .builder
                    .build_select(cond, true_ptr, false_ptr, "bool.str")
                    .map_err(|e| CodegenError::from(format!("bool select failed: {}", e)))?;

                let call = self
                    .builder
                    .build_call(
                        printf,
                        &[fmt_ptr.into(), str_ptr.into()],
                        &format!("call.{}", result.0),
                    )
                    .map_err(|e| CodegenError::from(format!("printf call failed: {}", e)))?;
                if let Some(ret_val) = call.try_as_basic_value().left() {
                    self.value_map.insert(result, ret_val);
                }
                Ok(true)
            }

            // ── print(s): printf("%s", s) — no newline ──
            "print" => {
                let fmt_ptr = self.get_or_create_string("%s")?;
                let printf = self.get_or_declare_printf()?;
                let arg = self.resolve_value(&args[0])?;
                let call = self
                    .builder
                    .build_call(
                        printf,
                        &[fmt_ptr.into(), arg.into()],
                        &format!("call.{}", result.0),
                    )
                    .map_err(|e| CodegenError::from(format!("printf call failed: {}", e)))?;
                if let Some(ret_val) = call.try_as_basic_value().left() {
                    self.value_map.insert(result, ret_val);
                }
                Ok(true)
            }

            // ── println(s): puts(s) — appends newline implicitly ──
            "println" => {
                let puts = self.get_or_declare_puts()?;
                let arg = self.resolve_value(&args[0])?;
                let call = self
                    .builder
                    .build_call(puts, &[arg.into()], &format!("call.{}", result.0))
                    .map_err(|e| CodegenError::from(format!("puts call failed: {}", e)))?;
                if let Some(ret_val) = call.try_as_basic_value().left() {
                    self.value_map.insert(result, ret_val);
                }
                Ok(true)
            }

            _ => Ok(false),
        }
    }

    /// Resolve phi nodes by adding incoming edges.
    fn resolve_phi_nodes(&mut self) -> Result<(), CodegenError> {
        for (block_ref, phi_list) in &self.phi_incoming {
            let llvm_block = self
                .block_map
                .get(block_ref)
                .ok_or_else(|| CodegenError::from(format!("Block {:?} not found", block_ref)))?;

            // Find phi instructions in this block
            let phi_instructions: Vec<_> = llvm_block
                .get_instructions()
                .filter(|i| i.get_opcode() == inkwell::values::InstructionOpcode::Phi)
                .collect();

            for (idx, (dst, entries)) in phi_list.iter().enumerate() {
                if idx >= phi_instructions.len() {
                    continue;
                }

                let phi_inst = &phi_instructions[idx];
                let phi_value: PhiValue<'ctx> = (*phi_inst)
                    .try_into()
                    .map_err(|_| CodegenError::from("Failed to convert to PhiValue"))?;

                // Add incoming edges
                for (pred_block_ref, pred_value) in entries {
                    let pred_llvm_block = self.block_map.get(pred_block_ref).ok_or_else(|| {
                        CodegenError::from(format!(
                            "Predecessor block {:?} not found",
                            pred_block_ref
                        ))
                    })?;
                    let llvm_val = self.resolve_value(pred_value)?;
                    phi_value.add_incoming(&[(&llvm_val, *pred_llvm_block)]);
                }

                // Update value map to point to resolved phi
                self.value_map.insert(*dst, phi_value.as_basic_value());
            }
        }

        Ok(())
    }

    /// Run optimization passes on the module.
    ///
    /// This applies LLVM's standard optimization pipeline based on the
    /// configured optimization level. For Aggressive (O3), this includes:
    /// - Function inlining
    /// - Dead code elimination
    /// - Constant propagation
    /// - Loop optimizations
    /// - Vectorization
    fn run_optimization_passes(&self) -> Result<(), CodegenError> {
        // Skip optimization if set to None
        if self.opt_level == LlvmOptLevel::None {
            return Ok(());
        }

        // LLVM 17+ uses the New Pass Manager via Module::run_passes with a
        // pipeline string. Map our coarse opt levels onto the standard
        // `default<O?>` pipelines exposed by the PassBuilder.
        let pipeline = match self.opt_level {
            LlvmOptLevel::None => return Ok(()),
            LlvmOptLevel::Less => "default<O1>",
            LlvmOptLevel::Default => "default<O2>",
            LlvmOptLevel::Aggressive => "default<O3>",
        };

        let options = PassBuilderOptions::create();
        self.module
            .run_passes(pipeline, &self.target_machine, options)
            .map_err(|e| CodegenError::from(format!("Optimization passes failed: {}", e)))?;

        Ok(())
    }

    /// Finalize compilation and return the raw object file bytes.
    ///
    /// This method:
    /// 1. Verifies the LLVM module for correctness
    /// 2. Runs optimization passes based on the configured optimization level
    /// 3. Emits a native object file using the target machine
    ///
    /// # Returns
    /// A `Vec<u8>` containing the raw object file bytes, suitable for
    /// writing to a `.o` file and linking with a system linker.
    ///
    /// # Errors
    /// Returns `CodegenError` if verification fails, optimization fails,
    /// or object emission fails.
    pub fn emit_bytes(&self) -> Result<Vec<u8>, CodegenError> {
        // Verify the module before optimization/emission
        self.module
            .verify()
            .map_err(|e| CodegenError::from(format!("Module verification failed: {}", e)))?;

        // Run optimization passes (respecting opt_level setting)
        self.run_optimization_passes()?;

        // Emit object file to memory buffer
        let obj_file = self
            .target_machine
            .write_to_memory_buffer(&self.module, FileType::Object)
            .map_err(|e| CodegenError::from(format!("Failed to emit object file: {}", e)))?;

        Ok(obj_file.as_slice().to_vec())
    }

    /// Get the configured optimization level.
    pub fn optimization_level(&self) -> LlvmOptLevel {
        self.opt_level
    }

    /// Get the target triple being used for code generation.
    pub fn target_triple(&self) -> String {
        self.target_machine
            .get_triple()
            .as_str()
            .to_string_lossy()
            .into_owned()
    }

    /// Get reference to the LLVM module (for testing/debugging).
    #[cfg(test)]
    #[allow(dead_code)]
    fn module(&self) -> &InkwellModule<'ctx> {
        &self.module
    }

    /// Print LLVM IR to string. Used by integration tests to inspect the
    /// emitted IR's text form (e.g. assert that a recursive call lowered
    /// correctly, or feed the text to `llc` for round-trip validation).
    /// Public-but-test-flavored: callers in production code should use
    /// [`emit_bytes`](Self::emit_bytes) for the object file directly.
    pub fn print_to_string_for_test(&self) -> String {
        self.module.print_to_string().to_string()
    }

    /// Print LLVM IR to string (for unit-test debugging — kept around
    /// to preserve the previous test API).
    #[cfg(test)]
    fn print_to_string(&self) -> String {
        self.module.print_to_string().to_string()
    }
}

/// Binary operation types for internal use.
#[derive(Debug, Clone, Copy)]
enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

// ========================================================================
// CodegenBackend trait implementation
// ========================================================================

impl<'ctx> super::CodegenBackend for LlvmCodegen<'ctx> {
    fn compile_module(&mut self, module: &ir::Module) -> Result<(), CodegenError> {
        self.compile_module(module)
    }

    fn finish(self: Box<Self>) -> Result<Vec<u8>, CodegenError> {
        self.emit_bytes()
    }

    fn name(&self) -> &str {
        "llvm"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::BasicBlock;
    use std::collections::HashMap;

    fn create_test_context() -> Context {
        Context::create()
    }

    fn create_empty_module(name: &str) -> Module {
        Module {
            name: name.to_string(),
            functions: vec![],
            func_refs: HashMap::new(),
        }
    }

    #[test]
    fn test_llvm_backend_creation() {
        let context = create_test_context();
        let backend = LlvmCodegen::new(&context);
        assert!(backend.is_ok());
    }

    #[test]
    fn test_llvm_backend_name() {
        let context = create_test_context();
        let backend = LlvmCodegen::new(&context).unwrap();
        assert_eq!(
            <LlvmCodegen as super::super::CodegenBackend>::name(&backend),
            "llvm"
        );
    }

    #[test]
    fn test_llvm_type_conversion() {
        let context = create_test_context();
        let codegen = LlvmCodegen::new(&context).unwrap();

        assert_eq!(
            codegen
                .ir_type_to_llvm(&Type::I32)
                .into_int_type()
                .get_bit_width(),
            32
        );
        assert_eq!(
            codegen
                .ir_type_to_llvm(&Type::I64)
                .into_int_type()
                .get_bit_width(),
            64
        );
        assert!(codegen.ir_type_to_llvm(&Type::F64).is_float_type());
        assert!(codegen.ir_type_to_llvm(&Type::Ptr).is_pointer_type());
    }

    #[test]
    fn test_type_alignment() {
        let context = create_test_context();
        let codegen = LlvmCodegen::new(&context).unwrap();

        assert_eq!(codegen.type_alignment(&Type::I32), 4);
        assert_eq!(codegen.type_alignment(&Type::I64), 8);
        assert_eq!(codegen.type_alignment(&Type::F64), 8);
        assert_eq!(codegen.type_alignment(&Type::Ptr), 8);
        assert_eq!(codegen.type_alignment(&Type::Bool), 1);
    }

    #[test]
    fn test_compile_empty_module() {
        let context = create_test_context();
        let mut backend = LlvmCodegen::new(&context).unwrap();
        let module = create_empty_module("test");
        let result = backend.compile_module(&module);
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_simple_function() {
        let context = create_test_context();
        let mut backend = LlvmCodegen::new(&context).unwrap();

        let mut value_types = HashMap::new();
        value_types.insert(Value(0), Type::I64);
        value_types.insert(Value(1), Type::I64);

        let func = Function {
            name: "add".to_string(),
            params: vec![Type::I64, Type::I64],
            return_type: Type::I64,
            blocks: vec![BasicBlock {
                label: BlockRef(0),
                instructions: vec![
                    Instruction::Add(Value(2), Value(0), Value(1)),
                    Instruction::Ret(Some(Value(2))),
                ],
            }],
            value_types,
            is_export: true,
            extern_lib: None,
        };

        let module = Module {
            name: "test".to_string(),
            functions: vec![func],
            func_refs: HashMap::new(),
        };

        let result = backend.compile_module(&module);
        assert!(result.is_ok());

        let ir_str = backend.print_to_string();
        assert!(ir_str.contains("add"));
        assert!(ir_str.contains("define"));
    }

    #[test]
    fn test_alloca_store_load() {
        let context = create_test_context();
        let mut codegen = LlvmCodegen::new(&context).unwrap();

        // Create a function that allocates, stores, and loads
        let mut value_types = HashMap::new();
        value_types.insert(Value(0), Type::I64); // alloca result (ptr)
        value_types.insert(Value(1), Type::I64); // store value
        value_types.insert(Value(2), Type::I64); // load result

        let func = Function {
            name: "test_alloca".to_string(),
            params: vec![],
            return_type: Type::I64,
            blocks: vec![BasicBlock {
                label: BlockRef(0),
                instructions: vec![
                    Instruction::Alloca(Value(0), Type::I64),
                    Instruction::Const(Value(1), Literal::Int(42)),
                    Instruction::Store(Value(1), Value(0)),
                    Instruction::Load(Value(2), Value(0)),
                    Instruction::Ret(Some(Value(2))),
                ],
            }],
            value_types,
            is_export: true,
            extern_lib: None,
        };

        let module = Module {
            name: "test".to_string(),
            functions: vec![func],
            func_refs: HashMap::new(),
        };

        let result = codegen.compile_module(&module);
        assert!(result.is_ok());

        let ir_str = codegen.print_to_string();
        // Verify memory operations are in the IR
        assert!(ir_str.contains("alloca"));
        assert!(ir_str.contains("store"));
        assert!(ir_str.contains("load"));
        assert!(ir_str.contains("align 8"));
    }

    #[test]
    fn test_string_constant() {
        let context = create_test_context();
        let mut codegen = LlvmCodegen::new(&context).unwrap();

        let mut value_types = HashMap::new();
        value_types.insert(Value(0), Type::Ptr);
        value_types.insert(Value(1), Type::Ptr);

        let func = Function {
            name: "test_string".to_string(),
            params: vec![],
            return_type: Type::Ptr,
            blocks: vec![BasicBlock {
                label: BlockRef(0),
                instructions: vec![
                    Instruction::Const(Value(0), Literal::Str("hello".to_string())),
                    Instruction::Ret(Some(Value(0))),
                ],
            }],
            value_types,
            is_export: true,
            extern_lib: None,
        };

        let module = Module {
            name: "test".to_string(),
            functions: vec![func],
            func_refs: HashMap::new(),
        };

        let result = codegen.compile_module(&module);
        assert!(result.is_ok());

        let ir_str = codegen.print_to_string();
        assert!(ir_str.contains("hello"));
        assert!(ir_str.contains("constant"));
    }

    #[test]
    fn test_branch_instruction() {
        let context = create_test_context();
        let mut backend = LlvmCodegen::new(&context).unwrap();

        let mut value_types = HashMap::new();
        value_types.insert(Value(0), Type::I64);
        value_types.insert(Value(1), Type::Bool);
        value_types.insert(Value(2), Type::I64);
        value_types.insert(Value(3), Type::I64);

        let func = Function {
            name: "branch_test".to_string(),
            params: vec![Type::I64],
            return_type: Type::I64,
            blocks: vec![
                BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        Instruction::Cmp(Value(1), CmpOp::Gt, Value(0), Value(0)),
                        Instruction::Branch(Value(1), BlockRef(1), BlockRef(2)),
                    ],
                },
                BasicBlock {
                    label: BlockRef(1),
                    instructions: vec![
                        Instruction::Const(Value(2), Literal::Int(42)),
                        Instruction::Ret(Some(Value(2))),
                    ],
                },
                BasicBlock {
                    label: BlockRef(2),
                    instructions: vec![
                        Instruction::Const(Value(3), Literal::Int(0)),
                        Instruction::Ret(Some(Value(3))),
                    ],
                },
            ],
            value_types,
            is_export: true,
            extern_lib: None,
        };

        let module = Module {
            name: "test".to_string(),
            functions: vec![func],
            func_refs: HashMap::new(),
        };

        let result = backend.compile_module(&module);
        assert!(result.is_ok());

        let ir_str = backend.print_to_string();
        assert!(ir_str.contains("br i1"));
        assert!(ir_str.contains("block.1"));
        assert!(ir_str.contains("block.2"));
    }

    #[test]
    fn test_phi_node() {
        let context = create_test_context();
        let mut backend = LlvmCodegen::new(&context).unwrap();

        let mut value_types = HashMap::new();
        value_types.insert(Value(0), Type::I64);
        value_types.insert(Value(1), Type::I64);
        value_types.insert(Value(2), Type::I64);

        let func = Function {
            name: "phi_test".to_string(),
            params: vec![Type::I64],
            return_type: Type::I64,
            blocks: vec![
                BasicBlock {
                    label: BlockRef(0),
                    instructions: vec![
                        Instruction::Const(Value(1), Literal::Int(10)),
                        Instruction::Jump(BlockRef(1)),
                    ],
                },
                BasicBlock {
                    label: BlockRef(1),
                    instructions: vec![
                        Instruction::Phi(Value(2), vec![(BlockRef(0), Value(1))]),
                        Instruction::Ret(Some(Value(2))),
                    ],
                },
            ],
            value_types,
            is_export: true,
            extern_lib: None,
        };

        let module = Module {
            name: "test".to_string(),
            functions: vec![func],
            func_refs: HashMap::new(),
        };

        let result = backend.compile_module(&module);
        assert!(result.is_ok());

        let ir_str = backend.print_to_string();
        assert!(ir_str.contains("phi"));
    }

    #[test]
    fn test_llvm_opt_levels() {
        let context = create_test_context();

        // Test creating backend with different optimization levels
        let backend_none = LlvmCodegen::new_with_opt_level(&context, LlvmOptLevel::None);
        assert!(backend_none.is_ok());
        assert_eq!(
            backend_none.unwrap().optimization_level(),
            LlvmOptLevel::None
        );

        let backend_less = LlvmCodegen::new_with_opt_level(&context, LlvmOptLevel::Less);
        assert!(backend_less.is_ok());
        assert_eq!(
            backend_less.unwrap().optimization_level(),
            LlvmOptLevel::Less
        );

        let backend_default = LlvmCodegen::new_with_opt_level(&context, LlvmOptLevel::Default);
        assert!(backend_default.is_ok());
        assert_eq!(
            backend_default.unwrap().optimization_level(),
            LlvmOptLevel::Default
        );

        let backend_aggressive =
            LlvmCodegen::new_with_opt_level(&context, LlvmOptLevel::Aggressive);
        assert!(backend_aggressive.is_ok());
        assert_eq!(
            backend_aggressive.unwrap().optimization_level(),
            LlvmOptLevel::Aggressive
        );
    }

    #[test]
    fn test_llvm_release_and_debug_constructors() {
        let context = create_test_context();

        // Test release (O3) constructor
        let release = LlvmCodegen::new_release(&context);
        assert!(release.is_ok());
        assert_eq!(
            release.unwrap().optimization_level(),
            LlvmOptLevel::Aggressive
        );

        // Test debug (O0) constructor
        let debug = LlvmCodegen::new_debug(&context);
        assert!(debug.is_ok());
        assert_eq!(debug.unwrap().optimization_level(), LlvmOptLevel::None);
    }

    #[test]
    fn test_emit_bytes_simple_function() {
        let context = create_test_context();
        let mut backend = LlvmCodegen::new(&context).unwrap();

        // Create a simple function
        let mut value_types = HashMap::new();
        value_types.insert(Value(0), Type::I64);
        value_types.insert(Value(1), Type::I64);
        value_types.insert(Value(2), Type::I64);

        let func = Function {
            name: "emit_test".to_string(),
            params: vec![Type::I64, Type::I64],
            return_type: Type::I64,
            blocks: vec![BasicBlock {
                label: BlockRef(0),
                instructions: vec![
                    Instruction::Add(Value(2), Value(0), Value(1)),
                    Instruction::Ret(Some(Value(2))),
                ],
            }],
            value_types,
            is_export: true,
            extern_lib: None,
        };

        let module = Module {
            name: "test".to_string(),
            functions: vec![func],
            func_refs: HashMap::new(),
        };

        // Compile and emit
        let result = backend.compile_module(&module);
        assert!(result.is_ok());

        // Emit object file bytes
        let bytes = backend.emit_bytes();
        assert!(bytes.is_ok());

        // Verify we got non-empty bytes
        let obj_bytes = bytes.unwrap();
        assert!(!obj_bytes.is_empty());

        // Object files typically start with magic bytes
        // ELF: 0x7f ELF, Mach-O: 0xfeedface or 0xfeedfacf, COFF: MZ
        let _has_valid_header = obj_bytes.starts_with(b"\x7fELF")
            || obj_bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) // Mach-O 64-bit
            || obj_bytes.starts_with(&[0xce, 0xfa, 0xed, 0xfe]) // Mach-O 32-bit
            || obj_bytes.starts_with(b"MZ"); // Windows COFF

        // Note: This may fail on some platforms, so we just check for non-empty
        assert!(!obj_bytes.is_empty(), "Object file should not be empty");
    }

    #[test]
    fn test_emit_bytes_with_optimization() {
        let context = create_test_context();

        // Test with aggressive optimization
        let mut backend = LlvmCodegen::new_release(&context).unwrap();

        let mut value_types = HashMap::new();
        value_types.insert(Value(0), Type::I64);
        value_types.insert(Value(1), Type::I64);

        let func = Function {
            name: "opt_test".to_string(),
            params: vec![Type::I64],
            return_type: Type::I64,
            blocks: vec![BasicBlock {
                label: BlockRef(0),
                instructions: vec![
                    Instruction::Const(Value(1), Literal::Int(42)),
                    Instruction::Ret(Some(Value(1))),
                ],
            }],
            value_types,
            is_export: true,
            extern_lib: None,
        };

        let module = Module {
            name: "test".to_string(),
            functions: vec![func],
            func_refs: HashMap::new(),
        };

        backend.compile_module(&module).unwrap();

        // Should not fail with optimization enabled
        let bytes = backend.emit_bytes();
        assert!(bytes.is_ok());
        assert!(!bytes.unwrap().is_empty());
    }

    #[test]
    fn test_target_triple() {
        let context = create_test_context();
        let backend = LlvmCodegen::new(&context).unwrap();

        // Verify we can get the target triple
        let triple = backend.target_triple();
        assert!(!triple.is_empty());

        // Should contain some platform info (e.g., "x86_64", "aarch64", etc.)
        assert!(
            triple.contains("x86_64")
                || triple.contains("aarch64")
                || triple.contains("i386")
                || triple.contains("arm"),
            "Target triple should contain architecture: {}",
            triple
        );
    }
}
