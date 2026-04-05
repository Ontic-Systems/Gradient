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

    /// A record literal expression, e.g. `EffectSet: effects: combined is_pure: false`.
    RecordLit {
        /// The type name of the record.
        type_name: String,
        /// The field names and their values.
        fields: Vec<(String, Expr)>,
    },

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

    /// A list literal, e.g. `[1, 2, 3]` or `[]`.
    ListLit(Vec<Expr>),

    /// A closure (lambda) expression, e.g. `|x: Int| x + 1`.
    Closure {
        /// The closure's parameters with type annotations.
        params: Vec<ClosureParam>,
        /// Optional return type annotation.
        return_type: Option<Spanned<TypeExpr>>,
        /// The closure body -- a single expression.
        body: Box<Expr>,
    },

    /// A range expression, e.g. `0..10`.
    Range {
        /// The start of the range (inclusive).
        start: Box<Expr>,
        /// The end of the range (exclusive).
        end: Box<Expr>,
    },

    /// The try operator `expr?` — unwrap Ok or propagate Err.
    Try(Box<Expr>),

    /// A defer expression, e.g. `defer cleanup()`.
    /// Executes the body expression when the current scope exits.
    /// Multiple defers execute in reverse order of declaration (LIFO).
    Defer {
        /// The expression to execute when the scope exits.
        body: Box<Expr>,
    },

    /// A string interpolation expression, e.g. `f"hello {name}"`.
    /// Desugared to a series of string concatenations.
    StringInterp {
        /// The parts: string literals and expressions to be stringified.
        parts: Vec<StringInterpPart>,
    },

    /// A concurrent scope expression, e.g. `concurrent_scope: ...`.
    /// Spawns actors in a scope where all children are cancelled when
    /// the scope exits (structured concurrency).
    ConcurrentScope {
        /// The body of the concurrent scope.
        body: super::block::Block,
    },

    /// A supervisor expression, e.g. `supervisor strategy = one_for_one: ...`.
    /// Manages child actors according to a restart strategy.
    Supervisor {
        /// The restart strategy (one_for_one, one_for_all, rest_for_one).
        strategy: RestartStrategy,
        /// Maximum number of restarts allowed in a time period.
        max_restarts: Option<i64>,
        /// Child specifications for actors to supervise.
        children: Vec<ChildSpec>,
    },
}

/// Restart strategies for supervisors (Erlang/OTP-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartStrategy {
    /// Restart only the crashed child.
    OneForOne,
    /// Restart all children when one crashes.
    OneForAll,
    /// Restart crashed child and all younger siblings.
    RestForOne,
}

/// Restart policies for supervised children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Always restart the child (even on normal exit).
    Permanent,
    /// Restart only if child exits abnormally.
    Transient,
    /// Never restart the child.
    Temporary,
}

/// A child specification for a supervisor.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildSpec {
    /// The actor type name to spawn.
    pub actor_type: String,
    /// The restart policy for this child.
    pub restart_policy: RestartPolicy,
    /// Optional custom maximum restarts for this child.
    pub max_restarts: Option<i64>,
    /// Source span for error reporting.
    pub span: super::span::Span,
}

/// A single part of a string interpolation expression.
#[derive(Debug, Clone, PartialEq)]
pub enum StringInterpPart {
    /// A literal string segment.
    Literal(String),
    /// An expression to be evaluated and converted to string.
    Expr(Box<Expr>),
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
    /// Pipe operator (`|>`).
    Pipe,
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
/// Each arm consists of a pattern to match against, an optional guard
/// condition, a body block to execute if the pattern matches and the guard
/// (if any) is true, and a span covering the entire arm.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    /// The pattern for this arm.
    pub pattern: Pattern,
    /// Optional guard condition (`if condition`).
    pub guard: Option<Expr>,
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
    /// Match an enum variant, optionally binding its payload fields.
    ///
    /// For unit variants: `Red` — `variant = "Red"`, `bindings = []`.
    /// For single-field variants: `Some(x)` — `variant = "Some"`, `bindings = ["x"]`.
    /// For multi-field variants: `Task(id, title, done)` — `bindings = ["id", "title", "done"]`.
    Variant {
        /// The variant name being matched.
        variant: String,
        /// Binding names for the variant's payload fields (empty for unit variants).
        bindings: Vec<String>,
    },

    /// Destructuring tuple pattern, e.g. `(a, b, c)`.
    Tuple(Vec<Pattern>),

    /// Match a string literal, e.g. `"hello"`.
    StringLit(String),

    /// Bind the matched value to a variable name, e.g. `n` in `n if n > 0`.
    Variable(String),

    /// Pattern alternatives (OR patterns), e.g. `I8 | I16 | I32`.
    Or(Vec<Pattern>),
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
