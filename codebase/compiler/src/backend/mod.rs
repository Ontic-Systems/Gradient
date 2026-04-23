//! Backend code generation modules for the Gradient compiler.
//!
//! This module contains the various code generation backends that translate
//! Gradient IR into target-specific output formats.

#[cfg(feature = "wasm-unstable")]
pub mod wasm;

#[cfg(feature = "wasm-unstable")]
pub use wasm::WasmBackend;
