//! Compile-time evaluation (comptime)
//!
//! This module provides compile-time expression evaluation for Gradient.
//! It allows executing pure functions at compile time to produce constant values.
//!
//! # Example
//! ```
//! use gradient_compiler::comptime::{ComptimeEvaluator, ComptimeValue};
//!
//! let mut eval = ComptimeEvaluator::new();
//! // Evaluate expressions at compile time...
//! ```

pub mod evaluator;
pub mod value;

pub use evaluator::ComptimeEvaluator;
pub use value::ComptimeValue;
