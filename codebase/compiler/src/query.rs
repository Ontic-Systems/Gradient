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

use crate::ast::module::Module;
use crate::ast::span::Span;
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::parser::error::ParseError;
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
    /// Where this symbol is defined.
    pub span: Span,
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

                    symbols.push(SymbolInfo {
                        name: fn_def.name.clone(),
                        kind: SymbolKind::Function,
                        ty: sig,
                        effects,
                        inferred_effects,
                        is_pure,
                        params,
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
                        span: item.span,
                    });
                }

                crate::ast::item::ItemKind::Let {
                    name,
                    type_ann,
                    ..
                } => {
                    let ty = type_ann
                        .as_ref()
                        .map(|t| format_type_expr(&t.node))
                        .unwrap_or_else(|| "<inferred>".to_string());

                    symbols.push(SymbolInfo {
                        name: name.clone(),
                        kind: SymbolKind::Variable,
                        ty,
                        effects: Vec::new(),
                        inferred_effects: Vec::new(),
                        is_pure: true,
                        params: Vec::new(),
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
                        span: item.span,
                    });
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
}
