//! SMT-based contract verification for Gradient.
//!
//! This module provides the foundation for static contract verification using Z3.
//! It converts Gradient types and expressions to SMT-LIB format and uses Z3 to
//! check validity of contract conditions.
//!
//! # Feature Flag
//!
//! This module is only available when the `smt-verify` feature is enabled.
//! The SMT verification is disabled by default to avoid the Z3 dependency
//! for standard builds.
//!
//! # Architecture
//!
//! - [`Encoder`]: Converts Gradient AST expressions to Z3 expressions
//! - [`Solver`]: Wrapper around Z3 for checking validity/satisfiability
//! - [`VerificationError`]: Structured error output for failed proofs

pub mod encoder;
pub mod error;
pub mod solver;

pub use encoder::Encoder;
pub use error::{VerificationError, VerificationResult};
pub use solver::Solver;
