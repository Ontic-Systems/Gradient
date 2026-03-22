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

use crate::ir;
use super::CodegenError;

/// The LLVM-based code generator for Gradient.
///
/// Uses the `inkwell` crate to build LLVM IR from Gradient IR, then invokes
/// LLVM's optimization passes and emits a native object file.
///
/// # Lifecycle
///
/// ```text
/// let mut cg = LlvmCodegen::new()?;
/// cg.compile_module(&ir_module)?;
/// let bytes = cg.emit_bytes()?;
/// std::fs::write("output.o", bytes)?;
/// ```
pub struct LlvmCodegen {
    /// Placeholder for the LLVM context. In a full implementation, this would
    /// hold the inkwell `Context`, `Module`, and `Builder`.
    _private: (),
}

impl LlvmCodegen {
    /// Create a new LLVM code generator.
    ///
    /// In a full implementation, this would initialize an LLVM context and
    /// module targeting the host platform.
    pub fn new() -> Result<Self, CodegenError> {
        Ok(Self { _private: () })
    }

    /// Compile an entire IR module to LLVM IR and buffer it for emission.
    pub fn compile_module(&mut self, ir_module: &ir::Module) -> Result<(), CodegenError> {
        // ----------------------------------------------------------------
        // Stub implementation: translate each IR function to LLVM IR.
        //
        // In a full implementation, this would:
        // 1. Create LLVM function declarations for all IR functions
        // 2. For each function body, translate IR instructions to LLVM IR
        //    using inkwell's IRBuilder
        // 3. Run LLVM optimization passes (O2/O3)
        //
        // The IR instruction mapping would be:
        //   ir::Instruction::Const    -> LLVMBuildConst* / LLVMBuildGlobalString
        //   ir::Instruction::Add      -> LLVMBuildAdd / LLVMBuildFAdd
        //   ir::Instruction::Sub      -> LLVMBuildSub / LLVMBuildFSub
        //   ir::Instruction::Mul      -> LLVMBuildMul / LLVMBuildFMul
        //   ir::Instruction::Div      -> LLVMBuildSDiv / LLVMBuildFDiv
        //   ir::Instruction::Cmp      -> LLVMBuildICmp / LLVMBuildFCmp
        //   ir::Instruction::Call     -> LLVMBuildCall2
        //   ir::Instruction::Ret      -> LLVMBuildRet / LLVMBuildRetVoid
        //   ir::Instruction::Branch   -> LLVMBuildCondBr
        //   ir::Instruction::Jump     -> LLVMBuildBr
        //   ir::Instruction::Phi      -> LLVMBuildPhi + addIncoming
        //   ir::Instruction::Alloca   -> LLVMBuildAlloca
        //   ir::Instruction::Load     -> LLVMBuildLoad2
        //   ir::Instruction::Store    -> LLVMBuildStore
        // ----------------------------------------------------------------
        let _ = ir_module;
        Err(CodegenError::from(
            "LLVM backend is compiled but not yet fully implemented. \
             This is a structural stub — the inkwell-based translation \
             from Gradient IR to LLVM IR is not yet wired up. \
             Use the Cranelift backend (default) for now.",
        ))
    }

    /// Finalize compilation and return the raw object file bytes.
    pub fn emit_bytes(self) -> Result<Vec<u8>, CodegenError> {
        Err(CodegenError::from(
            "LLVM backend: emit_bytes not yet implemented",
        ))
    }
}

// ========================================================================
// CodegenBackend trait implementation
// ========================================================================

impl super::CodegenBackend for LlvmCodegen {
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

    #[test]
    fn test_llvm_backend_creation() {
        let backend = LlvmCodegen::new();
        assert!(backend.is_ok());
    }

    #[test]
    fn test_llvm_backend_name() {
        let backend = LlvmCodegen::new().unwrap();
        assert_eq!(
            <LlvmCodegen as super::super::CodegenBackend>::name(&backend),
            "llvm"
        );
    }

    #[test]
    fn test_llvm_compile_returns_stub_error() {
        use std::collections::HashMap;
        let mut backend = LlvmCodegen::new().unwrap();
        let module = ir::Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: HashMap::new(),
        };
        let result = backend.compile_module(&module);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.message.contains("not yet fully implemented"),
            "Expected stub error message, got: {}",
            err.message
        );
    }
}
