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

    /// A function type `(A, B) -> C`. Reserved for future use; the v0.1
    /// grammar does not surface this syntax to users, but other compiler
    /// passes may construct it internally.
    Fn {
        /// The types of the function's parameters.
        params: Vec<Spanned<TypeExpr>>,
        /// The return type of the function.
        ret: Box<Spanned<TypeExpr>>,
    },
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
