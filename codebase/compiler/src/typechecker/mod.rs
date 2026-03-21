//! Type checker for the Gradient programming language.
//!
//! This module implements semantic analysis for Gradient v0.1. It walks the
//! AST produced by the parser, resolves names, infers and checks types for
//! all expressions and statements, validates effect annotations on function
//! calls, and reports type errors with spans and structured JSON diagnostics.
//!
//! # Usage
//!
//! ```ignore
//! use gradient_compiler::typechecker::{check_module, TypeError};
//!
//! let errors = check_module(&module, file_id);
//! if errors.is_empty() {
//!     // proceed with IR lowering
//! } else {
//!     for e in &errors {
//!         eprintln!("{}", e);
//!     }
//! }
//! ```

pub mod checker;
pub mod effects;
pub mod env;
pub mod error;
pub mod types;

#[cfg(test)]
mod tests;

// ── Re-exports ──────────────────────────────────────────────────────────

pub use checker::{check_module, check_module_with_effects, TypeChecker};
pub use effects::{EffectInfo, ModuleEffectSummary};
pub use error::TypeError;
pub use types::Ty;
