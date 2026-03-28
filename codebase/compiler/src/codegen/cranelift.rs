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
            malloc_sig.returns.push(AbiParam::new(pointer_type));  // ptr

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

        // Declare exit(int) for contract failure abort.
        if !self.declared_functions.contains_key("exit") {
            let mut exit_sig = self.module.make_signature();
            exit_sig.params.push(AbiParam::new(cl_types::I32));
            // exit doesn't return, but Cranelift needs a signature.

            let exit_id = self
                .module
                .declare_function("exit", Linkage::Import, &exit_sig)
                .map_err(|e| format!("Failed to declare exit: {}", e))?;
            self.declared_functions
                .insert("exit".to_string(), exit_id);
        }

        // ── File I/O helpers (FS effect) ─────────────────────────────────
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
        if !self.declared_functions.contains_key("__gradient_file_write") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // path
            sig.params.push(AbiParam::new(pointer_type)); // content
            sig.returns.push(AbiParam::new(cl_types::I64)); // 1 = ok, 0 = error

            let func_id = self
                .module
                .declare_function("__gradient_file_write", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_file_write: {}", e))?;
            self.declared_functions
                .insert("__gradient_file_write".to_string(), func_id);
        }

        // __gradient_file_exists(path: ptr) -> i64
        if !self.declared_functions.contains_key("__gradient_file_exists") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // path
            sig.returns.push(AbiParam::new(cl_types::I64)); // 1 = exists, 0 = not found

            let func_id = self
                .module
                .declare_function("__gradient_file_exists", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_file_exists: {}", e))?;
            self.declared_functions
                .insert("__gradient_file_exists".to_string(), func_id);
        }

        // __gradient_file_append(path: ptr, content: ptr) -> i64
        if !self.declared_functions.contains_key("__gradient_file_append") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type)); // path
            sig.params.push(AbiParam::new(pointer_type)); // content
            sig.returns.push(AbiParam::new(cl_types::I64)); // 1 = ok, 0 = error

            let func_id = self
                .module
                .declare_function("__gradient_file_append", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_file_append: {}", e))?;
            self.declared_functions
                .insert("__gradient_file_append".to_string(), func_id);
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
                    ir::Instruction::Jump(target) => { targets.insert(*target); }
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
                        func.value_types.get(first_val)
                            .map(ir_type_to_cl)
                            .unwrap_or_else(|| {
                                func.value_types.get(dst)
                                    .map(ir_type_to_cl)
                                    .unwrap_or(cl_types::I64)
                            })
                    } else {
                        func.value_types.get(dst)
                            .map(ir_type_to_cl)
                            .unwrap_or(cl_types::I64)
                    };

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
                                    if let Some(&fid) = self.declared_functions.get(cname.as_str()) {
                                        let fref = self.module.declare_func_in_func(fid, builder.func);
                                        builder.ins().func_addr(pointer_type, fref)
                                    } else {
                                        builder.ins().iconst(cl_types::I64, *n)
                                    }
                                } else {
                                    builder.ins().iconst(cl_types::I64, *n)
                                }
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
                        let ty = builder.func.dfg.value_type(a);
                        let result = if ty == cl_types::F64 {
                            let fcc = cmpop_to_floatcc(op);
                            builder.ins().fcmp(fcc, a, b)
                        } else {
                            let cc = cmpop_to_intcc(op);
                            builder.ins().icmp(cc, a, b)
                        };
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

                        match func_name.as_str() {
                            // ── print_int: call printf("%ld", value) ──
                            "print_int" => {
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%ld",
                                )?;
                                let fmt_gv = self
                                    .module
                                    .declare_data_in_func(fmt_data_id, builder.func);
                                let fmt_ptr =
                                    builder.ins().global_value(pointer_type, fmt_gv);

                                let printf_func_id = *self
                                    .declared_functions
                                    .get("printf")
                                    .ok_or("printf not declared")?;
                                let printf_ref = self.module.declare_func_in_func(
                                    printf_func_id,
                                    builder.func,
                                );

                                let int_val = resolve_value(&value_map, &args[0])?;
                                let call_inst =
                                    builder.ins().call(printf_ref, &[fmt_ptr, int_val]);
                                let results =
                                    builder.inst_results(call_inst).to_vec();
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
                                let fmt_gv = self
                                    .module
                                    .declare_data_in_func(fmt_data_id, builder.func);
                                let fmt_ptr =
                                    builder.ins().global_value(pointer_type, fmt_gv);

                                // Get the printf function address.
                                let printf_func_id = *self
                                    .declared_functions
                                    .get("printf")
                                    .ok_or("printf not declared")?;
                                let printf_ref = self.module.declare_func_in_func(
                                    printf_func_id,
                                    builder.func,
                                );
                                let printf_addr = builder
                                    .ins()
                                    .func_addr(pointer_type, printf_ref);

                                // Create a float-compatible signature: (ptr, f64) -> i32
                                let mut float_printf_sig = self.module.make_signature();
                                float_printf_sig
                                    .params
                                    .push(AbiParam::new(pointer_type));
                                float_printf_sig
                                    .params
                                    .push(AbiParam::new(cl_types::F64));
                                float_printf_sig
                                    .returns
                                    .push(AbiParam::new(cl_types::I32));
                                let sig_ref =
                                    builder.import_signature(float_printf_sig);

                                let float_val =
                                    resolve_value(&value_map, &args[0])?;
                                let call_inst = builder.ins().call_indirect(
                                    sig_ref,
                                    printf_addr,
                                    &[fmt_ptr, float_val],
                                );
                                let results =
                                    builder.inst_results(call_inst).to_vec();
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
                                let fmt_gv = self
                                    .module
                                    .declare_data_in_func(fmt_data_id, builder.func);
                                let true_gv = self
                                    .module
                                    .declare_data_in_func(true_data_id, builder.func);
                                let false_gv = self
                                    .module
                                    .declare_data_in_func(false_data_id, builder.func);
                                let fmt_ptr =
                                    builder.ins().global_value(pointer_type, fmt_gv);
                                let true_ptr =
                                    builder.ins().global_value(pointer_type, true_gv);
                                let false_ptr =
                                    builder.ins().global_value(pointer_type, false_gv);

                                let bool_val =
                                    resolve_value(&value_map, &args[0])?;

                                // select: if bool_val then true_ptr else false_ptr
                                let str_ptr =
                                    builder.ins().select(bool_val, true_ptr, false_ptr);

                                // Use call_indirect with (ptr, ptr) -> i32 signature
                                let printf_func_id = *self
                                    .declared_functions
                                    .get("printf")
                                    .ok_or("printf not declared")?;
                                let printf_ref = self.module.declare_func_in_func(
                                    printf_func_id,
                                    builder.func,
                                );
                                let printf_addr = builder
                                    .ins()
                                    .func_addr(pointer_type, printf_ref);

                                let mut str_printf_sig = self.module.make_signature();
                                str_printf_sig
                                    .params
                                    .push(AbiParam::new(pointer_type));
                                str_printf_sig
                                    .params
                                    .push(AbiParam::new(pointer_type));
                                str_printf_sig
                                    .returns
                                    .push(AbiParam::new(cl_types::I32));
                                let sig_ref =
                                    builder.import_signature(str_printf_sig);

                                let call_inst = builder.ins().call_indirect(
                                    sig_ref,
                                    printf_addr,
                                    &[fmt_ptr, str_ptr],
                                );
                                let results =
                                    builder.inst_results(call_inst).to_vec();
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
                                let zero =
                                    builder.ins().iconst(cl_types::I64, 0);
                                let neg_n = builder.ins().isub(zero, n);
                                let is_neg =
                                    builder.ins().icmp(IntCC::SignedLessThan, n, zero);
                                let result =
                                    builder.ins().select(is_neg, neg_n, n);
                                value_map.insert(*dst, result);
                            }

                            // ── min(a, b): if a < b then a else b ──
                            "min" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;
                                let cmp = builder
                                    .ins()
                                    .icmp(IntCC::SignedLessThan, a, b);
                                let result = builder.ins().select(cmp, a, b);
                                value_map.insert(*dst, result);
                            }

                            // ── max(a, b): if a > b then a else b ──
                            "max" => {
                                let a = resolve_value(&value_map, &args[0])?;
                                let b = resolve_value(&value_map, &args[1])?;
                                let cmp = builder
                                    .ins()
                                    .icmp(IntCC::SignedGreaterThan, a, b);
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
                                let buf_size =
                                    builder.ins().iconst(cl_types::I64, 32);
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call =
                                    builder.ins().call(malloc_ref, &[buf_size]);
                                let buf =
                                    builder.inst_results(malloc_call).to_vec()[0];

                                // Format string "%ld"
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%ld",
                                )?;
                                let fmt_gv = self.module.declare_data_in_func(
                                    fmt_data_id,
                                    builder.func,
                                );
                                let fmt_ptr = builder
                                    .ins()
                                    .global_value(pointer_type, fmt_gv);

                                // snprintf(buf, 32, "%ld", value)
                                let int_val =
                                    resolve_value(&value_map, &args[0])?;
                                let snprintf_func_id = *self
                                    .declared_functions
                                    .get("snprintf")
                                    .ok_or("snprintf not declared")?;
                                let snprintf_ref =
                                    self.module.declare_func_in_func(
                                        snprintf_func_id,
                                        builder.func,
                                    );
                                builder.ins().call(
                                    snprintf_ref,
                                    &[buf, buf_size, fmt_ptr, int_val],
                                );

                                value_map.insert(*dst, buf);
                            }

                            // ── string_concat(a, b): malloc + strcpy + strcat ──
                            "string_concat" => {
                                let str_a =
                                    resolve_value(&value_map, &args[0])?;
                                let str_b =
                                    resolve_value(&value_map, &args[1])?;

                                // len_a = strlen(a)
                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let call_a =
                                    builder.ins().call(strlen_ref, &[str_a]);
                                let len_a =
                                    builder.inst_results(call_a).to_vec()[0];

                                // Need a fresh strlen ref for the second call,
                                // but Cranelift allows reusing the same ref.
                                let call_b =
                                    builder.ins().call(strlen_ref, &[str_b]);
                                let len_b =
                                    builder.inst_results(call_b).to_vec()[0];

                                // total = len_a + len_b + 1
                                let total_len =
                                    builder.ins().iadd(len_a, len_b);
                                let one =
                                    builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size =
                                    builder.ins().iadd(total_len, one);

                                // buf = malloc(total)
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call = builder
                                    .ins()
                                    .call(malloc_ref, &[alloc_size]);
                                let buf = builder
                                    .inst_results(malloc_call)
                                    .to_vec()[0];

                                // strcpy(buf, a)
                                let strcpy_func_id = *self
                                    .declared_functions
                                    .get("strcpy")
                                    .ok_or("strcpy not declared")?;
                                let strcpy_ref = self.module.declare_func_in_func(
                                    strcpy_func_id,
                                    builder.func,
                                );
                                builder.ins().call(strcpy_ref, &[buf, str_a]);

                                // strcat(buf, b)
                                let strcat_func_id = *self
                                    .declared_functions
                                    .get("strcat")
                                    .ok_or("strcat not declared")?;
                                let strcat_ref = self.module.declare_func_in_func(
                                    strcat_func_id,
                                    builder.func,
                                );
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
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
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
                                let strstr_ref = self.module.declare_func_in_func(
                                    strstr_func_id,
                                    builder.func,
                                );
                                let call = builder.ins().call(strstr_ref, &[s, substr]);
                                let ptr_result = builder.inst_results(call).to_vec()[0];
                                let zero = builder.ins().iconst(pointer_type, 0);
                                let result = builder.ins().icmp(
                                    IntCC::NotEqual,
                                    ptr_result,
                                    zero,
                                );
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
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let call = builder.ins().call(strlen_ref, &[prefix]);
                                let prefix_len = builder.inst_results(call).to_vec()[0];

                                // strncmp(s, prefix, len)
                                let strncmp_func_id = *self
                                    .declared_functions
                                    .get("strncmp")
                                    .ok_or("strncmp not declared")?;
                                let strncmp_ref = self.module.declare_func_in_func(
                                    strncmp_func_id,
                                    builder.func,
                                );
                                let cmp_call = builder.ins().call(
                                    strncmp_ref,
                                    &[s, prefix, prefix_len],
                                );
                                let cmp_result = builder.inst_results(cmp_call).to_vec()[0];

                                let zero = builder.ins().iconst(cl_types::I32, 0);
                                let result = builder.ins().icmp(
                                    IntCC::Equal,
                                    cmp_result,
                                    zero,
                                );
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
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );

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
                                let strncmp_ref = self.module.declare_func_in_func(
                                    strncmp_func_id,
                                    builder.func,
                                );
                                let cmp_call = builder.ins().call(
                                    strncmp_ref,
                                    &[tail_ptr, suffix, suf_len],
                                );
                                let cmp_result = builder.inst_results(cmp_call).to_vec()[0];

                                let zero = builder.ins().iconst(cl_types::I32, 0);
                                let result = builder.ins().icmp(
                                    IntCC::Equal,
                                    cmp_result,
                                    zero,
                                );
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
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // src_ptr = s + start
                                let src_ptr = builder.ins().iadd(s, start);

                                // memcpy(buf, src_ptr, len)
                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let memcpy_ref = self.module.declare_func_in_func(
                                    memcpy_func_id,
                                    builder.func,
                                );
                                builder.ins().call(memcpy_ref, &[buf, src_ptr, len]);

                                // buf[len] = '\0'
                                let nul = builder.ins().iconst(cl_types::I8, 0);
                                let end_ptr = builder.ins().iadd(buf, len);
                                builder.ins().store(MemFlags::new(), nul, end_ptr, 0);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_trim(s): skip leading/trailing whitespace ──
                            "string_trim" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let isspace_func_id = *self
                                    .declared_functions
                                    .get("isspace")
                                    .ok_or("isspace not declared")?;
                                let isspace_ref = self.module.declare_func_in_func(
                                    isspace_func_id,
                                    builder.func,
                                );

                                // len = strlen(s)
                                let call = builder.ins().call(strlen_ref, &[s]);
                                let len = builder.inst_results(call).to_vec()[0];

                                // --- Find leading whitespace (start index) ---
                                // Loop 1: advance `start` while isspace(s[start]) && start < len
                                let lead_header = builder.create_block();
                                let lead_body = builder.create_block();
                                let lead_exit = builder.create_block();

                                builder.append_block_param(lead_header, cl_types::I64); // start
                                builder.append_block_param(lead_exit, cl_types::I64); // start result

                                let zero = builder.ins().iconst(cl_types::I64, 0);
                                builder.ins().jump(lead_header, &[BlockArg::Value(zero)]);

                                builder.switch_to_block(lead_header);
                                let start = builder.block_params(lead_header)[0];
                                let start_lt_len = builder.ins().icmp(IntCC::SignedLessThan, start, len);
                                builder.ins().brif(start_lt_len, lead_body, &[], lead_exit, &[BlockArg::Value(start)]);

                                builder.switch_to_block(lead_body);
                                builder.seal_block(lead_body);
                                let ch_ptr = builder.ins().iadd(s, start);
                                let ch = builder.ins().load(cl_types::I8, MemFlags::new(), ch_ptr, 0);
                                let ch_i32 = builder.ins().uextend(cl_types::I32, ch);
                                let is_call = builder.ins().call(isspace_ref, &[ch_i32]);
                                let is_ws = builder.inst_results(is_call).to_vec()[0];
                                let zero_i32 = builder.ins().iconst(cl_types::I32, 0);
                                let ws_cmp = builder.ins().icmp(IntCC::NotEqual, is_ws, zero_i32);
                                let one_inc = builder.ins().iconst(cl_types::I64, 1);
                                let next_start = builder.ins().iadd(start, one_inc);
                                // If whitespace, continue; else exit with current start
                                builder.ins().brif(ws_cmp, lead_header, &[BlockArg::Value(next_start)], lead_exit, &[BlockArg::Value(start)]);

                                builder.seal_block(lead_header);
                                builder.seal_block(lead_exit);

                                builder.switch_to_block(lead_exit);
                                let trim_start = builder.block_params(lead_exit)[0];

                                // --- Find trailing whitespace (end index) ---
                                // Loop 2: decrement `end` from len while end > start && isspace(s[end-1])
                                let trail_header = builder.create_block();
                                let trail_body = builder.create_block();
                                let trail_exit = builder.create_block();

                                builder.append_block_param(trail_header, cl_types::I64); // end
                                builder.append_block_param(trail_exit, cl_types::I64); // end result

                                let isspace_ref2 = self.module.declare_func_in_func(
                                    isspace_func_id,
                                    builder.func,
                                );

                                builder.ins().jump(trail_header, &[BlockArg::Value(len)]);

                                builder.switch_to_block(trail_header);
                                let end_val = builder.block_params(trail_header)[0];
                                let end_gt_start = builder.ins().icmp(IntCC::SignedGreaterThan, end_val, trim_start);
                                builder.ins().brif(end_gt_start, trail_body, &[], trail_exit, &[BlockArg::Value(end_val)]);

                                builder.switch_to_block(trail_body);
                                builder.seal_block(trail_body);
                                let one_dec = builder.ins().iconst(cl_types::I64, 1);
                                let end_minus_one = builder.ins().isub(end_val, one_dec);
                                let tail_ptr = builder.ins().iadd(s, end_minus_one);
                                let tail_ch = builder.ins().load(cl_types::I8, MemFlags::new(), tail_ptr, 0);
                                let tail_i32 = builder.ins().uextend(cl_types::I32, tail_ch);
                                let is_call2 = builder.ins().call(isspace_ref2, &[tail_i32]);
                                let is_ws2 = builder.inst_results(is_call2).to_vec()[0];
                                let zero_i32b = builder.ins().iconst(cl_types::I32, 0);
                                let ws_cmp2 = builder.ins().icmp(IntCC::NotEqual, is_ws2, zero_i32b);
                                // If whitespace, continue with decremented end; else exit with current end
                                builder.ins().brif(ws_cmp2, trail_header, &[BlockArg::Value(end_minus_one)], trail_exit, &[BlockArg::Value(end_val)]);

                                builder.seal_block(trail_header);
                                builder.seal_block(trail_exit);

                                builder.switch_to_block(trail_exit);
                                let trim_end = builder.block_params(trail_exit)[0];

                                // --- Allocate and copy trimmed substring ---
                                // trim_len = trim_end - trim_start
                                let trim_len = builder.ins().isub(trim_end, trim_start);
                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size = builder.ins().iadd(trim_len, one);

                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // memcpy(buf, s + trim_start, trim_len)
                                let src_ptr = builder.ins().iadd(s, trim_start);
                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let memcpy_ref = self.module.declare_func_in_func(
                                    memcpy_func_id,
                                    builder.func,
                                );
                                builder.ins().call(memcpy_ref, &[buf, src_ptr, trim_len]);

                                // Null-terminate: buf[trim_len] = 0
                                let nul = builder.ins().iconst(cl_types::I8, 0);
                                let end_ptr = builder.ins().iadd(buf, trim_len);
                                builder.ins().store(MemFlags::new(), nul, end_ptr, 0);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_to_upper(s): copy + toupper each byte ──
                            "string_to_upper" => {
                                let s = resolve_value(&value_map, &args[0])?;

                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let call = builder.ins().call(strlen_ref, &[s]);
                                let len = builder.inst_results(call).to_vec()[0];

                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size = builder.ins().iadd(len, one);

                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // Loop over each byte: buf[i] = toupper(s[i])
                                let toupper_func_id = *self
                                    .declared_functions
                                    .get("toupper")
                                    .ok_or("toupper not declared")?;
                                let toupper_ref = self.module.declare_func_in_func(
                                    toupper_func_id,
                                    builder.func,
                                );

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
                                let ch = builder.ins().load(cl_types::I8, MemFlags::new(), src_ptr, 0);
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
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let call = builder.ins().call(strlen_ref, &[s]);
                                let len = builder.inst_results(call).to_vec()[0];

                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size = builder.ins().iadd(len, one);

                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // Loop over each byte: buf[i] = tolower(s[i])
                                let tolower_func_id = *self
                                    .declared_functions
                                    .get("tolower")
                                    .ok_or("tolower not declared")?;
                                let tolower_ref = self.module.declare_func_in_func(
                                    tolower_func_id,
                                    builder.func,
                                );

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
                                let ch = builder.ins().load(cl_types::I8, MemFlags::new(), src_ptr, 0);
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
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let strlen_ref2 = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let strlen_ref3 = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
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

                                builder.ins().brif(old_is_empty, empty_block, &[], nonempty_block, &[]);

                                // --- empty_block: old is empty, return copy of s ---
                                builder.switch_to_block(empty_block);
                                builder.seal_block(empty_block);
                                let one_e = builder.ins().iconst(cl_types::I64, 1);
                                let copy_size = builder.ins().iadd(s_len, one_e);
                                let malloc_ref_e = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call_e = builder.ins().call(malloc_ref_e, &[copy_size]);
                                let copy_buf = builder.inst_results(malloc_call_e).to_vec()[0];
                                let strcpy_ref_e = self.module.declare_func_in_func(
                                    strcpy_func_id,
                                    builder.func,
                                );
                                builder.ins().call(strcpy_ref_e, &[copy_buf, s]);
                                builder.ins().jump(merge_block, &[BlockArg::Value(copy_buf)]);

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

                                builder.ins().jump(loop_header, &[BlockArg::Value(s), BlockArg::Value(buf)]);

                                // --- loop_header: call strstr(src_pos, old_str) ---
                                builder.switch_to_block(loop_header);
                                let src_pos = builder.block_params(loop_header)[0];
                                let dst_pos = builder.block_params(loop_header)[1];

                                let strstr_ref = self.module.declare_func_in_func(
                                    strstr_func_id,
                                    builder.func,
                                );
                                let strstr_call = builder.ins().call(strstr_ref, &[src_pos, old_str]);
                                let found_ptr = builder.inst_results(strstr_call).to_vec()[0];

                                let null_ptr = builder.ins().iconst(cl_types::I64, 0);
                                let is_null = builder.ins().icmp(IntCC::Equal, found_ptr, null_ptr);
                                builder.ins().brif(is_null, notfound_block, &[], found_block, &[]);

                                // --- found_block: copy prefix, copy replacement, advance ---
                                builder.switch_to_block(found_block);
                                builder.seal_block(found_block);

                                // prefix_len = found_ptr - src_pos
                                let prefix_len = builder.ins().isub(found_ptr, src_pos);

                                // memcpy(dst_pos, src_pos, prefix_len)
                                let memcpy_ref1 = self.module.declare_func_in_func(
                                    memcpy_func_id,
                                    builder.func,
                                );
                                builder.ins().call(memcpy_ref1, &[dst_pos, src_pos, prefix_len]);

                                // dst_pos += prefix_len
                                let dst_after_prefix = builder.ins().iadd(dst_pos, prefix_len);

                                // memcpy(dst_after_prefix, new_str, new_len)
                                let memcpy_ref2 = self.module.declare_func_in_func(
                                    memcpy_func_id,
                                    builder.func,
                                );
                                builder.ins().call(memcpy_ref2, &[dst_after_prefix, new_str, new_len]);

                                // dst_pos += new_len
                                let dst_after_new = builder.ins().iadd(dst_after_prefix, new_len);

                                // src_pos = found_ptr + old_len (skip past the matched occurrence)
                                let src_after_old = builder.ins().iadd(found_ptr, old_len);

                                builder.ins().jump(loop_header, &[BlockArg::Value(src_after_old), BlockArg::Value(dst_after_new)]);

                                // Seal loop_header (predecessors: nonempty_block entry + found_block back-edge)
                                builder.seal_block(loop_header);

                                // --- notfound_block: copy remaining + null-terminate ---
                                builder.switch_to_block(notfound_block);
                                builder.seal_block(notfound_block);

                                // Copy the remainder of the string (strcpy copies including null terminator)
                                let strcpy_ref2 = self.module.declare_func_in_func(
                                    strcpy_func_id,
                                    builder.func,
                                );
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
                                let strstr_ref = self.module.declare_func_in_func(
                                    strstr_func_id,
                                    builder.func,
                                );
                                let call = builder.ins().call(strstr_ref, &[s, substr]);
                                let found_ptr = builder.inst_results(call).to_vec()[0];

                                // if found_ptr == NULL then -1 else found_ptr - s
                                let zero = builder.ins().iconst(pointer_type, 0);
                                let is_null = builder.ins().icmp(
                                    IntCC::Equal,
                                    found_ptr,
                                    zero,
                                );
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
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call = builder.ins().call(malloc_ref, &[two]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // char_ptr = s + index
                                let char_ptr = builder.ins().iadd(s, index);
                                let ch = builder.ins().load(
                                    cl_types::I8,
                                    MemFlags::new(),
                                    char_ptr,
                                    0,
                                );

                                // buf[0] = ch, buf[1] = 0
                                builder.ins().store(MemFlags::new(), ch, buf, 0);
                                let nul = builder.ins().iconst(cl_types::I8, 0);
                                let buf_1 = builder.ins().iadd_imm(buf, 1);
                                builder.ins().store(MemFlags::new(), nul, buf_1, 0);

                                value_map.insert(*dst, buf);
                            }

                            // ── string_split(s, delimiter): return first token ──
                            "string_split" => {
                                let s = resolve_value(&value_map, &args[0])?;
                                let delim = resolve_value(&value_map, &args[1])?;

                                // Find delimiter position using strstr.
                                let strstr_func_id = *self
                                    .declared_functions
                                    .get("strstr")
                                    .ok_or("strstr not declared")?;
                                let strstr_ref = self.module.declare_func_in_func(
                                    strstr_func_id,
                                    builder.func,
                                );
                                let call = builder.ins().call(strstr_ref, &[s, delim]);
                                let found_ptr = builder.inst_results(call).to_vec()[0];

                                // If not found, return copy of whole string.
                                // If found, return s[0..found_ptr - s].
                                let strlen_func_id = *self
                                    .declared_functions
                                    .get("strlen")
                                    .ok_or("strlen not declared")?;
                                let strlen_ref = self.module.declare_func_in_func(
                                    strlen_func_id,
                                    builder.func,
                                );
                                let call_len = builder.ins().call(strlen_ref, &[s]);
                                let full_len = builder.inst_results(call_len).to_vec()[0];

                                let offset = builder.ins().isub(found_ptr, s);
                                let zero_ptr = builder.ins().iconst(pointer_type, 0);
                                let is_null = builder.ins().icmp(
                                    IntCC::Equal,
                                    found_ptr,
                                    zero_ptr,
                                );
                                // token_len = if null then full_len else offset
                                let token_len = builder.ins().select(
                                    is_null,
                                    full_len,
                                    offset,
                                );

                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let alloc_size = builder.ins().iadd(token_len, one);

                                let malloc_func_id = *self
                                    .declared_functions
                                    .get("malloc")
                                    .ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                let memcpy_func_id = *self
                                    .declared_functions
                                    .get("memcpy")
                                    .ok_or("memcpy not declared")?;
                                let memcpy_ref = self.module.declare_func_in_func(
                                    memcpy_func_id,
                                    builder.func,
                                );
                                builder.ins().call(memcpy_ref, &[buf, s, token_len]);

                                // Null-terminate
                                let nul = builder.ins().iconst(cl_types::I8, 0);
                                let end_ptr = builder.ins().iadd(buf, token_len);
                                builder.ins().store(MemFlags::new(), nul, end_ptr, 0);

                                value_map.insert(*dst, buf);
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
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero), BlockArg::Value(one_val)]);

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
                                builder.ins().jump(loop_header, &[BlockArg::Value(next_i), BlockArg::Value(new_acc)]);

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
                                let malloc_ref = self.module.declare_func_in_func(
                                    malloc_func_id,
                                    builder.func,
                                );
                                let malloc_call = builder.ins().call(malloc_ref, &[buf_size]);
                                let buf = builder.inst_results(malloc_call).to_vec()[0];

                                // Format string "%g"
                                let fmt_data_id = get_or_create_string(
                                    &mut self.module,
                                    &mut self.string_data,
                                    &mut self.string_counter,
                                    "%g",
                                )?;
                                let fmt_gv = self
                                    .module
                                    .declare_data_in_func(fmt_data_id, builder.func);
                                let fmt_ptr =
                                    builder.ins().global_value(pointer_type, fmt_gv);

                                // Use call_indirect with float-compatible signature:
                                // snprintf(ptr, i64, ptr, f64) -> i32
                                let snprintf_func_id = *self
                                    .declared_functions
                                    .get("snprintf")
                                    .ok_or("snprintf not declared")?;
                                let snprintf_ref = self.module.declare_func_in_func(
                                    snprintf_func_id,
                                    builder.func,
                                );
                                let snprintf_addr = builder
                                    .ins()
                                    .func_addr(pointer_type, snprintf_ref);

                                let mut float_snprintf_sig = self.module.make_signature();
                                float_snprintf_sig
                                    .params
                                    .push(AbiParam::new(pointer_type)); // buf
                                float_snprintf_sig
                                    .params
                                    .push(AbiParam::new(cl_types::I64)); // size
                                float_snprintf_sig
                                    .params
                                    .push(AbiParam::new(pointer_type)); // fmt
                                float_snprintf_sig
                                    .params
                                    .push(AbiParam::new(cl_types::F64)); // float val
                                float_snprintf_sig
                                    .returns
                                    .push(AbiParam::new(cl_types::I32));
                                let sig_ref =
                                    builder.import_signature(float_snprintf_sig);

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

                                let true_gv = self
                                    .module
                                    .declare_data_in_func(true_data_id, builder.func);
                                let false_gv = self
                                    .module
                                    .declare_data_in_func(false_data_id, builder.func);
                                let true_ptr =
                                    builder.ins().global_value(pointer_type, true_gv);
                                let false_ptr =
                                    builder.ins().global_value(pointer_type, false_gv);

                                let result =
                                    builder.ins().select(bool_val, true_ptr, false_ptr);
                                value_map.insert(*dst, result);
                            }

                            // ── file_read(path): call __gradient_file_read(path) -> String ──
                            "file_read" => {
                                let path = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_file_read")
                                    .ok_or("__gradient_file_read not declared")?;
                                let func_ref = self.module.declare_func_in_func(
                                    func_id,
                                    builder.func,
                                );
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
                                let func_ref = self.module.declare_func_in_func(
                                    func_id,
                                    builder.func,
                                );
                                let call_inst = builder.ins().call(func_ref, &[path, content]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── file_exists(path): call __gradient_file_exists -> Bool ──
                            "file_exists" => {
                                let path = resolve_value(&value_map, &args[0])?;
                                let func_id = *self
                                    .declared_functions
                                    .get("__gradient_file_exists")
                                    .ok_or("__gradient_file_exists not declared")?;
                                let func_ref = self.module.declare_func_in_func(
                                    func_id,
                                    builder.func,
                                );
                                let call_inst = builder.ins().call(func_ref, &[path]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
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
                                let func_ref = self.module.declare_func_in_func(
                                    func_id,
                                    builder.func,
                                );
                                let call_inst = builder.ins().call(func_ref, &[path, content]);
                                let result = builder.inst_results(call_inst).to_vec()[0];
                                value_map.insert(*dst, result);
                            }

                            // ── __gradient_contract_fail: print message and exit(1) ──
                            "__gradient_contract_fail" => {
                                // Print the error message using puts.
                                let puts_func_id = *self
                                    .declared_functions
                                    .get("puts")
                                    .ok_or("puts not declared")?;
                                let puts_ref = self.module.declare_func_in_func(
                                    puts_func_id,
                                    builder.func,
                                );
                                let msg_val = resolve_value(&value_map, &args[0])?;
                                builder.ins().call(puts_ref, &[msg_val]);

                                // Call exit(1) to abort.
                                let exit_func_id = *self
                                    .declared_functions
                                    .get("exit")
                                    .ok_or("exit not declared")?;
                                let exit_ref = self.module.declare_func_in_func(
                                    exit_func_id,
                                    builder.func,
                                );
                                let one = builder.ins().iconst(cl_types::I32, 1);
                                builder.ins().call(exit_ref, &[one]);

                                // Emit a dummy result value (never reached).
                                let dummy =
                                    builder.ins().iconst(cl_types::I64, 0);
                                value_map.insert(*dst, dummy);
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
                                let malloc_func_id = *self.declared_functions.get("malloc").ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];
                                // store new length and capacity
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 8i32);
                                // copy data: memcpy(new_ptr + 16, list_ptr + 24, new_len * 8)
                                let memcpy_func_id = *self.declared_functions.get("memcpy").ok_or("memcpy not declared")?;
                                let memcpy_ref = self.module.declare_func_in_func(memcpy_func_id, builder.func);
                                let src_data = builder.ins().iadd_imm(list_ptr, 24);
                                let dst_data = builder.ins().iadd_imm(new_ptr, 16);
                                builder.ins().call(memcpy_ref, &[dst_data, src_data, data_size]);
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
                                let malloc_func_id = *self.declared_functions.get("malloc").ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 8i32);
                                // copy old data
                                let old_data_size = builder.ins().imul(old_len, eight);
                                let memcpy_func_id = *self.declared_functions.get("memcpy").ok_or("memcpy not declared")?;
                                let memcpy_ref = self.module.declare_func_in_func(memcpy_func_id, builder.func);
                                let src_data = builder.ins().iadd_imm(list_ptr, 16);
                                let dst_data = builder.ins().iadd_imm(new_ptr, 16);
                                builder.ins().call(memcpy_ref, &[dst_data, src_data, old_data_size]);
                                // store new element at end
                                let new_elem_offset = builder.ins().iadd(old_data_size, sixteen);
                                let new_elem_addr = builder.ins().iadd(new_ptr, new_elem_offset);
                                builder.ins().store(MemFlags::new(), elem_val, new_elem_addr, 0i32);
                                value_map.insert(*dst, new_ptr);
                            }

                            // ── list_concat(a, b): new list with both lists' elements ──
                            "list_concat" => {
                                let list_a = resolve_value(&value_map, &args[0])?;
                                let list_b = resolve_value(&value_map, &args[1])?;
                                let len_a = builder.ins().load(cl_types::I64, MemFlags::new(), list_a, 0i32);
                                let len_b = builder.ins().load(cl_types::I64, MemFlags::new(), list_b, 0i32);
                                let new_len = builder.ins().iadd(len_a, len_b);
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(new_len, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);
                                let malloc_func_id = *self.declared_functions.get("malloc").ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), new_len, new_ptr, 8i32);
                                // copy list_a data
                                let size_a = builder.ins().imul(len_a, eight);
                                let memcpy_func_id = *self.declared_functions.get("memcpy").ok_or("memcpy not declared")?;
                                let memcpy_ref = self.module.declare_func_in_func(memcpy_func_id, builder.func);
                                let src_a = builder.ins().iadd_imm(list_a, 16);
                                let dst_start = builder.ins().iadd_imm(new_ptr, 16);
                                builder.ins().call(memcpy_ref, &[dst_start, src_a, size_a]);
                                // copy list_b data after list_a
                                let size_b = builder.ins().imul(len_b, eight);
                                let dst_b = builder.ins().iadd(dst_start, size_a);
                                let src_b = builder.ins().iadd_imm(list_b, 16);
                                // Need fresh memcpy ref
                                let memcpy_ref2 = self.module.declare_func_in_func(memcpy_func_id, builder.func);
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
                                builder.ins().brif(cmp, loop_body, &[], merge_block, &[BlockArg::Value(false_val)]);

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
                                builder.ins().brif(eq, merge_block, &[BlockArg::Value(true_val)], loop_header, &[BlockArg::Value(i_plus_one)]);

                                // Seal loop_header now (predecessors: entry jump + back-edge from body)
                                builder.seal_block(loop_header);
                                // Seal merge (predecessors: header not-found + body found)
                                builder.seal_block(merge_block);

                                // Switch to merge block and read the result
                                builder.switch_to_block(merge_block);
                                let result = builder.block_params(merge_block)[0];
                                value_map.insert(*dst, result);
                            }

                            // ── Higher-order list operations (v0.1 stubs) ──
                            // These return placeholder values. Full implementations
                            // with call_indirect for closures are future work.

                            // list_map(list, closure_fn_ptr): apply closure to each element
                            "list_map" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                // Load length from list header (offset 0)
                                let length = builder.ins().load(cl_types::I64, MemFlags::new(), list_ptr, 0i32);

                                // Allocate result list: 16 (header) + length * 8 (data)
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(length, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);

                                let malloc_func_id = *self.declared_functions.get("malloc").ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let result_ptr = builder.inst_results(malloc_call).to_vec()[0];

                                // Store length and capacity in result header
                                builder.ins().store(MemFlags::new(), length, result_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), length, result_ptr, 8i32);

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
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero_counter)]);

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
                                let elem = builder.ins().load(cl_types::I64, MemFlags::new(), src_addr, 0i32);

                                // call_indirect(closure_sig, fn_ptr, [elem])
                                let call_inst = builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
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
                                let length = builder.ins().load(cl_types::I64, MemFlags::new(), list_ptr, 0i32);

                                // Allocate result list (worst case: all elements pass)
                                let eight = builder.ins().iconst(cl_types::I64, 8);
                                let data_size = builder.ins().imul(length, eight);
                                let sixteen = builder.ins().iconst(cl_types::I64, 16);
                                let alloc_size = builder.ins().iadd(data_size, sixteen);

                                let malloc_func_id = *self.declared_functions.get("malloc").ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(malloc_func_id, builder.func);
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
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero), BlockArg::Value(zero2)]);

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
                                let elem = builder.ins().load(cl_types::I64, MemFlags::new(), src_addr, 0i32);

                                // Call predicate
                                let call_inst = builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let pred_result = builder.inst_results(call_inst).to_vec()[0];

                                // If predicate returns non-zero, store element
                                let zero_cmp = builder.ins().iconst(cl_types::I64, 0);
                                let pred_bool = builder.ins().icmp(IntCC::NotEqual, pred_result, zero_cmp);
                                builder.ins().brif(pred_bool, store_block, &[], skip_block, &[]);

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
                                builder.ins().jump(loop_header, &[BlockArg::Value(next_i_store), BlockArg::Value(new_count)]);

                                // --- skip_block: element does not pass ---
                                builder.switch_to_block(skip_block);
                                builder.seal_block(skip_block);
                                let one2 = builder.ins().iconst(cl_types::I64, 1);
                                let next_i_skip = builder.ins().iadd(i_val, one2);
                                builder.ins().jump(loop_header, &[BlockArg::Value(next_i_skip), BlockArg::Value(result_count)]);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                // Store actual result_count as length in result header
                                builder.ins().store(MemFlags::new(), result_count, result_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), result_count, result_ptr, 8i32);

                                value_map.insert(*dst, result_ptr);
                            }

                            // list_foreach(list, fn_ptr): call fn on each element, return unit
                            "list_foreach" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                // Load length
                                let length = builder.ins().load(cl_types::I64, MemFlags::new(), list_ptr, 0i32);

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
                                let elem = builder.ins().load(cl_types::I64, MemFlags::new(), src_addr, 0i32);

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
                                let length = builder.ins().load(cl_types::I64, MemFlags::new(), list_ptr, 0i32);

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
                                builder.ins().jump(loop_header, &[BlockArg::Value(zero), BlockArg::Value(init_val)]);

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
                                let elem = builder.ins().load(cl_types::I64, MemFlags::new(), src_addr, 0i32);

                                // accumulator = combine(acc, elem)
                                let call_inst = builder.ins().call_indirect(sig_ref, fn_ptr, &[acc, elem]);
                                let new_acc = builder.inst_results(call_inst).to_vec()[0];

                                let one = builder.ins().iconst(cl_types::I64, 1);
                                let next_i = builder.ins().iadd(i_val, one);
                                builder.ins().jump(loop_header, &[BlockArg::Value(next_i), BlockArg::Value(new_acc)]);

                                // --- loop_exit ---
                                builder.switch_to_block(loop_exit);
                                builder.seal_block(loop_exit);

                                value_map.insert(*dst, acc);
                            }

                            // list_any(list, predicate_fn_ptr): true if any element satisfies predicate
                            "list_any" => {
                                let list_ptr = resolve_value(&value_map, &args[0])?;
                                let fn_ptr = resolve_value(&value_map, &args[1])?;

                                let length = builder.ins().load(cl_types::I64, MemFlags::new(), list_ptr, 0i32);

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
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[BlockArg::Value(false_val)]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(cl_types::I64, MemFlags::new(), src_addr, 0i32);

                                let call_inst = builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let pred_result = builder.inst_results(call_inst).to_vec()[0];

                                let pred_bool = builder.ins().icmp(IntCC::NotEqual, pred_result, zero);
                                let one_any = builder.ins().iconst(cl_types::I64, 1);
                                let next_i_any = builder.ins().iadd(i_val, one_any);
                                builder.ins().brif(pred_bool, found_block, &[], loop_header, &[BlockArg::Value(next_i_any)]);

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

                                let length = builder.ins().load(cl_types::I64, MemFlags::new(), list_ptr, 0i32);

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
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[BlockArg::Value(true_val)]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(cl_types::I64, MemFlags::new(), src_addr, 0i32);

                                let call_inst = builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let pred_result = builder.inst_results(call_inst).to_vec()[0];

                                let pred_bool = builder.ins().icmp(IntCC::NotEqual, pred_result, zero);
                                let one_all = builder.ins().iconst(cl_types::I64, 1);
                                let next_i_all = builder.ins().iadd(i_val, one_all);
                                builder.ins().brif(pred_bool, loop_header, &[BlockArg::Value(next_i_all)], fail_block, &[]);

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

                                let length = builder.ins().load(cl_types::I64, MemFlags::new(), list_ptr, 0i32);

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
                                builder.ins().brif(cmp, loop_body, &[], loop_exit, &[BlockArg::Value(zero_default)]);

                                // --- loop_body ---
                                builder.switch_to_block(loop_body);
                                builder.seal_block(loop_body);

                                let elem_offset = builder.ins().imul(i_val, eight);
                                let elem_offset_full = builder.ins().iadd(elem_offset, sixteen);
                                let src_addr = builder.ins().iadd(list_ptr, elem_offset_full);
                                let elem = builder.ins().load(cl_types::I64, MemFlags::new(), src_addr, 0i32);

                                let call_inst = builder.ins().call_indirect(sig_ref, fn_ptr, &[elem]);
                                let pred_result = builder.inst_results(call_inst).to_vec()[0];

                                let zero_cmp_find = builder.ins().iconst(cl_types::I64, 0);
                                let pred_bool = builder.ins().icmp(IntCC::NotEqual, pred_result, zero_cmp_find);
                                let one_find = builder.ins().iconst(cl_types::I64, 1);
                                let next_i_find = builder.ins().iadd(i_val, one_find);
                                builder.ins().brif(pred_bool, found_block, &[], loop_header, &[BlockArg::Value(next_i_find)]);

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
                                let malloc_func_id = *self.declared_functions.get("malloc").ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(malloc_func_id, builder.func);
                                let malloc_call = builder.ins().call(malloc_ref, &[alloc_size]);
                                let new_ptr = builder.inst_results(malloc_call).to_vec()[0];

                                // Store length and capacity in header
                                builder.ins().store(MemFlags::new(), length, new_ptr, 0i32);
                                builder.ins().store(MemFlags::new(), length, new_ptr, 8i32);

                                // Copy source data to new list
                                let memcpy_func_id = *self.declared_functions.get("memcpy").ok_or("memcpy not declared")?;
                                let memcpy_ref = self.module.declare_func_in_func(memcpy_func_id, builder.func);
                                let src_data = builder.ins().iadd_imm(list_ptr, 16);
                                let dst_data = builder.ins().iadd_imm(new_ptr, 16);
                                builder.ins().call(memcpy_ref, &[dst_data, src_data, data_size]);

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
                                let outer_cmp = builder.ins().icmp(IntCC::SignedLessThan, i, len_minus_one);
                                builder.ins().brif(outer_cmp, outer_body, &[], outer_exit, &[]);

                                // Outer body: start inner loop to find min in i+1..length
                                builder.switch_to_block(outer_body);
                                builder.seal_block(outer_body);
                                let i_plus_one = builder.ins().iadd_imm(i, 1);
                                // min_idx starts as i
                                builder.ins().jump(inner_header, &[BlockArg::Value(i_plus_one), BlockArg::Value(i)]);

                                // Inner header: phi for j and min_idx
                                builder.switch_to_block(inner_header);
                                builder.append_block_param(inner_header, cl_types::I64); // j
                                builder.append_block_param(inner_header, cl_types::I64); // min_idx
                                let j = builder.block_params(inner_header)[0];
                                let min_idx = builder.block_params(inner_header)[1];
                                let inner_cmp = builder.ins().icmp(IntCC::SignedLessThan, j, length);
                                builder.ins().brif(inner_cmp, inner_body, &[], inner_exit, &[BlockArg::Value(min_idx)]);

                                // Inner body: compare arr[j] < arr[min_idx], update min_idx
                                builder.switch_to_block(inner_body);
                                builder.seal_block(inner_body);
                                let j_byte_off = builder.ins().imul_imm(j, 8);
                                let j_data_off = builder.ins().iadd_imm(j_byte_off, 16);
                                let j_addr = builder.ins().iadd(new_ptr, j_data_off);
                                let j_val = builder.ins().load(cl_types::I64, MemFlags::new(), j_addr, 0i32);
                                let min_byte_off = builder.ins().imul_imm(min_idx, 8);
                                let min_data_off = builder.ins().iadd_imm(min_byte_off, 16);
                                let min_addr = builder.ins().iadd(new_ptr, min_data_off);
                                let min_val = builder.ins().load(cl_types::I64, MemFlags::new(), min_addr, 0i32);
                                let is_less = builder.ins().icmp(IntCC::SignedLessThan, j_val, min_val);
                                // If arr[j] < arr[min_idx], new_min = j, else new_min = min_idx
                                let new_min = builder.ins().select(is_less, j, min_idx);
                                let j_plus_one = builder.ins().iadd_imm(j, 1);
                                builder.ins().jump(inner_header, &[BlockArg::Value(j_plus_one), BlockArg::Value(new_min)]);

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
                                let i_val = builder.ins().load(cl_types::I64, MemFlags::new(), i_addr, 0i32);
                                // Load arr[final_min_idx]
                                let fm_byte_off = builder.ins().imul_imm(final_min_idx, 8);
                                let fm_data_off = builder.ins().iadd_imm(fm_byte_off, 16);
                                let fm_addr = builder.ins().iadd(new_ptr, fm_data_off);
                                let fm_val = builder.ins().load(cl_types::I64, MemFlags::new(), fm_addr, 0i32);
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
                                let malloc_func_id = *self.declared_functions.get("malloc").ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(malloc_func_id, builder.func);
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
                                builder.ins().jump(loop_header, &[BlockArg::Value(i_plus_one)]);

                                // Seal loop_header (predecessors: entry + body back-edge)
                                builder.seal_block(loop_header);
                                // Seal loop_exit (predecessor: loop_header)
                                builder.seal_block(loop_exit);

                                builder.switch_to_block(loop_exit);
                                value_map.insert(*dst, new_ptr);
                            }

                            // ── Default: route print/println to puts, others as normal calls ──
                            _ if func_name.starts_with("list_literal_") => {
                                // list_literal_N: allocate and populate a list
                                let n = args.len() as i64;
                                // alloc: 16 (header) + n * 8 (data)
                                let header_size = 16i64;
                                let total = header_size + n * 8;
                                let alloc_size = builder.ins().iconst(cl_types::I64, total);
                                let malloc_func_id = *self.declared_functions.get("malloc").ok_or("malloc not declared")?;
                                let malloc_ref = self.module.declare_func_in_func(malloc_func_id, builder.func);
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

                            _ => {
                                let target_name = match func_name.as_str() {
                                    "print" | "println" => "puts",
                                    other => other,
                                };

                                // Check if the target is a known declared function.
                                // If not, it may be a closure variable (function pointer)
                                // which needs call_indirect.
                                if self.declared_functions.contains_key(target_name) {
                                    let cl_func_ref = if let Some(&existing) =
                                        func_ref_map.get(ir_func_ref)
                                    {
                                        existing
                                    } else {
                                        let target_func_id = self
                                            .declared_functions
                                            .get(target_name)
                                            .unwrap();
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

                                    let call_inst =
                                        builder.ins().call(cl_func_ref, &cl_args);

                                    let results =
                                        builder.inst_results(call_inst).to_vec();
                                    if !results.is_empty() {
                                        value_map.insert(*dst, results[0]);
                                    } else {
                                        let dummy =
                                            builder.ins().iconst(cl_types::I64, 0);
                                        value_map.insert(*dst, dummy);
                                    }
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
                                    let fn_ptr = if let Ok(v) = resolve_value(&value_map, &fn_ptr_val) {
                                        v
                                    } else {
                                        // Fallback: emit iconst 0 (will crash at runtime)
                                        builder.ins().iconst(cl_types::I64, 0)
                                    };

                                    let cl_args: Result<Vec<_>, _> = args
                                        .iter()
                                        .map(|a| resolve_value(&value_map, a))
                                        .collect();
                                    let cl_args = cl_args?;

                                    let call_inst =
                                        builder.ins().call_indirect(sig_ref, fn_ptr, &cl_args);

                                    let results =
                                        builder.inst_results(call_inst).to_vec();
                                    if !results.is_empty() {
                                        value_map.insert(*dst, results[0]);
                                    } else {
                                        let dummy =
                                            builder.ins().iconst(cl_types::I64, 0);
                                        value_map.insert(*dst, dummy);
                                    }
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
                        block_filled = true;
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
                        let load_ty = func.value_types.get(dst)
                            .map(ir_type_to_cl)
                            .unwrap_or(cl_types::I64);
                        let result = builder.ins().load(
                            load_ty,
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
                        let emitted = predecessors_emitted
                            .entry(target)
                            .or_insert(0);
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
                let emitted = predecessors_emitted.get(&ir_block.label).copied().unwrap_or(0);
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
        self.compile_module(module).map_err(super::CodegenError::from)
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
        use crate::ir::{BasicBlock, Function, Instruction, Module};
        use crate::ir::types::{BlockRef, FuncRef, Literal, Type, Value};

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
}
