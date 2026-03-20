//! Statement AST nodes for the Gradient language.
//!
//! Statements appear inside blocks. Unlike expressions, statements do not
//! produce a value that can be used in a surrounding context. However, an
//! expression can appear in statement position (e.g. a bare function call).

use super::expr::Expr;
use super::span::Spanned;
use super::types::TypeExpr;

/// A fully located statement node.
pub type Stmt = Spanned<StmtKind>;

/// The different kinds of statements in Gradient.
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// A `let` binding, e.g. `let x: i32 = 42`.
    ///
    /// The type annotation is optional; if omitted, the type checker will
    /// attempt to infer it.
    Let {
        /// The name being bound.
        name: String,
        /// An optional explicit type annotation.
        type_ann: Option<Spanned<TypeExpr>>,
        /// The initializer expression.
        value: Expr,
    },

    /// A `ret` (return) statement, e.g. `ret 0`.
    Ret(Expr),

    /// An expression used as a statement (e.g. a function call whose result
    /// is discarded).
    Expr(Expr),
}
