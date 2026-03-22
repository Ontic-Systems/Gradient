//! Expression AST nodes for the Gradient language.
//!
//! Every expression is represented as `Spanned<ExprKind>`, aliased to
//! [`Expr`]. The parser constructs these bottom-up, boxing sub-expressions
//! wherever recursion occurs.

use super::block::Block;
use super::span::{Span, Spanned};
use super::types::TypeExpr;

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

    /// A `while` loop expression, e.g. `while x > 0: ...`.
    While {
        /// The loop condition.
        condition: Box<Expr>,
        /// The loop body.
        body: Block,
    },

    /// A `match` expression, e.g. `match n: 0: ... 1: ... _: ...`.
    ///
    /// Matches the scrutinee expression against a series of patterns
    /// (integer literals, boolean literals, or wildcard `_`), executing
    /// the first matching arm's body.
    Match {
        /// The expression being matched on.
        scrutinee: Box<Expr>,
        /// The match arms, checked in order.
        arms: Vec<MatchArm>,
    },

    /// A parenthesized expression, e.g. `(a + b)`.
    ///
    /// Preserved in the AST so that pretty-printers and source-map tools
    /// can reproduce the original grouping.
    Paren(Box<Expr>),

    /// A tuple expression, e.g. `(1, "hello", true)`.
    Tuple(Vec<Expr>),

    /// Tuple field access by index, e.g. `pair.0`.
    TupleField {
        /// The tuple expression.
        tuple: Box<Expr>,
        /// The field index.
        index: usize,
    },

    /// Spawn an actor instance, e.g. `spawn Counter`.
    ///
    /// Returns a value of type `Actor[ActorName]`. Requires the `Actor` effect.
    Spawn {
        /// The name of the actor type to spawn.
        actor_name: String,
    },

    /// Send a fire-and-forget message to an actor, e.g. `send c Increment`.
    ///
    /// Returns `()`. Requires the `Actor` effect.
    Send {
        /// The expression evaluating to the actor handle.
        target: Box<Expr>,
        /// The message name to send.
        message: String,
    },

    /// Send a request message and wait for a reply, e.g. `ask c GetCount`.
    ///
    /// Returns the handler's declared return type. Requires the `Actor` effect.
    Ask {
        /// The expression evaluating to the actor handle.
        target: Box<Expr>,
        /// The message name to ask.
        message: String,
    },

    /// A closure (lambda) expression, e.g. `|x: Int| x + 1`.
    Closure {
        /// The closure's parameters with type annotations.
        params: Vec<ClosureParam>,
        /// Optional return type annotation.
        return_type: Option<Spanned<TypeExpr>>,
        /// The closure body -- a single expression.
        body: Box<Expr>,
    },

    /// The try operator `expr?` — unwrap Ok or propagate Err.
    Try(Box<Expr>),
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

/// A single arm in a `match` expression.
///
/// Each arm consists of a pattern to match against, a body block to execute
/// if the pattern matches, and a span covering the entire arm.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    /// The pattern for this arm.
    pub pattern: Pattern,
    /// The body to execute if the pattern matches.
    pub body: Block,
    /// The source span of this arm (pattern through end of body).
    pub span: Span,
}

/// A pattern in a `match` arm.
///
/// Patterns can match integer literals, boolean literals, wildcard `_`,
/// or enum variant names (with optional binding for tuple variants).
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// Match an exact integer value.
    IntLit(i64),
    /// Match a boolean value (`true` or `false`).
    BoolLit(bool),
    /// Wildcard pattern `_` — matches anything.
    Wildcard,
    /// Match an enum variant, optionally binding its payload.
    ///
    /// For unit variants: `Red` — `variant = "Red"`, `binding = None`.
    /// For tuple variants: `Some(x)` — `variant = "Some"`, `binding = Some("x")`.
    Variant {
        /// The variant name being matched.
        variant: String,
        /// An optional binding name for the variant's payload.
        binding: Option<String>,
    },

    /// Destructuring tuple pattern, e.g. `(a, b, c)`.
    Tuple(Vec<Pattern>),
}

/// A single parameter in a closure expression.
///
/// Each parameter has a name and an optional type annotation. When the type
/// annotation is omitted, the type checker will infer the parameter type
/// from usage context.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureParam {
    /// The parameter name.
    pub name: String,
    /// Optional type annotation, e.g. `Int` in `|x: Int|`.
    pub type_ann: Option<Spanned<TypeExpr>>,
    /// The source span of this parameter.
    pub span: Span,
}
