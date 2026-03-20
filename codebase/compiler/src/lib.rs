//! The Gradient compiler library.
//!
//! This crate contains the complete compiler pipeline for the Gradient
//! programming language:
//!
//! - **Lexer** ([`lexer`]) — Hand-written tokenizer with INDENT/DEDENT injection.
//! - **Parser** ([`parser`]) — Recursive descent parser producing a typed AST.
//! - **AST** ([`ast`]) — Abstract syntax tree node definitions with source spans.
//! - **Type Checker** ([`typechecker`]) — Static type checking with inference.
//! - **IR** ([`ir`]) — SSA-form intermediate representation and AST-to-IR builder.
//! - **Codegen** ([`codegen`]) — Cranelift backend producing native object files.
//!
//! # Compilation pipeline
//!
//! ```text
//!   Source Code (.gr)
//!       |
//!       v
//!   Lexer (tokenization + INDENT/DEDENT)
//!       |
//!       v
//!   Parser (AST construction with error recovery)
//!       |
//!       v
//!   Type Checker (static types, inference, effect validation)
//!       |
//!       v
//!   IR Builder (AST -> SSA IR)
//!       |
//!       v
//!   Cranelift Codegen -> Object file (.o)
//!       |
//!       v
//!   System linker (cc) -> Native executable
//! ```
//!
//! # Current status
//!
//! The full pipeline is wired end-to-end. Gradient source files compile to
//! native binaries. Working programs include hello world, factorial,
//! fibonacci, arithmetic, string concatenation, and math builtins.

pub mod ast;
pub mod codegen;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod typechecker;
