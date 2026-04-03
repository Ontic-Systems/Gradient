//! Type expression and effect set AST nodes.
//!
//! These structures represent the type annotations that appear in function
//! signatures, let bindings, and type declarations. The current v0.1 grammar
//! supports named types (`i32`, `String`, etc.), the unit type `()`, and a
//! forward-looking function type constructor for future use.

use super::span::{Span, Spanned};

/// A type expression as written in source code.
///
/// The parser produces `Spanned<TypeExpr>` values wherever a type annotation
/// appears so that downstream passes can report errors at the correct
/// location.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// A named type, e.g. `i32` or `String`.
    Named(String),

    /// The unit type, written `()`.
    Unit,

    /// A function type `(A, B) -> C` or `(A) -> !{e} C`.
    ///
    /// Used in function parameter annotations to express higher-order function
    /// types, including effect annotations.
    Fn {
        /// The types of the function's parameters.
        params: Vec<Spanned<TypeExpr>>,
        /// The return type of the function.
        ret: Box<Spanned<TypeExpr>>,
        /// The effect set on this function type, if any.
        effects: Option<EffectSet>,
    },

    /// A generic (parameterized) type, e.g. `List[Int]` or `Option[String]`.
    ///
    /// Produced by the parser when a named type is followed by `[arg1, arg2]`.
    Generic {
        /// The base type name, e.g. `List`.
        name: String,
        /// The type arguments, e.g. `[Int]`.
        args: Vec<Spanned<TypeExpr>>,
    },

    /// A tuple type, e.g. `(Int, String, Bool)`.
    Tuple(Vec<Spanned<TypeExpr>>),

    /// A linear type, written `!linear T` in source code.
    ///
    /// Linear types enforce "use exactly once" semantics. Values of linear
    /// type must be explicitly consumed and cannot be silently dropped.
    Linear(Box<Spanned<TypeExpr>>),
}

/// A set of effects declared on a function's return type.
///
/// In Gradient source this is written as `!{IO, Fail}` immediately before
/// the return type in a function signature. The set contains one or more
/// effect names.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectSet {
    /// The effect names listed inside the `!{ ... }` brackets.
    pub effects: Vec<String>,
    /// The span covering the entire `!{ ... }` syntax, including the
    /// exclamation mark and braces.
    pub span: Span,
}
