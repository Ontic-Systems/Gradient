//! Module-level AST nodes for the Gradient language.
//!
//! A [`Module`] is the root node of every parsed Gradient source file. It
//! contains an optional module declaration, a list of use (import)
//! declarations, and the top-level items that make up the module's body.

use super::item::Item;
use super::span::Span;

/// The root AST node for a single Gradient source file.
///
/// Corresponds to the grammar rule:
/// ```text
/// program → module_decl? use_decl* top_item*
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// An optional `mod` declaration that names this module, e.g.
    /// `mod std.io`.
    pub module_decl: Option<ModuleDecl>,
    /// Zero or more `use` declarations that import names from other modules.
    pub uses: Vec<UseDecl>,
    /// The top-level items (functions, type declarations, let bindings, etc.)
    /// defined in this module.
    pub items: Vec<Item>,
    /// The span covering the entire source file.
    pub span: Span,
}

/// A module declaration at the top of a source file.
///
/// Corresponds to the grammar rule:
/// ```text
/// module_decl → `mod` module_path
/// module_path → IDENT (`.` IDENT)*
/// ```
///
/// For example, `mod std.collections.list` would be represented with
/// `path: vec!["std", "collections", "list"]`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDecl {
    /// The segments of the module path, e.g. `["std", "io"]` for `mod std.io`.
    pub path: Vec<String>,
    /// The span covering the entire `mod` declaration.
    pub span: Span,
}

/// A use (import) declaration.
///
/// Corresponds to the grammar rule:
/// ```text
/// use_decl → `use` module_path (`.` `{` use_list `}`)?
/// ```
///
/// Examples:
/// - `use std.io` imports the module `std.io` as a whole
///   (`path: ["std", "io"], specific_imports: None`).
/// - `use std.io.{read, write}` imports specific names from `std.io`
///   (`path: ["std", "io"], specific_imports: Some(["read", "write"])`).
#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    /// The module path segments, e.g. `["std", "io"]`.
    pub path: Vec<String>,
    /// If the import uses the `{ name1, name2 }` syntax, this holds the
    /// list of specific names being imported. `None` means the entire
    /// module is imported.
    pub specific_imports: Option<Vec<String>>,
    /// The span covering the entire `use` declaration.
    pub span: Span,
}
