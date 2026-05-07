//! Module-level AST nodes for the Gradient language.
//!
//! A [`Module`] is the root node of every parsed Gradient source file. It
//! contains an optional module declaration, a list of use (import)
//! declarations, and the top-level items that make up the module's body.

use super::item::Item;
use super::span::Span;

/// The trust posture for a module (#360).
///
/// Each Gradient source file is implicitly `Trusted` unless it is
/// annotated with the module-level `@untrusted` attribute (or the
/// workspace default has been flipped — see #359). Untrusted modules
/// are produced by AI agents from external prompts and must not have
/// the same compile-time superpowers as human-authored code.
///
/// **Restrictions enforced on `@untrusted` modules** (addresses the related finding input
/// surface for #360):
///
/// 1. No comptime evaluation (`comptime { ... }` blocks rejected).
/// 2. No FFI (`@extern` rejected).
/// 3. Effects must be explicit on every function (no `effect_set: None`
///    inference at the function-signature level).
/// 4. No type / effect inference at module boundaries: every public
///    item must have an explicit type annotation.
///
/// See also: `docs/security/untrusted-source-mode.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustMode {
    /// Default. Full language available — comptime, FFI, inference, etc.
    #[default]
    Trusted,
    /// `@untrusted` module — restricted to a safe subset.
    Untrusted,
}

impl TrustMode {
    /// True iff this module is in `@untrusted` mode.
    pub fn is_untrusted(self) -> bool {
        matches!(self, TrustMode::Untrusted)
    }

    /// String form for diagnostics ("trusted" / "untrusted").
    pub fn as_str(self) -> &'static str {
        match self {
            TrustMode::Trusted => "trusted",
            TrustMode::Untrusted => "untrusted",
        }
    }
}

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
    /// Trust posture (#360). `Trusted` by default; flipped to
    /// `Untrusted` by a top-of-file `@untrusted` attribute.
    pub trust: TrustMode,
}

/// How a module was imported - by name or by file path.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportKind {
    /// Import by module path, e.g. `use std.io` or `use math.utils`
    ModulePath(Vec<String>),
    /// Import by file path, e.g. `use "./token.gr"` or `use "../lib/helper.gr"`
    FilePath(String),
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
/// use_decl → `use` (module_path | file_path) (`.` `{` use_list `}`)?
/// file_path → STRING_LITERAL
/// ```
///
/// Examples:
/// - `use std.io` imports the module `std.io` as a whole
///   (`import: ImportKind::ModulePath(["std", "io"]), specific_imports: None`).
/// - `use std.io.{read, write}` imports specific names from `std.io`
///   (`import: ImportKind::ModulePath(["std", "io"]), specific_imports: Some(["read", "write"])`).
/// - `use "./token.gr"` imports from a file path
///   (`import: ImportKind::FilePath("./token.gr"), specific_imports: None`).
#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    /// How this module is being imported - by module path or file path.
    pub import: ImportKind,
    /// If the import uses the `{ name1, name2 }` syntax, this holds the
    /// list of specific names being imported. `None` means the entire
    /// module is imported.
    pub specific_imports: Option<Vec<String>>,
    /// The span covering the entire `use` declaration.
    pub span: Span,
}

impl UseDecl {
    /// Get the module name for this import (for lookups and identification).
    /// For module paths, returns the last segment. For file paths, returns
    /// the filename without extension.
    pub fn module_name(&self) -> String {
        match &self.import {
            ImportKind::ModulePath(path) => path.last().cloned().unwrap_or_default(),
            ImportKind::FilePath(path) => std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("module")
                .to_string(),
        }
    }

    /// Get the full import path as a string for error messages.
    pub fn import_path_string(&self) -> String {
        match &self.import {
            ImportKind::ModulePath(path) => path.join("."),
            ImportKind::FilePath(path) => path.clone(),
        }
    }
}
