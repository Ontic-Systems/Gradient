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

use crate::ast::module::Module;
use crate::ast::span::Span;
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::parser::error::ParseError;
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
    /// Where this symbol is defined.
    pub span: Span,
}

/// Information about a design-by-contract annotation.
#[derive(Debug, Clone, Serialize)]
pub struct ContractInfo {
    /// The kind of contract: "requires" or "ensures".
    pub kind: String,
    /// A human-readable representation of the condition expression.
    pub condition: String,
}

/// The kind of a top-level symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    ExternFunction,
    Variable,
    TypeAlias,
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
            let (errors, summary) =
                typechecker::check_module_with_effects(&module, 0);
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
            let source = std::fs::read_to_string(path)
                .unwrap_or_default();
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
        let entry = result.modules.get(&result.entry_module)
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
                typechecker::check_module_with_imports(
                    &entry_module_ast,
                    entry_file_id,
                    &imports,
                );
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

    /// Build a map of imported module function signatures from resolved modules.
    ///
    /// For each `use` declaration in the entry module, this extracts function
    /// signatures from the corresponding resolved module and builds the
    /// `ImportedModules` map that the type checker needs.
    pub fn build_import_map(
        entry_module: &Module,
        all_modules: &std::collections::HashMap<String, crate::resolve::ResolvedModule>,
    ) -> typechecker::ImportedModules {
        use crate::ast::item::ItemKind;

        let mut imports = typechecker::ImportedModules::new();

        for use_decl in &entry_module.uses {
            let dep_name = use_decl.path.join(".");

            if let Some(dep) = all_modules.get(&dep_name) {
                let mut fns = std::collections::HashMap::new();

                for item in &dep.module.items {
                    match &item.node {
                        ItemKind::FnDef(fn_def) => {
                            let sig = Self::ast_fn_to_sig(fn_def);
                            // If specific imports are requested, only include those.
                            if let Some(ref specific) = use_decl.specific_imports {
                                if specific.contains(&fn_def.name) {
                                    fns.insert(fn_def.name.clone(), sig);
                                }
                            } else {
                                fns.insert(fn_def.name.clone(), sig);
                            }
                        }
                        ItemKind::ExternFn(decl) => {
                            let sig = Self::ast_extern_fn_to_sig(decl);
                            if let Some(ref specific) = use_decl.specific_imports {
                                if specific.contains(&decl.name) {
                                    fns.insert(decl.name.clone(), sig);
                                }
                            } else {
                                fns.insert(decl.name.clone(), sig);
                            }
                        }
                        _ => {}
                    }
                }

                imports.insert(dep_name, fns);
            }
        }

        imports
    }

    /// Convert an AST function definition to a type checker FnSig.
    fn ast_fn_to_sig(fn_def: &crate::ast::item::FnDef) -> typechecker::FnSig {
        let params: Vec<(String, typechecker::Ty)> = fn_def
            .params
            .iter()
            .map(|p| (p.name.clone(), Self::resolve_type_expr_static(&p.type_ann.node)))
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
            params,
            ret,
            effects,
        }
    }

    /// Convert an AST extern function declaration to a type checker FnSig.
    fn ast_extern_fn_to_sig(decl: &crate::ast::item::ExternFnDecl) -> typechecker::FnSig {
        let params: Vec<(String, typechecker::Ty)> = decl
            .params
            .iter()
            .map(|p| (p.name.clone(), Self::resolve_type_expr_static(&p.type_ann.node)))
            .collect();

        let ret = decl
            .return_type
            .as_ref()
            .map(|t| Self::resolve_type_expr_static(&t.node))
            .unwrap_or(typechecker::Ty::Unit);

        let effects = decl
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        typechecker::FnSig {
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
            TypeExpr::Named(name) => match name.as_str() {
                "Int" => typechecker::Ty::Int,
                "Float" => typechecker::Ty::Float,
                "String" => typechecker::Ty::String,
                "Bool" => typechecker::Ty::Bool,
                _ => typechecker::Ty::Error, // Unknown types in imports
            },
            TypeExpr::Unit => typechecker::Ty::Unit,
            TypeExpr::Fn { params, ret } => {
                let param_tys: Vec<typechecker::Ty> = params
                    .iter()
                    .map(|p| Self::resolve_type_expr_static(&p.node))
                    .collect();
                let ret_ty = Self::resolve_type_expr_static(&ret.node);
                typechecker::Ty::Fn {
                    params: param_tys,
                    ret: Box::new(ret_ty),
                    effects: vec![],
                }
            }
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
            });
        }

        for te in &self.type_errors {
            diagnostics.push(Diagnostic {
                phase: Phase::Typechecker,
                severity: Severity::Error,
                message: te.message.clone(),
                span: te.span,
                expected: te.expected.as_ref().map(|t| t.to_string()),
                found: te.found.as_ref().map(|t| t.to_string()),
                notes: te.notes.clone(),
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

                    let sig = format!(
                        "fn {}({}){}{}",
                        fn_def.name,
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
                        })
                        .collect();

                    symbols.push(SymbolInfo {
                        name: fn_def.name.clone(),
                        kind: SymbolKind::Function,
                        ty: sig,
                        effects,
                        inferred_effects,
                        is_pure,
                        params,
                        contracts,
                        span: item.span,
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
                        span: item.span,
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
                        span: item.span,
                    });
                }

                crate::ast::item::ItemKind::TypeDecl { name, type_expr } => {
                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::TypeAlias,
                        ty: format!("type {} = {}", name, format_type_expr(&type_expr.node)),
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        span: item.span,
                    });
                }

                crate::ast::item::ItemKind::EnumDecl { name, variants } => {
                    let variant_names: Vec<String> = variants
                        .iter()
                        .map(|v| {
                            if let Some(ref field) = v.field {
                                format!("{}({})", v.name, format_type_expr(&field.node))
                            } else {
                                v.name.clone()
                            }
                        })
                        .collect();
                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::TypeAlias,
                        ty: format!("type {} = {}", name, variant_names.join(" | ")),
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
                        contracts: Vec::new(),
                        span: item.span,
                    });
                }

                crate::ast::item::ItemKind::CapDecl { .. } => {
                    // Capability declarations are not symbols — they constrain the module.
                }
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
            .filter(|s| !s.is_pure && matches!(s.kind, SymbolKind::Function | SymbolKind::ExternFunction))
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
        // Validate the new name is a valid identifier.
        if new_name.is_empty()
            || !new_name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        {
            return Err(format!("`{}` is not a valid identifier", new_name));
        }

        // Find all occurrences of the old name in the source.
        let mut new_source = String::new();
        let mut locations = Vec::new();
        let mut offset = 0usize;

        for (line_num, line) in self.source.lines().enumerate() {
            let mut col = 0usize;
            let mut result_line = String::new();

            while col < line.len() {
                // Check if old_name starts at this position.
                if line[col..].starts_with(old_name) {
                    let end = col + old_name.len();
                    // Verify it's a whole word (not a substring of a longer identifier).
                    let before_ok = col == 0 || !is_ident_char(line.as_bytes()[col - 1]);
                    let after_ok = end >= line.len() || !is_ident_char(line.as_bytes()[end]);

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
                result_line.push(line.as_bytes()[col] as char);
                col += 1;
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
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!("{{\"error\": \"serialization failed: {}\"}}", e)
        })
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| {
            format!("{{\"error\": \"serialization failed: {}\"}}", e)
        })
    }
}

impl RenameResult {
    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!("{{\"error\": \"serialization failed: {}\"}}", e)
        })
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| {
            format!("{{\"error\": \"serialization failed: {}\"}}", e)
        })
    }
}

impl ModuleContract {
    /// Serialize to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!("{{\"error\": \"serialization failed: {}\"}}", e)
        })
    }

    /// Serialize to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| {
            format!("{{\"error\": \"serialization failed: {}\"}}", e)
        })
    }
}

// =========================================================================
// Helpers
// =========================================================================

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
fn format_type_expr(te: &crate::ast::types::TypeExpr) -> String {
    match te {
        crate::ast::types::TypeExpr::Named(name) => name.clone(),
        crate::ast::types::TypeExpr::Unit => "()".to_string(),
        crate::ast::types::TypeExpr::Fn { params, ret } => {
            let params_str = params
                .iter()
                .map(|p| format_type_expr(&p.node))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({}) -> {}", params_str, format_type_expr(&ret.node))
        }
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
        ExprKind::TypedHole(label) => {
            label.as_ref().map_or("?".to_string(), |l| format!("?{}", l))
        }
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
        let source = r#"fn helper():
    print("hello")
"#;
        let session = Session::from_source(source);
        let result = session.check();
        // print requires IO effect — should produce an error with a note
        assert!(!result.is_ok());
        let diag = &result.diagnostics[0];
        assert!(!diag.notes.is_empty());
        assert!(diag.notes[0].contains("IO"));
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
        let main_info = summary.functions.iter().find(|f| f.function == "main").unwrap();
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
            result.diagnostics.iter().any(|d| d.message.contains("unknown effect")),
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
        let unknown_errors: Vec<_> = result.diagnostics.iter()
            .filter(|d| d.message.contains("unknown effect"))
            .collect();
        assert!(unknown_errors.is_empty());
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
        assert!(result.is_ok(), "should compile: function uses only IO, cap allows IO");
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
            result.diagnostics.iter().any(|d| d.message.contains("exceeds the module capability ceiling")),
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
        assert!(result.is_ok(), "pure module with pure functions should compile");
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
        assert!(
            result.diagnostics.iter().any(|d| d.message.contains("exceeds the module capability ceiling")),
        );
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
        let dir = create_test_dir(&[
            ("main.gr", "fn add(a: Int, b: Int) -> Int:\n    a + b\n"),
        ]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(result.is_ok(), "single file should type-check: {:?}", result.diagnostics);
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
            result.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
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
            result.diagnostics.iter().any(|d| d.message.contains("expected `Int`, found `Bool`")),
            "should report type mismatch, got: {:?}",
            result.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn session_from_file_missing_import() {
        let dir = create_test_dir(&[
            (
                "main.gr",
                "mod main\n\nuse nonexistent\n\nfn main():\n    ()\n",
            ),
        ]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(
            !result.is_ok(),
            "should report error for missing import"
        );
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
            result.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
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
            (
                "utils.gr",
                "mod utils\n\nfn internal() -> Int:\n    42\n",
            ),
        ]);
        let entry = dir.path().join("main.gr");
        let session = Session::from_file(&entry).unwrap();
        let result = session.check();
        assert!(
            result.is_ok(),
            "transitive deps should resolve, got: {:?}",
            result.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
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
        assert!(json.contains("requires"), "JSON should contain contract kind 'requires'");
        assert!(json.contains("x > 0"), "JSON should contain the contract condition");
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
}
