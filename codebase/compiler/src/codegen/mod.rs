//! Code generation subsystem for the Gradient compiler.
//!
//! This module is responsible for translating Gradient IR into native machine
//! code. Two backends are supported:
//!
//! - [`cranelift`] -- The Cranelift-based code generator. This is the default
//!   backend, optimized for fast compilation (ideal for debug builds).
//! - [`llvm`] -- The LLVM-based code generator (behind the `llvm` feature
//!   flag). This produces more aggressively optimized output, suitable for
//!   release builds.
//!
//! Both backends implement the [`CodegenBackend`] trait, which provides a
//! uniform interface for the compiler driver.
//!
//! # Pipeline
//!
//! ```text
//!   Gradient IR
//!       |
//!       v
//!   CodegenBackend::compile_module()
//!       |
//!       +--- CraneliftBackend (default)
//!       |        |
//!       |        v
//!       |    Cranelift IR -> native object file (.o)
//!       |
//!       +--- LlvmBackend (--release, requires `llvm` feature)
//!                |
//!                v
//!            LLVM IR -> native object file (.o)
//!       |
//!       v
//!   System linker (cc) produces executable
//! ```

pub mod cranelift;
#[cfg(feature = "llvm")]
pub mod llvm;
#[cfg(feature = "wasm")]
pub mod wasm;

use crate::ir;
use std::fmt;

// Re-export the main codegen type for convenience.
pub use self::cranelift::CraneliftCodegen;

/// Errors that can occur during code generation.
#[derive(Debug)]
pub struct CodegenError {
    pub message: String,
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<String> for CodegenError {
    fn from(message: String) -> Self {
        CodegenError { message }
    }
}

impl From<&str> for CodegenError {
    fn from(message: &str) -> Self {
        CodegenError {
            message: message.to_string(),
        }
    }
}

/// The backend trait that all code generators must implement.
///
/// This trait abstracts over the concrete code generation strategy (Cranelift,
/// LLVM, or any future backend). The compiler driver uses this trait to
/// compile IR modules without knowing which backend is active.
///
/// # Lifecycle
///
/// ```text
/// let mut backend = SomeBackend::new()?;
/// backend.compile_module(&ir_module)?;
/// let bytes = backend.finish()?;
/// std::fs::write("output.o", bytes)?;
/// ```
pub trait CodegenBackend {
    /// Compile an IR module into native code.
    ///
    /// After calling this method, the compiled code is buffered internally.
    /// Call [`finish`](CodegenBackend::finish) to retrieve the final object
    /// file bytes.
    fn compile_module(&mut self, module: &ir::Module) -> Result<(), CodegenError>;

    /// Finalize compilation and return the raw object file bytes.
    ///
    /// This consumes the backend. The returned bytes are a valid native
    /// object file (.o / .obj) that can be linked with a system linker.
    fn finish(self: Box<Self>) -> Result<Vec<u8>, CodegenError>;

    /// Returns a human-readable name for this backend (e.g. "cranelift", "llvm").
    fn name(&self) -> &str;
}

/// Returns `true` if the LLVM backend is available (compiled with the `llvm` feature).
pub fn llvm_available() -> bool {
    cfg!(feature = "llvm")
}

/// Backend wrapper enum that manages backend selection and context lifetimes.
///
/// This enum provides a unified interface for using either the Cranelift or LLVM
/// backend without the caller needing to know which is active. It handles the
/// lifetime requirements of the inkwell Context for the LLVM backend by keeping
/// the context alive as long as the backend is in use.
///
/// # Usage
///
/// ```rust,ignore
/// let backend = BackendWrapper::new(true)?; // true = use LLVM if available
/// // Use backend via CodegenBackend trait
/// ```
// CraneliftCodegen is intentionally inline (~6KB) — there's only ever one
// active backend and boxing it would force allocation on the hot path for
// every compile. The size disparity is acceptable.
#[allow(clippy::large_enum_variant)]
pub enum BackendWrapper {
    #[cfg(feature = "llvm")]
    Llvm {
        /// The LLVM context must outlive the codegen, so we keep it here.
        /// Boxed to ensure stable address for the 'static transmute in new().
        context: Box<inkwell::context::Context>,
        codegen: llvm::LlvmCodegen<'static>,
    },
    #[cfg(feature = "wasm")]
    Wasm(wasm::WasmBackend),
    Cranelift(CraneliftCodegen),
}

impl BackendWrapper {
    /// Create a new backend wrapper, selecting the backend based on release mode.
    ///
    /// When `release_mode` is `true` and the `llvm` feature is enabled, uses the
    /// LLVM backend. Otherwise uses the Cranelift backend.
    ///
    /// # Errors
    ///
    /// Returns an error if backend initialization fails.
    pub fn new(release_mode: bool) -> Result<Self, CodegenError> {
        if release_mode {
            #[cfg(feature = "llvm")]
            {
                let context = Box::new(inkwell::context::Context::create());
                // SAFETY: We transmute to 'static because the context is boxed and will
                // live as long as the BackendWrapper (they are dropped together).
                // The context address is stable because it's boxed.
                let context_ref: &'static inkwell::context::Context =
                    unsafe { std::mem::transmute(&*context) };
                let codegen = llvm::LlvmCodegen::new(context_ref)?;
                Ok(BackendWrapper::Llvm { context, codegen })
            }
            #[cfg(not(feature = "llvm"))]
            {
                // LLVM requested but not available - fall back to Cranelift with warning
                eprintln!(
                    "Warning: --release specified but LLVM backend not available. Using Cranelift."
                );
                let codegen = CraneliftCodegen::new()?;
                Ok(BackendWrapper::Cranelift(codegen))
            }
        } else {
            let codegen = CraneliftCodegen::new()?;
            Ok(BackendWrapper::Cranelift(codegen))
        }
    }

    /// Create a new backend wrapper with explicit backend type selection.
    ///
    /// # Arguments
    /// * `backend_type` - The type of backend to use ("cranelift", "llvm", "wasm")
    ///
    /// # Errors
    /// Returns an error if the requested backend is not available or initialization fails.
    pub fn new_with_backend(backend_type: &str) -> Result<Self, CodegenError> {
        match backend_type {
            "wasm" => {
                #[cfg(feature = "wasm")]
                {
                    Ok(BackendWrapper::Wasm(wasm::WasmBackend::new()?))
                }
                #[cfg(not(feature = "wasm"))]
                {
                    Err(CodegenError::from(
                        "WASM backend not available (compiled without wasm feature)",
                    ))
                }
            }
            "llvm" => {
                #[cfg(feature = "llvm")]
                {
                    let context = Box::new(inkwell::context::Context::create());
                    let context_ref: &'static inkwell::context::Context =
                        unsafe { std::mem::transmute(&*context) };
                    let codegen = llvm::LlvmCodegen::new(context_ref)?;
                    Ok(BackendWrapper::Llvm { context, codegen })
                }
                #[cfg(not(feature = "llvm"))]
                {
                    Err(CodegenError::from(
                        "LLVM backend not available (compiled without llvm feature)",
                    ))
                }
            }
            "cranelift" => {
                let codegen = CraneliftCodegen::new()?;
                Ok(BackendWrapper::Cranelift(codegen))
            }
            _ => Err(CodegenError::from(format!(
                "Unknown backend type: {}",
                backend_type
            ))),
        }
    }

    /// Returns the name of the active backend.
    pub fn backend_name(&self) -> &str {
        match self {
            #[cfg(feature = "llvm")]
            BackendWrapper::Llvm { codegen, .. } => codegen.name(),
            #[cfg(feature = "wasm")]
            BackendWrapper::Wasm(backend) => backend.name(),
            BackendWrapper::Cranelift(cg) => cg.name(),
        }
    }
}

impl CodegenBackend for BackendWrapper {
    fn compile_module(&mut self, module: &ir::Module) -> Result<(), CodegenError> {
        match self {
            #[cfg(feature = "llvm")]
            BackendWrapper::Llvm { codegen, .. } => codegen.compile_module(module),
            #[cfg(feature = "wasm")]
            BackendWrapper::Wasm(backend) => backend.compile_module(module),
            BackendWrapper::Cranelift(cg) => cg.compile_module(module).map_err(CodegenError::from),
        }
    }

    fn finish(self: Box<Self>) -> Result<Vec<u8>, CodegenError> {
        match *self {
            #[cfg(feature = "llvm")]
            BackendWrapper::Llvm { codegen, .. } => {
                // codegen is owned by self, so we can convert it to a box and finish
                // Note: we can't easily box codegen separately because of lifetime
                // So we use the trait method through the wrapper
                codegen.emit_bytes()
            }
            #[cfg(feature = "wasm")]
            BackendWrapper::Wasm(backend) => {
                let boxed: Box<dyn CodegenBackend> = Box::new(backend);
                boxed.finish()
            }
            BackendWrapper::Cranelift(cg) => {
                let boxed: Box<dyn CodegenBackend> = Box::new(cg);
                boxed.finish()
            }
        }
    }

    fn name(&self) -> &str {
        self.backend_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codegen_error_display() {
        let err = CodegenError::from("test error");
        assert_eq!(err.to_string(), "test error");
    }

    #[test]
    fn test_codegen_error_from_string() {
        let err = CodegenError::from(String::from("string error"));
        assert_eq!(err.message, "string error");
    }

    #[test]
    fn test_llvm_available_without_feature() {
        // Without the llvm feature compiled in, this should be false.
        // (This test verifies the function exists and returns a boolean.)
        let available = llvm_available();
        // In the default test build (no llvm feature), this is false.
        assert!(!available);
    }
}
