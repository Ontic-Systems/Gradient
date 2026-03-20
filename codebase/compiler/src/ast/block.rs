//! Block AST node.
//!
//! A block is a sequence of statements that forms the body of a function,
//! loop, or conditional branch. In the Gradient grammar, blocks are
//! introduced by a colon and are indentation-delimited.

use super::span::Spanned;
use super::stmt::Stmt;

/// A block of statements, carrying the span of the entire block
/// (from the colon through the last statement).
///
/// In the grammar a block appears after `:` in function definitions,
/// `if`/`else` branches, and `for` loops.
pub type Block = Spanned<Vec<Stmt>>;
