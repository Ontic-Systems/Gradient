//! Internal type representation for the Gradient type checker.
//!
//! These types are the compiler's internal model of the Gradient type system.
//! They are distinct from [`TypeExpr`](crate::ast::types::TypeExpr), which
//! represents type annotations as written in source code. The type checker
//! resolves `TypeExpr` into `Ty` during analysis.

use serde::Serialize;
use std::fmt;

/// Pony-style reference capability for compile-time data-race freedom.
///
/// Reference capabilities control aliasing and mutability at the type level,
/// enabling compile-time data-race freedom without a borrow checker.
///
/// | Capability | Mutable | Sendable | Description |
/// |------------|---------|----------|-------------|
/// | `Iso`      | Yes     | Yes      | Isolated - unique ownership, can send to other actors |
/// | `Val`      | No      | Yes      | Immutable - shared read-only, can send to other actors |
/// | `Ref`      | Yes     | No       | Mutable - confined to current actor (default) |
/// | `Box`      | No      | No       | Read-only - can read but not mutate |
/// | `Trn`      | Yes     | No       | Transitioning - becoming immutable, can become val |
/// | `Tag`      | No      | Yes      | Opaque identity - can't read/write, only identify |
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum RefCap {
    /// Isolated - unique ownership, can be sent to other actors.
    /// No other references to this object exist.
    Iso,
    /// Immutable - shared read-only, can be sent to other actors.
    /// Any number of read-only references may exist.
    Val,
    /// Mutable - confined to current actor (default for most types).
    /// Only one mutable reference, cannot cross actor boundaries.
    #[default]
    Ref,
    /// Read-only - can read but not mutate.
    /// May have multiple readers, but no writers.
    Box,
    /// Transitioning - mutable but intended to become immutable.
    /// Can be "consumed" to become val. Cannot be aliased during transition.
    Trn,
    /// Opaque identity - can't read/write, only identify (compare for equality).
    /// Used for lightweight actor references.
    Tag,
}

impl RefCap {
    /// Returns true if this capability allows mutation.
    pub fn is_mutable(&self) -> bool {
        matches!(self, RefCap::Iso | RefCap::Ref | RefCap::Trn)
    }

    /// Returns true if this capability can be sent to other actors.
    /// Only iso and val can cross actor boundaries safely.
    pub fn is_sendable(&self) -> bool {
        matches!(self, RefCap::Iso | RefCap::Val | RefCap::Tag)
    }

    /// Returns true if this capability allows reading.
    pub fn is_readable(&self) -> bool {
        !matches!(self, RefCap::Tag)
    }

    /// Returns true if this capability is immutable (cannot be mutated through).
    pub fn is_immutable(&self) -> bool {
        matches!(self, RefCap::Val | RefCap::Box)
    }

    /// Returns true if this capability is unique (no aliases allowed).
    pub fn is_unique(&self) -> bool {
        matches!(self, RefCap::Iso | RefCap::Trn)
    }

    /// Default capability for struct types (mutable, confined to actor).
    pub fn default_struct() -> Self {
        RefCap::Ref
    }

    /// Default capability for primitives (immutable, sendable).
    pub fn default_primitive() -> Self {
        RefCap::Val
    }
}

impl fmt::Display for RefCap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RefCap::Iso => write!(f, "iso"),
            RefCap::Val => write!(f, "val"),
            RefCap::Ref => write!(f, "ref"),
            RefCap::Box => write!(f, "box"),
            RefCap::Trn => write!(f, "trn"),
            RefCap::Tag => write!(f, "tag"),
        }
    }
}

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
    /// An enum (algebraic data type).
    ///
    /// Contains the enum's name and its variants. Each variant has a name
    /// and an optional field type (for tuple variants).
    Enum {
        /// The enum type name.
        name: std::string::String,
        /// The variants: `(variant_name, optional_field_type)`.
        variants: Vec<(std::string::String, Option<Ty>)>,
    },

    /// An unresolved type variable, introduced by generic type parameters.
    ///
    /// For example, in `fn identity[T](x: T) -> T`, the parameter `T` is
    /// represented as `TypeVar("T")` before being unified with a concrete type
    /// at each call site.
    TypeVar(std::string::String),

    /// An actor handle type, written `Actor[ActorName]` in source code.
    ///
    /// Created by `spawn ActorName` expressions. The `name` field records
    /// which actor type this handle refers to, enabling the type checker to
    /// validate that `send` and `ask` messages are handled by that actor.
    Actor {
        /// The actor type name this handle refers to.
        name: std::string::String,
    },

    /// A list type, e.g. `List[Int]`.
    List(Box<Ty>),

    /// A tuple type containing two or more elements.
    Tuple(Vec<Ty>),

    /// A range type, representing an integer sequence from start to end.
    Range,

    /// A map type, e.g. `Map[String, Int]` or `Map[String, String]`.
    ///
    /// In v0.1 the key type is always `String`; the value type is the second
    /// parameter and may be any type.
    Map(Box<Ty>, Box<Ty>),

    /// A set type, e.g. `Set[Int]` or `Set[String]`.
    ///
    /// Stores unique elements of type T. Backed by a hash table for O(1)
    /// average-case operations.
    Set(Box<Ty>),

    /// A sentinel type used for error recovery.
    ///
    /// FIFO (First-In-First-Out) queue with O(1) enqueue and dequeue operations.
    Queue(Box<Ty>),

    /// LIFO (Last-In-First-Out) stack with O(1) push and pop operations.
    ///
    /// Stack[T] is a persistent stack implemented as a linked list.
    Stack(Box<Ty>),

    /// A generational reference for mutable aliasing without borrow checker.
    ///
    /// GenRef[T] stores a pointer to T along with a generation number.
    /// Dereference checks the generation - returns Option[T] on get.
    /// This is Tier 2 of Gradient's memory model for graph structures.
    /// The capability controls what operations are allowed on this reference.
    GenRef {
        /// The inner type being referenced.
        inner: Box<Ty>,
        /// The reference capability for this generational reference.
        cap: RefCap,
    },

    /// A struct type with named fields and a reference capability.
    ///
    /// Structs are the primary user-defined data type in Gradient.
    /// The capability controls how the struct can be aliased and mutated.
    Struct {
        /// The struct type name.
        name: std::string::String,
        /// The fields: (field_name, field_type).
        fields: Vec<(std::string::String, Ty)>,
        /// The reference capability (default: Ref).
        cap: RefCap,
    },

    /// A linear type - must be used exactly once.
    ///
    /// Linear types enforce "use exactly once" semantics for kernel/drivers code.
    /// Values of linear type cannot be dropped implicitly and cannot be used
    /// more than once. This is Tier 3 of Gradient's memory model.
    Linear(Box<Ty>),

    /// The type of types - used for comptime type parameters.
    ///
    /// In `fn Vector(comptime T: type)`, the parameter `T` has type `Type`.
    /// This represents the type of type values at compile time.
    /// Values of this type cannot exist at runtime; they are compile-time only.
    Type,

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

    /// Returns `true` if this type is a type variable.
    pub fn is_type_var(&self) -> bool {
        matches!(self, Ty::TypeVar(_))
    }

    /// Returns `true` if this type is a linear type.
    pub fn is_linear(&self) -> bool {
        matches!(self, Ty::Linear(_))
    }

    /// Returns `true` if this type is only valid at compile time.
    pub fn is_comptime_only(&self) -> bool {
        matches!(self, Ty::Type)
    }

    /// Returns the inner type if this is a linear type, otherwise returns self.
    pub fn unwrap_linear(&self) -> &Ty {
        match self {
            Ty::Linear(inner) => inner,
            _ => self,
        }
    }

    /// Returns the reference capability of this type, if any.
    pub fn capability(&self) -> Option<RefCap> {
        match self {
            Ty::Struct { cap, .. } => Some(*cap),
            Ty::GenRef { cap, .. } => Some(*cap),
            // Primitives are always val (immutable, sendable)
            Ty::Int | Ty::Float | Ty::Bool | Ty::Unit => Some(RefCap::Val),
            // Strings are immutable by default
            Ty::String => Some(RefCap::Val),
            _ => None,
        }
    }

    /// Returns true if this type can be sent to another actor.
    /// Only iso, val, and tag capabilities are sendable.
    pub fn is_sendable(&self) -> bool {
        match self.capability() {
            Some(cap) => cap.is_sendable(),
            None => false, // Unknown capability = not sendable for safety
        }
    }

    /// Returns true if this type is mutable (can be written through).
    pub fn is_mutable(&self) -> bool {
        match self.capability() {
            Some(cap) => cap.is_mutable(),
            None => false,
        }
    }

    /// Returns a new type with the given capability.
    pub fn with_capability(&self, new_cap: RefCap) -> Self {
        match self {
            Ty::Struct { name, fields, .. } => Ty::Struct {
                name: name.clone(),
                fields: fields.clone(),
                cap: new_cap,
            },
            Ty::GenRef { inner, .. } => Ty::GenRef {
                inner: inner.clone(),
                cap: new_cap,
            },
            // For other types, just return a clone
            _ => self.clone(),
        }
    }

    /// Check if this type can be safely consumed (transitioned) to another capability.
    /// Returns true if the capability transition is valid.
    pub fn can_transition_to(&self, target: RefCap) -> bool {
        match self.capability() {
            Some(source) => can_subcap(source, target),
            None => false,
        }
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
            Ty::Enum { name, .. } => write!(f, "{}", name),
            Ty::TypeVar(name) => write!(f, "{}", name),
            Ty::List(elem) => write!(f, "List[{}]", elem),
            Ty::Tuple(elems) => {
                write!(f, "(")?;
                for (i, t) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", t)?;
                }
                write!(f, ")")
            }
            Ty::Actor { name } => write!(f, "Actor[{}]", name),
            Ty::Range => write!(f, "Range"),
            Ty::Map(k, v) => write!(f, "Map[{}, {}]", k, v),
            Ty::Set(elem) => write!(f, "Set[{}]", elem),
            Ty::Queue(elem) => write!(f, "Queue[{}]", elem),
            Ty::Stack(elem) => write!(f, "Stack[{}]", elem),
            Ty::GenRef { inner, cap } => {
                if *cap == RefCap::Ref {
                    write!(f, "GenRef[{}]", inner)
                } else {
                    write!(f, "GenRef[{}, {}]", inner, cap)
                }
            }
            Ty::Struct { name, fields, cap } => {
                if *cap == RefCap::Ref {
                    write!(f, "{}", name)?;
                } else {
                    write!(f, "{} {}", cap, name)?;
                }
                if !fields.is_empty() {
                    write!(f, " {{")?;
                    for (i, (fname, fty)) in fields.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}: {}", fname, fty)?;
                    }
                    write!(f, "}}")?;
                }
                Ok(())
            }
            Ty::Linear(elem) => write!(f, "!linear {}", elem),
            Ty::Error => write!(f, "<error>"),
            Ty::Type => write!(f, "Type"),
        }
    }
}

/// Capability subtyping rules for Pony-style reference capabilities.
///
/// Returns true if a reference with capability `source` can be safely
/// used where capability `target` is expected (source <: target).
///
/// # Subtyping Rules:
///
/// | Source | Can become         |
/// |--------|-------------------|
/// | `iso`  | val, box, tag     |
/// | `val`  | box, tag          |
/// | `ref`  | box, tag          |
/// | `box`  | tag               |
/// | `trn`  | val, box, tag     |
/// | `tag`  | tag only          |
pub fn can_subcap(source: RefCap, target: RefCap) -> bool {
    use RefCap::*;

    match (source, target) {
        // Iso is unique and sendable - can become anything except itself in certain contexts
        (Iso, Iso) => true,
        (Iso, Val) => true, // Iso can become immutable
        (Iso, Box) => true, // Iso can become read-only
        (Iso, Tag) => true, // Iso can become opaque

        // Val is immutable - can become read-only or tag
        (Val, Val) => true,
        (Val, Box) => true,
        (Val, Tag) => true,

        // Ref is mutable but confined - can become read-only or tag
        (Ref, Ref) => true,
        (Ref, Box) => true,
        (Ref, Tag) => true,

        // Box is read-only - can only become tag
        (Box, Box) => true,
        (Box, Tag) => true,

        // Trn is transitioning to immutable - can become val, box, or tag
        (Trn, Trn) => true,
        (Trn, Val) => true, // Trn can be "consumed" to become val
        (Trn, Box) => true,
        (Trn, Tag) => true,

        // Tag is opaque - can only be tag
        (Tag, Tag) => true,

        // All other transitions are invalid
        _ => false,
    }
}

/// Check if a capability transition is valid for actor message sending.
/// Returns an error message if the transition is not allowed.
pub fn check_actor_send_cap(ty: &Ty) -> Result<(), String> {
    match ty.capability() {
        Some(cap) if cap.is_sendable() => Ok(()),
        Some(cap) => Err(format!(
            "cannot send type with '{}' capability to another actor - only 'iso', 'val', and 'tag' can cross actor boundaries",
            cap
        )),
        None => Err("cannot send type with unknown capability to another actor".to_string()),
    }
}
