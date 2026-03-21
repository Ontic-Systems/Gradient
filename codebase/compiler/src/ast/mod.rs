//! Abstract Syntax Tree definitions for the Gradient programming language.
//!
//! This module defines every AST node that the parser can produce. The data
//! structures here form the contract between the parser and all downstream
//! compiler passes (name resolution, type checking, lowering, etc.).
//!
//! # Design principles
//!
//! - **Span tracking**: every node carries a [`Span`] (either via the
//!   [`Spanned<T>`] wrapper or an explicit `span` field) so that error
//!   messages can point at the exact source location.
//! - **Simplicity**: identifiers are plain `String`s (interning comes
//!   later), and there is no visitor infrastructure yet.
//! - **Faithfulness to the grammar**: the tree can represent every construct
//!   in the v0.1 PEG grammar, with no desugaring at the AST level.

pub mod block;
pub mod expr;
pub mod item;
pub mod module;
pub mod span;
pub mod stmt;
pub mod types;

// ── Re-exports ──────────────────────────────────────────────────────────

// Span primitives
pub use span::{Position, Span, Spanned};

// Type expressions and effects
pub use types::{EffectSet, TypeExpr};

// Expressions
pub use expr::{BinOp, Expr, ExprKind, MatchArm, Pattern, UnaryOp};

// Blocks
pub use block::Block;

// Statements
pub use stmt::{Stmt, StmtKind};

// Top-level items
pub use item::{Annotation, EnumVariant, ExternFnDecl, FnDef, Item, ItemKind, Param};

// Module-level nodes
pub use module::{Module, ModuleDecl, UseDecl};
