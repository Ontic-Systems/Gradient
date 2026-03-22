//! Top-level item AST nodes for the Gradient language.
//!
//! Items are the declarations that appear at the top level of a module:
//! function definitions, extern function declarations, let bindings, and
//! type aliases. Each item may carry zero or more annotations.

use super::block::Block;
use super::expr::Expr;
use super::span::{Span, Spanned};
use super::types::{EffectSet, TypeExpr};

/// A fully located top-level item node.
pub type Item = Spanned<ItemKind>;

/// The different kinds of top-level items in Gradient.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// A function definition with a body.
    FnDef(FnDef),

    /// An external function declaration (no body). Typically used for FFI or
    /// linking to functions defined outside the Gradient module.
    ExternFn(ExternFnDecl),

    /// A top-level `let` binding, e.g. `let PI: f64 = 3.14159`.
    Let {
        /// The name being bound.
        name: String,
        /// An optional explicit type annotation.
        type_ann: Option<Spanned<TypeExpr>>,
        /// The initializer expression.
        value: Expr,
        /// Whether this binding is mutable (`let mut`).
        mutable: bool,
    },

    /// A type alias declaration, e.g. `type Meters = f64`.
    TypeDecl {
        /// The name of the new type alias.
        name: String,
        /// The type expression on the right-hand side of `=`.
        type_expr: Spanned<TypeExpr>,
        /// Optional `///` doc comment attached to this type.
        doc_comment: Option<String>,
    },

    /// A module capability declaration, e.g. `@cap(IO, Net)`.
    ///
    /// Limits the effects any function in this module can use. If a function
    /// tries to use an effect not in the capability set, it's an error. This
    /// lets agents trust that an entire module only performs declared effects.
    CapDecl {
        /// The effects this module is allowed to use.
        allowed_effects: Vec<String>,
    },

    /// An enum (algebraic data type) declaration, e.g.
    /// `type Color = Red | Green | Blue`.
    EnumDecl {
        /// The name of the enum type.
        name: String,
        /// Type parameters for generic enums, e.g. `[T]` in `type Option[T]`.
        /// Empty for non-generic enums.
        type_params: Vec<String>,
        /// The variants of the enum.
        variants: Vec<EnumVariant>,
        /// Optional `///` doc comment attached to this enum.
        doc_comment: Option<String>,
    },

    /// An actor declaration, e.g.
    /// ```text
    /// actor Counter:
    ///     state count: Int = 0
    ///     on Increment:
    ///         count = count + 1
    ///     on GetCount -> Int:
    ///         ret count
    /// ```
    ActorDecl {
        /// The actor name.
        name: String,
        /// State fields declared with `state name: Type = default`.
        state_fields: Vec<StateField>,
        /// Message handlers declared with `on MessageName ...`.
        handlers: Vec<MessageHandler>,
        /// Optional `///` doc comment attached to this actor.
        doc_comment: Option<String>,
    },
}

/// A single variant in an enum declaration.
///
/// Variants can be either unit variants (no data, e.g. `Red`) or tuple
/// variants with a single field (e.g. `Some(Int)`).
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    /// The variant name.
    pub name: String,
    /// The optional field type for tuple variants. `None` for unit variants.
    pub field: Option<Spanned<TypeExpr>>,
    /// The span covering this variant declaration.
    pub span: Span,
}

/// A function definition, including its signature, body, and annotations.
///
/// Corresponds to the grammar rule:
/// ```text
/// fn IDENT ( params ) return_clause? : block
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct FnDef {
    /// The function name.
    pub name: String,
    /// Type parameters for generic functions, e.g. `[T, U]`.
    /// Empty for non-generic functions.
    pub type_params: Vec<String>,
    /// The function's formal parameters.
    pub params: Vec<Param>,
    /// The declared return type, if any. When omitted, the return type is
    /// inferred (or defaults to unit).
    pub return_type: Option<Spanned<TypeExpr>>,
    /// The effect set declared on the return type, if any (e.g. `!{IO}`).
    pub effects: Option<EffectSet>,
    /// The function body.
    pub body: Block,
    /// Annotations attached to this function (e.g. `@inline`).
    pub annotations: Vec<Annotation>,
    /// Design-by-contract annotations (`@requires`, `@ensures`).
    pub contracts: Vec<Contract>,
    /// Runtime capability budget annotation (`@budget(cpu: 5s, mem: 100mb)`).
    pub budget: Option<BudgetConstraint>,
    /// Whether this function is marked `@export` for C-compatible FFI.
    pub is_export: bool,
    /// Optional `///` doc comment attached to this function.
    pub doc_comment: Option<String>,
}

/// An external function declaration (no body).
///
/// Corresponds to the grammar rule:
/// ```text
/// fn IDENT ( params ) return_clause?
/// ```
/// appearing inside an `extern` context or annotated accordingly.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternFnDecl {
    /// The function name.
    pub name: String,
    /// The function's formal parameters.
    pub params: Vec<Param>,
    /// The declared return type, if any.
    pub return_type: Option<Spanned<TypeExpr>>,
    /// The effect set declared on the return type, if any.
    pub effects: Option<EffectSet>,
    /// Annotations attached to this extern function declaration.
    pub annotations: Vec<Annotation>,
    /// Optional library name for the linker, e.g. `@extern("libm")`.
    pub extern_lib: Option<String>,
    /// Optional `///` doc comment attached to this extern function.
    pub doc_comment: Option<String>,
}

/// A single function parameter.
///
/// Corresponds to the grammar rule `IDENT : type_expr`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The parameter name.
    pub name: String,
    /// The parameter's type annotation.
    pub type_ann: Spanned<TypeExpr>,
    /// The span covering the entire parameter (name and type annotation).
    pub span: Span,
}

/// An annotation attached to a top-level item.
///
/// Annotations are written `@name` or `@name(arg1, arg2)` and appear
/// immediately before the item they annotate.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// The annotation name (without the leading `@`).
    pub name: String,
    /// Optional argument expressions passed to the annotation.
    pub args: Vec<Expr>,
    /// The span covering the entire annotation from `@` through the
    /// closing `)` (or just `@name` if there are no arguments).
    pub span: Span,
}

/// The kind of a design-by-contract annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractKind {
    /// A precondition: must hold on function entry.
    Requires,
    /// A postcondition: must hold on function exit.
    Ensures,
}

/// A design-by-contract annotation on a function.
///
/// Contracts are written `@requires(condition)` (precondition) or
/// `@ensures(condition)` (postcondition). In `@ensures`, the special
/// identifier `result` refers to the function's return value.
#[derive(Debug, Clone, PartialEq)]
pub struct Contract {
    /// Whether this is a precondition or postcondition.
    pub kind: ContractKind,
    /// The boolean condition expression.
    pub condition: Expr,
    /// The span covering the entire contract annotation.
    pub span: Span,
}

/// A runtime capability budget annotation on a function.
///
/// Budget annotations are written `@budget(cpu: 5s, mem: 100mb)` and
/// declare resource limits that the compiler tracks at the type level.
/// Full runtime enforcement will come in a later phase.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetConstraint {
    /// CPU time budget, e.g. `"5s"`, `"100ms"`.
    pub cpu: Option<String>,
    /// Memory budget, e.g. `"100mb"`, `"1gb"`.
    pub mem: Option<String>,
    /// The span covering the entire `@budget(...)` annotation.
    pub span: Span,
}

/// A state field in an actor declaration.
///
/// State fields are declared with `state name: Type = default_value`.
#[derive(Debug, Clone, PartialEq)]
pub struct StateField {
    /// The field name.
    pub name: String,
    /// The field's type annotation.
    pub type_ann: Spanned<TypeExpr>,
    /// The default value expression.
    pub default_value: Expr,
    /// The span covering this state declaration.
    pub span: Span,
}

/// A message handler in an actor declaration.
///
/// Handlers are declared with `on MessageName -> ReturnType: body`.
/// The return type is optional (defaults to `()`).
#[derive(Debug, Clone, PartialEq)]
pub struct MessageHandler {
    /// The message name this handler responds to.
    pub message_name: String,
    /// The return type, if any. `None` means the handler returns `()`.
    pub return_type: Option<Spanned<TypeExpr>>,
    /// The handler body.
    pub body: Block,
    /// The span covering this handler declaration.
    pub span: Span,
}
