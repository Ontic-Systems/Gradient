//! Internal type representation for the Gradient type checker.
//!
//! These types are the compiler's internal model of the Gradient type system.
//! They are distinct from [`TypeExpr`](crate::ast::types::TypeExpr), which
//! represents type annotations as written in source code. The type checker
//! resolves `TypeExpr` into `Ty` during analysis.

use std::fmt;
use serde::Serialize;

/// The internal representation of a Gradient type.
///
/// Every expression in a well-typed program is assigned a `Ty`. The type
/// checker infers or checks these during its walk over the AST.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Ty {
    /// A 64-bit signed integer.
    Int,
    /// A 64-bit floating-point number.
    Float,
    /// A UTF-8 string.
    String,
    /// A boolean value.
    Bool,
    /// The unit type, written `()`. This is the return type of functions and
    /// expressions that produce no meaningful value.
    Unit,
    /// A function type. Not exposed in the v0.1 surface syntax, but used
    /// internally by the type checker to model function signatures.
    Fn {
        /// The types of the function's parameters.
        params: Vec<Ty>,
        /// The return type.
        ret: Box<Ty>,
        /// The effects declared on this function.
        effects: Vec<std::string::String>,
    },
    /// A sentinel type used for error recovery.
    ///
    /// When a type error is detected, the erroneous sub-expression is given
    /// type `Error`. This type is compatible with everything: operations on
    /// `Error` silently produce `Error` without generating further diagnostics,
    /// preventing cascading error messages.
    Error,
}

impl Ty {
    /// Returns `true` if this type is a numeric type (`Int` or `Float`).
    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }

    /// Returns `true` if this type is the error sentinel.
    pub fn is_error(&self) -> bool {
        matches!(self, Ty::Error)
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Int => write!(f, "Int"),
            Ty::Float => write!(f, "Float"),
            Ty::String => write!(f, "String"),
            Ty::Bool => write!(f, "Bool"),
            Ty::Unit => write!(f, "()"),
            Ty::Fn {
                params,
                ret,
                effects,
            } => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ")")?;
                if !effects.is_empty() {
                    write!(f, " !{{{}}}", effects.join(", "))?;
                }
                write!(f, " -> {}", ret)
            }
            Ty::Error => write!(f, "<error>"),
        }
    }
}
