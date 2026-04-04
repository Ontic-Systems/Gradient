#![cfg(feature = "smt")]
//! Static Contract Verification using SMT Solvers
//!
//! This module provides compile-time proof of contract conditions using Z3.
//! It translates Gradient expressions into SMT-LIB constraints and attempts
//! to prove that:
//!
//! 1. @requires preconditions are satisfiable (function can be called)
//! 2. @ensures postconditions hold given the preconditions and function body
//!
//! # Architecture
//!
//! - `SmtEncoder`: Translates Gradient AST expressions to Z3 AST
//! - `ContractVerifier`: Manages the SMT context and proves contract properties
//! - `VerificationResult`: Outcome of verification (proved, counterexample, unknown)
//!
//! # Supported Constructs
//!
//! - Integer arithmetic (+, -, *, /, %)
//! - Comparisons (==, !=, <, <=, >, >=)
//! - Boolean logic (and, or, not)
//! - Simple linear arithmetic (LIA) and bitvectors
//!
//! # Example
//!
//! ```ignore
//! @requires(n >= 0)
//! @ensures(result >= 1)
//! fn factorial(n: Int) -> Int:
//!     if n <= 1:
//!         ret 1
//!     else:
//!         ret n * factorial(n - 1)
//! ```
//!
//! The verifier will prove that:
//! - There exist values of `n` satisfying `n >= 0` (precondition is satisfiable)
//! - For all `n >= 0`, the return value is >= 1 (postcondition holds)

use crate::ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use crate::ast::item::{Contract, ContractKind, FnDef, Param};
use crate::ast::span::Span;
use crate::ast::types::TypeExpr;
use std::collections::HashMap;

// WASM encoder types
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, ImportSection, Instruction as WasmInstr, MemArg,
    MemorySection, MemoryType, Module, Section, ValType,
};

/// Unique identifier for data segments (string literals, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DataId(pub u32);
