//! The Gradient compiler library.
//!
//! This crate contains the core compiler infrastructure for the Gradient
//! programming language. It is organized into two main subsystems:
//!
//! - **IR** ([`ir`]) — The intermediate representation that bridges the
//!   frontend (parser/typechecker) and the backend (code generator). The IR
//!   is an SSA-based, target-independent representation of Gradient programs.
//!
//! - **Codegen** ([`codegen`]) — The code generation backend that translates
//!   Gradient IR into native machine code via Cranelift, producing object files
//!   that can be linked into executables.
//!
//! # Compilation pipeline
//!
//! ```text
//!   Source Code (.grad)
//!       |
//!       v
//!   Lexer (tokenization)          -- not in this crate yet
//!       |
//!       v
//!   Parser (AST construction)      -- not in this crate yet
//!       |
//!       v
//!   Type Checker (semantic analysis)  -- not in this crate yet
//!       |
//!       v
//!   IR Builder (AST -> IR)         -- not yet implemented
//!       |
//!       v
//!   +-----------------------+
//!   | gradient-compiler     |
//!   |                       |
//!   |  ir::Module           |  <-- You are here
//!   |       |               |
//!   |       v               |
//!   |  codegen::Cranelift   |
//!   |       |               |
//!   |       v               |
//!   |  Object file (.o)     |
//!   +-----------------------+
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
pub mod typechecker;
