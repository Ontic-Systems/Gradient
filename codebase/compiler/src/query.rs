//! Structured query API for the Gradient compiler.
//!
//! This module turns the Gradient compiler from a batch-mode pipeline into a
//! queryable service. Instead of shelling out to a binary and parsing stdout,
//! agents call [`Session::from_source`] and query the resulting session for
//! structured, JSON-serializable data.
//!
//! # Example
//!
//! ```rust
//! use gradient_compiler::query::Session;
//!
//! let source = r#"
//! fn main() -> !{IO} ():
//!     print("hello")
//! "#;
//!
//! let session = Session::from_source(source);
//! let result = session.check();
//!
//! // Structured access
//! assert!(result.is_ok());
//! assert_eq!(result.diagnostics.len(), 0);
//!
//! // JSON for agent consumption
//! let json = result.to_json();
//! ```

use serde::Serialize;

use std::path::Path;

use crate::ast::item::{Repr, VariantField};
use crate::ast::module::Module;
use crate::ast::span::Span;
use crate::lexer::Lexer;
use crate::parser::error::ParseError;
use crate::parser::Parser;
use crate::resolve::ModuleResolver;
use crate::typechecker;
use crate::typechecker::effects::ModuleEffectSummary;
use crate::typechecker::error::TypeError;

// =========================================================================
// Core types
// =========================================================================

/// A compilation session holding the results of parsing and type checking.
///
/// Create one via [`Session::from_source`], then query it with methods like
/// [`check`](Session::check), [`symbols`](Session::symbols), or
/// [`type_at`](Session::type_at).
pub struct Session {
    /// The original source text, retained for positional queries.
    source: String,
    /// The parsed AST (if parsing succeeded or partially succeeded).
    module: Option<Module>,
    /// Parse errors collected during parsing.
    parse_errors: Vec<ParseError>,
    /// Type errors collected during type checking.
    type_errors: Vec<TypeError>,
    /// Effect analysis results (per-function inferred effects, purity).
    effect_summary: Option<ModuleEffectSummary>,
    /// Whether the session has been type-checked.
    type_checked: bool,
}

/// The severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// The compilation phase that produced a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Lexer,
    Parser,
    Typechecker,
    Ir,
    Codegen,
}

/// A unified diagnostic that agents can consume without knowing which
/// compiler phase produced it.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    /// Which compiler phase produced this diagnostic.
    pub phase: Phase,
    /// Error, warning, or info.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// Where in the source the problem was detected.
    pub span: Span,
    /// The type that was expected, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// The type that was found, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub found: Option<String>,
    /// Additional notes or suggestions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Structured typed-hole context. Set only for typed-hole diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hole: Option<TypedHoleInfo>,
}

/// Structured typed-hole context attached to a typed-hole diagnostic.
///
/// Mirrors the typechecker's [`crate::typechecker::error::TypedHoleData`] in
/// the public query surface so consumers (LSP, agent mode) can read fields
/// directly without parsing diagnostic notes.
#[derive(Debug, Clone, Serialize)]
pub struct TypedHoleInfo {
    /// The hole label as written in source, e.g. `"?"` or `"?goal"`.
    pub label: String,
    /// The expected type at the hole, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<String>,
    /// In-scope bindings whose type matches the expected type.
    pub matching_bindings: Vec<HoleBinding>,
    /// Functions whose return type matches the expected type.
    pub matching_functions: Vec<HoleFunction>,
}

/// A binding that matches a typed hole's expected type.
#[derive(Debug, Clone, Serialize)]
pub struct HoleBinding {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A function that returns a typed hole's expected type.
#[derive(Debug, Clone, Serialize)]
pub struct HoleFunction {
    pub name: String,
    pub signature: String,
}

/// The result of checking a source file.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    /// Whether the source is free of errors.
    pub ok: bool,
    /// Total number of errors.
    pub error_count: usize,
    /// All diagnostics (errors, warnings, info).
    pub diagnostics: Vec<Diagnostic>,
}

/// Information about a top-level symbol defined in the source.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolInfo {
    /// The symbol's name.
    pub name: String,
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// The type signature as a string.
    #[serde(rename = "type")]
    pub ty: String,
    /// The declared effects, if any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<String>,
    /// Effects inferred from the function body (what it actually uses).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inferred_effects: Vec<String>,
    /// Whether this function is provably pure (no effects).
    /// Only meaningful for functions; always true for variables/types.
    pub is_pure: bool,
    /// Parameter information (for functions).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamInfo>,
    /// Design-by-contract annotations on this function.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contracts: Vec<ContractInfo>,
    /// Whether this function uses effect polymorphism (has effect variables).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_effect_polymorphic: bool,
    /// Runtime capability budget annotation (`@budget(cpu: 5s, mem: 100mb)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetInfo>,
    /// Whether this is an extern function (FFI import).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_extern: bool,
    /// Optional library name for extern functions, e.g. `"libm"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extern_lib: Option<String>,
    /// Whether this function is marked `@export` for C-compatible FFI.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_export: bool,
    /// Whether this function is marked `@test` for the test framework.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_test: bool,
    /// Whether this function is marked `@bench` for the benchmark harness
    /// (E11 #371). Surfaced so `gradient bench` can discover bench functions
    /// without re-parsing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_bench: bool,
    /// Where this symbol is defined.
    pub span: Span,
    /// Optional `///` doc comment attached to this symbol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_comment: Option<String>,
    /// The fully-annotated inferred signature for functions without explicit
    /// effect annotations (#350 / #353). For example, if a local function
    /// body infers `!{IO, Heap}`, this field contains the complete signature
    /// string with effects applied. `None` when the function already has
    /// explicit effects or is not a function.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_signature: Option<String>,
}

/// Information about a design-by-contract annotation.
#[derive(Debug, Clone, Serialize)]
pub struct ContractInfo {
    /// The kind of contract: "requires" or "ensures".
    pub kind: String,
    /// A human-readable representation of the condition expression.
    pub condition: String,
    /// Whether this contract is stripped from release builds and audited.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub runtime_only_off_in_release: bool,
}

/// Information about a runtime capability budget annotation.
#[derive(Debug, Clone, Serialize)]
pub struct BudgetInfo {
    /// CPU time budget, e.g. `"5s"`, `"100ms"`. `None` if not specified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Memory budget, e.g. `"100mb"`, `"1gb"`. `None` if not specified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem: Option<String>,
}

/// The kind of a top-level symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    ExternFunction,
    Variable,
    TypeAlias,
    Actor,
    Trait,
    Impl,
}

/// Information about a function parameter.
#[derive(Debug, Clone, Serialize)]
pub struct ParamInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// The result of a rename operation.
#[derive(Debug, Clone, Serialize)]
pub struct RenameResult {
    /// The transformed source code with all renames applied.
    pub new_source: String,
    /// How many locations were renamed.
    pub locations_changed: usize,
    /// The specific locations that were changed.
    pub locations: Vec<RenameLocation>,
    /// Whether the renamed source still passes type checking.
    pub verification: RenameVerification,
}

/// A single location where a rename was applied.
#[derive(Debug, Clone, Serialize)]
pub struct RenameLocation {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number.
    pub col: u32,
    /// 0-based byte offset.
    pub offset: u32,
}

/// Verification result for a rename.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status")]
pub enum RenameVerification {
    /// The renamed source is error-free.
    #[serde(rename = "ok")]
    Ok,
    /// The renamed source has errors (rename may be incomplete or invalid).
    #[serde(rename = "has_errors")]
    HasErrors {
        error_count: usize,
        diagnostics: Vec<Diagnostic>,
    },
}

/// An entry in the call graph showing which functions a function calls.
#[derive(Debug, Clone, Serialize)]
pub struct CallGraphEntry {
    /// The function name.
    pub function: String,
    /// Functions called by this function.
    pub calls: Vec<String>,
}

// =========================================================================
// Context budget types
// =========================================================================

/// The result of a context budget query: the optimal context for editing
/// a specific function within a token budget.
///
/// Items are ranked by relevance and included greedily until the budget
/// is exhausted. This is designed for AI agents that need to understand
/// a function's context without reading entire source files.
#[derive(Debug, Clone, Serialize)]
pub struct ContextBudget {
    /// The function being edited.
    pub target_function: String,
    /// The requested token budget.
    pub budget_tokens: usize,
    /// The actual tokens used by the included items.
    pub used_tokens: usize,
    /// The context items, ordered by relevance (highest first).
    pub items: Vec<ContextItem>,
}

/// A single item in a context budget result.
#[derive(Debug, Clone, Serialize)]
pub struct ContextItem {
    /// The kind of context: "function_signature", "contract", "type_def",
    /// "capability", or "builtin".
    pub kind: String,
    /// The name of the item (function name, type name, etc.).
    pub name: String,
    /// The actual text to include in context.
    pub content: String,
    /// Approximate number of tokens for this item.
    pub token_estimate: usize,
    /// Relevance score from 0.0 (least) to 1.0 (most relevant).
    pub relevance: f32,
}

// =========================================================================
// Project index types
// =========================================================================

/// A structural index of the entire project, similar to Aider's RepoMap.
/// Provides a compact overview of all modules, functions, types, and their
/// relationships for AI agents.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectIndex {
    /// All modules in the project.
    pub modules: Vec<ModuleIndex>,
    /// The project-wide call graph.
    pub call_graph: Vec<CallGraphEntry>,
}

/// Index entry for a single module.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleIndex {
    /// Module name (from `mod` declaration or "main").
    pub name: String,
    /// Functions defined in this module.
    pub functions: Vec<FunctionIndex>,
    /// Types defined in this module (enums, type aliases).
    pub types: Vec<TypeIndex>,
    /// Module capability ceiling, if declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_ceiling: Option<Vec<String>>,
    /// Module panic strategy from the `@panic(abort|unwind|none)` attribute
    /// (#318/#337). Serialized as the string `"abort"`, `"unwind"`, or
    /// `"none"`. Defaults to `"unwind"` when no attribute is declared.
    ///
    /// Consumers (notably `gradient build`) use this field to pick the
    /// matching `runtime_panic_*.c` runtime crate at link time. See
    /// `codebase/compiler/runtime/panic/README.md` for the runtime layout.
    pub panic_strategy: String,
    /// Module alloc strategy derived from the effect summary (#333).
    /// Serialized as the string `"full"` or `"minimal"`.
    ///
    /// `"full"` when the module's `effects_used` contains `"Heap"` —
    /// program needs the refcount + COW machinery linked in.
    ///
    /// `"minimal"` when the module is provably heap-free — the
    /// `runtime_alloc_full.o` object is omitted at link time; only the
    /// canonical runtime + `runtime_alloc_minimal.o` (a tag-only object)
    /// are linked.
    ///
    /// The selection is automatic, NOT a user-facing module attribute.
    /// This mirrors ADR 0005's commitment to effect-driven DCE: the
    /// runtime closure is determined by the program's effect surface,
    /// not by explicit opt-in.
    ///
    /// See `codebase/compiler/runtime/alloc/README.md` for the runtime
    /// layout and `codebase/build-system/src/commands/build.rs` for the
    /// `detect_alloc_strategy` helper.
    pub alloc_strategy: String,
    /// Module actor strategy derived from the effect summary (#334).
    /// Serialized as the string `"full"` or `"none"`.
    ///
    /// `"full"` when the module's `effects_used` contains `"Actor"` —
    /// program needs the actor scheduler linked in.
    ///
    /// `"none"` when the module is provably actor-free — the
    /// `runtime_actor_full.o` object is omitted at link time; only the
    /// canonical runtime + `runtime_actor_none.o` (a tag-only object)
    /// are linked.
    ///
    /// The selection is automatic, NOT a user-facing module attribute.
    /// Sibling of `alloc_strategy` (#333): ADR 0005's commitment to
    /// effect-driven DCE applies to the actor scheduler closure too —
    /// programs that never spawn an actor shouldn't pay for one.
    ///
    /// See `codebase/compiler/runtime/actor/README.md` for the runtime
    /// layout and `codebase/build-system/src/commands/build.rs` for the
    /// `detect_actor_strategy` helper.
    pub actor_strategy: String,
    /// Module async strategy derived from the effect summary (#335).
    /// Serialized as the string `"full"` or `"none"`.
    ///
    /// `"full"` when the module's `effects_used` contains `"Async"` —
    /// program needs the async executor linked in.
    ///
    /// `"none"` when the module is provably async-free — the
    /// `runtime_async_full.o` object is omitted at link time; only the
    /// canonical runtime + `runtime_async_none.o` (a tag-only object)
    /// are linked.
    ///
    /// The selection is automatic, NOT a user-facing module attribute.
    /// Sibling of `actor_strategy` (#334): ADR 0005's commitment to
    /// effect-driven DCE applies to the async executor closure too —
    /// programs that never await shouldn't pay for the executor.
    ///
    /// See `codebase/compiler/runtime/async/README.md` for the runtime
    /// layout and `codebase/build-system/src/commands/build.rs` for the
    /// `detect_async_strategy` helper.
    pub async_strategy: String,
    /// Module allocator strategy (#336). Serialized as `"default"`,
    /// `"pluggable"`, or `"arena"`.
    ///
    /// `"default"` (the AST default) means the runtime ships a system
    /// allocator built on `malloc(3)` / `free(3)`. The
    /// `runtime_allocator_default.o` object is linked.
    ///
    /// `"pluggable"` means the embedder must supply
    /// `__gradient_alloc(size_t)` / `__gradient_free(void*)` at link
    /// time. The `runtime_allocator_pluggable.o` object is linked, and
    /// it forwards into the embedder's vtable.
    ///
    /// `"arena"` means the runtime crate itself supplies a
    /// process-global bump-pointer arena allocator backed by
    /// `runtime/memory/arena.{c,h}`. The
    /// `runtime_allocator_arena.o` object is linked. Frees are no-ops;
    /// the entire arena is reclaimed at process exit. First concrete
    /// `pluggable`-class implementation; closes the runtime-crate
    /// half of E3 #320.
    ///
    /// The selection is attribute-driven, NOT effect-driven — the
    /// embedder's deployment target (system host vs `no_std` /
    /// embedded board) is not derivable from the program's effect
    /// surface. Sibling of `panic_strategy` in that respect (#337);
    /// distinct from the effect-driven trio (#333/#334/#335).
    ///
    /// See `codebase/compiler/runtime/allocator/README.md` for the
    /// runtime layout and
    /// `codebase/build-system/src/commands/build.rs` for the
    /// `detect_allocator_strategy` helper.
    pub allocator_strategy: String,
    /// Module-level mode (#352, Epic #301): one of `"app"` or `"system"`.
    ///
    /// Mirrors the AST `Module.mode` field. `"app"` is the
    /// inference-everywhere default; `"system"` requires every `FnDef`
    /// to declare an explicit return type AND effect set (rejection
    /// enforced by the typechecker's `check_system_mode_restrictions`
    /// pass).
    ///
    /// Set by a top-of-file `@app` or `@system` attribute. Default is
    /// `"app"`.
    pub module_mode: String,
}

/// Index entry for a single function.
#[derive(Debug, Clone, Serialize)]
pub struct FunctionIndex {
    /// Function name.
    pub name: String,
    /// Full signature string, e.g. "fn factorial(n: Int) -> Int".
    pub signature: String,
    /// Declared effects.
    pub effects: Vec<String>,
    /// Whether this function is provably pure.
    pub is_pure: bool,
    /// Design-by-contract annotations.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub contracts: Vec<String>,
}

/// Index entry for a type definition.
#[derive(Debug, Clone, Serialize)]
pub struct TypeIndex {
    /// Type name.
    pub name: String,
    /// The kind of type: "enum", "alias".
    pub kind: String,
    /// A compact representation of the type definition.
    pub definition: String,
}

/// Type information at a specific source position.
#[derive(Debug, Clone, Serialize)]
pub struct TypeAtResult {
    /// The inferred or annotated type at this position.
    #[serde(rename = "type")]
    pub ty: String,
    /// The source span of the expression.
    pub span: Span,
    /// The kind of construct (variable, literal, call, etc.).
    pub kind: String,
}

/// Completion context at a source position: all the information an agent needs
/// to generate correct code at a cursor location.
#[derive(Debug, Clone, Serialize)]
pub struct CompletionContext {
    /// The expected type at the cursor position, if determinable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<String>,
    /// All variable bindings visible at the cursor position.
    pub bindings_in_scope: Vec<BindingInfo>,
    /// Functions whose return type matches the expected type.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matching_functions: Vec<String>,
    /// Enum variants available in scope.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub available_variants: Vec<String>,
    /// Builtin functions available in scope.
    pub available_builtins: Vec<String>,
}

/// Information about a single binding in scope.
#[derive(Debug, Clone, Serialize)]
pub struct BindingInfo {
    /// The binding name.
    pub name: String,
    /// The type of the binding as a human-readable string.
    #[serde(rename = "type")]
    pub ty: String,
    /// Whether this binding is mutable.
    pub mutable: bool,
}

/// A module contract: the machine-readable summary of a module's public API.
/// Designed to fit in <200 tokens for agent context windows.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleContract {
    /// Module name (from `mod` declaration or filename).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// All public symbols with their signatures.
    pub symbols: Vec<SymbolInfo>,
    /// Effects used anywhere in this module.
    pub effects_used: Vec<String>,
    /// Number of provably pure functions (no effects).
    pub pure_count: usize,
    /// Number of effectful functions.
    pub effectful_count: usize,
    /// Module-level capability ceiling (from `@cap(...)` declaration).
    /// If present, the compiler guarantees no function exceeds this set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_ceiling: Option<Vec<String>>,
    /// Call graph: which functions call which.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub call_graph: Vec<CallGraphEntry>,
    /// Whether this module has any errors.
    pub has_errors: bool,
}

// =========================================================================
// Documentation output types
// =========================================================================

/// Full documentation output for a module, designed for both JSON agent
/// consumption and human-readable text rendering.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleDocumentation {
    /// The module name (from `mod` declaration or "main").
    pub module: String,
    /// Module capability ceiling, if declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_ceiling: Option<Vec<String>>,
    /// Documentation for each function in the module.
    pub functions: Vec<FunctionDoc>,
    /// Documentation for each type in the module.
    pub types: Vec<TypeDoc>,
    /// Call graph summary.
    pub call_graph: Vec<CallGraphEntry>,
}

/// Documentation for a single function.
#[derive(Debug, Clone, Serialize)]
pub struct FunctionDoc {
    /// The function name.
    pub name: String,
    /// Full signature string, e.g. "fn factorial(n: Int) -> Int".
    pub signature: String,
    /// Type parameters (empty for non-generic functions).
    pub type_params: Vec<String>,
    /// Declared effects.
    pub effects: Vec<String>,
    /// Whether this function is provably pure.
    pub is_pure: bool,
    /// Design-by-contract annotations.
    pub contracts: Vec<ContractInfo>,
    /// Runtime capability budget annotation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetInfo>,
    /// Functions called by this function.
    pub calls: Vec<String>,
    /// Optional `///` doc comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_comment: Option<String>,
}

/// Documentation for a single type.
#[derive(Debug, Clone, Serialize)]
pub struct TypeDoc {
    /// The type name.
    pub name: String,
    /// The full definition string.
    pub definition: String,
    /// For enums: the variant descriptions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<String>,
    /// Optional `///` doc comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_comment: Option<String>,
}

// =========================================================================
// Session implementation
// =========================================================================

impl Session {
    /// Create a new compilation session from source code.
    ///
    /// This runs the lexer, parser, and type checker, storing all results
    /// for subsequent queries. The entire frontend pipeline completes
    /// eagerly so that all queries are O(1) lookups.
    pub fn from_source(source: &str) -> Self {
        let mut lexer = Lexer::new(source, 0);
        let tokens = lexer.tokenize();
        let (module, parse_errors) = Parser::parse(tokens, 0);

        let (type_errors, effect_summary) = if parse_errors.is_empty() {
            let (errors, summary) = typechecker::check_module_with_effects(&module, 0);
            (errors, Some(summary))
        } else {
            (Vec::new(), None)
        };

        Session {
            source: source.to_string(),
            module: Some(module),
            parse_errors,
            type_errors,
            effect_summary,
            type_checked: true,
        }
    }

    /// Create a new compilation session from a file path.
    ///
    /// This resolves all `use` declarations to files on disk, parses
    /// dependent modules, and builds a combined type environment spanning
    /// all modules. The session is created for the entry file.
    ///
    /// Returns `Err` if the file cannot be read.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let resolver = ModuleResolver::new(path);
        let result = resolver.resolve_all(path);

        if !result.errors.is_empty() {
            // Convert resolution errors into a session with type errors.
            let source = std::fs::read_to_string(path).unwrap_or_default();
            let mut session = Session::from_source(&source);
            // Add resolution errors as type errors (with a synthetic span).
            for err in &result.errors {
                session.type_errors.push(TypeError::new(
                    err.clone(),
                    Span::point(0, crate::ast::span::Position::new(1, 1, 0)),
                ));
            }
            return Ok(session);
        }

        // Get the entry module.
        let entry = result
            .modules
            .get(&result.entry_module)
            .ok_or_else(|| "entry module not found after resolution".to_string())?;

        let source = entry.source.clone();
        let entry_module_ast = entry.module.clone();
        let entry_parse_errors = entry.parse_errors.clone();
        let entry_file_id = entry.file_id;

        // Build import map: for each `use` declaration in the entry module,
        // extract function signatures from the imported module.
        let imports = Self::build_import_map(&entry_module_ast, &result.modules);

        // Type-check with imports.
        let (type_errors, effect_summary) = if entry_parse_errors.is_empty() {
            let (errors, summary) =
                typechecker::check_module_with_imports(&entry_module_ast, entry_file_id, &imports);
            (errors, Some(summary))
        } else {
            (Vec::new(), None)
        };

        Ok(Session {
            source,
            module: Some(entry_module_ast),
            parse_errors: entry_parse_errors,
            type_errors,
            effect_summary,
            type_checked: true,
        })
    }

    /// Build a map of imported module information from resolved modules.
    ///
    /// For each `use` declaration in the entry module, this extracts function
    /// signatures AND type definitions from the corresponding resolved module and
    /// builds the `ImportedModules` map that the type checker needs.
    pub fn build_import_map(
        entry_module: &Module,
        all_modules: &std::collections::HashMap<String, crate::resolve::ResolvedModule>,
    ) -> typechecker::ImportedModules {
        use crate::ast::item::{EnumVariant, ItemKind};
        use typechecker::ImportedModuleInfo;

        let mut imports = typechecker::ImportedModules::new();

        for use_decl in &entry_module.uses {
            let dep_name = use_decl.import_path_string();

            if let Some(dep) = all_modules.get(&dep_name) {
                let mut info = ImportedModuleInfo::default();

                for item in &dep.module.items {
                    match &item.node {
                        ItemKind::FnDef(fn_def) => {
                            let sig = Self::ast_fn_to_sig(fn_def);
                            // If specific imports are requested, only include those.
                            let include = if let Some(ref specific) = use_decl.specific_imports {
                                specific.contains(&fn_def.name)
                            } else {
                                true
                            };
                            if include {
                                info.functions.insert(fn_def.name.clone(), sig);
                            }
                        }
                        ItemKind::ExternFn(decl) => {
                            let sig = Self::ast_extern_fn_to_sig(decl);
                            let include = if let Some(ref specific) = use_decl.specific_imports {
                                specific.contains(&decl.name)
                            } else {
                                true
                            };
                            if include {
                                info.functions.insert(decl.name.clone(), sig);
                            }
                        }
                        ItemKind::EnumDecl {
                            name,
                            type_params,
                            variants,
                            doc_comment: _,
                        } => {
                            // Convert AST enum to typechecker Ty::Enum
                            // Ty::Enum stores Vec<(String, Option<Ty>)> for variants
                            use crate::ast::item::VariantField;
                            let variant_tys: Vec<(String, Option<typechecker::Ty>)> = variants
                                .iter()
                                .map(|v: &EnumVariant| {
                                    // Get the first field type if any (Gradient enums support single payload)
                                    let field_ty: Option<typechecker::Ty> =
                                        v.fields.as_ref().and_then(|fields| {
                                            fields.first().map(|f| match f {
                                                VariantField::Named { name: _, type_expr } => {
                                                    Self::resolve_type_expr_static(&type_expr.node)
                                                }
                                                VariantField::Anonymous(type_expr) => {
                                                    Self::resolve_type_expr_static(&type_expr.node)
                                                }
                                            })
                                        });
                                    (v.name.clone(), field_ty)
                                })
                                .collect();
                            let enum_ty = typechecker::Ty::Enum {
                                name: name.clone(),
                                variants: variant_tys,
                            };
                            info.enums.insert(name.clone(), enum_ty);

                            // Store type params for generic enum resolution
                            if !type_params.is_empty() {
                                info.enum_type_params
                                    .insert(name.clone(), type_params.clone());
                            }

                            // Register variant mappings for pattern matching
                            for (idx, variant) in variants.iter().enumerate() {
                                info.variant_mappings
                                    .insert(variant.name.clone(), (name.clone(), idx));
                            }
                        }
                        ItemKind::TypeDecl {
                            name, type_expr, ..
                        } => {
                            // Type alias - resolve the type expression
                            let resolved = Self::resolve_type_expr_static(&type_expr.node);
                            info.type_aliases.insert(name.clone(), resolved);
                        }
                        _ => {}
                    }
                }

                imports.insert(dep_name, info);
            }
        }

        imports
    }

    /// Convert an AST function definition to a type checker FnSig.
    fn ast_fn_to_sig(fn_def: &crate::ast::item::FnDef) -> typechecker::FnSig {
        let params: Vec<(String, typechecker::Ty, bool)> = fn_def
            .params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    Self::resolve_type_expr_static(&p.type_ann.node),
                    p.comptime,
                )
            })
            .collect();

        let ret = fn_def
            .return_type
            .as_ref()
            .map(|t| Self::resolve_type_expr_static(&t.node))
            .unwrap_or(typechecker::Ty::Unit);

        let effects = fn_def
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        typechecker::FnSig {
            type_params: fn_def
                .type_params
                .iter()
                .map(|tp| tp.name.clone())
                .collect(),
            params,
            ret,
            effects,
        }
    }

    /// Convert an AST extern function declaration to a type checker FnSig.
    fn ast_extern_fn_to_sig(decl: &crate::ast::item::ExternFnDecl) -> typechecker::FnSig {
        let params: Vec<(String, typechecker::Ty, bool)> = decl
            .params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    Self::resolve_type_expr_static(&p.type_ann.node),
                    p.comptime,
                )
            })
            .collect();

        let ret = decl
            .return_type
            .as_ref()
            .map(|t| Self::resolve_type_expr_static(&t.node))
            .unwrap_or(typechecker::Ty::Unit);

        let mut effects: Vec<String> = decl
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        // ADR 0002 / `#322`: surface the `FFI(_)` audit-trail effect on
        // every extern fn so the public Query API matches what the type
        // checker actually enforces. If the user already declared a
        // recognized `FFI(...)` ABI variant, keep it; otherwise synthesize
        // the default `FFI(C)`.
        let has_ffi = effects
            .iter()
            .any(|eff| typechecker::effects::is_ffi_effect(eff));
        if !has_ffi {
            effects.push(typechecker::effects::DEFAULT_FFI_EFFECT.to_string());
        }

        typechecker::FnSig {
            type_params: vec![],
            params,
            ret,
            effects,
        }
    }

    /// Resolve a TypeExpr to a Ty without needing a TypeChecker instance.
    /// This is used for building import maps from parsed module signatures.
    fn resolve_type_expr_static(te: &crate::ast::types::TypeExpr) -> typechecker::Ty {
        use crate::ast::types::TypeExpr;

        match te {
            TypeExpr::Named { name, cap: _ } => match name.as_str() {
                "Int" => typechecker::Ty::Int,
                "Float" => typechecker::Ty::Float,
                "String" => typechecker::Ty::String,
                "Bool" => typechecker::Ty::Bool,
                _ => typechecker::Ty::Error, // Unknown types in imports
            },
            TypeExpr::Unit => typechecker::Ty::Unit,
            TypeExpr::Fn {
                params,
                ret,
                effects,
            } => {
                let param_tys: Vec<typechecker::Ty> = params
                    .iter()
                    .map(|p| Self::resolve_type_expr_static(&p.node))
                    .collect();
                let ret_ty = Self::resolve_type_expr_static(&ret.node);
                let eff_list = effects
                    .as_ref()
                    .map(|e| e.effects.clone())
                    .unwrap_or_default();
                typechecker::Ty::Fn {
                    params: param_tys,
                    ret: Box::new(ret_ty),
                    effects: eff_list,
                }
            }
            TypeExpr::Generic { .. } => typechecker::Ty::Error,
            TypeExpr::Tuple(elems) => {
                let elem_tys: Vec<typechecker::Ty> = elems
                    .iter()
                    .map(|e| Self::resolve_type_expr_static(&e.node))
                    .collect();
                typechecker::Ty::Tuple(elem_tys)
            }
            TypeExpr::Record(fields) => {
                let field_tys: Vec<(String, typechecker::Ty)> = fields
                    .iter()
                    .map(|(n, ty)| (n.clone(), Self::resolve_type_expr_static(&ty.node)))
                    .collect();
                typechecker::Ty::Struct {
                    name: String::new(),
                    fields: field_tys,
                    cap: typechecker::types::RefCap::default_struct(),
                }
            }
            TypeExpr::Linear(inner) => {
                // Linear types resolve to their inner type for static resolution
                Self::resolve_type_expr_static(&inner.node)
            }
            TypeExpr::Type => typechecker::Ty::Type,
        }
    }

    /// Check the source and return structured diagnostics.
    ///
    /// This is the primary entry point for agents. Returns a [`CheckResult`]
    /// with all errors and warnings, serializable to JSON.
    pub fn check(&self) -> CheckResult {
        let mut diagnostics = Vec::new();

        for pe in &self.parse_errors {
            diagnostics.push(Diagnostic {
                phase: Phase::Parser,
                severity: Severity::Error,
                message: pe.message.clone(),
                span: pe.span,
                expected: if pe.expected.is_empty() {
                    None
                } else {
                    Some(pe.expected.join(", "))
                },
                found: Some(pe.found.clone()),
                notes: Vec::new(),
                hole: None,
            });
        }

        for te in &self.type_errors {
            let hole = te.hole_data.as_ref().map(|h| TypedHoleInfo {
                label: h.label.clone(),
                expected_type: h.expected_type.clone(),
                matching_bindings: h
                    .matching_bindings
                    .iter()
                    .map(|b| HoleBinding {
                        name: b.name.clone(),
                        ty: b.ty.clone(),
                    })
                    .collect(),
                matching_functions: h
                    .matching_functions
                    .iter()
                    .map(|f| HoleFunction {
                        name: f.name.clone(),
                        signature: f.signature.clone(),
                    })
                    .collect(),
            });
            diagnostics.push(Diagnostic {
                phase: Phase::Typechecker,
                severity: if te.is_warning {
                    Severity::Warning
                } else {
                    Severity::Error
                },
                message: te.message.clone(),
                span: te.span,
                expected: te.expected.as_ref().map(|t| t.to_string()),
                found: te.found.as_ref().map(|t| t.to_string()),
                notes: te.notes.clone(),
                hole,
            });
        }

        let error_count = diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();

        CheckResult {
            ok: error_count == 0,
            error_count,
            diagnostics,
        }
    }

    /// Return the fully-annotated inferred signature for a function (#353).
    ///
    /// If the named function exists and has no explicit effect annotation but
    /// the body infers effects, this returns the complete `-> !{Effects} Ret`
    /// string that the agent can optionally promote to explicit. Returns `None`
    /// if the function already has explicit effects, has no effects, or does
    /// not exist.
    ///
    /// Round-trip guarantee: re-checking with the inferred signature applied
    /// produces the same type-check result.
    pub fn inferred_signature(&self, fn_name: &str) -> Option<String> {
        self.symbols()
            .into_iter()
            .find(|s| s.name == fn_name && s.kind == SymbolKind::Function)
            .and_then(|s| s.inferred_signature)
    }

    /// Return all top-level symbols defined in the source.
    ///
    /// Each symbol includes its name, kind, type signature, effects, and
    /// location. This is what agents use to understand a module's API
    /// without reading its implementation.
    pub fn symbols(&self) -> Vec<SymbolInfo> {
        let module = match &self.module {
            Some(m) => m,
            None => return Vec::new(),
        };

        let mut symbols = Vec::new();

        for item in &module.items {
            match &item.node {
                crate::ast::item::ItemKind::FnDef(fn_def) => {
                    let ret_ty = fn_def
                        .return_type
                        .as_ref()
                        .map(|t| format_type_expr(&t.node))
                        .unwrap_or_else(|| "()".to_string());

                    let effects = fn_def
                        .effects
                        .as_ref()
                        .map(|e| e.effects.clone())
                        .unwrap_or_default();

                    let params: Vec<ParamInfo> = fn_def
                        .params
                        .iter()
                        .map(|p| ParamInfo {
                            name: p.name.clone(),
                            ty: format_type_expr(&p.type_ann.node),
                        })
                        .collect();

                    let type_params_str = if fn_def.type_params.is_empty() {
                        String::new()
                    } else {
                        let tp_strs: Vec<String> = fn_def
                            .type_params
                            .iter()
                            .map(|tp| {
                                if tp.bounds.is_empty() {
                                    tp.name.clone()
                                } else {
                                    format!("{}: {}", tp.name, tp.bounds.join(" + "))
                                }
                            })
                            .collect();
                        format!("[{}]", tp_strs.join(", "))
                    };

                    let sig = format!(
                        "fn {}{}({}){}{}",
                        fn_def.name,
                        type_params_str,
                        params
                            .iter()
                            .map(|p| format!("{}: {}", p.name, p.ty))
                            .collect::<Vec<_>>()
                            .join(", "),
                        if effects.is_empty() {
                            String::new()
                        } else {
                            format!(" -> !{{{}}}", effects.join(", "))
                        },
                        if ret_ty == "()" && effects.is_empty() {
                            String::new()
                        } else if effects.is_empty() {
                            format!(" -> {}", ret_ty)
                        } else {
                            format!(" {}", ret_ty)
                        }
                    );

                    // Look up inferred effects from the effect summary.
                    let (inferred_effects, is_pure) = self
                        .effect_summary
                        .as_ref()
                        .and_then(|s| s.functions.iter().find(|f| f.function == fn_def.name))
                        .map(|info| (info.inferred.clone(), info.is_pure))
                        .unwrap_or((Vec::new(), effects.is_empty()));

                    // Build contract info from AST contract annotations.
                    let contracts: Vec<ContractInfo> = fn_def
                        .contracts
                        .iter()
                        .map(|c| ContractInfo {
                            kind: match c.kind {
                                crate::ast::item::ContractKind::Requires => "requires".to_string(),
                                crate::ast::item::ContractKind::Ensures => "ensures".to_string(),
                            },
                            condition: format_expr(&c.condition),
                            runtime_only_off_in_release: c.runtime_only_off_in_release,
                        })
                        .collect();

                    let is_effect_polymorphic = effects.iter().any(|e| {
                        e.chars()
                            .next()
                            .map(|c| c.is_ascii_lowercase())
                            .unwrap_or(false)
                    }) || fn_def.params.iter().any(|p| {
                        if let crate::ast::types::TypeExpr::Fn {
                            effects: Some(eff), ..
                        } = &p.type_ann.node
                        {
                            eff.effects.iter().any(|e| {
                                e.chars()
                                    .next()
                                    .map(|c| c.is_ascii_lowercase())
                                    .unwrap_or(false)
                            })
                        } else {
                            false
                        }
                    });

                    let budget = fn_def.budget.as_ref().map(|b| BudgetInfo {
                        cpu: b.cpu.clone(),
                        mem: b.mem.clone(),
                    });

                    symbols.push(SymbolInfo {
                        name: fn_def.name.clone(),
                        kind: SymbolKind::Function,
                        ty: sig,
                        effects,
                        inferred_effects: inferred_effects.clone(),
                        is_pure,
                        params,
                        contracts,
                        is_effect_polymorphic,
                        budget,
                        is_extern: false,
                        extern_lib: None,
                        is_export: fn_def.is_export,
                        is_test: fn_def.is_test,
                        is_bench: fn_def.is_bench,
                        span: item.span,
                        doc_comment: fn_def.doc_comment.clone(),
                        // #350/#353: surface inferred signature when fn omits
                        // explicit effects but the body uses effects.
                        inferred_signature: if fn_def.effects.is_none()
                            && !inferred_effects.is_empty()
                        {
                            let ret_str = fn_def
                                .return_type
                                .as_ref()
                                .map(|t| format_type_expr(&t.node))
                                .unwrap_or_else(|| "()".to_string());
                            Some(format!(
                                "-> !{{{}}} {}",
                                inferred_effects.join(", "),
                                ret_str
                            ))
                        } else {
                            None
                        },
                    });
                }

                crate::ast::item::ItemKind::ExternFn(decl) => {
                    let effects = decl
                        .effects
                        .as_ref()
                        .map(|e| e.effects.clone())
                        .unwrap_or_default();

                    let params: Vec<ParamInfo> = decl
                        .params
                        .iter()
                        .map(|p| ParamInfo {
                            name: p.name.clone(),
                            ty: format_type_expr(&p.type_ann.node),
                        })
                        .collect();

                    let is_pure = effects.is_empty();

                    symbols.push(SymbolInfo {
                        name: decl.name.clone(),
                        kind: SymbolKind::ExternFunction,
                        ty: format!("@extern fn {}", decl.name),
                        effects: effects.clone(),
                        inferred_effects: effects,
                        is_pure,
                        params,
                        contracts: Vec::new(),
                        is_effect_polymorphic: false,
                        budget: None,
                        is_extern: true,
                        extern_lib: decl.extern_lib.clone(),
                        is_export: false,
                        is_test: false,
                        is_bench: false,
                        span: item.span,
                        doc_comment: decl.doc_comment.clone(),
                        inferred_signature: None,
                    });
                }

                crate::ast::item::ItemKind::Let {
                    name,
                    type_ann,
                    mutable,
                    ..
                } => {
                    let base_ty = type_ann
                        .as_ref()
                        .map(|t| format_type_expr(&t.node))
                        .unwrap_or_else(|| "<inferred>".to_string());

                    let ty = if *mutable {
                        format!("let mut {}: {}", name, base_ty)
                    } else {
                        format!("let {}: {}", name, base_ty)
                    };

                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::Variable,
                        ty,
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        is_effect_polymorphic: false,
                        budget: None,
                        is_extern: false,
                        extern_lib: None,
                        is_export: false,
                        is_test: false,
                        is_bench: false,
                        span: item.span,
                        doc_comment: None,
                        inferred_signature: None,
                    });
                }

                crate::ast::item::ItemKind::TypeDecl {
                    name,
                    type_expr,
                    repr,
                    ref doc_comment,
                    ..
                } => {
                    let ty = format_type_decl(name, &type_expr.node, *repr);
                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::TypeAlias,
                        ty,
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        is_effect_polymorphic: false,
                        budget: None,
                        is_extern: false,
                        extern_lib: None,
                        is_export: false,
                        is_test: false,
                        is_bench: false,
                        span: item.span,
                        doc_comment: doc_comment.clone(),
                        inferred_signature: None,
                    });
                }

                crate::ast::item::ItemKind::EnumDecl {
                    name,
                    type_params,
                    variants,
                    ref doc_comment,
                } => {
                    let variant_names: Vec<String> = variants
                        .iter()
                        .map(|v| {
                            if let Some(ref fields) = v.fields {
                                if fields.is_empty() {
                                    v.name.clone()
                                } else {
                                    let field_strs: Vec<String> = fields
                                        .iter()
                                        .map(|f| match f {
                                            VariantField::Named { name, type_expr } => {
                                                format!(
                                                    "{}: {}",
                                                    name,
                                                    format_type_expr(&type_expr.node)
                                                )
                                            }
                                            VariantField::Anonymous(type_expr) => {
                                                format_type_expr(&type_expr.node)
                                            }
                                        })
                                        .collect();
                                    format!("{}({})", v.name, field_strs.join(", "))
                                }
                            } else {
                                v.name.clone()
                            }
                        })
                        .collect();
                    let tp_str = if type_params.is_empty() {
                        String::new()
                    } else {
                        format!("[{}]", type_params.join(", "))
                    };
                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::TypeAlias,
                        ty: format!("type {}{} = {}", name, tp_str, variant_names.join(" | ")),
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        is_effect_polymorphic: false,
                        budget: None,
                        is_extern: false,
                        extern_lib: None,
                        is_export: false,
                        is_test: false,
                        is_bench: false,
                        span: item.span,
                        doc_comment: doc_comment.clone(),
                        inferred_signature: None,
                    });
                }

                crate::ast::item::ItemKind::ActorDecl {
                    name,
                    state_fields,
                    handlers,
                    doc_comment,
                } => {
                    let handler_strs: Vec<String> = handlers
                        .iter()
                        .map(|h| {
                            let ret = h
                                .return_type
                                .as_ref()
                                .map(|t| format!(" -> {}", format_type_expr(&t.node)))
                                .unwrap_or_default();
                            format!("on {}{}", h.message_name, ret)
                        })
                        .collect();
                    let state_strs: Vec<String> = state_fields
                        .iter()
                        .map(|sf| format!("{}: {}", sf.name, format_type_expr(&sf.type_ann.node)))
                        .collect();

                    let mut parts = Vec::new();
                    if !state_strs.is_empty() {
                        parts.push(format!("state: {}", state_strs.join(", ")));
                    }
                    if !handler_strs.is_empty() {
                        parts.push(format!("handlers: {}", handler_strs.join(", ")));
                    }

                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::Actor,
                        ty: format!("actor {} {{ {} }}", name, parts.join("; ")),
                        effects: vec!["Actor".to_string(), "Async".to_string(), "Send".to_string()],
                        inferred_effects: Vec::new(),
                        is_pure: false,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        is_effect_polymorphic: false,
                        budget: None,
                        is_extern: false,
                        extern_lib: None,
                        is_export: false,
                        is_test: false,
                        is_bench: false,
                        span: item.span,
                        doc_comment: doc_comment.clone(),
                        inferred_signature: None,
                    });
                }

                crate::ast::item::ItemKind::LetTupleDestructure { names, .. } => {
                    // Each destructured name is a separate symbol.
                    for name in names {
                        symbols.push(SymbolInfo {
                            name: name.clone(),
                            kind: SymbolKind::Variable,
                            ty: format!("let {}: <inferred>", name),
                            effects: Vec::new(),
                            inferred_effects: Vec::new(),
                            is_pure: true,
                            params: Vec::new(),
                            contracts: Vec::new(),
                            is_effect_polymorphic: false,
                            budget: None,
                            is_extern: false,
                            extern_lib: None,
                            is_export: false,
                            is_test: false,
                            is_bench: false,
                            span: item.span,
                            doc_comment: None,
                            inferred_signature: None,
                        });
                    }
                }

                crate::ast::item::ItemKind::CapDecl { .. } => {
                    // Module capability declarations are not symbols -- they constrain effects.
                }

                crate::ast::item::ItemKind::CapTypeDecl {
                    name,
                    ref doc_comment,
                } => {
                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::TypeAlias,
                        ty: format!("cap {}", name),
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        is_effect_polymorphic: false,
                        budget: None,
                        is_extern: false,
                        extern_lib: None,
                        is_export: false,
                        is_test: false,
                        is_bench: false,
                        span: item.span,
                        doc_comment: doc_comment.clone(),
                        inferred_signature: None,
                    });
                }

                crate::ast::item::ItemKind::TraitDecl {
                    name,
                    methods,
                    ref doc_comment,
                } => {
                    let method_strs: Vec<String> = methods
                        .iter()
                        .map(|m| {
                            let params_str: String = m
                                .params
                                .iter()
                                .map(|p| {
                                    if p.name == "self" {
                                        "self".to_string()
                                    } else {
                                        format!(
                                            "{}: {}",
                                            p.name,
                                            format_type_expr(&p.type_ann.node)
                                        )
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|t| format!(" -> {}", format_type_expr(&t.node)))
                                .unwrap_or_default();
                            format!("fn {}({}){}", m.name, params_str, ret)
                        })
                        .collect();

                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::Trait,
                        ty: format!("trait {} {{ {} }}", name, method_strs.join("; ")),
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        is_effect_polymorphic: false,
                        budget: None,
                        is_extern: false,
                        extern_lib: None,
                        is_export: false,
                        is_test: false,
                        is_bench: false,
                        span: item.span,
                        doc_comment: doc_comment.clone(),
                        inferred_signature: None,
                    });
                }

                crate::ast::item::ItemKind::ImplBlock {
                    trait_name,
                    target_type,
                    methods,
                } => {
                    let method_names: Vec<String> =
                        methods.iter().map(|m| m.name.clone()).collect();
                    symbols.push(SymbolInfo {
                        name: format!("{} for {}", trait_name, target_type),
                        kind: SymbolKind::Impl,
                        ty: format!(
                            "impl {} for {} {{ {} }}",
                            trait_name,
                            target_type,
                            method_names.join(", ")
                        ),
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        is_effect_polymorphic: false,
                        budget: None,
                        is_extern: false,
                        extern_lib: None,
                        is_export: false,
                        is_test: false,
                        is_bench: false,
                        span: item.span,
                        doc_comment: None,
                        inferred_signature: None,
                    });
                }
                crate::ast::item::ItemKind::ModBlock {
                    items: mod_items, ..
                } => {
                    // Recursively collect symbols from module block items
                    for mod_item in mod_items {
                        // Create a minimal symbol info for items in mod blocks
                        // that are accessible through recursive processing
                        match &mod_item.node {
                            crate::ast::item::ItemKind::FnDef(fn_def) => {
                                let params: Vec<ParamInfo> = fn_def
                                    .params
                                    .iter()
                                    .map(|p| ParamInfo {
                                        name: p.name.clone(),
                                        ty: format_type_expr(&p.type_ann.node),
                                    })
                                    .collect();

                                symbols.push(SymbolInfo {
                                    name: fn_def.name.clone(),
                                    kind: SymbolKind::Function,
                                    ty: format!("fn {}(...)", fn_def.name),
                                    effects: Vec::new(),
                                    inferred_effects: Vec::new(),
                                    is_pure: true,
                                    params,
                                    contracts: Vec::new(),
                                    is_effect_polymorphic: false,
                                    budget: None,
                                    is_extern: false,
                                    extern_lib: None,
                                    is_export: fn_def.is_export,
                                    is_test: fn_def.is_test,
                                    is_bench: fn_def.is_bench,
                                    span: mod_item.span,
                                    doc_comment: fn_def.doc_comment.clone(),
                                    inferred_signature: None,
                                });
                            }
                            _ => {
                                // Other item types in mod blocks are handled separately
                            }
                        }
                    }
                }
                // Import declarations don't create symbols directly.
                crate::ast::item::ItemKind::Import { .. } => {}
            }
        }

        symbols
    }

    /// Generate a module contract: a compact, machine-readable summary of the
    /// module's public API designed for agent context windows.
    pub fn module_contract(&self) -> ModuleContract {
        let module_name = self
            .module
            .as_ref()
            .and_then(|m| m.module_decl.as_ref())
            .map(|d| d.path.join("."));

        let symbols = self.symbols();

        let mut effects_used: Vec<String> = symbols
            .iter()
            .flat_map(|s| s.effects.iter().cloned())
            .collect();
        effects_used.sort();
        effects_used.dedup();

        let has_errors = !self.parse_errors.is_empty() || !self.type_errors.is_empty();

        let capability_ceiling = self
            .effect_summary
            .as_ref()
            .and_then(|s| s.capability_ceiling.clone());

        let pure_count = symbols.iter().filter(|s| s.is_pure).count();
        let effectful_count = symbols
            .iter()
            .filter(|s| {
                !s.is_pure && matches!(s.kind, SymbolKind::Function | SymbolKind::ExternFunction)
            })
            .count();

        ModuleContract {
            name: module_name,
            symbols,
            effects_used,
            pure_count,
            effectful_count,
            capability_ceiling,
            call_graph: self.call_graph(),
            has_errors,
        }
    }

    /// Generate full API documentation for the module.
    ///
    /// Produces a [`ModuleDocumentation`] structure containing everything
    /// an agent or human needs to understand a module's public API: function
    /// signatures, contracts, effects, types, doc comments, and call graph.
    pub fn documentation(&self) -> ModuleDocumentation {
        let module_name = self
            .module
            .as_ref()
            .and_then(|m| m.module_decl.as_ref())
            .map(|d| d.path.join("."))
            .unwrap_or_else(|| "main".to_string());

        let capability_ceiling = self
            .effect_summary
            .as_ref()
            .and_then(|s| s.capability_ceiling.clone());

        let symbols = self.symbols();
        let call_graph = self.call_graph();

        // Build function docs.
        let mut functions = Vec::new();
        for sym in &symbols {
            match sym.kind {
                SymbolKind::Function | SymbolKind::ExternFunction => {
                    // Find callees for this function.
                    let calls = call_graph
                        .iter()
                        .find(|e| e.function == sym.name)
                        .map(|e| e.calls.clone())
                        .unwrap_or_default();

                    // Extract type params from the signature.
                    let type_params = self.extract_type_params(&sym.name);

                    functions.push(FunctionDoc {
                        name: sym.name.clone(),
                        signature: sym.ty.clone(),
                        type_params,
                        effects: sym.effects.clone(),
                        is_pure: sym.is_pure,
                        contracts: sym.contracts.clone(),
                        budget: sym.budget.clone(),
                        calls,
                        doc_comment: sym.doc_comment.clone(),
                    });
                }
                _ => {}
            }
        }

        // Build type docs.
        let mut types = Vec::new();
        if let Some(module) = &self.module {
            for item in &module.items {
                match &item.node {
                    crate::ast::item::ItemKind::EnumDecl {
                        name,
                        type_params,
                        variants,
                        ref doc_comment,
                    } => {
                        let tp_str = if type_params.is_empty() {
                            String::new()
                        } else {
                            format!("[{}]", type_params.join(", "))
                        };
                        let variant_strs: Vec<String> = variants
                            .iter()
                            .map(|v| {
                                if let Some(ref fields) = v.fields {
                                    if fields.is_empty() {
                                        v.name.clone()
                                    } else {
                                        let field_strs: Vec<String> = fields
                                            .iter()
                                            .map(|f| match f {
                                                VariantField::Named { name, type_expr } => {
                                                    format!(
                                                        "{}: {}",
                                                        name,
                                                        format_type_expr(&type_expr.node)
                                                    )
                                                }
                                                VariantField::Anonymous(type_expr) => {
                                                    format_type_expr(&type_expr.node)
                                                }
                                            })
                                            .collect();
                                        format!("{}({})", v.name, field_strs.join(", "))
                                    }
                                } else {
                                    v.name.clone()
                                }
                            })
                            .collect();
                        let definition =
                            format!("type {}{} = {}", name, tp_str, variant_strs.join(" | "));
                        types.push(TypeDoc {
                            name: name.clone(),
                            definition,
                            variants: variant_strs,
                            doc_comment: doc_comment.clone(),
                        });
                    }
                    crate::ast::item::ItemKind::TypeDecl {
                        name,
                        type_expr,
                        ref doc_comment,
                        repr,
                        ..
                    } => {
                        let definition = format_type_decl(name, &type_expr.node, *repr);
                        types.push(TypeDoc {
                            name: name.clone(),
                            definition,
                            variants: Vec::new(),
                            doc_comment: doc_comment.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }

        ModuleDocumentation {
            module: module_name,
            capability_ceiling,
            functions,
            types,
            call_graph,
        }
    }

    /// Extract type parameters for a function from the AST.
    fn extract_type_params(&self, fn_name: &str) -> Vec<String> {
        if let Some(module) = &self.module {
            for item in &module.items {
                if let crate::ast::item::ItemKind::FnDef(fn_def) = &item.node {
                    if fn_def.name == fn_name {
                        return fn_def
                            .type_params
                            .iter()
                            .map(|tp| tp.name.clone())
                            .collect();
                    }
                }
            }
        }
        Vec::new()
    }

    /// Format documentation as human-readable text.
    pub fn documentation_text(&self) -> String {
        let doc = self.documentation();
        let mut out = String::new();

        // Module header.
        out.push_str(&format!("Module: {}\n", doc.module));
        if let Some(ref ceiling) = doc.capability_ceiling {
            out.push_str(&format!("Capability: {}\n", ceiling.join(", ")));
        } else {
            out.push_str("Capability: unrestricted\n");
        }
        out.push('\n');

        // Functions.
        for func in &doc.functions {
            // Signature with purity marker.
            if func.is_pure {
                out.push_str(&format!("{}  [pure]\n", func.signature));
            } else if !func.effects.is_empty() {
                out.push_str(&format!(
                    "{}  [effects: {}]\n",
                    func.signature,
                    func.effects.join(", ")
                ));
            } else {
                out.push_str(&format!("{}\n", func.signature));
            }

            // Doc comment.
            if let Some(ref comment) = func.doc_comment {
                for line in comment.lines() {
                    out.push_str(&format!("  {}\n", line));
                }
            }

            // Contracts.
            for contract in &func.contracts {
                out.push_str(&format!("  @{}({})\n", contract.kind, contract.condition));
            }

            // Budget.
            if let Some(ref budget) = func.budget {
                let mut parts = Vec::new();
                if let Some(ref cpu) = budget.cpu {
                    parts.push(format!("cpu: {}", cpu));
                }
                if let Some(ref mem) = budget.mem {
                    parts.push(format!("mem: {}", mem));
                }
                out.push_str(&format!("  @budget({})\n", parts.join(", ")));
            }

            // Calls.
            if !func.calls.is_empty() {
                let calls_str = func
                    .calls
                    .iter()
                    .map(|c| {
                        if *c == func.name {
                            format!("{} (recursive)", c)
                        } else {
                            c.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("  Calls: {}\n", calls_str));
            }

            out.push('\n');
        }

        // Types.
        for ty in &doc.types {
            out.push_str(&format!("{}\n", ty.definition));
            if let Some(ref comment) = ty.doc_comment {
                for line in comment.lines() {
                    out.push_str(&format!("  {}\n", line));
                }
            }
            out.push('\n');
        }

        out
    }

    /// Query the type at a specific source position (line, column).
    ///
    /// Returns `None` if no expression spans the given position.
    /// Lines and columns are 1-based.
    pub fn type_at(&self, line: u32, col: u32) -> Option<TypeAtResult> {
        // For now, we do a simple lookup: find the symbol whose span contains
        // this position. Full expression-level type_at requires a type map
        // from the checker (future enhancement).
        let symbols = self.symbols();

        for sym in &symbols {
            if position_in_span(line, col, &sym.span) {
                return Some(TypeAtResult {
                    ty: sym.ty.clone(),
                    span: sym.span,
                    kind: format!("{:?}", sym.kind),
                });
            }
        }

        None
    }

    /// Return the completion context at a specific source position (line, column).
    ///
    /// This provides agents with everything they need to generate correct code at
    /// a cursor position:
    /// - The expected type (if determinable from the surrounding context)
    /// - All bindings in scope with their types
    /// - Functions whose return type matches the expected type
    /// - Available enum variants
    /// - Available builtin functions
    ///
    /// Lines and columns are 1-based.
    pub fn completion_context(&self, line: u32, col: u32) -> CompletionContext {
        let module = match &self.module {
            Some(m) => m,
            None => {
                return CompletionContext {
                    expected_type: None,
                    bindings_in_scope: Vec::new(),
                    matching_functions: Vec::new(),
                    available_variants: Vec::new(),
                    available_builtins: Vec::new(),
                };
            }
        };

        // Walk the AST to determine:
        // 1. Which function (if any) the cursor is inside
        // 2. What the expected type is at the cursor position
        // 3. What bindings are in scope
        //
        // We do this by re-running a lightweight scope analysis that tracks
        // bindings as it walks into the function containing the cursor.

        let mut expected_type: Option<typechecker::Ty> = None;
        let mut bindings: Vec<BindingInfo> = Vec::new();
        let mut in_function: Option<&crate::ast::item::FnDef> = None;

        // Find which function contains this position.
        for item in &module.items {
            match &item.node {
                crate::ast::item::ItemKind::FnDef(fn_def)
                    if position_in_span(line, col, &item.span) =>
                {
                    in_function = Some(fn_def);
                    break;
                }
                crate::ast::item::ItemKind::Let {
                    name,
                    type_ann,
                    value,
                    mutable,
                } => {
                    // Top-level let bindings are always in scope.
                    let ty_str = type_ann
                        .as_ref()
                        .map(|t| format_type_expr(&t.node))
                        .unwrap_or_else(|| infer_expr_type_static(value));
                    bindings.push(BindingInfo {
                        name: name.clone(),
                        ty: ty_str,
                        mutable: *mutable,
                    });

                    // If cursor is on the RHS of this let binding, the expected
                    // type is the annotation type.
                    if position_in_span(line, col, &value.span) {
                        if let Some(ann) = type_ann {
                            expected_type = Some(Self::resolve_type_expr_static(&ann.node));
                        }
                    }
                }
                _ => {}
            }
        }

        // Collect all top-level functions as bindings (they're callable).
        for item in &module.items {
            match &item.node {
                crate::ast::item::ItemKind::FnDef(fn_def) => {
                    let ret_ty = fn_def
                        .return_type
                        .as_ref()
                        .map(|t| format_type_expr(&t.node))
                        .unwrap_or_else(|| "()".to_string());
                    let params_str = fn_def
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, format_type_expr(&p.type_ann.node)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    bindings.push(BindingInfo {
                        name: fn_def.name.clone(),
                        ty: format!("fn({}) -> {}", params_str, ret_ty),
                        mutable: false,
                    });
                }
                crate::ast::item::ItemKind::ExternFn(decl) => {
                    let ret_ty = decl
                        .return_type
                        .as_ref()
                        .map(|t| format_type_expr(&t.node))
                        .unwrap_or_else(|| "()".to_string());
                    let params_str = decl
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, format_type_expr(&p.type_ann.node)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    bindings.push(BindingInfo {
                        name: decl.name.clone(),
                        ty: format!("fn({}) -> {}", params_str, ret_ty),
                        mutable: false,
                    });
                }
                _ => {}
            }
        }

        // If inside a function, add its parameters and walk its body for
        // local let bindings visible at the cursor.
        if let Some(fn_def) = in_function {
            // Parameters are in scope.
            for param in &fn_def.params {
                bindings.push(BindingInfo {
                    name: param.name.clone(),
                    ty: format_type_expr(&param.type_ann.node),
                    mutable: false,
                });
            }

            // If the cursor is in the function body, the function's return type
            // is a candidate expected type (when at a tail position or after `ret`).
            let ret_ty = fn_def
                .return_type
                .as_ref()
                .map(|t| Self::resolve_type_expr_static(&t.node));

            // Walk the body to find let bindings defined before the cursor and
            // to determine expected type from context.
            self.walk_block_for_completion(
                &fn_def.body,
                line,
                col,
                &mut bindings,
                &mut expected_type,
                &ret_ty,
            );

            // If we still have no expected type and the cursor is in the body,
            // use the function return type as a fallback (tail position).
            if expected_type.is_none() {
                if let Some(ref rt) = ret_ty {
                    if position_in_span(line, col, &fn_def.body.span) {
                        expected_type = Some(rt.clone());
                    }
                }
            }
        }

        // Build the list of all available builtins.
        let builtins = vec![
            "print",
            "println",
            "range",
            "to_string",
            "print_int",
            "print_float",
            "print_bool",
            "int_to_string",
            "abs",
            "min",
            "max",
            "mod_int",
            // Map operations (Phase OO)
            "map_new",
            "map_set",
            "map_get",
            "map_contains",
            "map_remove",
            "map_size",
            "map_keys",
            // HashMap operations (Self-Hosting Phase 1.1)
            "hashmap_new",
            "hashmap_insert",
            "hashmap_get",
            "hashmap_remove",
            "hashmap_contains",
            "hashmap_len",
        ];
        let available_builtins: Vec<String> = builtins.iter().map(|s| s.to_string()).collect();

        // Collect available enum variants.
        let mut available_variants: Vec<String> = Vec::new();
        for item in &module.items {
            if let crate::ast::item::ItemKind::EnumDecl { name, variants, .. } = &item.node {
                for variant in variants {
                    if let Some(ref fields) = variant.fields {
                        if fields.is_empty() {
                            available_variants.push(format!("{}::{}", name, variant.name));
                        } else {
                            let field_strs: Vec<String> = fields
                                .iter()
                                .map(|f| match f {
                                    VariantField::Named { name, type_expr } => {
                                        format!("{}: {}", name, format_type_expr(&type_expr.node))
                                    }
                                    VariantField::Anonymous(type_expr) => {
                                        format_type_expr(&type_expr.node)
                                    }
                                })
                                .collect();
                            available_variants.push(format!(
                                "{}::{}({})",
                                name,
                                variant.name,
                                field_strs.join(", ")
                            ));
                        }
                    } else {
                        available_variants.push(format!("{}::{}", name, variant.name));
                    }
                }
            }
        }

        // Find functions whose return type matches the expected type.
        let mut matching_functions: Vec<String> = Vec::new();
        if let Some(ref expected) = expected_type {
            // Check user-defined functions.
            for item in &module.items {
                match &item.node {
                    crate::ast::item::ItemKind::FnDef(fn_def) => {
                        let ret_ty = fn_def
                            .return_type
                            .as_ref()
                            .map(|t| Self::resolve_type_expr_static(&t.node))
                            .unwrap_or(typechecker::Ty::Unit);
                        if ret_ty == *expected {
                            matching_functions.push(fn_def.name.clone());
                        }
                    }
                    crate::ast::item::ItemKind::ExternFn(decl) => {
                        let ret_ty = decl
                            .return_type
                            .as_ref()
                            .map(|t| Self::resolve_type_expr_static(&t.node))
                            .unwrap_or(typechecker::Ty::Unit);
                        if ret_ty == *expected {
                            matching_functions.push(decl.name.clone());
                        }
                    }
                    _ => {}
                }
            }

            // Check builtins by constructing a temporary env.
            let env = typechecker::env::TypeEnv::new();
            for (name, sig) in env.all_functions() {
                if sig.ret == *expected {
                    matching_functions.push(name.clone());
                }
            }

            matching_functions.sort();
            matching_functions.dedup();

            // Also check if any enum variants match the expected type.
            // (Already listed in available_variants, but useful for matching.)
        }

        // Sort bindings by name for deterministic output.
        bindings.sort_by(|a, b| a.name.cmp(&b.name));

        CompletionContext {
            expected_type: expected_type.map(|t| t.to_string()),
            bindings_in_scope: bindings,
            matching_functions,
            available_variants,
            available_builtins,
        }
    }

    /// Walk a block's statements to collect bindings visible before the cursor
    /// and determine expected type from context (let binding RHS, function call
    /// argument, return position).
    fn walk_block_for_completion(
        &self,
        block: &crate::ast::block::Block,
        line: u32,
        col: u32,
        bindings: &mut Vec<BindingInfo>,
        expected_type: &mut Option<typechecker::Ty>,
        fn_ret_ty: &Option<typechecker::Ty>,
    ) {
        for (i, stmt) in block.node.iter().enumerate() {
            let is_last = i == block.node.len() - 1;

            // If the cursor is before this statement, stop collecting.
            if stmt.span.start.line > line
                || (stmt.span.start.line == line && stmt.span.start.col > col)
            {
                break;
            }

            match &stmt.node {
                crate::ast::stmt::StmtKind::Let {
                    name,
                    type_ann,
                    value,
                    mutable,
                } => {
                    // If cursor is in the value expression of this let, the
                    // expected type is the annotation type.
                    if position_in_span(line, col, &value.span) {
                        if let Some(ann) = type_ann {
                            *expected_type = Some(Self::resolve_type_expr_static(&ann.node));
                        }
                    }

                    // If this statement is before the cursor, the binding is in scope.
                    if stmt.span.end.line < line
                        || (stmt.span.end.line == line && stmt.span.end.col <= col)
                    {
                        let ty_str = type_ann
                            .as_ref()
                            .map(|t| format_type_expr(&t.node))
                            .unwrap_or_else(|| infer_expr_type_static(value));
                        bindings.push(BindingInfo {
                            name: name.clone(),
                            ty: ty_str,
                            mutable: *mutable,
                        });
                    }
                }
                crate::ast::stmt::StmtKind::Ret(expr) => {
                    // If cursor is in the ret expression, expected type is the
                    // function return type.
                    if position_in_span(line, col, &expr.span) {
                        if let Some(ref rt) = fn_ret_ty {
                            *expected_type = Some(rt.clone());
                        }
                    }
                }
                crate::ast::stmt::StmtKind::Expr(expr) => {
                    // If this is the last expression (tail position), the
                    // expected type is the function return type.
                    if is_last && position_in_span(line, col, &expr.span) {
                        if let Some(ref rt) = fn_ret_ty {
                            *expected_type = Some(rt.clone());
                        }
                    }

                    // Walk into call expressions to determine expected argument types.
                    self.walk_expr_for_completion(expr, line, col, bindings, expected_type);
                }
                crate::ast::stmt::StmtKind::LetTupleDestructure {
                    names, value: _, ..
                } => {
                    // If this statement is before the cursor, add all bindings.
                    if stmt.span.end.line < line
                        || (stmt.span.end.line == line && stmt.span.end.col <= col)
                    {
                        for name in names {
                            bindings.push(BindingInfo {
                                name: name.clone(),
                                ty: "<inferred>".to_string(),
                                mutable: false,
                            });
                        }
                    }
                }
                crate::ast::stmt::StmtKind::Assign { value, .. } => {
                    self.walk_expr_for_completion(value, line, col, bindings, expected_type);
                }
            }
        }
    }

    /// Walk an expression looking for call argument positions where the cursor
    /// might be, and set the expected type to the parameter type.
    #[allow(clippy::only_used_in_recursion)]
    fn walk_expr_for_completion(
        &self,
        expr: &crate::ast::expr::Expr,
        line: u32,
        col: u32,
        bindings: &mut Vec<BindingInfo>,
        expected_type: &mut Option<typechecker::Ty>,
    ) {
        use crate::ast::expr::ExprKind;

        match &expr.node {
            ExprKind::Call { func, args } => {
                // If the cursor is inside one of the arguments, look up the
                // function signature to determine the expected parameter type.
                if let ExprKind::Ident(fn_name) = &func.node {
                    let module = self.module.as_ref();
                    let sig = self.lookup_fn_sig(fn_name, module);

                    if let Some(sig_params) = sig {
                        for (i, arg) in args.iter().enumerate() {
                            if position_in_span(line, col, &arg.span) && i < sig_params.len() {
                                *expected_type = Some(sig_params[i].clone());
                            }
                        }
                    }
                }

                // Recurse into arguments.
                for arg in args {
                    self.walk_expr_for_completion(arg, line, col, bindings, expected_type);
                }
            }
            ExprKind::If {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                for stmt in &then_block.node {
                    if let crate::ast::stmt::StmtKind::Expr(e) = &stmt.node {
                        self.walk_expr_for_completion(e, line, col, bindings, expected_type);
                    }
                }
                for (_, block) in else_ifs {
                    for stmt in &block.node {
                        if let crate::ast::stmt::StmtKind::Expr(e) = &stmt.node {
                            self.walk_expr_for_completion(e, line, col, bindings, expected_type);
                        }
                    }
                }
                if let Some(block) = else_block {
                    for stmt in &block.node {
                        if let crate::ast::stmt::StmtKind::Expr(e) = &stmt.node {
                            self.walk_expr_for_completion(e, line, col, bindings, expected_type);
                        }
                    }
                }
            }
            ExprKind::BinaryOp { left, right, .. } => {
                self.walk_expr_for_completion(left, line, col, bindings, expected_type);
                self.walk_expr_for_completion(right, line, col, bindings, expected_type);
            }
            ExprKind::UnaryOp { operand, .. } => {
                self.walk_expr_for_completion(operand, line, col, bindings, expected_type);
            }
            ExprKind::Paren(inner) => {
                self.walk_expr_for_completion(inner, line, col, bindings, expected_type);
            }
            _ => {}
        }
    }

    /// Look up a function's parameter types by name (user-defined or builtin).
    fn lookup_fn_sig(
        &self,
        fn_name: &str,
        module: Option<&Module>,
    ) -> Option<Vec<typechecker::Ty>> {
        // Check user-defined functions.
        if let Some(m) = module {
            for item in &m.items {
                match &item.node {
                    crate::ast::item::ItemKind::FnDef(fn_def) if fn_def.name == fn_name => {
                        return Some(
                            fn_def
                                .params
                                .iter()
                                .map(|p| Self::resolve_type_expr_static(&p.type_ann.node))
                                .collect(),
                        );
                    }
                    crate::ast::item::ItemKind::ExternFn(decl) if decl.name == fn_name => {
                        return Some(
                            decl.params
                                .iter()
                                .map(|p| Self::resolve_type_expr_static(&p.type_ann.node))
                                .collect(),
                        );
                    }
                    _ => {}
                }
            }
        }

        // Check builtins.
        let env = typechecker::env::TypeEnv::new();
        if let Some(sig) = env.lookup_fn(fn_name) {
            return Some(sig.params.iter().map(|(_, ty, _)| ty.clone()).collect());
        }

        None
    }

    /// Return the effect analysis summary for this module.
    ///
    /// Returns `None` if the source had parse errors (type checking was skipped).
    pub fn effect_summary(&self) -> Option<&ModuleEffectSummary> {
        self.effect_summary.as_ref()
    }

    /// Rename a symbol across the source code.
    ///
    /// Returns the transformed source with all references to `old_name`
    /// replaced with `new_name`. The rename is verified: the result is
    /// re-parsed and re-checked to ensure correctness.
    ///
    /// Returns `Err` if the rename would introduce errors.
    pub fn rename(&self, old_name: &str, new_name: &str) -> Result<RenameResult, String> {
        // Validate old_name: must be a non-empty identifier-shaped string.
        // Empty old_name would otherwise cause an infinite loop in the rename scanner
        // (every position matches an empty needle and the scanner would not advance).
        if old_name.is_empty() {
            return Err("`old_name` must not be empty".to_string());
        }
        if !old_name
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
            || !old_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(format!("`{}` is not a valid identifier", old_name));
        }

        // Validate the new name is a valid identifier.
        if new_name.is_empty()
            || !new_name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        {
            return Err(format!("`{}` is not a valid identifier", new_name));
        }

        // Find all occurrences of the old name in the source.
        // We walk by char_indices so non-ASCII identifier neighbors don't cause us
        // to slice in the middle of a UTF-8 codepoint, and we copy non-matching
        // characters verbatim to preserve unicode content.
        let mut new_source = String::new();
        let mut locations = Vec::new();
        let mut offset = 0usize;
        let old_len = old_name.len();

        for (line_num, line) in self.source.lines().enumerate() {
            let mut result_line = String::new();
            let bytes = line.as_bytes();
            let mut col = 0usize;

            while col < line.len() {
                // Cheap fast-path: only attempt a needle compare when remaining length
                // fits AND the slice would land on UTF-8 char boundaries (otherwise
                // string slicing panics on non-ASCII content adjacent to ASCII matches).
                if col + old_len <= line.len()
                    && line.is_char_boundary(col)
                    && line.is_char_boundary(col + old_len)
                    && &line[col..col + old_len] == old_name
                {
                    let end = col + old_len;
                    // Whole-word check using surrounding bytes. old_name is ASCII-only
                    // (validated above), so neighboring bytes that ARE ident chars are
                    // also ASCII; if a neighbor byte is non-ASCII (>= 0x80) it cannot
                    // be an identifier char by our rule, so the match is whole-word.
                    let before_ok = col == 0 || !is_ident_char(bytes[col - 1]);
                    let after_ok = end >= line.len() || !is_ident_char(bytes[end]);

                    if before_ok && after_ok {
                        locations.push(RenameLocation {
                            line: (line_num + 1) as u32,
                            col: (col + 1) as u32,
                            offset: (offset + col) as u32,
                        });
                        result_line.push_str(new_name);
                        col = end;
                        continue;
                    }
                }
                // Copy one character (not one byte) to keep UTF-8 intact.
                let ch = line[col..]
                    .chars()
                    .next()
                    .expect("col < line.len() implies a char boundary remains");
                result_line.push(ch);
                col += ch.len_utf8();
            }

            new_source.push_str(&result_line);
            new_source.push('\n');
            offset += line.len() + 1; // +1 for newline
        }

        if locations.is_empty() {
            return Err(format!("symbol `{}` not found in source", old_name));
        }

        // Verify the renamed source is still valid.
        let verify = Session::from_source(&new_source);
        let check = verify.check();

        Ok(RenameResult {
            new_source,
            locations_changed: locations.len(),
            locations,
            verification: if check.is_ok() {
                RenameVerification::Ok
            } else {
                RenameVerification::HasErrors {
                    error_count: check.error_count,
                    diagnostics: check.diagnostics,
                }
            },
        })
    }

    /// List all function calls made within a specific function.
    ///
    /// Returns the names of functions called, useful for dependency analysis.
    pub fn callees(&self, function_name: &str) -> Vec<String> {
        let module = match &self.module {
            Some(m) => m,
            None => return Vec::new(),
        };

        for item in &module.items {
            if let crate::ast::item::ItemKind::FnDef(fn_def) = &item.node {
                if fn_def.name == function_name {
                    let mut calls = Vec::new();
                    collect_calls_from_block(&fn_def.body, &mut calls);
                    calls.sort();
                    calls.dedup();
                    return calls;
                }
            }
        }

        Vec::new()
    }

    /// Build a dependency graph showing which functions call which.
    ///
    /// Returns a map from function name to the list of functions it calls.
    pub fn call_graph(&self) -> Vec<CallGraphEntry> {
        let module = match &self.module {
            Some(m) => m,
            None => return Vec::new(),
        };

        let mut entries = Vec::new();

        for item in &module.items {
            if let crate::ast::item::ItemKind::FnDef(fn_def) = &item.node {
                let mut calls = Vec::new();
                collect_calls_from_block(&fn_def.body, &mut calls);
                calls.sort();
                calls.dedup();

                entries.push(CallGraphEntry {
                    function: fn_def.name.clone(),
                    calls,
                });
            }
        }

        entries
    }

    /// Return the optimal context for editing a specific function within a
    /// token budget. Items are ranked by relevance and included greedily.
    ///
    /// Relevance priority:
    /// 1. The function's own signature and contracts (1.0)
    /// 2. Called functions' signatures and contracts (0.8)
    /// 3. Relevant type definitions (0.6)
    /// 4. Module capability ceiling (0.4)
    /// 5. Available builtin functions used by the function (0.3)
    pub fn context_budget(&self, function_name: &str, budget: usize) -> ContextBudget {
        let mut items: Vec<ContextItem> = Vec::new();

        let symbols = self.symbols();

        // 1. The target function's own signature (relevance 1.0) and contracts (0.95)
        if let Some(sym) = symbols.iter().find(|s| s.name == function_name) {
            items.push(ContextItem {
                kind: "function_signature".to_string(),
                name: sym.name.clone(),
                content: sym.ty.clone(),
                token_estimate: estimate_tokens(&sym.ty),
                relevance: 1.0,
            });

            for contract in &sym.contracts {
                let content = format!("@{}({})", contract.kind, contract.condition);
                items.push(ContextItem {
                    kind: "contract".to_string(),
                    name: sym.name.clone(),
                    content: content.clone(),
                    token_estimate: estimate_tokens(&content),
                    relevance: 0.95,
                });
            }
        }

        // 2. Called functions' signatures and contracts (relevance 0.8)
        let callees = self.callees(function_name);
        let env = crate::typechecker::env::TypeEnv::new();
        let builtin_names: std::collections::HashSet<String> =
            env.all_functions().keys().cloned().collect();

        for callee_name in &callees {
            // Skip the target function itself (recursive calls)
            if callee_name == function_name {
                continue;
            }
            // Check if it's a user-defined function
            if let Some(sym) = symbols.iter().find(|s| s.name == *callee_name) {
                items.push(ContextItem {
                    kind: "function_signature".to_string(),
                    name: sym.name.clone(),
                    content: sym.ty.clone(),
                    token_estimate: estimate_tokens(&sym.ty),
                    relevance: 0.8,
                });

                for contract in &sym.contracts {
                    let content = format!("@{}({})", contract.kind, contract.condition);
                    items.push(ContextItem {
                        kind: "contract".to_string(),
                        name: sym.name.clone(),
                        content: content.clone(),
                        token_estimate: estimate_tokens(&content),
                        relevance: 0.75,
                    });
                }
            }
        }

        // 3. Relevant type definitions (relevance 0.6)
        //    Include enum/type declarations that appear in the function's
        //    parameters, return type, or contracts.
        if let Some(module) = &self.module {
            for item in &module.items {
                match &item.node {
                    crate::ast::item::ItemKind::EnumDecl {
                        name,
                        type_params,
                        variants,
                        ..
                    } => {
                        // Check if this type is referenced by the target function
                        if let Some(sym) = symbols.iter().find(|s| s.name == function_name) {
                            if sym.ty.contains(name.as_str())
                                || sym.params.iter().any(|p| p.ty.contains(name.as_str()))
                                || sym
                                    .contracts
                                    .iter()
                                    .any(|c| c.condition.contains(name.as_str()))
                                || callees.iter().any(|c| {
                                    symbols
                                        .iter()
                                        .any(|s| s.name == *c && s.ty.contains(name.as_str()))
                                })
                            {
                                let tp_str = if type_params.is_empty() {
                                    String::new()
                                } else {
                                    format!("[{}]", type_params.join(", "))
                                };
                                let variant_strs: Vec<String> = variants
                                    .iter()
                                    .map(|v| {
                                        if let Some(ref fields) = v.fields {
                                            if fields.is_empty() {
                                                v.name.clone()
                                            } else {
                                                let field_strs: Vec<String> = fields
                                                    .iter()
                                                    .map(|f| match f {
                                                        VariantField::Named { name, type_expr } => {
                                                            format!(
                                                                "{}: {}",
                                                                name,
                                                                format_type_expr(&type_expr.node)
                                                            )
                                                        }
                                                        VariantField::Anonymous(type_expr) => {
                                                            format_type_expr(&type_expr.node)
                                                        }
                                                    })
                                                    .collect();
                                                format!("{}({})", v.name, field_strs.join(", "))
                                            }
                                        } else {
                                            v.name.clone()
                                        }
                                    })
                                    .collect();
                                let content = format!(
                                    "type {}{} = {}",
                                    name,
                                    tp_str,
                                    variant_strs.join(" | ")
                                );
                                items.push(ContextItem {
                                    kind: "type_def".to_string(),
                                    name: name.clone(),
                                    content: content.clone(),
                                    token_estimate: estimate_tokens(&content),
                                    relevance: 0.6,
                                });
                            }
                        }
                    }
                    crate::ast::item::ItemKind::TypeDecl {
                        name,
                        type_expr,
                        repr,
                        ..
                    } => {
                        if let Some(sym) = symbols.iter().find(|s| s.name == function_name) {
                            if sym.ty.contains(name.as_str())
                                || sym.params.iter().any(|p| p.ty.contains(name.as_str()))
                            {
                                let content = format_type_decl(name, &type_expr.node, *repr);
                                items.push(ContextItem {
                                    kind: "type_def".to_string(),
                                    name: name.clone(),
                                    content: content.clone(),
                                    token_estimate: estimate_tokens(&content),
                                    relevance: 0.6,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // 4. Module capability ceiling (relevance 0.4)
        if let Some(ref summary) = self.effect_summary {
            if let Some(ref ceiling) = summary.capability_ceiling {
                let content = format!("@cap({})", ceiling.join(", "));
                items.push(ContextItem {
                    kind: "capability".to_string(),
                    name: "module".to_string(),
                    content: content.clone(),
                    token_estimate: estimate_tokens(&content),
                    relevance: 0.4,
                });
            }
        }

        // 5. Builtin functions used by the target function (relevance 0.3)
        for callee_name in &callees {
            if builtin_names.contains(callee_name) {
                if let Some(sig) = env.lookup_fn(callee_name) {
                    let params_str = sig
                        .params
                        .iter()
                        .map(|(n, t, _)| format!("{}: {}", n, t))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let effects_str = if sig.effects.is_empty() {
                        String::new()
                    } else {
                        format!(" -> !{{{}}}", sig.effects.join(", "))
                    };
                    let ret_str = format!("{}", sig.ret);
                    let content = format!(
                        "fn {}({}){}{}",
                        callee_name,
                        params_str,
                        effects_str,
                        if ret_str == "()" && effects_str.is_empty() {
                            String::new()
                        } else if effects_str.is_empty() {
                            format!(" -> {}", ret_str)
                        } else {
                            format!(" {}", ret_str)
                        }
                    );
                    items.push(ContextItem {
                        kind: "builtin".to_string(),
                        name: callee_name.clone(),
                        content: content.clone(),
                        token_estimate: estimate_tokens(&content),
                        relevance: 0.3,
                    });
                }
            }
        }

        // Sort by relevance (highest first), then by token cost (smallest first)
        items.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.token_estimate.cmp(&b.token_estimate))
        });

        // Greedily include items until budget exhausted
        let mut used_tokens = 0;
        let mut included = Vec::new();
        for item in items {
            if used_tokens + item.token_estimate <= budget {
                used_tokens += item.token_estimate;
                included.push(item);
            }
        }

        ContextBudget {
            target_function: function_name.to_string(),
            budget_tokens: budget,
            used_tokens,
            items: included,
        }
    }

    /// Generate a structural index of the project.
    ///
    /// This is like Aider's RepoMap: a compact overview of all modules,
    /// functions, types, and their relationships for AI agents.
    pub fn project_index(&self) -> ProjectIndex {
        let symbols = self.symbols();
        let call_graph = self.call_graph();

        let module_name = self
            .module
            .as_ref()
            .and_then(|m| m.module_decl.as_ref())
            .map(|d| d.path.join("."))
            .unwrap_or_else(|| "main".to_string());

        let mut functions = Vec::new();
        let mut types = Vec::new();

        for sym in &symbols {
            match sym.kind {
                SymbolKind::Function | SymbolKind::ExternFunction => {
                    let contract_strs: Vec<String> = sym
                        .contracts
                        .iter()
                        .map(|c| format!("@{}({})", c.kind, c.condition))
                        .collect();
                    functions.push(FunctionIndex {
                        name: sym.name.clone(),
                        signature: sym.ty.clone(),
                        effects: sym.effects.clone(),
                        is_pure: sym.is_pure,
                        contracts: contract_strs,
                    });
                }
                SymbolKind::TypeAlias => {
                    // Determine if this is an enum or alias based on the type string
                    let kind = if sym.ty.contains('|') {
                        "enum"
                    } else {
                        "alias"
                    };
                    types.push(TypeIndex {
                        name: sym.name.clone(),
                        kind: kind.to_string(),
                        definition: sym.ty.clone(),
                    });
                }
                SymbolKind::Variable => {
                    // Variables are not included in the index
                }
                SymbolKind::Actor => {
                    // Actors are not yet included in the index
                }
                SymbolKind::Trait => {
                    types.push(TypeIndex {
                        name: sym.name.clone(),
                        kind: "trait".to_string(),
                        definition: sym.ty.clone(),
                    });
                }
                SymbolKind::Impl => {
                    // Impl blocks are not included as standalone index entries
                }
            }
        }

        let capability_ceiling = self
            .effect_summary
            .as_ref()
            .and_then(|s| s.capability_ceiling.clone());

        let panic_strategy = self
            .module
            .as_ref()
            .map(|m| m.panic_strategy.as_str().to_string())
            .unwrap_or_else(|| {
                crate::ast::module::PanicStrategy::default()
                    .as_str()
                    .to_string()
            });

        // Allocator strategy (#336): attribute-driven, like panic_strategy
        // above. Reads `Module.allocator_strategy` directly. Default is
        // `default` (system malloc). Distinct from the effect-driven
        // trio (alloc_strategy/actor_strategy/async_strategy below) —
        // the embedder's deployment target isn't derivable from effects.
        let allocator_strategy = self
            .module
            .as_ref()
            .map(|m| m.allocator_strategy.as_str().to_string())
            .unwrap_or_else(|| {
                crate::ast::module::AllocatorStrategy::default()
                    .as_str()
                    .to_string()
            });

        // Alloc strategy (#333): "full" if any symbol declares `Heap`,
        // otherwise "minimal". Derived from the module-contract effects
        // surface so it stays in sync with the same data the JSON
        // contract publishes.
        //
        // The detection is intentionally automatic, not driven by a user
        // attribute: ADR 0005 commits to effect-driven DCE for the
        // runtime closure. See codebase/compiler/runtime/alloc/README.md
        // for the link-time dispatch.
        let alloc_strategy = {
            let symbols = self.symbols();
            let heap_used = symbols
                .iter()
                .any(|s| s.effects.iter().any(|e| e == "Heap"));
            if heap_used {
                "full".to_string()
            } else {
                "minimal".to_string()
            }
        };

        // Actor strategy (#334): "full" if any symbol declares `Actor`,
        // otherwise "none". Sibling of `alloc_strategy` above — same
        // effect-surface scan, different trigger effect. ADR 0005 commits
        // the runtime closure to effect-driven DCE: programs that never
        // spawn an actor shouldn't pay for the scheduler.
        let actor_strategy = {
            let symbols = self.symbols();
            let actor_used = symbols
                .iter()
                .any(|s| s.effects.iter().any(|e| e == "Actor"));
            if actor_used {
                "full".to_string()
            } else {
                "none".to_string()
            }
        };

        // Async strategy (#335): "full" if any symbol declares `Async`,
        // otherwise "none". Sibling of `actor_strategy` above — same
        // effect-surface scan, different trigger effect. ADR 0005 commits
        // the runtime closure to effect-driven DCE: programs that never
        // await shouldn't pay for the async executor.
        let async_strategy = {
            let symbols = self.symbols();
            let async_used = symbols
                .iter()
                .any(|s| s.effects.iter().any(|e| e == "Async"));
            if async_used {
                "full".to_string()
            } else {
                "none".to_string()
            }
        };

        // Module mode (#352, Epic #301): attribute-driven, like
        // panic_strategy and allocator_strategy. Reads `Module.mode`
        // directly. Default is `app` (inference everywhere). `system`
        // requires every fn to declare an explicit return type + effect
        // set (rejection enforced by the typechecker).
        let module_mode = self
            .module
            .as_ref()
            .map(|m| m.mode.as_str().to_string())
            .unwrap_or_else(|| {
                crate::ast::module::ModuleMode::default()
                    .as_str()
                    .to_string()
            });

        let module_index = ModuleIndex {
            name: module_name,
            functions,
            types,
            capability_ceiling,
            panic_strategy,
            alloc_strategy,
            actor_strategy,
            async_strategy,
            allocator_strategy,
            module_mode,
        };

        ProjectIndex {
            modules: vec![module_index],
            call_graph,
        }
    }

    /// Return the original source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Return the parsed AST module, if available.
    pub fn module(&self) -> Option<&Module> {
        self.module.as_ref()
    }

    /// Whether this session completed type checking (vs stopping at parse).
    pub fn is_type_checked(&self) -> bool {
        self.type_checked
    }

    /// Return the parse errors.
    pub fn parse_errors(&self) -> &[ParseError] {
        &self.parse_errors
    }

    /// Return the type errors.
    pub fn type_errors(&self) -> &[TypeError] {
        &self.type_errors
    }
}

impl CheckResult {
    /// Convenience: is the source error-free?
    pub fn is_ok(&self) -> bool {
        self.ok
    }

    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }
}

impl RenameResult {
    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }
}

impl ModuleContract {
    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }
}

impl CompletionContext {
    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }
}

impl ContextBudget {
    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }
}

impl ProjectIndex {
    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }
}

impl ModuleDocumentation {
    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e))
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Estimate the number of tokens for a string.
/// Heuristic: approximately 4 characters per token for code.
fn estimate_tokens(text: &str) -> usize {
    let len = text.len();
    if len == 0 {
        1
    } else {
        len.div_ceil(4)
    }
}

/// Check if a (line, col) position falls within a span.
fn position_in_span(line: u32, col: u32, span: &Span) -> bool {
    if line < span.start.line || line > span.end.line {
        return false;
    }
    if line == span.start.line && col < span.start.col {
        return false;
    }
    if line == span.end.line && col > span.end.col {
        return false;
    }
    true
}

/// Check if a byte is a valid identifier character.
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Collect all function call names from a block (recursively).
fn collect_calls_from_block(block: &crate::ast::block::Block, calls: &mut Vec<String>) {
    for stmt in &block.node {
        collect_calls_from_stmt(stmt, calls);
    }
}

/// Collect call names from a statement.
fn collect_calls_from_stmt(stmt: &crate::ast::stmt::Stmt, calls: &mut Vec<String>) {
    match &stmt.node {
        crate::ast::stmt::StmtKind::Expr(expr) => collect_calls_from_expr(expr, calls),
        crate::ast::stmt::StmtKind::Let { value, .. } => collect_calls_from_expr(value, calls),
        crate::ast::stmt::StmtKind::LetTupleDestructure { value, .. } => {
            collect_calls_from_expr(value, calls)
        }
        crate::ast::stmt::StmtKind::Assign { value, .. } => collect_calls_from_expr(value, calls),
        crate::ast::stmt::StmtKind::Ret(expr) => collect_calls_from_expr(expr, calls),
    }
}

/// Collect call names from an expression (recursive).
fn collect_calls_from_expr(expr: &crate::ast::expr::Expr, calls: &mut Vec<String>) {
    match &expr.node {
        crate::ast::expr::ExprKind::Call { func, args } => {
            if let crate::ast::expr::ExprKind::Ident(name) = &func.node {
                calls.push(name.clone());
            }
            collect_calls_from_expr(func, calls);
            for arg in args {
                collect_calls_from_expr(arg, calls);
            }
        }
        crate::ast::expr::ExprKind::BinaryOp { left, right, .. } => {
            collect_calls_from_expr(left, calls);
            collect_calls_from_expr(right, calls);
        }
        crate::ast::expr::ExprKind::UnaryOp { operand, .. } => {
            collect_calls_from_expr(operand, calls);
        }
        crate::ast::expr::ExprKind::If {
            condition,
            then_block,
            else_ifs,
            else_block,
        } => {
            collect_calls_from_expr(condition, calls);
            collect_calls_from_block(then_block, calls);
            for (cond, block) in else_ifs {
                collect_calls_from_expr(cond, calls);
                collect_calls_from_block(block, calls);
            }
            if let Some(block) = else_block {
                collect_calls_from_block(block, calls);
            }
        }
        crate::ast::expr::ExprKind::For { iter, body, .. } => {
            collect_calls_from_expr(iter, calls);
            collect_calls_from_block(body, calls);
        }
        crate::ast::expr::ExprKind::While { condition, body } => {
            collect_calls_from_expr(condition, calls);
            collect_calls_from_block(body, calls);
        }
        crate::ast::expr::ExprKind::Match { scrutinee, arms } => {
            collect_calls_from_expr(scrutinee, calls);
            for arm in arms {
                collect_calls_from_block(&arm.body, calls);
            }
        }
        crate::ast::expr::ExprKind::Paren(inner) => {
            collect_calls_from_expr(inner, calls);
        }
        crate::ast::expr::ExprKind::FieldAccess { object, .. } => {
            collect_calls_from_expr(object, calls);
        }
        _ => {}
    }
}

/// Format a TypeExpr to a human-readable string.
/// Infer a simple type string from an expression without running the full type
/// checker. Used for completion context when no type annotation is available.
fn infer_expr_type_static(expr: &crate::ast::expr::Expr) -> String {
    use crate::ast::expr::ExprKind;
    match &expr.node {
        ExprKind::IntLit(_) => "Int".to_string(),
        ExprKind::FloatLit(_) => "Float".to_string(),
        ExprKind::StringLit(_) => "String".to_string(),
        ExprKind::BoolLit(_) => "Bool".to_string(),
        ExprKind::UnitLit => "()".to_string(),
        _ => "<inferred>".to_string(),
    }
}

fn format_type_decl(
    name: &str,
    type_expr: &crate::ast::types::TypeExpr,
    repr: Option<Repr>,
) -> String {
    let decl = format!("type {} = {}", name, format_type_expr(type_expr));
    match repr {
        Some(Repr::C) => format!("@repr(C)\n{}", decl),
        None => decl,
    }
}

fn format_type_expr(te: &crate::ast::types::TypeExpr) -> String {
    match te {
        crate::ast::types::TypeExpr::Named { name, cap: _ } => name.clone(),
        crate::ast::types::TypeExpr::Unit => "()".to_string(),
        crate::ast::types::TypeExpr::Fn {
            params,
            ret,
            effects,
        } => {
            let params_str = params
                .iter()
                .map(|p| format_type_expr(&p.node))
                .collect::<Vec<_>>()
                .join(", ");
            let eff_str = match effects {
                Some(eff) if !eff.effects.is_empty() => {
                    format!(" !{{{}}}", eff.effects.join(", "))
                }
                _ => String::new(),
            };
            format!(
                "({}) ->{} {}",
                params_str,
                eff_str,
                format_type_expr(&ret.node)
            )
        }
        crate::ast::types::TypeExpr::Generic { name, args, cap: _ } => {
            let args_str = args
                .iter()
                .map(|a| format_type_expr(&a.node))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}[{}]", name, args_str)
        }
        crate::ast::types::TypeExpr::Tuple(elems) => {
            let elem_strs = elems
                .iter()
                .map(|e| format_type_expr(&e.node))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({})", elem_strs)
        }
        crate::ast::types::TypeExpr::Record(fields) => {
            let field_strs = fields
                .iter()
                .map(|(n, ty)| format!("{}: {}", n, format_type_expr(&ty.node)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {} }}", field_strs)
        }
        crate::ast::types::TypeExpr::Linear(inner) => {
            format!("@linear {}", format_type_expr(&inner.node))
        }
        crate::ast::types::TypeExpr::Type => "type".to_string(),
    }
}

/// Format an expression to a human-readable string (for contract display).
fn format_expr(expr: &crate::ast::expr::Expr) -> String {
    use crate::ast::expr::{BinOp, ExprKind, UnaryOp};
    match &expr.node {
        ExprKind::IntLit(n) => n.to_string(),
        ExprKind::FloatLit(f) => f.to_string(),
        ExprKind::StringLit(s) => format!("\"{}\"", s),
        ExprKind::BoolLit(b) => b.to_string(),
        ExprKind::UnitLit => "()".to_string(),
        ExprKind::Ident(name) => name.clone(),
        ExprKind::TypedHole(label) => label
            .as_ref()
            .map_or("?".to_string(), |l| format!("?{}", l)),
        ExprKind::BinaryOp { op, left, right } => {
            let op_str = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Mod => "%",
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::And => "and",
                BinOp::Or => "or",
                BinOp::Pipe => "|>",
            };
            format!("{} {} {}", format_expr(left), op_str, format_expr(right))
        }
        ExprKind::UnaryOp { op, operand } => {
            let op_str = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "not ",
            };
            format!("{}{}", op_str, format_expr(operand))
        }
        ExprKind::Call { func, args } => {
            let args_str = args.iter().map(format_expr).collect::<Vec<_>>().join(", ");
            format!("{}({})", format_expr(func), args_str)
        }
        ExprKind::FieldAccess { object, field } => {
            format!("{}.{}", format_expr(object), field)
        }
        ExprKind::Paren(inner) => format!("({})", format_expr(inner)),
        ExprKind::Spawn { actor_name } => format!("spawn {}", actor_name),
        ExprKind::Send { target, message } => format!("send {} {}", format_expr(target), message),
        ExprKind::Ask { target, message } => format!("ask {} {}", format_expr(target), message),
        _ => "<expr>".to_string(),
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_valid_program() {
        let source = r#"fn main() -> !{IO} ():
    print("hello")
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(result.is_ok());
        assert_eq!(result.error_count, 0);
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn check_type_error() {
        let source = r#"fn main():
    let x: Int = "not an int"
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(!result.is_ok());
        assert!(result.error_count > 0);
        assert_eq!(result.diagnostics[0].phase, Phase::Typechecker);
    }

    #[test]
    fn check_parse_error() {
        let source = "fn (\n";
        let session = Session::from_source(source);
        let result = session.check();
        assert!(!result.is_ok());
        assert!(result.diagnostics.iter().any(|d| d.phase == Phase::Parser));
    }

    #[test]
    fn check_result_json_serialization() {
        let source = r#"fn add(a: Int, b: Int) -> Int:
    a + b
"#;
        let session = Session::from_source(source);
        let result = session.check();
        let json = result.to_json();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"error_count\":0"));
    }

    #[test]
    fn symbols_function() {
        let source = r#"fn factorial(n: Int) -> Int:
    if n <= 1:
        1
    else:
        n * factorial(n - 1)
"#;
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "factorial");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[0].params.len(), 1);
        assert_eq!(symbols[0].params[0].name, "n");
        assert_eq!(symbols[0].params[0].ty, "Int");
    }

    #[test]
    fn symbols_with_effects() {
        let source = r#"fn greet(name: String) -> !{IO} ():
    print(name)
"#;
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].effects, vec!["IO".to_string()]);
    }

    #[test]
    fn symbols_type_alias() {
        let source = "type Count = Int\n";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Count");
        assert_eq!(symbols[0].kind, SymbolKind::TypeAlias);
        assert!(symbols[0].ty.contains("Count"));
        assert!(symbols[0].ty.contains("Int"));
    }

    #[test]
    fn module_contract_json() {
        let source = r#"fn add(a: Int, b: Int) -> Int:
    a + b

fn greet(name: String) -> !{IO} ():
    print(name)
"#;
        let session = Session::from_source(source);
        let contract = session.module_contract();
        assert_eq!(contract.symbols.len(), 2);
        assert_eq!(contract.effects_used, vec!["IO".to_string()]);
        assert!(!contract.has_errors);

        let json = contract.to_json();
        assert!(json.contains("\"name\":\"add\""));
        assert!(json.contains("\"name\":\"greet\""));
        assert!(json.contains("\"effects_used\":[\"IO\"]"));
    }

    #[test]
    fn module_contract_compact() {
        // Verify the contract is compact enough for agent context windows.
        let source = r#"fn compute(x: Int, y: Int) -> Int:
    x + y

fn display(msg: String) -> !{IO} ():
    print(msg)

type Score = Int
"#;
        let session = Session::from_source(source);
        let contract = session.module_contract();
        let json = contract.to_json();

        // A 3-symbol module contract should be well under 500 characters.
        assert!(
            json.len() < 1000,
            "contract JSON should be compact, got {} bytes",
            json.len()
        );
    }

    #[test]
    fn check_result_with_notes() {
        // #350: under @app mode (default), effects are inferred for local fns.
        // Use @system to force explicit-annotation enforcement so the error fires.
        let source = r#"@system
fn helper():
    print("hello")
"#;
        let session = Session::from_source(source);
        let result = session.check();
        // @system produces multiple diagnostics: missing return type, missing
        // effect set, and the specific "requires effect IO" from the body.
        // At least one diagnostic must have a note mentioning IO or effects.
        assert!(!result.is_ok());
        assert!(
            result.diagnostics.iter().any(|d| {
                !d.notes.is_empty()
                    && d.notes
                        .iter()
                        .any(|n| n.contains("IO") || n.contains("effect"))
            }),
            "expected at least one diagnostic note about IO/effects, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiple_errors_collected() {
        let source = r#"fn main():
    let x: Int = "string"
    let y: Bool = 42
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(!result.is_ok());
        assert!(result.error_count >= 2);
    }

    #[test]
    fn empty_source() {
        let session = Session::from_source("");
        let result = session.check();
        assert!(result.is_ok());
        assert_eq!(session.symbols().len(), 0);
    }

    #[test]
    fn pretty_json_output() {
        let source = r#"fn main() -> !{IO} ():
    print("test")
"#;
        let session = Session::from_source(source);
        let result = session.check();
        let pretty = result.to_json_pretty();
        assert!(pretty.contains('\n'));
        assert!(pretty.contains("  "));
    }

    // ── Effect system tests ──────────────────────────────────────────

    #[test]
    fn pure_function_detected() {
        let source = r#"fn add(a: Int, b: Int) -> Int:
    a + b
"#;
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols[0].name, "add");
        assert!(symbols[0].is_pure);
        assert!(symbols[0].inferred_effects.is_empty());
    }

    #[test]
    fn effectful_function_detected() {
        let source = r#"fn greet(name: String) -> !{IO} ():
    print(name)
"#;
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols[0].name, "greet");
        assert!(!symbols[0].is_pure);
        assert_eq!(symbols[0].inferred_effects, vec!["IO".to_string()]);
    }

    #[test]
    fn effect_inference_through_calls() {
        // main calls print → inferred effects should include IO
        let source = r#"fn main() -> !{IO} ():
    print("hello")
"#;
        let session = Session::from_source(source);
        let summary = session.effect_summary().unwrap();
        let main_info = summary
            .functions
            .iter()
            .find(|f| f.function == "main")
            .unwrap();
        assert_eq!(main_info.inferred, vec!["IO".to_string()]);
        assert!(!main_info.is_pure);
    }

    #[test]
    fn pure_function_in_module_contract() {
        let source = r#"fn compute(x: Int) -> Int:
    x * 2

fn display(msg: String) -> !{IO} ():
    print(msg)
"#;
        let session = Session::from_source(source);
        let contract = session.module_contract();
        assert_eq!(contract.pure_count, 1);
        assert_eq!(contract.effectful_count, 1);

        let json = contract.to_json();
        assert!(json.contains("\"pure_count\":1"));
        assert!(json.contains("\"effectful_count\":1"));
    }

    #[test]
    fn effect_summary_available() {
        let source = r#"fn pure_fn(x: Int) -> Int:
    x + 1

fn io_fn() -> !{IO} ():
    print("hi")
"#;
        let session = Session::from_source(source);
        let summary = session.effect_summary().unwrap();
        assert_eq!(summary.pure_count, 1);
        assert_eq!(summary.effectful_count, 1);
        assert_eq!(summary.effects_used, vec!["IO".to_string()]);
    }

    #[test]
    fn unknown_effect_rejected() {
        let source = r#"fn bad() -> !{Foo} ():
    ()
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(!result.is_ok());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("unknown effect")),
            "should report unknown effect, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn multiple_known_effects_accepted() {
        // IO and Mut are both known effects — no error about unknown effects
        let source = r#"fn handler() -> !{IO, Mut} ():
    print("mutating")
"#;
        let session = Session::from_source(source);
        let result = session.check();
        // The only error should be about Mut not being used, not about unknown effects
        let unknown_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("unknown effect"))
            .collect();
        assert!(unknown_errors.is_empty());
    }

    #[test]
    fn query_api_reports_stack_static_marker_effects() {
        let source = r#"fn frame_probe(n: Int) -> !{Stack, Static} Int:
    n + 1
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(result.is_ok(), "diagnostics: {:?}", result.diagnostics);

        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "frame_probe");
        assert_eq!(
            symbols[0].effects,
            vec!["Stack".to_string(), "Static".to_string()]
        );
    }

    #[test]
    fn query_api_reports_concurrency_marker_effects() {
        let source = r#"fn hop(addr: Int) -> !{Async, Send, Atomic} Int:
    addr
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(result.is_ok(), "diagnostics: {:?}", result.diagnostics);

        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "hop");
        assert_eq!(
            symbols[0].effects,
            vec![
                "Async".to_string(),
                "Send".to_string(),
                "Atomic".to_string()
            ]
        );
    }

    #[test]
    fn query_api_reports_volatile_marker_effect() {
        let source = r#"fn read_register(addr: Int) -> !{Volatile} Int:
    addr
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(result.is_ok(), "diagnostics: {:?}", result.diagnostics);

        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "read_register");
        assert_eq!(symbols[0].effects, vec!["Volatile".to_string()]);
    }

    // ── Capability constraint tests ──────────────────────────────────

    #[test]
    fn cap_declaration_allows_compliant_functions() {
        let source = r#"@cap(IO)

fn greet(name: String) -> !{IO} ():
    print(name)
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(
            result.is_ok(),
            "should compile: function uses only IO, cap allows IO"
        );
    }

    #[test]
    fn cap_declaration_rejects_exceeding_function() {
        let source = r#"@cap(IO)

fn sneaky() -> !{IO, Net} ():
    print("trying to use Net")
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(!result.is_ok());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("exceeds the module capability ceiling")),
            "should reject Net because @cap only allows IO, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn cap_declaration_in_module_contract() {
        let source = r#"@cap(IO)

fn hello() -> !{IO} ():
    print("hi")

fn compute(x: Int) -> Int:
    x + 1
"#;
        let session = Session::from_source(source);
        let contract = session.module_contract();
        assert_eq!(contract.capability_ceiling, Some(vec!["IO".to_string()]));

        let json = contract.to_json();
        assert!(json.contains("\"capability_ceiling\":[\"IO\"]"));
    }

    #[test]
    fn cap_pure_module() {
        // @cap() with no effects = module must be entirely pure
        let source = r#"@cap()

fn add(a: Int, b: Int) -> Int:
    a + b
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(
            result.is_ok(),
            "pure module with pure functions should compile"
        );
    }

    #[test]
    fn cap_pure_module_rejects_io() {
        let source = r#"@cap()

fn bad() -> !{IO} ():
    print("not allowed")
"#;
        let session = Session::from_source(source);
        let result = session.check();
        assert!(!result.is_ok());
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("exceeds the module capability ceiling")),);
    }

    #[test]
    fn cap_in_effects_output() {
        let source = r#"@cap(IO, FS)

fn log(msg: String) -> !{IO} ():
    print(msg)
"#;
        let session = Session::from_source(source);
        let summary = session.effect_summary().unwrap();
        assert_eq!(
            summary.capability_ceiling,
            Some(vec!["IO".to_string(), "FS".to_string()])
        );
    }

    // ── Rename tests ─────────────────────────────────────────────────

    #[test]
    fn rename_function() {
        let source = r#"fn add(a: Int, b: Int) -> Int:
    a + b

fn main() -> !{IO} ():
    let result = add(1, 2)
    print_int(result)
"#;
        let session = Session::from_source(source);
        let result = session.rename("add", "sum").unwrap();
        assert!(result.locations_changed >= 2); // definition + call
        assert!(result.new_source.contains("fn sum("));
        assert!(result.new_source.contains("sum(1, 2)"));
        assert!(!result.new_source.contains("add"));
        assert!(matches!(result.verification, RenameVerification::Ok));
    }

    #[test]
    fn rename_not_found() {
        let source = "fn main():\n    ()\n";
        let session = Session::from_source(source);
        let result = session.rename("nonexistent", "new_name");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn rename_preserves_substrings() {
        // "add" should not rename "address" or "adding"
        let source = r#"fn add(a: Int, b: Int) -> Int:
    a + b

fn main():
    let adding = add(1, 2)
"#;
        let session = Session::from_source(source);
        let result = session.rename("add", "sum").unwrap();
        assert!(result.new_source.contains("adding")); // preserved
        assert!(result.new_source.contains("fn sum(")); // renamed
        assert!(result.new_source.contains("sum(1, 2)")); // renamed
    }

    #[test]
    fn rename_result_json() {
        let source = "fn foo() -> Int:\n    42\n";
        let session = Session::from_source(source);
        let result = session.rename("foo", "bar").unwrap();
        let json = result.to_json();
        assert!(json.contains("\"locations_changed\":"));
        assert!(json.contains("\"new_source\""));
    }

    #[test]
    fn rename_rejects_empty_old_name() {
        // Regression: empty old_name previously caused an infinite loop because
        // `line[col..].starts_with("")` is always true and `col` did not advance.
        let source = "fn foo() -> Int:\n    42\n";
        let session = Session::from_source(source);
        let result = session.rename("", "bar");
        assert!(result.is_err(), "empty old_name must be rejected");
        let msg = result.unwrap_err();
        assert!(msg.contains("must not be empty") || msg.contains("not a valid identifier"));
    }

    #[test]
    fn rename_rejects_non_identifier_old_name() {
        let source = "fn foo() -> Int:\n    42\n";
        let session = Session::from_source(source);
        // Whitespace, punctuation, or starting with a digit are not valid identifiers.
        for bad in [" ", "\t", "1foo", "foo bar", "foo.bar", "-x"] {
            let result = session.rename(bad, "x");
            assert!(result.is_err(), "old_name {:?} should be rejected", bad);
        }
    }

    #[test]
    fn rename_preserves_unicode_neighbors() {
        // Source contains a non-ASCII string literal; renaming an ASCII identifier
        // must not corrupt the multi-byte characters in the rest of the source.
        let source = "fn foo() -> Int:\n    let msg = \"héllo Ω 漢字\"\n    42\n";
        let session = Session::from_source(source);
        let result = session.rename("foo", "bar").unwrap();
        assert!(
            result.new_source.contains("héllo Ω 漢字"),
            "non-ASCII content must be preserved verbatim, got: {:?}",
            result.new_source
        );
        assert!(result.new_source.contains("fn bar()"));
        assert_eq!(result.locations_changed, 1);
    }

    #[test]
    fn rename_handles_unicode_identifier_neighbor() {
        // Identifier `foo` adjacent to non-ASCII content in a string. The whole-word
        // check must not panic on the byte just past the match when that byte is
        // the first byte of a multi-byte codepoint.
        let source = "fn main():\n    let s = \"foo漢\"\n";
        let session = Session::from_source(source);
        // Should not panic; either no match (because `foo` is bordered by an ident
        // char or treated as part of a larger token by the whole-word rule) or a
        // safe rename. Either way, no panic and original unicode preserved.
        let _ = session.rename("foo", "bar");
    }

    // ── Call graph tests ─────────────────────────────────────────────

    #[test]
    fn call_graph_basic() {
        let source = r#"fn helper(x: Int) -> Int:
    x + 1

fn main() -> !{IO} ():
    let y = helper(5)
    print_int(y)
"#;
        let session = Session::from_source(source);
        let graph = session.call_graph();
        assert_eq!(graph.len(), 2);

        let main_entry = graph.iter().find(|e| e.function == "main").unwrap();
        assert!(main_entry.calls.contains(&"helper".to_string()));
        assert!(main_entry.calls.contains(&"print_int".to_string()));

        let helper_entry = graph.iter().find(|e| e.function == "helper").unwrap();
        assert!(helper_entry.calls.is_empty());
    }

    #[test]
    fn callees_for_function() {
        let source = r#"fn compute(x: Int) -> Int:
    abs(x) + min(x, 10)

fn main() -> !{IO} ():
    print_int(compute(5))
"#;
        let session = Session::from_source(source);
        let calls = session.callees("compute");
        assert!(calls.contains(&"abs".to_string()));
        assert!(calls.contains(&"min".to_string()));

        let main_calls = session.callees("main");
        assert!(main_calls.contains(&"print_int".to_string()));
        assert!(main_calls.contains(&"compute".to_string()));
    }

    #[test]
    fn call_graph_in_contract() {
        let source = r#"fn helper() -> Int:
    42

fn main() -> !{IO} ():
    print_int(helper())
"#;
        let session = Session::from_source(source);
        let contract = session.module_contract();
        let json = contract.to_json();
        assert!(json.contains("\"call_graph\""));
        assert!(json.contains("\"helper\""));
    }

    // ── Match expression call graph tests ─────────────────────────────

    #[test]
    fn call_graph_match_expression() {
        let source = "\
fn classify(n: Int) -> !{IO} ():
    match n:
        0:
            print(\"zero\")
        1:
            print_int(n)
        _:
            display(n)
";
        let session = Session::from_source(source);
        let graph = session.call_graph();
        assert_eq!(graph.len(), 1);
        let entry = &graph[0];
        assert_eq!(entry.function, "classify");
        assert!(
            entry.calls.contains(&"print".to_string()),
            "should find `print` in match arm, got: {:?}",
            entry.calls
        );
        assert!(
            entry.calls.contains(&"print_int".to_string()),
            "should find `print_int` in match arm, got: {:?}",
            entry.calls
        );
        assert!(
            entry.calls.contains(&"display".to_string()),
            "should find `display` in match arm, got: {:?}",
            entry.calls
        );
    }

    #[test]
    fn callees_match_expression() {
        let source = "\
fn handler(code: Int) -> !{IO} ():
    match code:
        0:
            success()
        _:
            fail(code)
";
        let session = Session::from_source(source);
        let calls = session.callees("handler");
        assert!(calls.contains(&"success".to_string()));
        assert!(calls.contains(&"fail".to_string()));
    }

    // ── Mutable binding tests ─────────────────────────────────────────

    #[test]
    fn symbols_mutable_let_binding() {
        let source = "let mut counter: Int = 0\n";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "counter");
        assert_eq!(symbols[0].kind, SymbolKind::Variable);
        assert!(
            symbols[0].ty.contains("let mut"),
            "mutable binding type should contain 'let mut', got: {}",
            symbols[0].ty
        );
    }

    #[test]
    fn symbols_immutable_let_binding() {
        let source = "let pi: Int = 3\n";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "pi");
        assert!(
            symbols[0].ty.starts_with("let pi:"),
            "immutable binding type should start with 'let pi:', got: {}",
            symbols[0].ty
        );
        assert!(
            !symbols[0].ty.contains("mut"),
            "immutable binding should not contain 'mut', got: {}",
            symbols[0].ty
        );
    }

    // ── Multi-file session tests ────────────────────────────────────────

    fn create_test_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let file_path = dir.path().join(name);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&file_path, content).unwrap();
        }
        dir
    }

    #[test]
    fn session_from_file_single() {
        let dir = create_test_dir(&[("main.gr", "fn add(a: Int, b: Int) -> Int:\n    a + b\n")]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(
            result.is_ok(),
            "single file should type-check: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn session_from_file_multifile() {
        let dir = create_test_dir(&[
            (
                "main.gr",
                "mod main\n\nuse helper\n\nfn main() -> Int:\n    ret helper.add(3, 4)\n",
            ),
            (
                "helper.gr",
                "mod helper\n\nfn add(a: Int, b: Int) -> Int:\n    a + b\n",
            ),
        ]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(
            result.is_ok(),
            "multi-file session should type-check, got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn session_from_file_multifile_type_error() {
        let dir = create_test_dir(&[
            (
                "main.gr",
                "mod main\n\nuse helper\n\nfn main() -> Int:\n    ret helper.add(3, true)\n",
            ),
            (
                "helper.gr",
                "mod helper\n\nfn add(a: Int, b: Int) -> Int:\n    a + b\n",
            ),
        ]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(
            !result.is_ok(),
            "should detect type error in qualified call"
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("expected `Int`, found `Bool`")),
            "should report type mismatch, got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn session_from_file_missing_import() {
        let dir = create_test_dir(&[(
            "main.gr",
            "mod main\n\nuse nonexistent\n\nfn main():\n    ()\n",
        )]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(!result.is_ok(), "should report error for missing import");
    }

    #[test]
    fn session_from_file_multifile_effects() {
        let dir = create_test_dir(&[
            (
                "main.gr",
                concat!(
                    "mod main\n\n",
                    "use io_helper\n\n",
                    "fn main() -> !{IO} ():\n",
                    "    io_helper.greet(\"world\")\n",
                ),
            ),
            (
                "io_helper.gr",
                concat!(
                    "mod io_helper\n\n",
                    "fn greet(name: String) -> !{IO} ():\n",
                    "    print(name)\n",
                ),
            ),
        ]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(
            result.is_ok(),
            "multi-file with effects should type-check, got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn session_from_file_transitive_deps() {
        // main -> helper -> utils (transitive dependency).
        // main uses helper.compute() which internally calls utils functions.
        // main should be able to call helper.compute() without needing to
        // import utils directly.
        let dir = create_test_dir(&[
            (
                "main.gr",
                "mod main\n\nuse helper\n\nfn main() -> Int:\n    ret helper.compute(5)\n",
            ),
            (
                "helper.gr",
                "mod helper\n\nuse utils\n\nfn compute(x: Int) -> Int:\n    x * 2\n",
            ),
            ("utils.gr", "mod utils\n\nfn internal() -> Int:\n    42\n"),
        ]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(
            result.is_ok(),
            "transitive deps should resolve, got: {:?}",
            result
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Design-by-contract: @requires / @ensures in query API
    // -----------------------------------------------------------------------

    #[test]
    fn contracts_visible_in_symbols() {
        let source = "\
@requires(x > 0)
@ensures(result >= 0)
fn abs_val(x: Int) -> Int:
    if x >= 0:
        x
    else:
        0 - x
";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "abs_val");
        assert_eq!(symbols[0].contracts.len(), 2);
        assert_eq!(symbols[0].contracts[0].kind, "requires");
        assert!(symbols[0].contracts[0].condition.contains("x > 0"));
        assert_eq!(symbols[0].contracts[1].kind, "ensures");
        assert!(symbols[0].contracts[1].condition.contains("result >= 0"));
    }

    #[test]
    fn runtime_only_contract_visible_in_symbols() {
        let source = "\
@runtime_only(off_in_release)
@requires(x > 0)
fn f(x: Int) -> Int:
    ret x
";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].contracts.len(), 1);
        assert_eq!(symbols[0].contracts[0].kind, "requires");
        assert!(symbols[0].contracts[0].runtime_only_off_in_release);
    }

    #[test]
    fn contracts_visible_in_module_contract() {
        let source = "\
@requires(n >= 0)
fn factorial(n: Int) -> Int:
    if n == 0:
        1
    else:
        n * factorial(n - 1)
";
        let session = Session::from_source(source);
        let contract = session.module_contract();
        assert!(!contract.has_errors);
        assert_eq!(contract.symbols.len(), 1);
        assert_eq!(contract.symbols[0].contracts.len(), 1);
        assert_eq!(contract.symbols[0].contracts[0].kind, "requires");
    }

    #[test]
    fn contracts_in_json_output() {
        let source = "\
@requires(x > 0)
fn f(x: Int) -> Int:
    ret x
";
        let session = Session::from_source(source);
        let contract = session.module_contract();
        let json = contract.to_json();
        assert!(
            json.contains("requires"),
            "JSON should contain contract kind 'requires'"
        );
        assert!(
            json.contains("x > 0"),
            "JSON should contain the contract condition"
        );
    }

    #[test]
    fn function_without_contracts_has_empty_contracts() {
        let source = "\
fn f(x: Int) -> Int:
    ret x
";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert!(symbols[0].contracts.is_empty());
    }

    // -----------------------------------------------------------------------
    // Completion context tests
    // -----------------------------------------------------------------------

    #[test]
    fn completion_context_in_function_body() {
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        // Line 2, col 5: inside the function body.
        let ctx = session.completion_context(2, 5);

        // Should have the function parameters as bindings.
        assert!(
            ctx.bindings_in_scope
                .iter()
                .any(|b| b.name == "a" && b.ty == "Int"),
            "parameter `a` should be in scope, got: {:?}",
            ctx.bindings_in_scope
        );
        assert!(
            ctx.bindings_in_scope
                .iter()
                .any(|b| b.name == "b" && b.ty == "Int"),
            "parameter `b` should be in scope, got: {:?}",
            ctx.bindings_in_scope
        );

        // Expected type should be Int (function return type).
        assert_eq!(ctx.expected_type, Some("Int".to_string()));

        // Should have builtins available.
        assert!(!ctx.available_builtins.is_empty());
        assert!(ctx.available_builtins.contains(&"print".to_string()));

        // Functions returning Int should be listed.
        assert!(
            ctx.matching_functions.contains(&"add".to_string()),
            "add should be in matching_functions since it returns Int, got: {:?}",
            ctx.matching_functions
        );
    }

    #[test]
    fn completion_context_let_rhs_expected_type() {
        let source = "\
fn example() -> ():
    let x: Int = 42
    let y: String = \"hello\"
    x
";
        let session = Session::from_source(source);
        // Line 2, col 18: in the RHS of `let x: Int = 42`.
        let ctx = session.completion_context(2, 18);
        assert_eq!(
            ctx.expected_type,
            Some("Int".to_string()),
            "expected type should be Int from the type annotation"
        );
    }

    #[test]
    fn completion_context_bindings_accumulate() {
        let source = "\
fn example(x: Int) -> Int:
    let a: Int = 10
    let b: String = \"hi\"
    a + x
";
        let session = Session::from_source(source);
        // Line 4, col 5: after both let bindings.
        let ctx = session.completion_context(4, 5);

        // Parameters and previous let bindings should be in scope.
        assert!(
            ctx.bindings_in_scope.iter().any(|b| b.name == "x"),
            "parameter x should be in scope"
        );
        assert!(
            ctx.bindings_in_scope.iter().any(|b| b.name == "a"),
            "let binding a should be in scope"
        );
        assert!(
            ctx.bindings_in_scope.iter().any(|b| b.name == "b"),
            "let binding b should be in scope"
        );
    }

    #[test]
    fn completion_context_call_argument_expected_type() {
        let source = "\
fn process(x: Int) -> ():
    print_int(x)
";
        let session = Session::from_source(source);
        // Line 2, col 15: on the `x` argument in `print_int(x)`.
        let ctx = session.completion_context(2, 15);
        // The expected type should be Int (print_int takes an Int parameter).
        assert_eq!(
            ctx.expected_type,
            Some("Int".to_string()),
            "expected type for print_int argument should be Int"
        );
    }

    #[test]
    fn completion_context_return_position() {
        let source = "\
fn get_name() -> String:
    ret \"Alice\"
";
        let session = Session::from_source(source);
        // Line 2, col 9: on the ret expression.
        let ctx = session.completion_context(2, 9);
        assert_eq!(
            ctx.expected_type,
            Some("String".to_string()),
            "expected type at ret should be String (function return type)"
        );
    }

    #[test]
    fn completion_context_with_enum_variants() {
        let source = "\
type Color = Red | Green | Blue

fn pick() -> Color:
    Red
";
        let session = Session::from_source(source);
        let ctx = session.completion_context(4, 5);

        // Enum variants should be available.
        assert!(
            ctx.available_variants.iter().any(|v| v.contains("Red")),
            "Red variant should be available, got: {:?}",
            ctx.available_variants
        );
        assert!(
            ctx.available_variants.iter().any(|v| v.contains("Green")),
            "Green variant should be available"
        );
        assert!(
            ctx.available_variants.iter().any(|v| v.contains("Blue")),
            "Blue variant should be available"
        );
    }

    #[test]
    fn completion_context_matching_functions() {
        let source = "\
fn make_int() -> Int:
    42

fn double(n: Int) -> Int:
    n * 2

fn use_it() -> Int:
    make_int()
";
        let session = Session::from_source(source);
        // Line 8, col 5: in the body of use_it, which returns Int.
        let ctx = session.completion_context(8, 5);
        assert_eq!(ctx.expected_type, Some("Int".to_string()));

        // Both make_int and double return Int, so they should be listed.
        assert!(
            ctx.matching_functions.contains(&"make_int".to_string()),
            "make_int should match, got: {:?}",
            ctx.matching_functions
        );
        assert!(
            ctx.matching_functions.contains(&"double".to_string()),
            "double should match, got: {:?}",
            ctx.matching_functions
        );
    }

    #[test]
    fn completion_context_json_serialization() {
        let source = "\
fn f(x: Int) -> Int:
    x
";
        let session = Session::from_source(source);
        let ctx = session.completion_context(2, 5);
        let json = ctx.to_json();
        assert!(json.contains("expected_type"));
        assert!(json.contains("bindings_in_scope"));
        assert!(json.contains("available_builtins"));

        // Pretty print should also work.
        let pretty = ctx.to_json_pretty();
        assert!(pretty.contains("expected_type"));
    }

    #[test]
    fn completion_context_outside_function() {
        let source = "\
let x: Int = 42
fn f() -> ():
    print(\"hi\")
";
        let session = Session::from_source(source);
        // Line 1 is not inside any function.
        let ctx = session.completion_context(1, 5);

        // Should still have the top-level let binding.
        assert!(
            ctx.bindings_in_scope.iter().any(|b| b.name == "x"),
            "top-level binding x should be available"
        );
        // Should have function `f` as a binding.
        assert!(
            ctx.bindings_in_scope.iter().any(|b| b.name == "f"),
            "function f should be listed as a binding"
        );
    }

    #[test]
    fn completion_context_mutable_bindings() {
        let source = "\
fn counter() -> Int:
    let mut count: Int = 0
    count
";
        let session = Session::from_source(source);
        let ctx = session.completion_context(3, 5);

        let count_binding = ctx.bindings_in_scope.iter().find(|b| b.name == "count");
        assert!(count_binding.is_some(), "count should be in scope");
        assert!(
            count_binding.unwrap().mutable,
            "count should be marked mutable"
        );
    }

    // -----------------------------------------------------------------------
    // Typed hole resolution enhancement tests
    // -----------------------------------------------------------------------

    #[test]
    fn typed_hole_reports_expected_type() {
        let source = "\
fn get_val(x: Int) -> Int:
    ?
";
        let session = Session::from_source(source);
        let result = session.check();
        assert!(!result.is_ok());

        // Should have a diagnostic about the typed hole.
        let hole_diag = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("typed hole"));
        assert!(hole_diag.is_some(), "should have a typed hole diagnostic");

        let diag = hole_diag.unwrap();
        // Should mention the expected type in the notes.
        assert!(
            diag.notes.iter().any(|n| n.contains("expected type: Int")),
            "should mention expected type Int, got notes: {:?}",
            diag.notes
        );
    }

    #[test]
    fn typed_hole_reports_matching_bindings() {
        let source = "\
fn pick(a: Int, b: Int) -> Int:
    ?
";
        let session = Session::from_source(source);
        let result = session.check();
        let hole_diag = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("typed hole"));
        assert!(hole_diag.is_some());

        let diag = hole_diag.unwrap();
        // Should mention matching bindings `a` and `b`.
        let binding_note = diag.notes.iter().find(|n| n.contains("matching bindings"));
        assert!(
            binding_note.is_some(),
            "should have a note about matching bindings, got: {:?}",
            diag.notes
        );
        let note = binding_note.unwrap();
        assert!(
            note.contains("`a`"),
            "should mention binding `a` in: {}",
            note
        );
        assert!(
            note.contains("`b`"),
            "should mention binding `b` in: {}",
            note
        );
    }

    #[test]
    fn typed_hole_reports_matching_functions() {
        let source = "\
fn helper() -> Int:
    42

fn main_fn() -> Int:
    ?
";
        let session = Session::from_source(source);
        let result = session.check();
        let hole_diag = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("typed hole"));
        assert!(hole_diag.is_some());

        let diag = hole_diag.unwrap();
        // Should mention `helper` as a matching function.
        let fn_note = diag.notes.iter().find(|n| n.contains("matching functions"));
        assert!(
            fn_note.is_some(),
            "should have a note about matching functions, got: {:?}",
            diag.notes
        );
        assert!(
            fn_note.unwrap().contains("helper"),
            "should mention function `helper`"
        );
    }

    #[test]
    fn generic_function_in_symbols() {
        let source = r#"fn identity[T](x: T) -> T:
    ret x
"#;
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert!(
            symbols[0].ty.contains("[T]"),
            "symbol signature should include type params, got: {}",
            symbols[0].ty
        );
    }

    #[test]
    fn generic_enum_in_symbols() {
        let source = r#"type Option[T] = Some(Int) | None
"#;
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert!(
            symbols[0].ty.contains("Option[T]"),
            "symbol type should include type params, got: {}",
            symbols[0].ty
        );
    }

    #[test]
    fn generic_function_in_module_contract() {
        let source = r#"fn identity[T](x: T) -> T:
    ret x

fn main() -> !{IO} ():
    let n: Int = identity(42)
    print_int(n)
"#;
        let session = Session::from_source(source);
        let contract = session.module_contract();
        assert!(!contract.has_errors);
        let id_sym = contract.symbols.iter().find(|s| s.name == "identity");
        assert!(id_sym.is_some(), "identity should be in contract symbols");
        assert!(
            id_sym.unwrap().ty.contains("[T]"),
            "identity signature should include [T]"
        );
    }

    #[test]
    fn effect_poly_in_module_contract() {
        let src = r#"
fn apply(f: (Int) -> !{e} Int, x: Int) -> !{e} Int:
    ret f(x)

fn pure_double(x: Int) -> Int:
    ret x * 2
"#;
        let session = Session::from_source(src);
        let contract = session.module_contract();
        assert!(!contract.has_errors);

        let apply_sym = contract.symbols.iter().find(|s| s.name == "apply");
        assert!(apply_sym.is_some(), "apply should be in contract symbols");
        let apply = apply_sym.unwrap();
        assert!(
            apply.is_effect_polymorphic,
            "apply should be effect-polymorphic"
        );
        assert!(
            apply.effects.contains(&"e".to_string()),
            "apply should declare effect variable `e`"
        );

        let double_sym = contract.symbols.iter().find(|s| s.name == "pure_double");
        assert!(double_sym.is_some());
        assert!(
            !double_sym.unwrap().is_effect_polymorphic,
            "pure_double should not be effect-polymorphic"
        );
    }

    // -----------------------------------------------------------------------
    // Phase Q: Context Budget Tooling tests
    // -----------------------------------------------------------------------

    #[test]
    fn context_budget_returns_items_within_budget() {
        let source = "\
fn helper(x: Int) -> Int:
    x + 1

fn main() -> !{IO} ():
    let y = helper(5)
    print_int(y)
";
        let session = Session::from_source(source);
        let result = session.context_budget("main", 500);

        assert_eq!(result.target_function, "main");
        assert!(
            result.used_tokens <= result.budget_tokens,
            "used_tokens ({}) should not exceed budget_tokens ({})",
            result.used_tokens,
            result.budget_tokens
        );
        assert!(!result.items.is_empty(), "should have context items");
    }

    #[test]
    fn context_budget_prioritizes_by_relevance() {
        let source = "\
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        1
    else:
        n * factorial(n - 1)

fn helper() -> Int:
    42
";
        let session = Session::from_source(source);
        let result = session.context_budget("factorial", 5000);

        // The target function's signature should come first (relevance 1.0)
        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].name, "factorial");
        assert_eq!(result.items[0].kind, "function_signature");

        // Verify items are sorted by relevance (non-increasing)
        for window in result.items.windows(2) {
            assert!(
                window[0].relevance >= window[1].relevance,
                "items should be sorted by relevance: {} >= {} failed for {:?} vs {:?}",
                window[0].relevance,
                window[1].relevance,
                window[0].kind,
                window[1].kind
            );
        }
    }

    #[test]
    fn context_budget_includes_called_functions() {
        let source = "\
fn helper(x: Int) -> Int:
    x * 2

fn main() -> Int:
    helper(5)
";
        let session = Session::from_source(source);
        let result = session.context_budget("main", 5000);

        // Should include helper's signature as a called function
        let helper_item = result
            .items
            .iter()
            .find(|i| i.name == "helper" && i.kind == "function_signature");
        assert!(
            helper_item.is_some(),
            "should include called function helper's signature, items: {:?}",
            result
                .items
                .iter()
                .map(|i| (&i.kind, &i.name))
                .collect::<Vec<_>>()
        );

        // Helper should have lower relevance than the target function
        let main_item = result
            .items
            .iter()
            .find(|i| i.name == "main" && i.kind == "function_signature");
        assert!(main_item.is_some(), "should include main's signature");
        assert!(
            main_item.unwrap().relevance > helper_item.unwrap().relevance,
            "target function should have higher relevance than callees"
        );
    }

    #[test]
    fn context_budget_includes_type_definitions() {
        let source = "\
type Color = Red | Green | Blue

fn pick_color(n: Color) -> Color:
    n
";
        let session = Session::from_source(source);
        let result = session.context_budget("pick_color", 5000);

        let type_item = result
            .items
            .iter()
            .find(|i| i.kind == "type_def" && i.name == "Color");
        assert!(
            type_item.is_some(),
            "should include Color type definition, items: {:?}",
            result
                .items
                .iter()
                .map(|i| (&i.kind, &i.name))
                .collect::<Vec<_>>()
        );
        assert!(type_item.unwrap().content.contains("Red"));
        assert!(type_item.unwrap().content.contains("Green"));
        assert!(type_item.unwrap().content.contains("Blue"));
    }

    #[test]
    fn context_budget_includes_contracts() {
        let source = "\
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        1
    else:
        n * factorial(n - 1)
";
        let session = Session::from_source(source);
        let result = session.context_budget("factorial", 5000);

        let contract_items: Vec<_> = result
            .items
            .iter()
            .filter(|i| i.kind == "contract")
            .collect();
        assert!(
            contract_items.len() >= 2,
            "should include @requires and @ensures contracts, got: {:?}",
            contract_items
                .iter()
                .map(|i| &i.content)
                .collect::<Vec<_>>()
        );
        assert!(contract_items
            .iter()
            .any(|i| i.content.contains("requires")));
        assert!(contract_items.iter().any(|i| i.content.contains("ensures")));
    }

    #[test]
    fn context_budget_respects_small_budget() {
        let source = "\
fn a() -> Int:
    42

fn b() -> Int:
    a()

fn c() -> Int:
    b()

fn d() -> Int:
    c()

fn target() -> !{IO} ():
    let x = a()
    let y = b()
    let z = c()
    let w = d()
    print_int(x)
";
        let session = Session::from_source(source);

        // With a very small budget, should include fewer items
        let small = session.context_budget("target", 20);
        let large = session.context_budget("target", 5000);

        assert!(
            small.items.len() <= large.items.len(),
            "small budget ({} items) should have <= items than large budget ({} items)",
            small.items.len(),
            large.items.len()
        );
        assert!(
            small.used_tokens <= 20,
            "small budget used_tokens ({}) should not exceed budget (20)",
            small.used_tokens
        );
    }

    #[test]
    fn context_budget_includes_builtin_functions() {
        let source = "\
fn display(n: Int) -> !{IO} ():
    print_int(n)
";
        let session = Session::from_source(source);
        let result = session.context_budget("display", 5000);

        let builtin_item = result
            .items
            .iter()
            .find(|i| i.kind == "builtin" && i.name == "print_int");
        assert!(
            builtin_item.is_some(),
            "should include builtin print_int, items: {:?}",
            result
                .items
                .iter()
                .map(|i| (&i.kind, &i.name))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn context_budget_includes_capability_ceiling() {
        let source = "\
@cap(IO)

fn hello() -> !{IO} ():
    print(\"hi\")
";
        let session = Session::from_source(source);
        let result = session.context_budget("hello", 5000);

        let cap_item = result.items.iter().find(|i| i.kind == "capability");
        assert!(
            cap_item.is_some(),
            "should include capability ceiling, items: {:?}",
            result
                .items
                .iter()
                .map(|i| (&i.kind, &i.name))
                .collect::<Vec<_>>()
        );
        assert!(cap_item.unwrap().content.contains("IO"));
    }

    #[test]
    fn context_budget_json_serialization() {
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let result = session.context_budget("add", 5000);
        let json = result.to_json();
        assert!(json.contains("\"target_function\":\"add\""));
        assert!(json.contains("\"budget_tokens\":5000"));
        assert!(json.contains("\"items\""));

        // Pretty print should also work.
        let pretty = result.to_json_pretty();
        assert!(pretty.contains("target_function"));
        assert!(pretty.contains('\n'));
    }

    // -----------------------------------------------------------------------
    // Project index tests
    // -----------------------------------------------------------------------

    #[test]
    fn project_index_includes_all_functions() {
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b

fn greet(name: String) -> !{IO} ():
    print(name)
";
        let session = Session::from_source(source);
        let index = session.project_index();

        assert_eq!(index.modules.len(), 1);
        let module = &index.modules[0];
        assert_eq!(module.functions.len(), 2);

        let add_fn = module.functions.iter().find(|f| f.name == "add");
        assert!(add_fn.is_some(), "should include add function");
        assert!(add_fn.unwrap().is_pure, "add should be pure");
        assert!(add_fn.unwrap().effects.is_empty());

        let greet_fn = module.functions.iter().find(|f| f.name == "greet");
        assert!(greet_fn.is_some(), "should include greet function");
        assert!(!greet_fn.unwrap().is_pure, "greet should not be pure");
        assert_eq!(greet_fn.unwrap().effects, vec!["IO".to_string()]);
    }

    #[test]
    fn project_index_surfaces_arena_parameterized_effect() {
        // E3 / `#320`: the Query API mirrors the parser's effect-string
        // verbatim, so `!{Arena(scratch)}` surfaces as `"Arena(scratch)"`
        // in the symbol's effect row. This is load-bearing for agent
        // tooling: `gradient query effects` must report the same set of
        // effects the type checker enforces.
        let source = "\
fn alloc_in(scratch: Int) -> !{Arena(scratch)} Int:
    scratch
";
        let session = Session::from_source(source);
        let index = session.project_index();

        let module = &index.modules[0];
        let alloc_fn = module
            .functions
            .iter()
            .find(|f| f.name == "alloc_in")
            .expect("expected alloc_in function in project index");
        assert_eq!(
            alloc_fn.effects,
            vec!["Arena(scratch)".to_string()],
            "Arena(<name>) effect must surface verbatim through Query API"
        );
        // `is_pure` reflects INFERRED effects (what the body actually does),
        // not DECLARED effects. The body here is a passthrough, so the
        // function is provably pure at the body level even though its
        // signature carries an Arena tag — `Arena(_)` is a marker effect
        // (region tag), not a heap-effect gate. The audit-trail / lifetime
        // / typestate enforcement story is deferred to issue #321.
        assert!(
            alloc_fn.is_pure,
            "alloc_in body has no inferred effects; body-level purity holds even though declared row carries Arena(scratch)"
        );
    }

    #[test]
    fn project_index_includes_types() {
        let source = "\
type Color = Red | Green | Blue

type Count = Int

fn pick() -> Int:
    42
";
        let session = Session::from_source(source);
        let index = session.project_index();

        let module = &index.modules[0];
        assert_eq!(module.types.len(), 2, "should have 2 type definitions");

        let color_type = module.types.iter().find(|t| t.name == "Color");
        assert!(color_type.is_some(), "should include Color type");
        assert_eq!(color_type.unwrap().kind, "enum");

        let count_type = module.types.iter().find(|t| t.name == "Count");
        assert!(count_type.is_some(), "should include Count type");
        assert_eq!(count_type.unwrap().kind, "alias");
    }

    #[test]
    fn project_index_includes_call_graph() {
        let source = "\
fn helper() -> Int:
    42

fn main() -> !{IO} ():
    print_int(helper())
";
        let session = Session::from_source(source);
        let index = session.project_index();

        assert!(!index.call_graph.is_empty());
        let main_entry = index.call_graph.iter().find(|e| e.function == "main");
        assert!(main_entry.is_some());
        assert!(main_entry.unwrap().calls.contains(&"helper".to_string()));
        assert!(main_entry.unwrap().calls.contains(&"print_int".to_string()));
    }

    #[test]
    fn project_index_includes_contracts() {
        let source = "\
@requires(n >= 0)
fn factorial(n: Int) -> Int:
    if n == 0:
        1
    else:
        n * factorial(n - 1)
";
        let session = Session::from_source(source);
        let index = session.project_index();

        let module = &index.modules[0];
        let factorial_fn = module.functions.iter().find(|f| f.name == "factorial");
        assert!(factorial_fn.is_some());
        assert!(
            !factorial_fn.unwrap().contracts.is_empty(),
            "factorial should have contracts"
        );
        assert!(factorial_fn.unwrap().contracts[0].contains("requires"));
    }

    #[test]
    fn project_index_json_is_valid() {
        let source = "\
type Color = Red | Green | Blue

@requires(n >= 0)
fn factorial(n: Int) -> Int:
    if n == 0:
        1
    else:
        n * factorial(n - 1)

fn main() -> !{IO} ():
    print_int(factorial(5))
";
        let session = Session::from_source(source);
        let index = session.project_index();
        let json = index.to_json();

        // Verify the JSON is parseable
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("project index JSON should be valid");
        assert!(parsed.get("modules").is_some(), "should have modules key");
        assert!(
            parsed.get("call_graph").is_some(),
            "should have call_graph key"
        );

        let pretty = index.to_json_pretty();
        let parsed_pretty: serde_json::Value =
            serde_json::from_str(&pretty).expect("pretty JSON should also be valid");
        assert!(parsed_pretty.get("modules").is_some());
    }

    #[test]
    fn project_index_module_name() {
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        // Without a mod declaration, default module name should be "main"
        assert_eq!(index.modules[0].name, "main");
    }

    #[test]
    fn context_budget_for_nonexistent_function() {
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let result = session.context_budget("nonexistent", 5000);
        assert_eq!(result.target_function, "nonexistent");
        // Should return empty items since the function doesn't exist
        assert!(result.items.is_empty());
        assert_eq!(result.used_tokens, 0);
    }

    #[test]
    fn project_index_with_capability_ceiling() {
        let source = "\
@cap(IO)

fn hello() -> !{IO} ():
    print(\"hi\")

fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        let module = &index.modules[0];
        assert_eq!(
            module.capability_ceiling,
            Some(vec!["IO".to_string()]),
            "should include capability ceiling in index"
        );
    }

    // -----------------------------------------------------------------------
    // Module panic_strategy: @panic(abort|unwind|none) in project_index (#337)
    // -----------------------------------------------------------------------

    #[test]
    fn project_index_panic_strategy_defaults_to_unwind() {
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].panic_strategy, "unwind",
            "module without `@panic` attribute should default to `unwind` in project_index"
        );
    }

    #[test]
    fn project_index_panic_strategy_abort() {
        let source = "\
@panic(abort)

fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].panic_strategy, "abort",
            "@panic(abort) should surface as `abort` in project_index"
        );
    }

    #[test]
    fn project_index_panic_strategy_none() {
        // @panic(none) forbids panic-able ops; the body here is panic-free
        // (just an int addition + literal multiply), so the checker accepts.
        let source = "\
@panic(none)

fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].panic_strategy, "none",
            "@panic(none) should surface as `none` in project_index"
        );
    }

    #[test]
    fn project_index_panic_strategy_unwind_explicit() {
        let source = "\
@panic(unwind)

fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(index.modules[0].panic_strategy, "unwind");
    }

    #[test]
    fn project_index_panic_strategy_serializes_to_json() {
        let source = "\
@panic(abort)

fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        let json = index.to_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("project index JSON should be valid");
        let modules = parsed
            .get("modules")
            .and_then(|m| m.as_array())
            .expect("modules array");
        let panic_strategy = modules[0]
            .get("panic_strategy")
            .and_then(|p| p.as_str())
            .expect("modules[0].panic_strategy should be a string in JSON");
        assert_eq!(panic_strategy, "abort");
    }

    // -----------------------------------------------------------------------
    // Module alloc_strategy: derived from effects_used in project_index (#333)
    // -----------------------------------------------------------------------

    #[test]
    fn project_index_alloc_strategy_minimal_for_pure_arithmetic() {
        // No heap-allocating builtins -> alloc_strategy "minimal"
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].alloc_strategy, "minimal",
            "pure arithmetic should classify as minimal alloc strategy"
        );
    }

    #[test]
    fn project_index_alloc_strategy_full_when_heap_declared() {
        // A function that declares !{Heap} should flip alloc_strategy to full.
        let source = "\
fn make_string(n: Int) -> !{Heap} String:
    int_to_string(n)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].alloc_strategy, "full",
            "module with heap effect declared should classify as full alloc strategy"
        );
    }

    #[test]
    fn project_index_alloc_strategy_full_when_string_concat_used() {
        // String + String requires Heap (#532); the checker propagates Heap
        // into the function's effect row, which surfaces as alloc_strategy
        // = "full" in project_index.
        let source = "\
fn greet(name: String) -> !{Heap} String:
    \"hello, \" + name
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].alloc_strategy, "full",
            "string concatenation should propagate Heap and flip to full"
        );
    }

    #[test]
    fn project_index_alloc_strategy_minimal_when_only_io_declared() {
        // IO is a heap-free effect -> alloc_strategy stays minimal.
        let source = "\
fn shout(n: Int) -> !{IO} ():
    print_int(n)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].alloc_strategy, "minimal",
            "IO effect alone should not flip alloc strategy to full"
        );
    }

    #[test]
    fn project_index_alloc_strategy_serializes_to_json() {
        let source = "\
fn make_string(n: Int) -> !{Heap} String:
    int_to_string(n)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        let json = index.to_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("project index JSON should be valid");
        let modules = parsed
            .get("modules")
            .and_then(|m| m.as_array())
            .expect("modules array");
        let alloc_strategy = modules[0]
            .get("alloc_strategy")
            .and_then(|p| p.as_str())
            .expect("modules[0].alloc_strategy should be a string in JSON");
        assert_eq!(alloc_strategy, "full");
    }

    // -----------------------------------------------------------------------
    // Module actor_strategy: derived from effects_used in project_index (#334)
    // -----------------------------------------------------------------------

    #[test]
    fn project_index_actor_strategy_none_for_pure_arithmetic() {
        // No actor builtins -> actor_strategy "none"
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].actor_strategy, "none",
            "pure arithmetic should classify as none actor strategy"
        );
    }

    #[test]
    fn project_index_actor_strategy_full_when_actor_declared() {
        // A function that declares Actor effect should flip actor_strategy to full.
        let source = "\
fn launch(n: Int) -> !{Actor} ():
    ret ()
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].actor_strategy, "full",
            "module with actor effect declared should classify as full actor strategy"
        );
    }

    #[test]
    fn project_index_actor_strategy_none_when_only_io_declared() {
        // IO is an actor-free effect -> actor_strategy stays none.
        let source = "\
fn shout(n: Int) -> !{IO} ():
    print_int(n)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].actor_strategy, "none",
            "IO effect alone should not flip actor strategy to full"
        );
    }

    #[test]
    fn project_index_actor_strategy_none_when_only_heap_declared() {
        // Heap is orthogonal to Actor — heap-using programs may still be
        // actor-free. Pin this so future strategies don't accidentally
        // promote heap-using programs into actor-full builds.
        let source = "\
fn make(n: Int) -> !{Heap} String:
    int_to_string(n)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].actor_strategy, "none",
            "Heap effect alone should not flip actor strategy to full"
        );
    }

    #[test]
    fn project_index_actor_strategy_serializes_to_json() {
        let source = "\
fn launch(n: Int) -> !{Actor} ():
    ret ()
";
        let session = Session::from_source(source);
        let index = session.project_index();
        let json = index.to_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("project index JSON should be valid");
        let modules = parsed
            .get("modules")
            .and_then(|m| m.as_array())
            .expect("modules array");
        let actor_strategy = modules[0]
            .get("actor_strategy")
            .and_then(|p| p.as_str())
            .expect("modules[0].actor_strategy should be a string in JSON");
        assert_eq!(actor_strategy, "full");
    }

    // -----------------------------------------------------------------------
    // Module async_strategy: derived from effects_used in project_index (#335)
    // -----------------------------------------------------------------------

    #[test]
    fn project_index_async_strategy_none_for_pure_arithmetic() {
        // No async builtins -> async_strategy "none"
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].async_strategy, "none",
            "pure arithmetic should classify as none async strategy"
        );
    }

    #[test]
    fn project_index_async_strategy_full_when_async_declared() {
        // A function that declares Async effect should flip async_strategy to full.
        let source = "\
fn await_thing(n: Int) -> !{Async} Int:
    ret n
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].async_strategy, "full",
            "module with async effect declared should classify as full async strategy"
        );
    }

    #[test]
    fn project_index_async_strategy_none_when_only_io_declared() {
        // IO is an async-free effect -> async_strategy stays none.
        let source = "\
fn shout(n: Int) -> !{IO} ():
    print_int(n)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].async_strategy, "none",
            "IO effect alone should not flip async strategy to full"
        );
    }

    #[test]
    fn project_index_async_strategy_none_when_only_actor_declared() {
        // Actor is orthogonal to Async — actor-using programs may still be
        // synchronous (an actor's mailbox loop is not the same as an async
        // executor). Pin this so future strategies don't accidentally
        // promote actor-using programs into async-full builds.
        let source = "\
fn launch(n: Int) -> !{Actor} ():
    ret ()
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].async_strategy, "none",
            "Actor effect alone should not flip async strategy to full"
        );
    }

    #[test]
    fn project_index_async_strategy_serializes_to_json() {
        let source = "\
fn await_thing(n: Int) -> !{Async} Int:
    ret n
";
        let session = Session::from_source(source);
        let index = session.project_index();
        let json = index.to_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("project index JSON should be valid");
        let modules = parsed
            .get("modules")
            .and_then(|m| m.as_array())
            .expect("modules array");
        let async_strategy = modules[0]
            .get("async_strategy")
            .and_then(|p| p.as_str())
            .expect("modules[0].async_strategy should be a string in JSON");
        assert_eq!(async_strategy, "full");
    }

    // -----------------------------------------------------------------------
    // Module allocator_strategy: derived from the AST `@allocator(...)`
    // attribute in project_index (#336). Sibling of panic_strategy above —
    // attribute-driven, NOT effect-driven (the deployment target isn't
    // derivable from effects).
    // -----------------------------------------------------------------------

    #[test]
    fn project_index_allocator_strategy_default_when_unannotated() {
        // No @allocator attribute -> allocator_strategy defaults to "default".
        let source = "\
fn main() -> !{IO} ():
    print_int(0)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].allocator_strategy, "default",
            "unannotated module should default to allocator_strategy = default"
        );
    }

    #[test]
    fn project_index_allocator_strategy_default_when_explicitly_default() {
        let source = "\
@allocator(default)

fn main() -> !{IO} ():
    print_int(0)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].allocator_strategy, "default",
            "@allocator(default) should classify as default"
        );
    }

    #[test]
    fn project_index_allocator_strategy_pluggable_when_annotated() {
        let source = "\
@allocator(pluggable)

fn main() -> !{IO} ():
    print_int(0)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].allocator_strategy, "pluggable",
            "@allocator(pluggable) should classify as pluggable"
        );
    }

    #[test]
    fn project_index_allocator_strategy_does_not_flip_with_heap() {
        // Orthogonality pin: the EFFECT-driven alloc_strategy axis flips
        // to "full" when Heap is reachable, but the ATTRIBUTE-driven
        // allocator_strategy axis MUST stay at its declared value
        // regardless of effect surface. Future refactors that conflate
        // the two axes get caught here.
        let source = "\
fn build(s: String) -> !{Heap} String:
    ret s + s
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].alloc_strategy, "full",
            "Heap-reachable program should flip alloc_strategy to full"
        );
        assert_eq!(
            index.modules[0].allocator_strategy, "default",
            "allocator_strategy is attribute-driven; Heap effect must NOT flip it"
        );
    }

    #[test]
    fn project_index_allocator_strategy_serializes_to_json() {
        let source = "\
@allocator(pluggable)

fn main() -> !{IO} ():
    print_int(0)
";
        let session = Session::from_source(source);
        let index = session.project_index();
        let json = index.to_json();
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("project index JSON should be valid");
        let modules = parsed
            .get("modules")
            .and_then(|m| m.as_array())
            .expect("modules array");
        let allocator_strategy = modules[0]
            .get("allocator_strategy")
            .and_then(|p| p.as_str())
            .expect("modules[0].allocator_strategy should be a string in JSON");
        assert_eq!(allocator_strategy, "pluggable");
    }

    #[test]
    fn project_index_allocator_strategy_arena_when_annotated() {
        // #320 / #336 follow-on: a third allocator variant `arena`
        // backed by a process-global bump-pointer arena. Annotated
        // modules surface as `"arena"` through the same Query API
        // field used by `gradient build` to pick the runtime crate.
        let src = "\
@allocator(arena)

fn main() -> Int:
    ret 0
";
        let session = Session::from_source(src);
        let index = session.project_index();
        assert_eq!(index.modules.len(), 1);
        assert_eq!(
            index.modules[0].allocator_strategy, "arena",
            "@allocator(arena) should classify as arena"
        );
    }

    #[test]
    fn project_index_allocator_strategy_slab_when_annotated() {
        // #545: a fourth allocator variant `slab` backed by a
        // fixed-size-class slab allocator. Annotated modules surface
        // as `"slab"` through the same Query API field used by
        // `gradient build` to pick the runtime crate. Sibling pin to
        // `project_index_allocator_strategy_arena_when_annotated`.
        let src = "\
@allocator(slab)

fn main() -> Int:
    ret 0
";
        let session = Session::from_source(src);
        let index = session.project_index();
        assert_eq!(index.modules.len(), 1);
        assert_eq!(
            index.modules[0].allocator_strategy, "slab",
            "@allocator(slab) should classify as slab"
        );
    }

    #[test]
    fn project_index_allocator_strategy_bumpalo_when_annotated() {
        // #547: a fifth allocator variant `bumpalo` backed by a
        // multi-chunk bump-arena allocator. Annotated modules surface
        // as `"bumpalo"` through the same Query API field used by
        // `gradient build` to pick the runtime crate. Sibling pin to
        // `project_index_allocator_strategy_slab_when_annotated`.
        let src = "\
@allocator(bumpalo)

fn main() -> Int:
    ret 0
";
        let session = Session::from_source(src);
        let index = session.project_index();
        assert_eq!(index.modules.len(), 1);
        assert_eq!(
            index.modules[0].allocator_strategy, "bumpalo",
            "@allocator(bumpalo) should classify as bumpalo"
        );
    }

    // Module mode: @app/@system in project_index (#352, Epic #301)

    #[test]
    fn project_index_module_mode_defaults_to_app() {
        // No @app or @system attribute -> module_mode defaults to "app".
        let source = "\
fn main() -> Int:
    ret 0
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].module_mode, "app",
            "unannotated module should default to module_mode = app"
        );
    }

    #[test]
    fn project_index_module_mode_app_when_explicit() {
        let source = "\
@app

fn main() -> Int:
    ret 0
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(index.modules[0].module_mode, "app");
    }

    #[test]
    fn project_index_module_mode_system_when_annotated() {
        // #352: @system surfaces as "system" through the same Query API
        // field future tooling (LSP, agent harness) reads to know
        // whether the module is in inference-everywhere or
        // explicit-everywhere mode.
        let source = "\
@system

fn main() -> !{IO} Int:
    ret 0
";
        let session = Session::from_source(source);
        let index = session.project_index();
        assert_eq!(
            index.modules[0].module_mode, "system",
            "@system module should classify as system"
        );
    }

    #[test]
    fn project_index_module_mode_serializes_to_json() {
        // Pin the JSON shape: `module_mode` is a top-level string on
        // each `ModuleIndex` entry. Same shape contract as
        // `panic_strategy` / `allocator_strategy`.
        let source = "\
@system

fn main() -> !{IO} Int:
    ret 0
";
        let session = Session::from_source(source);
        let index = session.project_index();
        let json = serde_json::to_value(&index).expect("serialize ProjectIndex");
        let modules = json
            .get("modules")
            .and_then(|m| m.as_array())
            .expect("project_index should have modules array");
        let module_mode = modules[0]
            .get("module_mode")
            .and_then(|v| v.as_str())
            .expect("modules[0].module_mode should be a string in JSON");
        assert_eq!(module_mode, "system");
    }

    #[test]
    fn estimate_tokens_heuristic() {
        // Verify the token estimation helper works correctly.
        assert_eq!(super::estimate_tokens(""), 1); // empty string -> 1 token min
        assert_eq!(super::estimate_tokens("fn a() -> Int"), 4); // 13 chars / 4 = 3.25 -> 4
        assert_eq!(super::estimate_tokens("abcd"), 1); // 4 chars / 4 = 1
        assert_eq!(super::estimate_tokens("ab"), 1); // 2 chars / 4 = 0.5 -> 1
    }

    // -----------------------------------------------------------------------
    // Runtime capability budgets: @budget in query API
    // -----------------------------------------------------------------------

    #[test]
    fn budget_visible_in_symbols() {
        let src = "\
@budget(cpu: 5s, mem: 100mb)
fn process(x: Int) -> Int:
    ret x * 2
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "process");
        let budget = symbols[0].budget.as_ref().expect("expected budget info");
        assert_eq!(budget.cpu.as_deref(), Some("5s"));
        assert_eq!(budget.mem.as_deref(), Some("100mb"));
    }

    #[test]
    fn budget_visible_in_module_contract() {
        let src = "\
@budget(cpu: 10s, mem: 256mb)
fn heavy(x: Int) -> Int:
    ret x

fn light(x: Int) -> Int:
    ret x + 1
";
        let session = Session::from_source(src);
        let contract = session.module_contract();
        assert!(!contract.has_errors);
        assert_eq!(contract.symbols.len(), 2);

        let heavy = contract.symbols.iter().find(|s| s.name == "heavy").unwrap();
        let budget = heavy.budget.as_ref().expect("heavy should have budget");
        assert_eq!(budget.cpu.as_deref(), Some("10s"));
        assert_eq!(budget.mem.as_deref(), Some("256mb"));

        let light = contract.symbols.iter().find(|s| s.name == "light").unwrap();
        assert!(light.budget.is_none(), "light should have no budget");
    }

    #[test]
    fn budget_in_json_output() {
        let src = "\
@budget(cpu: 3s)
fn fast(x: Int) -> Int:
    ret x
";
        let session = Session::from_source(src);
        let contract = session.module_contract();
        let json = contract.to_json();
        assert!(
            json.contains("budget"),
            "JSON should contain budget field: {}",
            json
        );
        assert!(
            json.contains("3s"),
            "JSON should contain budget value: {}",
            json
        );
    }

    #[test]
    fn symbols_extern_fn_visible() {
        let src = "\
@extern
fn puts(s: String) -> Int
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "puts");
        assert_eq!(symbols[0].kind, SymbolKind::ExternFunction);
        assert!(symbols[0].is_extern);
        assert!(symbols[0].extern_lib.is_none());
        assert!(!symbols[0].is_export);
    }

    #[test]
    fn symbols_extern_fn_with_lib() {
        let src = r#"
@extern("libm")
fn sin(x: Float) -> Float
"#;
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "sin");
        assert!(symbols[0].is_extern);
        assert_eq!(symbols[0].extern_lib.as_deref(), Some("libm"));
    }

    #[test]
    fn symbols_export_fn_visible() {
        let src = "\
@export
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "add");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert!(symbols[0].is_export);
        assert!(!symbols[0].is_extern);
    }
}

#[cfg(test)]
mod test_annotation_query_tests {
    use super::*;

    #[test]
    fn symbols_test_fn_visible() {
        let src = "\
@test
fn test_add() -> Bool:
    1 + 1 == 2
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "test_add");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert!(symbols[0].is_test);
        assert!(!symbols[0].is_export);
        assert!(!symbols[0].is_extern);
    }

    #[test]
    fn symbols_non_test_fn_has_is_test_false() {
        let src = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert!(!symbols[0].is_test);
    }

    #[test]
    fn symbols_test_fn_unit_return() {
        let src = "\
@test
fn test_unit():
    let x: Int = 1
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert!(symbols[0].is_test);
    }

    #[test]
    fn symbols_test_fn_in_json() {
        let src = "\
@test
fn test_check() -> Bool:
    true
";
        let session = Session::from_source(src);
        let contract = session.module_contract();
        let json = contract.to_json();
        assert!(
            json.contains("\"is_test\":true"),
            "JSON should contain is_test:true: {}",
            json
        );
    }
}

#[cfg(test)]
mod actor_tests {
    use super::*;

    #[test]
    fn symbols_actor_visible() {
        let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1
    on GetCount -> Int:
        ret count
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Counter");
        assert_eq!(symbols[0].kind, SymbolKind::Actor);
        assert!(symbols[0].ty.contains("actor Counter"));
        assert!(symbols[0].ty.contains("on Increment"));
        assert!(symbols[0].ty.contains("on GetCount -> Int"));
    }

    #[test]
    fn actor_module_contract() {
        let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1
    on GetCount -> Int:
        ret count

fn main() -> !{Actor, Async, Send} ():
    let c = spawn Counter
    send c Increment
    let n: Int = ask c GetCount
";
        let session = Session::from_source(src);
        let result = session.check();
        assert!(
            result.is_ok(),
            "expected no errors, got {:?}",
            result.diagnostics
        );

        let symbols = session.symbols();
        assert_eq!(symbols.len(), 2); // Counter actor + main fn
        let actor_sym = symbols.iter().find(|s| s.name == "Counter").unwrap();
        assert_eq!(actor_sym.kind, SymbolKind::Actor);
        assert_eq!(
            actor_sym.effects,
            vec!["Actor".to_string(), "Async".to_string(), "Send".to_string()]
        );
    }

    #[test]
    fn symbols_trait_visible() {
        let src = "\
trait Display:
    fn display(self) -> String
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "Display");
        assert_eq!(symbols[0].kind, SymbolKind::Trait);
        assert!(symbols[0].ty.contains("trait Display"));
        assert!(symbols[0].ty.contains("fn display(self) -> String"));
    }

    #[test]
    fn symbols_impl_visible() {
        let src = "\
trait Display:
    fn display(self) -> String

impl Display for Int:
    fn display(self) -> String:
        ret int_to_string(self)
";
        let session = Session::from_source(src);
        let symbols = session.symbols();
        let trait_sym = symbols
            .iter()
            .find(|s| s.kind == SymbolKind::Trait)
            .unwrap();
        assert_eq!(trait_sym.name, "Display");
        let impl_sym = symbols.iter().find(|s| s.kind == SymbolKind::Impl).unwrap();
        assert_eq!(impl_sym.name, "Display for Int");
        assert!(impl_sym.ty.contains("impl Display for Int"));
    }
}

// =========================================================================
// Documentation generator tests (Phase W)
// =========================================================================

#[cfg(test)]
mod doc_tests {
    use super::*;

    #[test]
    fn doc_comment_parsing() {
        let source = "/// Compute factorial.\nfn factorial(n: Int) -> Int:\n    n\n";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(
            symbols[0].doc_comment.as_deref(),
            Some("Compute factorial.")
        );
    }

    #[test]
    fn doc_comment_multiline() {
        let source = "\
/// Line one.
/// Line two.
fn foo() -> Int:
    42
";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(
            symbols[0].doc_comment.as_deref(),
            Some("Line one.\nLine two.")
        );
    }

    #[test]
    fn doc_comment_attached_to_function() {
        let source = "\
/// Add two integers.
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let doc = session.documentation();
        assert_eq!(doc.functions.len(), 1);
        assert_eq!(doc.functions[0].name, "add");
        assert_eq!(
            doc.functions[0].doc_comment.as_deref(),
            Some("Add two integers.")
        );
    }

    #[test]
    fn doc_comment_in_json_output() {
        let source = "\
/// Compute factorial.
fn factorial(n: Int) -> Int:
    n
";
        let session = Session::from_source(source);
        let doc = session.documentation();
        let json = doc.to_json();
        assert!(json.contains("\"doc_comment\":\"Compute factorial.\""));
        assert!(json.contains("\"name\":\"factorial\""));
    }

    #[test]
    fn doc_human_readable_format() {
        let source = "\
/// Compute factorial.
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        1
    else:
        n * factorial(n - 1)
";
        let session = Session::from_source(source);
        let text = session.documentation_text();

        assert!(text.contains("Module: main"));
        assert!(text.contains("fn factorial(n: Int) -> Int"));
        assert!(text.contains("[pure]"));
        assert!(text.contains("Compute factorial."));
        assert!(text.contains("@requires(n >= 0)"));
        assert!(text.contains("@ensures(result >= 1)"));
        assert!(text.contains("Calls: factorial (recursive)"));
    }

    #[test]
    fn doc_full_module_documentation() {
        let source = "\
mod math

fn add(a: Int, b: Int) -> Int:
    a + b

fn greet(name: String) -> !{IO} ():
    print(name)
";
        let session = Session::from_source(source);
        let doc = session.documentation();

        assert_eq!(doc.module, "math");
        assert_eq!(doc.functions.len(), 2);
        assert_eq!(doc.functions[0].name, "add");
        assert!(doc.functions[0].is_pure);
        assert_eq!(doc.functions[1].name, "greet");
        assert!(!doc.functions[1].is_pure);
        assert_eq!(doc.functions[1].effects, vec!["IO".to_string()]);
    }

    #[test]
    fn doc_with_contracts_and_effects() {
        let source = "\
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        1
    else:
        n * factorial(n - 1)

fn greet(name: String) -> !{IO} ():
    print(name)
";
        let session = Session::from_source(source);
        let doc = session.documentation();

        // factorial has contracts and is pure.
        let factorial_doc = &doc.functions[0];
        assert_eq!(factorial_doc.name, "factorial");
        assert!(factorial_doc.is_pure);
        assert_eq!(factorial_doc.contracts.len(), 2);
        assert_eq!(factorial_doc.contracts[0].kind, "requires");
        assert_eq!(factorial_doc.contracts[0].condition, "n >= 0");
        assert_eq!(factorial_doc.contracts[1].kind, "ensures");
        assert_eq!(factorial_doc.contracts[1].condition, "result >= 1");

        // greet has IO effect.
        let greet_doc = &doc.functions[1];
        assert_eq!(greet_doc.name, "greet");
        assert!(!greet_doc.is_pure);
        assert_eq!(greet_doc.effects, vec!["IO".to_string()]);
    }

    #[test]
    fn doc_with_generics() {
        let source = "\
type Option[T] = Some(Int) | None

fn identity[T](x: Int) -> Int:
    x
";
        let session = Session::from_source(source);
        let doc = session.documentation();

        // Type doc for the generic enum.
        assert_eq!(doc.types.len(), 1);
        assert_eq!(doc.types[0].name, "Option");
        assert!(doc.types[0].definition.contains("Option[T]"));
        assert!(doc.types[0].definition.contains("Some(Int)"));
        assert!(doc.types[0].definition.contains("None"));

        // Function with type params.
        assert_eq!(doc.functions.len(), 1);
        assert_eq!(doc.functions[0].name, "identity");
        assert_eq!(doc.functions[0].type_params, vec!["T".to_string()]);
    }

    #[test]
    fn doc_no_doc_comment_is_none() {
        let source = "\
fn add(a: Int, b: Int) -> Int:
    a + b
";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert_eq!(symbols.len(), 1);
        assert!(symbols[0].doc_comment.is_none());
    }

    #[test]
    fn doc_type_alias_with_comment() {
        let source = "\
/// A count of items.
type Count = Int
";
        let session = Session::from_source(source);
        let doc = session.documentation();
        assert_eq!(doc.types.len(), 1);
        assert_eq!(doc.types[0].name, "Count");
        assert_eq!(
            doc.types[0].doc_comment.as_deref(),
            Some("A count of items.")
        );
    }

    #[test]
    fn doc_enum_with_comment() {
        let source = "\
/// Represents an optional value.
type Option[T] = Some(Int) | None
";
        let session = Session::from_source(source);
        let doc = session.documentation();
        assert_eq!(doc.types.len(), 1);
        assert_eq!(doc.types[0].name, "Option");
        assert_eq!(
            doc.types[0].doc_comment.as_deref(),
            Some("Represents an optional value.")
        );
        assert_eq!(doc.types[0].variants.len(), 2);
    }

    #[test]
    fn doc_json_format_structure() {
        let source = "\
/// Compute factorial.
@requires(n >= 0)
fn factorial(n: Int) -> Int:
    if n <= 1:
        1
    else:
        n * factorial(n - 1)
";
        let session = Session::from_source(source);
        let doc = session.documentation();
        let json = doc.to_json_pretty();

        // Verify JSON has expected top-level keys.
        assert!(json.contains("\"module\""));
        assert!(json.contains("\"functions\""));
        assert!(json.contains("\"types\""));
        assert!(json.contains("\"call_graph\""));

        // Verify function details.
        assert!(json.contains("\"name\": \"factorial\""));
        assert!(json.contains("\"is_pure\": true"));
        assert!(json.contains("\"doc_comment\": \"Compute factorial.\""));
        assert!(json.contains("\"kind\": \"requires\""));
        assert!(json.contains("\"condition\": \"n >= 0\""));
    }

    #[test]
    fn doc_call_graph_in_output() {
        let source = "\
fn helper(x: Int) -> Int:
    x + 1

fn main_fn(n: Int) -> Int:
    helper(n)
";
        let session = Session::from_source(source);
        let doc = session.documentation();

        // main_fn should call helper.
        let main_doc = doc.functions.iter().find(|f| f.name == "main_fn").unwrap();
        assert!(main_doc.calls.contains(&"helper".to_string()));

        // Call graph should be present.
        assert!(!doc.call_graph.is_empty());
    }

    #[test]
    fn doc_budget_in_output() {
        let source = "\
@budget(cpu: 5s, mem: 100mb)
fn limited(n: Int) -> Int:
    n
";
        let session = Session::from_source(source);
        let doc = session.documentation();
        let func = &doc.functions[0];
        assert!(func.budget.is_some());
        let budget = func.budget.as_ref().unwrap();
        assert_eq!(budget.cpu.as_deref(), Some("5s"));
        assert_eq!(budget.mem.as_deref(), Some("100mb"));

        // Human-readable output should contain budget.
        let text = session.documentation_text();
        assert!(text.contains("@budget(cpu: 5s, mem: 100mb)"));
    }

    // -----------------------------------------------------------------------
    // Built-in Result type tests
    // -----------------------------------------------------------------------

    #[test]
    fn result_type_in_function_signature_check() {
        // A function using the built-in Result type should type-check cleanly.
        let source = "\
fn try_parse(s: String) -> Result:
    ret Ok(42)
";
        let session = Session::from_source(source);
        let result = session.check();
        assert_eq!(
            result.error_count, 0,
            "expected no errors, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn result_type_function_in_symbols() {
        // A function returning Result should appear in the symbol list.
        let source = "\
fn try_parse(s: String) -> Result:
    ret Ok(42)
";
        let session = Session::from_source(source);
        let symbols = session.symbols();
        assert!(
            symbols.iter().any(|s| s.name == "try_parse"),
            "try_parse should appear in symbols, got: {:?}",
            symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn try_operator_in_check() {
        // The ? operator should type-check when used correctly.
        let source = "\
fn inner() -> Result:
    ret Ok(1)

fn outer() -> Result:
    let x = inner()?
    ret Ok(x)
";
        let session = Session::from_source(source);
        let result = session.check();
        assert_eq!(
            result.error_count, 0,
            "expected no errors with ? operator, got: {:?}",
            result.diagnostics
        );
    }
}
