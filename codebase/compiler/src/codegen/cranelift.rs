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
//! - `compile_function()` will be the real entry point once the IR is ready.
//!
//! # How Cranelift works (brief overview)
//!
//! 1. Create an `ObjectModule` targeting the host (or cross-compile target).
//! 2. Declare functions and data objects in the module.
//! 3. For each function, use `FunctionBuilder` to emit Cranelift IR instructions.
//! 4. Define the function in the module (this triggers compilation to machine code).
//! 5. Call `module.finish()` to get the serialized object file bytes.
//!
//! # Current status
//!
//! The `emit_hello_world()` method hardcodes a Cranelift IR program that:
//!   - Declares `puts` as an external libc function
//!   - Defines a `main` function that calls `puts("Hello from Gradient!")`
//!   - Returns 0
//!
//! When we have a real IR, `compile_function()` will walk the IR instructions
//! and emit the corresponding Cranelift IR for each one.

use cranelift_codegen::ir::types as cl_types;
use cranelift_codegen::ir::{AbiParam, InstBuilder, UserFuncName};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use target_lexicon::Triple;

use std::fs;
use std::path::Path;

use crate::ir;

/// The Cranelift-based code generator for Gradient.
///
/// Holds the compilation state needed to translate one or more functions
/// and produce a native object file.
///
/// # Lifecycle
///
/// ```text
/// let mut cg = CraneliftCodegen::new()?;
/// cg.emit_hello_world()?;          // PoC: hardcoded
/// // cg.compile_function(&ir_fn)?;  // Future: from IR
/// cg.finalize("output.o")?;
/// ```
pub struct CraneliftCodegen {
    /// The Cranelift object module — accumulates compiled functions and data,
    /// then serializes to an object file.
    module: ObjectModule,

    /// Shared compilation context — reused across function compilations to
    /// avoid repeated allocation.
    ctx: Context,
}

impl CraneliftCodegen {
    /// Create a new code generator targeting the host platform.
    ///
    /// This sets up the Cranelift ISA (instruction set architecture) for the
    /// current machine and creates an empty object module.
    ///
    /// # Future work
    /// - Accept a target triple for cross-compilation
    /// - Accept optimization level settings
    /// - Configure PIC/non-PIC based on output type (shared lib vs executable)
    pub fn new() -> Result<Self, String> {
        // Configure Cranelift settings.
        // `opt_level: speed` enables optimizations; for debug builds we might
        // want `none` or `speed_and_size`.
        let mut settings_builder = settings::builder();
        settings_builder
            .set("opt_level", "speed")
            .map_err(|e| format!("Failed to set opt_level: {}", e))?;

        // Enable PIC (position-independent code) — required on many platforms
        // and generally a good default for code that will be linked.
        settings_builder
            .set("is_pic", "true")
            .map_err(|e| format!("Failed to set is_pic: {}", e))?;

        let flags = settings::Flags::new(settings_builder);

        // Detect the host target triple and build the corresponding ISA.
        // In the future, this will accept an explicit triple for cross-compilation.
        let triple = Triple::host();
        let isa = cranelift_codegen::isa::lookup(triple.clone())
            .map_err(|e| format!("Failed to look up ISA for {}: {}", triple, e))?
            .finish(flags)
            .map_err(|e| format!("Failed to finish ISA: {}", e))?;

        // Create the object module builder. The module name appears in the
        // object file metadata and can aid debugging.
        let obj_builder = ObjectBuilder::new(
            isa,
            "gradient_module",
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| format!("Failed to create ObjectBuilder: {}", e))?;

        let module = ObjectModule::new(obj_builder);
        let ctx = module.make_context();

        Ok(Self { module, ctx })
    }

    /// Proof-of-concept: emit a hardcoded "Hello from Gradient!" program.
    ///
    /// This bypasses the Gradient IR entirely and directly constructs Cranelift
    /// IR for a `main` function that calls `puts`. It demonstrates that the
    /// full toolchain (Cranelift compile -> object file -> link -> run) works.
    ///
    /// # What this generates (pseudo-C equivalent)
    ///
    /// ```c
    /// extern int puts(const char *s);
    ///
    /// int main() {
    ///     puts("Hello from Gradient!");
    ///     return 0;
    /// }
    /// ```
    ///
    /// # What will change
    ///
    /// Once the IR layer is ready, this method will be replaced by
    /// `compile_module()` which iterates over IR functions and lowers each
    /// one via `compile_function()`.
    pub fn emit_hello_world(&mut self) -> Result<(), String> {
        // ----------------------------------------------------------------
        // Step 1: Create the string constant "Hello from Gradient!\0"
        // ----------------------------------------------------------------
        // In Cranelift, string data is stored as a named data object in the
        // module. We get back a DataId that we can reference from code.

        let mut data_desc = DataDescription::new();
        let hello_str = b"Hello from Gradient!\0";
        data_desc.define(hello_str.to_vec().into_boxed_slice());

        // Declare the data object in the module. `Export` linkage makes it
        // visible to the linker (though for a string constant we could use
        // `Local` — using Export here for simplicity in the PoC).
        let data_id = self
            .module
            .declare_data("hello_str", Linkage::Local, true, false)
            .map_err(|e| format!("Failed to declare data: {}", e))?;

        self.module
            .define_data(data_id, &data_desc)
            .map_err(|e| format!("Failed to define data: {}", e))?;

        // ----------------------------------------------------------------
        // Step 2: Declare the external `puts` function (from libc)
        // ----------------------------------------------------------------
        // `puts` has the C signature: int puts(const char *s)
        // In Cranelift terms: (i64) -> i32  [on 64-bit platforms]
        //
        // We use the system V calling convention since we're on Linux.
        // On other platforms, this would need to change.

        let pointer_type = self.module.target_config().pointer_type();

        let mut puts_sig = self.module.make_signature();
        puts_sig.params.push(AbiParam::new(pointer_type)); // const char *s
        puts_sig.returns.push(AbiParam::new(cl_types::I32)); // int return

        let puts_func_id = self
            .module
            .declare_function("puts", Linkage::Import, &puts_sig)
            .map_err(|e| format!("Failed to declare puts: {}", e))?;

        // ----------------------------------------------------------------
        // Step 3: Define the `main` function
        // ----------------------------------------------------------------
        // main() -> i32 (returns 0 on success)

        let mut main_sig = self.module.make_signature();
        main_sig.returns.push(AbiParam::new(cl_types::I32)); // int return

        let main_func_id = self
            .module
            .declare_function("main", Linkage::Export, &main_sig)
            .map_err(|e| format!("Failed to declare main: {}", e))?;

        // Set up the function context for building main's body.
        self.ctx.func.signature = main_sig;
        self.ctx.func.name = UserFuncName::user(0, 0);

        // FunctionBuilderContext is a temporary scratchpad used by the builder.
        let mut fb_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut fb_ctx);

        // Create the entry basic block. Every function needs at least one block.
        let entry_block = builder.create_block();

        // Seal the block immediately since it has no predecessors (it's the
        // entry point). Sealing tells Cranelift that no more edges will be
        // added to this block, enabling SSA construction to complete.
        builder.seal_block(entry_block);

        // Position the builder at the end of the entry block so we can
        // append instructions.
        builder.switch_to_block(entry_block);

        // ----------------------------------------------------------------
        // Step 3a: Get a pointer to the string constant
        // ----------------------------------------------------------------
        // We need to convert the DataId into a GlobalValue that Cranelift
        // can use as an operand, then materialize its address.

        let data_gv = self
            .module
            .declare_data_in_func(data_id, builder.func);

        let str_ptr = builder.ins().global_value(pointer_type, data_gv);

        // ----------------------------------------------------------------
        // Step 3b: Call puts(str_ptr)
        // ----------------------------------------------------------------
        // Import the puts function reference into this function's namespace.
        let puts_ref = self
            .module
            .declare_func_in_func(puts_func_id, builder.func);

        builder.ins().call(puts_ref, &[str_ptr]);

        // ----------------------------------------------------------------
        // Step 3c: Return 0
        // ----------------------------------------------------------------
        let zero = builder.ins().iconst(cl_types::I32, 0);
        builder.ins().return_(&[zero]);

        // Finalize the function — this runs verification and completes
        // SSA construction.
        builder.finalize();

        // ----------------------------------------------------------------
        // Step 4: Compile and define the function in the module
        // ----------------------------------------------------------------
        self.module
            .define_function(main_func_id, &mut self.ctx)
            .map_err(|e| format!("Failed to define main function: {}", e))?;

        // Clear the context for potential reuse with another function.
        self.module.clear_context(&mut self.ctx);

        Ok(())
    }

    /// Compile a Gradient IR function to Cranelift IR and add it to the module.
    ///
    /// This is a placeholder that will be implemented once the IR layer is
    /// producing real `ir::Function` values. The translation strategy is:
    ///
    /// 1. Create a Cranelift `Signature` from the IR function's parameter
    ///    and return types.
    /// 2. Declare the function in the module.
    /// 3. Create a `FunctionBuilder` and iterate over IR basic blocks:
    ///    a. Create a Cranelift block for each IR `BasicBlock`.
    ///    b. Translate IR phi nodes into Cranelift block parameters.
    ///    c. For each IR instruction, emit the corresponding Cranelift
    ///       instruction(s) using the builder.
    ///    d. Maintain a mapping from IR `Value`s to Cranelift `Value`s.
    /// 4. Finalize and define the function.
    ///
    /// # Type mapping
    ///
    /// | Gradient IR Type | Cranelift Type |
    /// |------------------|----------------|
    /// | `I32`            | `i32`          |
    /// | `I64`            | `i64`          |
    /// | `Ptr`            | `i64` (on 64-bit) |
    /// | `Bool`           | `i8`           |
    /// | `F64`            | `f64`          |
    /// | `Void`           | (no return)    |
    #[allow(unused_variables)]
    pub fn compile_function(&mut self, func: &ir::Function) -> Result<(), String> {
        // TODO: Implement IR -> Cranelift translation.
        //
        // Rough sketch:
        //
        // let sig = self.ir_sig_to_cranelift(&func.params, &func.return_type);
        // let func_id = self.module.declare_function(&func.name, Linkage::Export, &sig)?;
        // self.ctx.func.signature = sig;
        //
        // let mut fb_ctx = FunctionBuilderContext::new();
        // let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut fb_ctx);
        //
        // // Create all blocks first (forward references in branches).
        // let block_map: HashMap<BlockRef, Block> = ...;
        //
        // // Translate instructions block by block.
        // for ir_block in &func.blocks {
        //     let cl_block = block_map[&ir_block.label];
        //     builder.switch_to_block(cl_block);
        //
        //     for inst in &ir_block.instructions {
        //         match inst {
        //             Instruction::Const(val, lit) => { ... }
        //             Instruction::Add(dst, lhs, rhs) => { ... }
        //             Instruction::Call(dst, func_ref, args) => { ... }
        //             Instruction::Ret(val) => { ... }
        //             ...
        //         }
        //     }
        //
        //     builder.seal_block(cl_block);
        // }
        //
        // builder.finalize();
        // self.module.define_function(func_id, &mut self.ctx)?;
        // self.module.clear_context(&mut self.ctx);

        Err("compile_function() is not yet implemented — use emit_hello_world() for the PoC".into())
    }

    /// Write the compiled module to an object file on disk.
    ///
    /// After all functions and data have been compiled and added to the module,
    /// call this to serialize everything into a native object file (.o / .obj)
    /// that can be linked with `cc`.
    ///
    /// # Arguments
    ///
    /// * `path` — The output file path (e.g., "hello.o").
    ///
    /// # Note
    ///
    /// This method consumes the module's internal state. After calling
    /// `finalize()`, the `CraneliftCodegen` instance should not be reused.
    /// (The Cranelift `ObjectModule::finish()` takes ownership via `self`.)
    pub fn finalize(self, path: &str) -> Result<(), String> {
        // `finish()` consumes the module and returns the serialized object bytes.
        let object_product = self.module.finish();

        let bytes = object_product.emit().map_err(|e| format!("Failed to emit object: {}", e))?;

        // Write the object file to disk.
        fs::write(Path::new(path), &bytes)
            .map_err(|e| format!("Failed to write object file '{}': {}", path, e))?;

        println!("Wrote object file: {}", path);
        Ok(())
    }
}
