//! Gradient Test Framework
//!
//! Test infrastructure for the Gradient programming language compiler.
//!
//! This crate provides:
//!
//! - **Golden test runner** ([`golden`]): snapshot-based testing that compares
//!   compiler output against expected `.stdout`/`.stderr` files.
//! - **Test harness** ([`harness`]): core types (`TestResult`, `TestCase`) and
//!   functions for running individual tests or entire suites.
//!
//! # Quick Start
//!
//! ```no_run
//! use gradient_test_framework::golden::{GoldenConfig, run_golden_suite};
//!
//! let config = GoldenConfig::new(
//!     "./target/debug/gradient",
//!     "./tests/golden/cases",
//!     "./tests/golden/expected",
//! );
//!
//! let summary = run_golden_suite(&config);
//! println!("{summary}");
//! assert!(summary.all_ok());
//! ```

pub mod golden;
pub mod harness;

// Re-export key types at the crate root for convenience.
pub use golden::{GoldenConfig, GoldenOutcome, GoldenSummary, run_golden_suite};
pub use harness::{SuiteResult, TestCase, TestResult, run_suite, run_test_case};
