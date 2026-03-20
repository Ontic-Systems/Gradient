//! The Gradient compiler library.
//!
//! This crate contains the core compiler infrastructure for the Gradient
//! programming language. It is organized into the following subsystems:
//!
//! - **Lexer** ([`lexer`]) — Tokenisation of Gradient source files, including
//!   indentation tracking for significant-whitespace blocks.
//!
//! - **Parser** ([`parser`]) — Recursive-descent parser that builds an AST
//!   from the token stream, with error recovery.
//!
//! - **Type Checker** ([`typechecker`]) — Semantic analysis: name resolution,
//!   type inference, type checking, and effect validation.
//!
//! - **IR** ([`ir`]) — The intermediate representation that bridges the
//!   frontend and the backend. The IR is an SSA-based, target-independent
//!   representation of Gradient programs, built from the AST by the IR builder.
//!
//! - **Codegen** ([`codegen`]) — The code generation backend that translates
//!   Gradient IR into native machine code via Cranelift, producing object files
//!   that can be linked into executables.
//!
//! # Compilation pipeline
//!
//! ```text
//!   Source Code (.gr)
//!       |
//!       v
//!   Lexer (tokenization)
//!       |
//!       v
//!   Parser (AST construction)
//!       |
//!       v
//!   Type Checker (semantic analysis)
//!       |
//!       v
//!   IR Builder (AST -> IR)
//!       |
//!       v
//!   Cranelift Codegen
//!       |
//!       v
//!   Object file (.o)
//!       |
//!       v
//!   System linker (cc)
//!       |
//!       v
//!   Native executable
//! ```
//!
//! # Current status
//!
//! The compiler implements a full pipeline: lexer, parser, type checker,
//! IR builder, and Cranelift-based code generation.

pub mod ast;
pub mod codegen;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod query;
pub mod typechecker;
