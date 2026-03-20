//! Expression AST nodes for the Gradient language.
//!
//! Every expression is represented as `Spanned<ExprKind>`, aliased to
//! [`Expr`]. The parser constructs these bottom-up, boxing sub-expressions
//! wherever recursion occurs.

use super::block::Block;
use super::span::Spanned;

/// A fully located expression node.
///
/// This is the primary expression type used throughout the compiler. The
/// span records the source region of the entire expression, while the
/// `ExprKind` discriminant describes what kind of expression it is.
pub type Expr = Spanned<ExprKind>;

/// The different kinds of expressions in Gradient.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// An integer literal, e.g. `42` or `-1`.
    IntLit(i64),

    /// A floating-point literal, e.g. `3.14`.
    FloatLit(f64),

    /// A string literal, e.g. `"hello"`. The value is the content between
    /// the quotes with escape sequences already resolved.
    StringLit(String),

    /// A boolean literal: `true` or `false`.
    BoolLit(bool),

    /// The unit literal `()`.
    UnitLit,

    /// A variable or function reference, e.g. `x` or `println`.
    Ident(String),

    /// A typed hole, written `?name` or just `?`. The compiler will report
    /// the inferred type at this location, which is useful during
    /// development. The `Option<String>` holds the optional label.
    TypedHole(Option<String>),

    /// A binary operation, e.g. `a + b` or `x == y`.
    BinaryOp {
        /// The operator.
        op: BinOp,
        /// The left-hand operand.
        left: Box<Expr>,
        /// The right-hand operand.
        right: Box<Expr>,
    },

    /// A unary operation, e.g. `-x` or `not flag`.
    UnaryOp {
        /// The operator.
        op: UnaryOp,
        /// The operand.
        operand: Box<Expr>,
    },

    /// A function call, e.g. `f(1, 2)`.
    Call {
        /// The expression being called (usually an [`ExprKind::Ident`]).
        func: Box<Expr>,
        /// The argument expressions.
        args: Vec<Expr>,
    },

    /// A field access, e.g. `obj.field`.
    FieldAccess {
        /// The object expression.
        object: Box<Expr>,
        /// The field name.
        field: String,
    },

    /// An `if` / `else if` / `else` expression.
    ///
    /// In Gradient, `if` is an expression and can appear anywhere a value is
    /// expected.
    If {
        /// The condition of the leading `if`.
        condition: Box<Expr>,
        /// The body block of the leading `if`.
        then_block: Block,
        /// Zero or more `else if` arms, each with a condition and body.
        else_ifs: Vec<(Expr, Block)>,
        /// An optional trailing `else` block.
        else_block: Option<Block>,
    },

    /// A `for` loop expression, e.g. `for x in xs: ...`.
    For {
        /// The loop variable name.
        var: String,
        /// The expression being iterated over.
        iter: Box<Expr>,
        /// The loop body.
        body: Block,
    },

    /// A parenthesized expression, e.g. `(a + b)`.
    ///
    /// Preserved in the AST so that pretty-printers and source-map tools
    /// can reproduce the original grouping.
    Paren(Box<Expr>),
}

/// Binary operators, ordered by conventional precedence (lowest to highest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// Logical OR (`or`).
    Or,
    /// Logical AND (`and`).
    And,
    /// Equality (`==`).
    Eq,
    /// Inequality (`!=`).
    Ne,
    /// Less than (`<`).
    Lt,
    /// Less than or equal (`<=`).
    Le,
    /// Greater than (`>`).
    Gt,
    /// Greater than or equal (`>=`).
    Ge,
    /// Addition (`+`).
    Add,
    /// Subtraction (`-`).
    Sub,
    /// Multiplication (`*`).
    Mul,
    /// Division (`/`).
    Div,
    /// Modulo / remainder (`%`).
    Mod,
}

/// Unary (prefix) operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Arithmetic negation (`-`).
    Neg,
    /// Logical negation (`not`).
    Not,
}
