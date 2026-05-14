//! Issue #232: query service kernel for the self-hosted compiler.
//!
//! `compiler/query.gr` historically returned placeholder data: `new_session`
//! returned `tokens: 0`, `ast: 0`, errors `0`, `type_checked: false`; `check`
//! returned `ok: true, error_count: 0, diagnostics: 0`; `get_symbols` returned
//! `SymbolList { handle: 0 }`. The .gr code documented every method as full
//! implementation work.
//!
//! This module replaces those stubs with a runtime-backed query engine that
//! reuses the existing pipeline kernel (#230) for lex/parse/check and exposes
//! diagnostic and symbol queries via integer-handle accessors.
//!
//! ## Three-tier kernel boundary (from #228/#229/#230/#231)
//!
//! 1. **Runtime store**: process-wide `Mutex<QueryStore>` keyed by integer
//!    session ids. Each session caches the original source, the parsed
//!    `ast::Module`, and the parse / type-check error vectors so symbol /
//!    diagnostic / `type_at` queries can be served without re-parsing.
//! 2. **Rust adapter**: mirrors the .gr-side `query.gr` view of the world:
//!    sessions are integer ids, diagnostics and symbols are flat lists indexed
//!    by integer position, every accessor returns either an `i64`, a `String`,
//!    or `0` / `""` for unknown ids.
//! 3. **CI gate**: `tests/self_hosted_query.rs` drives this kernel through
//!    fixtures that exercise happy-path symbol enumeration, parse-error
//!    diagnostics, type-error diagnostics, and `type_at` lookups.
//!
//! The `.gr` source declares the externs but does NOT need typechecker-known
//! status for the gate to work — the gate exercises the kernel directly. When
//! .gr-side delegation is wanted later, register the externs in
//! `codebase/compiler/src/typechecker/env.rs` alongside the parser bootstrap
//! externs (`define_fn` calls around lines 1023-1175).
//!
//! ## Wire shapes
//!
//! Severity codes (match `query.gr::Severity`):
//!   1 = error, 2 = warning, 3 = info
//!
//! Phase codes (match `query.gr::Phase`):
//!   1 = lexer, 2 = parser, 3 = typechecker, 4 = ir, 5 = codegen
//!
//! Symbol kinds (match `query.gr::SymbolKind`):
//!   1 = function, 2 = extern_function, 3 = variable, 4 = type_alias,
//!   5 = actor, 6 = trait, 7 = impl

use std::sync::{Mutex, OnceLock};

use crate::ast::item::ItemKind;
use crate::ast::module::Module;
use crate::ast::types::TypeExpr;
use crate::lexer::Lexer;
use crate::parser as ast_parser;
use crate::parser::error::ParseError;
use crate::typechecker;
use crate::typechecker::error::TypeError;

// ── Wire-format constants ────────────────────────────────────────────────

pub const SEVERITY_ERROR: i64 = 1;
pub const SEVERITY_WARNING: i64 = 2;
#[allow(dead_code)]
pub const SEVERITY_INFO: i64 = 3;

#[allow(dead_code)]
pub const PHASE_LEXER: i64 = 1;
pub const PHASE_PARSER: i64 = 2;
pub const PHASE_TYPECHECKER: i64 = 3;
#[allow(dead_code)]
pub const PHASE_IR: i64 = 4;
#[allow(dead_code)]
pub const PHASE_CODEGEN: i64 = 5;

pub const SYMBOL_KIND_FUNCTION: i64 = 1;
pub const SYMBOL_KIND_EXTERN_FUNCTION: i64 = 2;
#[allow(dead_code)]
pub const SYMBOL_KIND_VARIABLE: i64 = 3;
pub const SYMBOL_KIND_TYPE_ALIAS: i64 = 4;
pub const SYMBOL_KIND_ACTOR: i64 = 5;
pub const SYMBOL_KIND_TRAIT: i64 = 6;
pub const SYMBOL_KIND_IMPL: i64 = 7;

// ── Snapshot types stored per session ────────────────────────────────────

#[derive(Debug, Clone)]
struct DiagnosticEntry {
    phase: i64,
    severity: i64,
    message: String,
    line: i64,
    col: i64,
}

#[derive(Debug, Clone)]
struct ParamEntry {
    name: String,
    ty: String,
}

#[derive(Debug, Clone)]
struct SymbolEntry {
    name: String,
    kind: i64,
    ty: String,
    is_pure: bool,
    is_extern: bool,
    is_export: bool,
    is_test: bool,
    line: i64,
    col: i64,
    params: Vec<ParamEntry>,
    effects: Vec<String>,
}

#[derive(Debug, Default)]
struct QuerySession {
    source: String,
    parse_errors: Vec<ParseError>,
    type_errors: Vec<TypeError>,
    type_checked: bool,
    /// Flat diagnostic snapshot — `diagnostics[i]` resolves to a stable
    /// integer-indexed view from the .gr side.
    diagnostics: Vec<DiagnosticEntry>,
    /// Flat symbol snapshot — same indexing model as diagnostics.
    symbols: Vec<SymbolEntry>,
    /// Cached parsed module, kept for `type_at`-style queries that don't
    /// need to re-run the parser.
    module: Option<Module>,
}

#[derive(Default, Debug)]
struct QueryStore {
    sessions: Vec<QuerySession>,
}

impl QueryStore {
    fn alloc(&mut self) -> i64 {
        let id = (self.sessions.len() as i64) + 1;
        self.sessions.push(QuerySession::default());
        id
    }

    fn get(&self, id: i64) -> Option<&QuerySession> {
        if id <= 0 {
            return None;
        }
        self.sessions.get((id as usize) - 1)
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut QuerySession> {
        if id <= 0 {
            return None;
        }
        self.sessions.get_mut((id as usize) - 1)
    }
}

fn store() -> &'static Mutex<QueryStore> {
    static STORE: OnceLock<Mutex<QueryStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(QueryStore::default()))
}

fn with_store<R>(f: impl FnOnce(&mut QueryStore) -> R) -> R {
    let mut s = store().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut s)
}

/// Reset the query session table. Test-only.
pub fn reset_query_store() {
    with_store(|s| s.sessions.clear());
}

// ── Type rendering helpers (match query.gr's flat String view) ──────────

fn render_type(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Named { name, .. } => name.clone(),
        TypeExpr::Unit => "()".to_string(),
        TypeExpr::Tuple(parts) => {
            let rendered: Vec<String> = parts.iter().map(|p| render_type(&p.node)).collect();
            format!("({})", rendered.join(", "))
        }
        TypeExpr::Fn { params, ret, .. } => {
            let p: Vec<String> = params.iter().map(|p| render_type(&p.node)).collect();
            let r = render_type(&ret.node);
            format!("({}) -> {}", p.join(", "), r)
        }
        TypeExpr::Generic { name, args, .. } => {
            let parts: Vec<String> = args.iter().map(|a| render_type(&a.node)).collect();
            format!("{}[{}]", name, parts.join(", "))
        }
        TypeExpr::Record(_) => "<record>".to_string(),
        TypeExpr::Linear(inner) => format!("!linear {}", render_type(&inner.node)),
        TypeExpr::Type => "type".to_string(),
    }
}

fn render_return(ret: Option<&crate::ast::span::Spanned<TypeExpr>>) -> String {
    match ret {
        Some(t) => render_type(&t.node),
        None => "()".to_string(),
    }
}

fn render_function_type(
    params: &[crate::ast::item::Param],
    ret: Option<&crate::ast::span::Spanned<TypeExpr>>,
) -> String {
    let pstr: Vec<String> = params
        .iter()
        .map(|p| render_type(&p.type_ann.node))
        .collect();
    format!("({}) -> {}", pstr.join(", "), render_return(ret))
}

fn collect_effects(set: Option<&crate::ast::types::EffectSet>) -> Vec<String> {
    match set {
        Some(s) => s.effects.clone(),
        None => Vec::new(),
    }
}

// ── Public session API ───────────────────────────────────────────────────

/// Create a new query session from in-memory source. Runs lex / parse /
/// check eagerly so subsequent diagnostic / symbol queries are O(1)
/// lookups against the cached snapshot. Returns the session id (>= 1).
/// Empty `source` still allocates a session, but with no errors and no
/// symbols (matches the existing `query.gr::new_session("")` semantics).
pub fn bootstrap_query_new_session(source: &str) -> i64 {
    let id = with_store(|s| s.alloc());
    populate_session(id, source);
    id
}

/// Number of sessions currently held by the store. Useful for tests
/// that want to confirm allocation behavior.
pub fn bootstrap_query_session_count() -> i64 {
    with_store(|s| s.sessions.len() as i64)
}

fn populate_session(id: i64, source: &str) {
    if source.is_empty() {
        with_store(|s| {
            if let Some(sess) = s.get_mut(id) {
                sess.source.clear();
                sess.type_checked = false;
            }
        });
        return;
    }

    let mut lexer = Lexer::new(source, 0);
    let tokens = lexer.tokenize();
    let (module, parse_errors) = ast_parser::parse(tokens, 0);

    let type_errors = if parse_errors.is_empty() {
        typechecker::check_module(&module, 0)
    } else {
        Vec::new()
    };

    let diagnostics = build_diagnostic_snapshot(&parse_errors, &type_errors);
    let symbols = build_symbol_snapshot(&module);

    with_store(|s| {
        if let Some(sess) = s.get_mut(id) {
            sess.source = source.to_string();
            sess.parse_errors = parse_errors;
            sess.type_errors = type_errors;
            sess.type_checked = true;
            sess.diagnostics = diagnostics;
            sess.symbols = symbols;
            sess.module = Some(module);
        }
    });
}

fn build_diagnostic_snapshot(
    parse_errors: &[ParseError],
    type_errors: &[TypeError],
) -> Vec<DiagnosticEntry> {
    let mut out = Vec::new();
    for pe in parse_errors {
        out.push(DiagnosticEntry {
            phase: PHASE_PARSER,
            severity: SEVERITY_ERROR,
            message: pe.message.clone(),
            line: pe.span.start.line as i64,
            col: pe.span.start.col as i64,
        });
    }
    for te in type_errors {
        out.push(DiagnosticEntry {
            phase: PHASE_TYPECHECKER,
            severity: if te.is_warning {
                SEVERITY_WARNING
            } else {
                SEVERITY_ERROR
            },
            message: te.message.clone(),
            line: te.span.start.line as i64,
            col: te.span.start.col as i64,
        });
    }
    out
}

fn build_symbol_snapshot(module: &Module) -> Vec<SymbolEntry> {
    let mut out = Vec::new();
    for item in &module.items {
        let line = item.span.start.line as i64;
        let col = item.span.start.col as i64;
        match &item.node {
            ItemKind::FnDef(fn_def) => {
                let effects = collect_effects(fn_def.effects.as_ref());
                let is_pure = effects.is_empty();
                let params: Vec<ParamEntry> = fn_def
                    .params
                    .iter()
                    .map(|p| ParamEntry {
                        name: p.name.clone(),
                        ty: render_type(&p.type_ann.node),
                    })
                    .collect();
                out.push(SymbolEntry {
                    name: fn_def.name.clone(),
                    kind: SYMBOL_KIND_FUNCTION,
                    ty: render_function_type(&fn_def.params, fn_def.return_type.as_ref()),
                    is_pure,
                    is_extern: false,
                    is_export: fn_def.is_export,
                    is_test: fn_def.is_test,
                    line,
                    col,
                    params,
                    effects,
                });
            }
            ItemKind::ExternFn(ext) => {
                let effects = collect_effects(ext.effects.as_ref());
                let is_pure = effects.is_empty();
                let params: Vec<ParamEntry> = ext
                    .params
                    .iter()
                    .map(|p| ParamEntry {
                        name: p.name.clone(),
                        ty: render_type(&p.type_ann.node),
                    })
                    .collect();
                out.push(SymbolEntry {
                    name: ext.name.clone(),
                    kind: SYMBOL_KIND_EXTERN_FUNCTION,
                    ty: render_function_type(&ext.params, ext.return_type.as_ref()),
                    is_pure,
                    is_extern: true,
                    is_export: false,
                    is_test: false,
                    line,
                    col,
                    params,
                    effects,
                });
            }
            ItemKind::TypeDecl {
                name, type_expr, ..
            } => {
                out.push(SymbolEntry {
                    name: name.clone(),
                    kind: SYMBOL_KIND_TYPE_ALIAS,
                    ty: render_type(&type_expr.node),
                    is_pure: true,
                    is_extern: false,
                    is_export: false,
                    is_test: false,
                    line,
                    col,
                    params: Vec::new(),
                    effects: Vec::new(),
                });
            }
            ItemKind::EnumDecl { name, .. } => {
                out.push(SymbolEntry {
                    name: name.clone(),
                    kind: SYMBOL_KIND_TYPE_ALIAS,
                    ty: name.clone(),
                    is_pure: true,
                    is_extern: false,
                    is_export: false,
                    is_test: false,
                    line,
                    col,
                    params: Vec::new(),
                    effects: Vec::new(),
                });
            }
            ItemKind::ActorDecl { name, .. } => {
                out.push(SymbolEntry {
                    name: name.clone(),
                    kind: SYMBOL_KIND_ACTOR,
                    ty: name.clone(),
                    is_pure: false,
                    is_extern: false,
                    is_export: false,
                    is_test: false,
                    line,
                    col,
                    params: Vec::new(),
                    effects: Vec::new(),
                });
            }
            ItemKind::TraitDecl { name, .. } => {
                out.push(SymbolEntry {
                    name: name.clone(),
                    kind: SYMBOL_KIND_TRAIT,
                    ty: name.clone(),
                    is_pure: true,
                    is_extern: false,
                    is_export: false,
                    is_test: false,
                    line,
                    col,
                    params: Vec::new(),
                    effects: Vec::new(),
                });
            }
            ItemKind::ImplBlock {
                trait_name,
                target_type,
                ..
            } => {
                out.push(SymbolEntry {
                    name: format!("{} for {}", trait_name, target_type),
                    kind: SYMBOL_KIND_IMPL,
                    ty: format!("impl {} for {}", trait_name, target_type),
                    is_pure: true,
                    is_extern: false,
                    is_export: false,
                    is_test: false,
                    line,
                    col,
                    params: Vec::new(),
                    effects: Vec::new(),
                });
            }
            _ => {}
        }
    }
    out
}

// ── Top-level scalar accessors ──────────────────────────────────────────

pub fn bootstrap_query_session_source(session_id: i64) -> String {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| sess.source.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_query_parse_error_count(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| sess.parse_errors.len() as i64)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_type_error_count(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| sess.type_errors.iter().filter(|e| !e.is_warning).count() as i64)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_is_type_checked(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| if sess.type_checked { 1 } else { 0 })
            .unwrap_or(0)
    })
}

/// Aggregate `check`-equivalent: 1 if the session has zero non-warning
/// errors AND has been type-checked, 0 otherwise.
pub fn bootstrap_query_check_ok(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| {
                let parse_count = sess.parse_errors.len() as i64;
                let type_count = sess.type_errors.iter().filter(|e| !e.is_warning).count() as i64;
                if sess.type_checked && parse_count == 0 && type_count == 0 {
                    1
                } else {
                    0
                }
            })
            .unwrap_or(0)
    })
}

/// Aggregate non-warning error count (parse + type).
pub fn bootstrap_query_error_count(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| {
                let parse_count = sess.parse_errors.len() as i64;
                let type_count = sess.type_errors.iter().filter(|e| !e.is_warning).count() as i64;
                parse_count + type_count
            })
            .unwrap_or(0)
    })
}

// ── Diagnostic accessors ────────────────────────────────────────────────

pub fn bootstrap_query_diagnostic_count(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| sess.diagnostics.len() as i64)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_diagnostic_phase(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.diagnostics.get(index as usize))
            .map(|d| d.phase)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_diagnostic_severity(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.diagnostics.get(index as usize))
            .map(|d| d.severity)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_diagnostic_message(session_id: i64, index: i64) -> String {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.diagnostics.get(index as usize))
            .map(|d| d.message.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_query_diagnostic_line(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.diagnostics.get(index as usize))
            .map(|d| d.line)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_diagnostic_col(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.diagnostics.get(index as usize))
            .map(|d| d.col)
            .unwrap_or(0)
    })
}

// ── Symbol accessors ────────────────────────────────────────────────────

pub fn bootstrap_query_symbol_count(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| sess.symbols.len() as i64)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_name(session_id: i64, index: i64) -> String {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| s.name.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_query_symbol_kind(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| s.kind)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_type(session_id: i64, index: i64) -> String {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| s.ty.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_query_symbol_is_pure(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| if s.is_pure { 1 } else { 0 })
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_is_extern(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| if s.is_extern { 1 } else { 0 })
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_is_export(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| if s.is_export { 1 } else { 0 })
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_is_test(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| if s.is_test { 1 } else { 0 })
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_line(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| s.line)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_col(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| s.col)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_param_count(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| s.params.len() as i64)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_param_name(
    session_id: i64,
    sym_index: i64,
    param_index: i64,
) -> String {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(sym_index as usize))
            .and_then(|sym| sym.params.get(param_index as usize))
            .map(|p| p.name.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_query_symbol_param_type(
    session_id: i64,
    sym_index: i64,
    param_index: i64,
) -> String {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(sym_index as usize))
            .and_then(|sym| sym.params.get(param_index as usize))
            .map(|p| p.ty.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_query_symbol_effect_count(session_id: i64, index: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(index as usize))
            .map(|s| s.effects.len() as i64)
            .unwrap_or(0)
    })
}

pub fn bootstrap_query_symbol_effect_at(
    session_id: i64,
    sym_index: i64,
    effect_index: i64,
) -> String {
    with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.symbols.get(sym_index as usize))
            .and_then(|sym| sym.effects.get(effect_index as usize))
            .cloned()
            .unwrap_or_default()
    })
}

/// Look up a symbol's index by name. Returns -1 if not found. The .gr
/// side can compose this with the `_symbol_*` accessors to implement
/// `find_symbol(name)`.
pub fn bootstrap_query_find_symbol(session_id: i64, name: &str) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| {
                for (i, sym) in sess.symbols.iter().enumerate() {
                    if sym.name == name {
                        return i as i64;
                    }
                }
                -1
            })
            .unwrap_or(-1)
    })
}

// ── type_at / symbol_at ─────────────────────────────────────────────────

/// Return the 0-based symbol index whose span covers (line, col), or -1
/// if no top-level symbol covers that position. Used to implement
/// `query.gr::symbol_at` on top of the symbol snapshot.
pub fn bootstrap_query_symbol_at(session_id: i64, line: i64, col: i64) -> i64 {
    with_store(|s| {
        let sess = match s.get(session_id) {
            Some(v) => v,
            None => return -1,
        };
        let module = match &sess.module {
            Some(m) => m,
            None => return -1,
        };
        // Walk items and pick the one whose span contains (line, col).
        for (idx, item) in module.items.iter().enumerate() {
            let span = item.span;
            if position_in_span(line, col, span) {
                // The symbol snapshot order tracks module.items order for
                // recognised kinds. Map AST index -> symbol index by
                // counting recognised items up to `idx`.
                return ast_index_to_symbol_index(module, idx);
            }
        }
        -1
    })
}

fn position_in_span(line: i64, col: i64, span: crate::ast::span::Span) -> bool {
    let start_line = span.start.line as i64;
    let start_col = span.start.col as i64;
    let end_line = span.end.line as i64;
    let end_col = span.end.col as i64;
    if line < start_line || line > end_line {
        return false;
    }
    if line == start_line && col < start_col {
        return false;
    }
    if line == end_line && col > end_col {
        return false;
    }
    true
}

fn ast_index_to_symbol_index(module: &Module, ast_index: usize) -> i64 {
    let mut sym_idx: i64 = -1;
    for (i, item) in module.items.iter().enumerate() {
        if recognised_symbol(&item.node) {
            sym_idx += 1;
        }
        if i == ast_index {
            return if recognised_symbol(&item.node) {
                sym_idx
            } else {
                -1
            };
        }
    }
    -1
}

fn recognised_symbol(kind: &ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::FnDef(_)
            | ItemKind::ExternFn(_)
            | ItemKind::TypeDecl { .. }
            | ItemKind::EnumDecl { .. }
            | ItemKind::ActorDecl { .. }
            | ItemKind::TraitDecl { .. }
            | ItemKind::ImplBlock { .. }
    )
}

/// Return the rendered type string of the symbol covering (line, col),
/// or "" if no symbol covers that position. This is the minimal
/// `type_at` semantics the bootstrap query layer offers — full
/// expression-level positional typing remains a future expansion.
/// Read a file and return its contents as a String.
///
/// This is the FS-capability-gated kernel function for #325.
/// The self-hosted `query.gr::new_session_from_file` passes an `FS`
/// capability token to prove it has authority to perform file I/O;
/// the capability is erased at the ABI boundary, so this function
/// only receives the path.
///
/// Returns the file contents on success, or an empty string on error
/// (matching the safe-default pattern used by other bootstrap accessors).
pub fn bootstrap_query_read_file(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

pub fn bootstrap_query_type_at(session_id: i64, line: i64, col: i64) -> String {
    let sym_idx = bootstrap_query_symbol_at(session_id, line, col);
    if sym_idx < 0 {
        return String::new();
    }
    bootstrap_query_symbol_type(session_id, sym_idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_ir_bridge::shared_test_lock;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        shared_test_lock()
    }

    fn reset() {
        reset_query_store();
    }

    #[test]
    fn empty_source_yields_empty_session() {
        let _g = lock();
        reset();
        let id = bootstrap_query_new_session("");
        assert!(id > 0);
        assert_eq!(bootstrap_query_diagnostic_count(id), 0);
        assert_eq!(bootstrap_query_symbol_count(id), 0);
        assert_eq!(bootstrap_query_check_ok(id), 0); // not type-checked
    }

    #[test]
    fn happy_path_session_reports_symbols() {
        let _g = lock();
        reset();
        let src = "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n";
        let id = bootstrap_query_new_session(src);
        assert_eq!(bootstrap_query_check_ok(id), 1);
        assert_eq!(bootstrap_query_error_count(id), 0);
        assert_eq!(bootstrap_query_symbol_count(id), 1);
        assert_eq!(bootstrap_query_symbol_name(id, 0), "add");
        assert_eq!(bootstrap_query_symbol_kind(id, 0), SYMBOL_KIND_FUNCTION);
        let ty = bootstrap_query_symbol_type(id, 0);
        assert!(ty.contains("Int"), "type render: {}", ty);
        assert_eq!(bootstrap_query_symbol_param_count(id, 0), 2);
        assert_eq!(bootstrap_query_symbol_param_name(id, 0, 0), "x");
        assert_eq!(bootstrap_query_symbol_param_type(id, 0, 0), "Int");
        assert_eq!(bootstrap_query_symbol_is_pure(id, 0), 1);
    }

    #[test]
    fn parse_error_surfaces_diagnostic() {
        let _g = lock();
        reset();
        let bad = "fn broken(x: Int) -> Int:\n    ret x +\n";
        let id = bootstrap_query_new_session(bad);
        assert!(bootstrap_query_parse_error_count(id) > 0);
        assert_eq!(bootstrap_query_check_ok(id), 0);
        assert!(bootstrap_query_diagnostic_count(id) > 0);
        assert_eq!(bootstrap_query_diagnostic_phase(id, 0), PHASE_PARSER);
        assert_eq!(bootstrap_query_diagnostic_severity(id, 0), SEVERITY_ERROR);
        let msg = bootstrap_query_diagnostic_message(id, 0);
        assert!(!msg.is_empty());
    }

    #[test]
    fn type_error_surfaces_typechecker_diagnostic() {
        let _g = lock();
        reset();
        let bad = "fn f(x: Int) -> Int:\n    ret bogus\n";
        let id = bootstrap_query_new_session(bad);
        assert_eq!(bootstrap_query_parse_error_count(id), 0);
        assert!(bootstrap_query_type_error_count(id) > 0);
        assert!(bootstrap_query_diagnostic_count(id) > 0);
        let phase = bootstrap_query_diagnostic_phase(id, 0);
        assert_eq!(phase, PHASE_TYPECHECKER);
        let msg = bootstrap_query_diagnostic_message(id, 0);
        assert!(msg.contains("bogus") || msg.to_lowercase().contains("undefined"));
    }

    #[test]
    fn unknown_session_returns_safe_defaults() {
        let _g = lock();
        reset();
        assert_eq!(bootstrap_query_diagnostic_count(99999), 0);
        assert_eq!(bootstrap_query_symbol_count(99999), 0);
        assert_eq!(bootstrap_query_check_ok(99999), 0);
        assert_eq!(bootstrap_query_symbol_name(99999, 0), "");
        assert_eq!(bootstrap_query_find_symbol(99999, "x"), -1);
        assert_eq!(bootstrap_query_type_at(99999, 1, 1), "");
    }

    #[test]
    fn extern_function_marked_extern() {
        let _g = lock();
        reset();
        let src = "extern fn print(s: String)\nfn main():\n    print(\"hi\")\n";
        let id = bootstrap_query_new_session(src);
        let print_idx = bootstrap_query_find_symbol(id, "print");
        assert!(print_idx >= 0, "expected to find print symbol");
        assert_eq!(bootstrap_query_symbol_is_extern(id, print_idx), 1);
        assert_eq!(
            bootstrap_query_symbol_kind(id, print_idx),
            SYMBOL_KIND_EXTERN_FUNCTION
        );
    }

    #[test]
    fn type_at_returns_function_type_string() {
        let _g = lock();
        reset();
        let src = "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n";
        let id = bootstrap_query_new_session(src);
        // Position cursor on the `fn` keyword, line 1 col 1.
        let ty = bootstrap_query_type_at(id, 1, 1);
        assert!(
            ty.contains("Int"),
            "type_at on first line of `add` should resolve to its function type, got {:?}",
            ty
        );
    }
}
