//! The Gradient compiler library.
//!
//! This crate is designed as a **library first, binary second**. The primary
//! interface for AI agents and tools is the [`query`] module, which provides
//! structured, JSON-serializable access to all compiler information.
//!
//! # For agents: the query API
//!
//! ```rust
//! use gradient_compiler::query::Session;
//!
//! let session = Session::from_source("fn add(a: Int, b: Int) -> Int:\n    a + b\n");
//!
//! // Structured diagnostics
//! let result = session.check();
//! assert!(result.is_ok());
//!
//! // Module contract (compact API summary)
//! let contract = session.module_contract();
//! println!("{}", contract.to_json());
//!
//! // Symbol table
//! let symbols = session.symbols();
//! ```
//!
//! # Internal modules
//!
//! - [`lexer`] — Tokenisation with indentation tracking
//! - [`parser`] — Recursive-descent parser with error recovery
//! - [`typechecker`] — Type inference, checking, and effect validation
//! - [`ir`] — SSA-based intermediate representation
//! - [`codegen`] — Cranelift-based native code generation
//! - [`query`] — **Structured query API** (the primary agent interface)

pub mod ast;
pub mod codegen;
pub mod fmt;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod query;
pub mod repl;
pub mod resolve;
pub mod typechecker;
