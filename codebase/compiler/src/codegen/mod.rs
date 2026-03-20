//! Code generation subsystem for the Gradient compiler.
//!
//! This module is responsible for translating Gradient IR into native machine
//! code. The current (and primary) backend is Cranelift, but the module
//! structure is designed to allow alternative backends in the future (e.g.,
//! LLVM, or a custom backend for a specific target).
//!
//! # Module structure
//!
//! - [`cranelift`] — The Cranelift-based code generator. This is the main
//!   backend and the only one implemented.
//!
//! # Pipeline
//!
//! ```text
//!   Gradient IR
//!       |
//!       v
//!   codegen::cranelift::CraneliftCodegen
//!       |
//!       v
//!   Cranelift IR (SSA, target-independent)
//!       |
//!       v
//!   Cranelift backend (register allocation, instruction selection)
//!       |
//!       v
//!   Native object file (.o)
//!       |
//!       v
//!   System linker (cc) produces executable
//! ```

pub mod cranelift;

// Re-export the main codegen type for convenience.
pub use self::cranelift::CraneliftCodegen;
