use crate::typechecker::types::Ty;
use std::collections::HashMap;

/// A value produced during compile-time evaluation.
///
/// This enum represents the possible results of evaluating an expression
/// at compile time. It can hold primitive values (integers, floats, booleans,
/// strings), types themselves (for type-level computation), unit, or errors.
#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeValue {
    /// A type value, used for type-level computation.
    Type(Ty),
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit floating-point number.
    Float(f64),
    /// A boolean value.
    Bool(bool),
    /// A UTF-8 string.
    String(String),
    /// A user-defined record/struct value.
    Record {
        type_name: String,
        fields: HashMap<String, ComptimeValue>,
    },
    /// A user-defined enum variant value.
    Variant {
        name: String,
        fields: Vec<(String, ComptimeValue)>,
    },
    /// A tuple value.
    Tuple(Vec<ComptimeValue>),
    /// A list literal value.
    List(Vec<ComptimeValue>),
    /// The unit value `()`.
    Unit,
    /// An error that occurred during evaluation.
    Error(String),
}

impl ComptimeValue {
    /// Convert this value to a type, if it represents one.
    ///
    /// Returns `Some(Ty)` if this value is `ComptimeValue::Type`,
    /// otherwise returns `None`.
    pub fn to_ty(&self) -> Option<Ty> {
        match self {
            Self::Type(t) => Some(t.clone()),
            _ => None,
        }
    }

    /// Returns the type name of this value as a static string.
    ///
    /// Useful for error messages and debugging.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Type(_) => "type",
            Self::Int(_) => "Int",
            Self::Float(_) => "Float",
            Self::Bool(_) => "Bool",
            Self::String(_) => "String",
            Self::Record { .. } => "record",
            Self::Variant { .. } => "variant",
            Self::Tuple(_) => "tuple",
            Self::List(_) => "list",
            Self::Unit => "Unit",
            Self::Error(_) => "Error",
        }
    }

    /// Returns true if this value is an error.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    /// Returns true if this value represents a unit type.
    pub fn is_unit(&self) -> bool {
        matches!(self, Self::Unit)
    }

    /// Convert this value to an integer, if it represents one.
    pub fn to_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Convert this value to a float, if it represents one.
    pub fn to_float(&self) -> Option<f64> {
        match self {
            Self::Float(n) => Some(*n),
            _ => None,
        }
    }

    /// Convert this value to a boolean, if it represents one.
    pub fn to_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Convert this value to a string, if it represents one.
    pub fn to_string_value(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }
}
