//! Issue #233: LSP service kernel for the self-hosted compiler.
//!
//! `compiler/lsp.gr` historically returned placeholder data: `did_open`,
//! `did_change`, `did_close`, `did_save` returned the server unchanged;
//! `hover` returned an empty string; `completion`, `document_symbol`,
//! `run_diagnostics` returned `0`; the document store was a no-op.
//!
//! This module replaces those stubs with a runtime-backed LSP kernel
//! that:
//!
//! 1. Stores documents in a per-server `Mutex<LspStore>` keyed by URI.
//! 2. Re-runs the bootstrap query kernel (`bootstrap_query_*`) every
//!    time a document changes, so `hover`, `document_symbol`, and
//!    `run_diagnostics` return real data driven by the actual lexer /
//!    parser / typechecker.
//! 3. Exposes integer-handle accessors so the .gr-side `lsp.gr` can
//!    later delegate via `Int -> Int` and `(Int, Int, Int) -> String`
//!    extern calls without ever passing complex types across the FFI.
//!
//! ## Three-tier kernel boundary (from #228/#229/#230/#231/#232)
//!
//! 1. **Runtime store**: process-wide `Mutex<LspStore>` mapping URI ->
//!    document state (text, version, language id, last query session id).
//! 2. **Rust adapter**: shapes the LSP-side view: server creation, doc
//!    open/change/close/save, diagnostic count + per-index range data,
//!    hover (Markdown), completion (label+kind+detail), document
//!    symbols (LSP symbol kinds), all with integer handles.
//! 3. **CI gate**: `tests/self_hosted_lsp.rs` drives this kernel through
//!    fixtures that exercise open->diagnostics, hover on identifiers,
//!    document symbols for bootstrap fixtures, completion for keywords
//!    and builtins, and didChange invalidation.
//!
//! ## Wire shapes
//!
//! LSP severity codes (match `lsp.gr::LspDiagnosticSeverity`, also
//! match the LSP protocol):
//!   1 = error, 2 = warning, 3 = information, 4 = hint
//!
//! LSP symbol kinds (subset; match `lsp.gr::LspSymbolKind`):
//!   12 = function, 5 = class (used for actor), 11 = interface (trait),
//!   13 = variable, 25 = type-parameter (used for type alias)
//!
//! Completion item kinds (match `lsp.gr::CompletionItemKind`):
//!   1 = text, 2 = method, 3 = function, 4 = constructor,
//!   5 = field, 6 = variable, 7 = class, 8 = interface,
//!   9 = module, 10 = property, 11 = unit, 12 = value,
//!   13 = enum, 14 = keyword, 15 = snippet, ... (LSP standard).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::bootstrap_query::{
    bootstrap_query_diagnostic_col, bootstrap_query_diagnostic_count,
    bootstrap_query_diagnostic_message, bootstrap_query_diagnostic_severity,
    bootstrap_query_find_symbol, bootstrap_query_new_session, bootstrap_query_symbol_col,
    bootstrap_query_symbol_count, bootstrap_query_symbol_kind, bootstrap_query_symbol_line,
    bootstrap_query_symbol_name, bootstrap_query_symbol_type, bootstrap_query_type_at,
    SEVERITY_WARNING, SYMBOL_KIND_ACTOR, SYMBOL_KIND_EXTERN_FUNCTION, SYMBOL_KIND_FUNCTION,
    SYMBOL_KIND_IMPL, SYMBOL_KIND_TRAIT, SYMBOL_KIND_TYPE_ALIAS,
};

// ── LSP wire-format constants ────────────────────────────────────────────

pub const LSP_SEVERITY_ERROR: i64 = 1;
pub const LSP_SEVERITY_WARNING: i64 = 2;
#[allow(dead_code)]
pub const LSP_SEVERITY_INFO: i64 = 3;
#[allow(dead_code)]
pub const LSP_SEVERITY_HINT: i64 = 4;

// LSP symbol kinds (LSP standard numbering). Only those we emit.
pub const LSP_SYMBOL_FUNCTION: i64 = 12;
pub const LSP_SYMBOL_CLASS: i64 = 5; // used for actor
pub const LSP_SYMBOL_INTERFACE: i64 = 11; // used for trait
pub const LSP_SYMBOL_TYPE_PARAMETER: i64 = 26; // used for type-alias / enum decl
pub const LSP_SYMBOL_VARIABLE: i64 = 13;
pub const LSP_SYMBOL_OBJECT: i64 = 19; // used for impl block

// Completion item kinds (LSP standard).
pub const COMPLETION_KIND_FUNCTION: i64 = 3;
pub const COMPLETION_KIND_KEYWORD: i64 = 14;
#[allow(dead_code)]
pub const COMPLETION_KIND_VARIABLE: i64 = 6;

// ── Per-server document store ────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct DocumentEntry {
    #[allow(dead_code)]
    uri: String,
    version: i64,
    text: String,
    #[allow(dead_code)]
    language_id: String,
    /// Cached query session id from the last lex/parse/check run.
    /// `0` means no session yet.
    query_session: i64,
}

#[derive(Debug, Default)]
struct LspServerState {
    initialized: bool,
    documents: HashMap<String, DocumentEntry>,
}

#[derive(Default, Debug)]
struct LspStore {
    servers: Vec<LspServerState>,
}

impl LspStore {
    fn alloc(&mut self) -> i64 {
        let id = (self.servers.len() as i64) + 1;
        self.servers.push(LspServerState::default());
        id
    }

    fn get(&self, id: i64) -> Option<&LspServerState> {
        if id <= 0 {
            return None;
        }
        self.servers.get((id as usize) - 1)
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut LspServerState> {
        if id <= 0 {
            return None;
        }
        self.servers.get_mut((id as usize) - 1)
    }
}

fn store() -> &'static Mutex<LspStore> {
    static STORE: OnceLock<Mutex<LspStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(LspStore::default()))
}

fn with_store<R>(f: impl FnOnce(&mut LspStore) -> R) -> R {
    let mut s = store().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut s)
}

/// Reset the LSP server table. Test-only.
pub fn reset_lsp_store() {
    with_store(|s| s.servers.clear());
}

// ── Built-in vocabularies for completion ────────────────────────────────

const KEYWORDS: &[&str] = &[
    "fn", "let", "if", "else", "for", "in", "ret", "type", "enum", "match", "actor", "state", "on",
    "as", "extern", "pub", "comptime", "trait", "impl", "mod", "use", "true", "false",
];

const BUILTINS: &[(&str, &str)] = &[
    ("print", "fn print(s: String)"),
    ("println", "fn println(s: String)"),
    ("range", "fn range(start: Int, end: Int) -> List[Int]"),
    ("abs", "fn abs(x: Int) -> Int"),
    ("min", "fn min(a: Int, b: Int) -> Int"),
    ("max", "fn max(a: Int, b: Int) -> Int"),
];

// ── Server lifecycle ────────────────────────────────────────────────────

/// Allocate a new LSP server and return its id (>= 1).
pub fn bootstrap_lsp_new_server() -> i64 {
    with_store(|s| s.alloc())
}

/// Mark the server as initialized (called from `lsp.gr::initialize`).
pub fn bootstrap_lsp_initialize(server_id: i64) -> i64 {
    with_store(|s| {
        if let Some(srv) = s.get_mut(server_id) {
            srv.initialized = true;
            1
        } else {
            0
        }
    })
}

/// Whether the server has received an `initialize` call yet.
pub fn bootstrap_lsp_is_initialized(server_id: i64) -> i64 {
    with_store(|s| {
        s.get(server_id)
            .map(|srv| if srv.initialized { 1 } else { 0 })
            .unwrap_or(0)
    })
}

// ── Document lifecycle ──────────────────────────────────────────────────

/// `textDocument/didOpen` analog. Stores the document and runs an
/// initial query session. Returns `1` on success, `0` if the server
/// id is unknown.
pub fn bootstrap_lsp_did_open(
    server_id: i64,
    uri: &str,
    language_id: &str,
    version: i64,
    text: &str,
) -> i64 {
    let session = bootstrap_query_new_session(text);
    with_store(|s| {
        if let Some(srv) = s.get_mut(server_id) {
            srv.documents.insert(
                uri.to_string(),
                DocumentEntry {
                    uri: uri.to_string(),
                    version,
                    text: text.to_string(),
                    language_id: language_id.to_string(),
                    query_session: session,
                },
            );
            1
        } else {
            0
        }
    })
}

/// `textDocument/didChange` analog (full-text sync). Replaces the
/// document text, bumps the version, and re-runs the query session
/// so subsequent diagnostic / hover / symbol queries reflect the new
/// content.
pub fn bootstrap_lsp_did_change(server_id: i64, uri: &str, version: i64, new_text: &str) -> i64 {
    let session = bootstrap_query_new_session(new_text);
    with_store(|s| {
        if let Some(srv) = s.get_mut(server_id) {
            if let Some(doc) = srv.documents.get_mut(uri) {
                doc.text = new_text.to_string();
                doc.version = version;
                doc.query_session = session;
                return 1;
            }
        }
        0
    })
}

/// `textDocument/didClose` analog.
pub fn bootstrap_lsp_did_close(server_id: i64, uri: &str) -> i64 {
    with_store(|s| {
        if let Some(srv) = s.get_mut(server_id) {
            if srv.documents.remove(uri).is_some() {
                return 1;
            }
        }
        0
    })
}

/// `textDocument/didSave` analog. If `text` is empty we keep the
/// existing content (matching the LSP spec where save text is
/// optional); otherwise we replace and re-query.
pub fn bootstrap_lsp_did_save(server_id: i64, uri: &str, text: &str) -> i64 {
    if text.is_empty() {
        // Just confirm the document exists.
        with_store(|s| {
            s.get(server_id)
                .map(|srv| {
                    if srv.documents.contains_key(uri) {
                        1
                    } else {
                        0
                    }
                })
                .unwrap_or(0)
        })
    } else {
        let session = bootstrap_query_new_session(text);
        with_store(|s| {
            if let Some(srv) = s.get_mut(server_id) {
                if let Some(doc) = srv.documents.get_mut(uri) {
                    doc.text = text.to_string();
                    doc.query_session = session;
                    return 1;
                }
            }
            0
        })
    }
}

/// Number of documents currently open on the server.
pub fn bootstrap_lsp_document_count(server_id: i64) -> i64 {
    with_store(|s| {
        s.get(server_id)
            .map(|srv| srv.documents.len() as i64)
            .unwrap_or(0)
    })
}

/// Document text by URI ("" if not open).
pub fn bootstrap_lsp_document_text(server_id: i64, uri: &str) -> String {
    with_store(|s| {
        s.get(server_id)
            .and_then(|srv| srv.documents.get(uri))
            .map(|doc| doc.text.clone())
            .unwrap_or_default()
    })
}

/// Document version by URI (-1 if not open — versions can be 0).
pub fn bootstrap_lsp_document_version(server_id: i64, uri: &str) -> i64 {
    with_store(|s| {
        s.get(server_id)
            .and_then(|srv| srv.documents.get(uri))
            .map(|doc| doc.version)
            .unwrap_or(-1)
    })
}

/// Underlying query session id for the document, or `0` if none.
/// Useful for advanced .gr-side callers that want to chain into
/// `bootstrap_query_*` directly.
pub fn bootstrap_lsp_document_session(server_id: i64, uri: &str) -> i64 {
    with_store(|s| {
        s.get(server_id)
            .and_then(|srv| srv.documents.get(uri))
            .map(|doc| doc.query_session)
            .unwrap_or(0)
    })
}

// ── Diagnostics ─────────────────────────────────────────────────────────

fn doc_session(server_id: i64, uri: &str) -> i64 {
    with_store(|s| {
        s.get(server_id)
            .and_then(|srv| srv.documents.get(uri))
            .map(|doc| doc.query_session)
            .unwrap_or(0)
    })
}

/// Number of diagnostics published by the document's last
/// open/change/save.
pub fn bootstrap_lsp_diagnostic_count(server_id: i64, uri: &str) -> i64 {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return 0;
    }
    bootstrap_query_diagnostic_count(s)
}

/// LSP severity for the diagnostic at `index`. `0` for invalid index.
pub fn bootstrap_lsp_diagnostic_severity(server_id: i64, uri: &str, index: i64) -> i64 {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return 0;
    }
    let qsev = bootstrap_query_diagnostic_severity(s, index);
    if qsev == SEVERITY_WARNING {
        LSP_SEVERITY_WARNING
    } else if qsev == 0 {
        0
    } else {
        LSP_SEVERITY_ERROR
    }
}

pub fn bootstrap_lsp_diagnostic_message(server_id: i64, uri: &str, index: i64) -> String {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return String::new();
    }
    bootstrap_query_diagnostic_message(s, index)
}

/// LSP uses 0-based line/character. The query layer surfaces 1-based
/// line/col, so we subtract 1 for the LSP wire shape (clamped to 0).
pub fn bootstrap_lsp_diagnostic_line(server_id: i64, uri: &str, index: i64) -> i64 {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return 0;
    }
    let l = crate::bootstrap_query::bootstrap_query_diagnostic_line(s, index);
    if l > 0 {
        l - 1
    } else {
        0
    }
}

pub fn bootstrap_lsp_diagnostic_character(server_id: i64, uri: &str, index: i64) -> i64 {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return 0;
    }
    let c = bootstrap_query_diagnostic_col(s, index);
    if c > 0 {
        c - 1
    } else {
        0
    }
}

// ── Document symbols ────────────────────────────────────────────────────

fn lsp_symbol_kind_for(query_kind: i64) -> i64 {
    if query_kind == SYMBOL_KIND_FUNCTION || query_kind == SYMBOL_KIND_EXTERN_FUNCTION {
        LSP_SYMBOL_FUNCTION
    } else if query_kind == SYMBOL_KIND_TYPE_ALIAS {
        LSP_SYMBOL_TYPE_PARAMETER
    } else if query_kind == SYMBOL_KIND_ACTOR {
        LSP_SYMBOL_CLASS
    } else if query_kind == SYMBOL_KIND_TRAIT {
        LSP_SYMBOL_INTERFACE
    } else if query_kind == SYMBOL_KIND_IMPL {
        LSP_SYMBOL_OBJECT
    } else {
        LSP_SYMBOL_VARIABLE
    }
}

pub fn bootstrap_lsp_document_symbol_count(server_id: i64, uri: &str) -> i64 {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return 0;
    }
    bootstrap_query_symbol_count(s)
}

pub fn bootstrap_lsp_document_symbol_name(server_id: i64, uri: &str, index: i64) -> String {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return String::new();
    }
    bootstrap_query_symbol_name(s, index)
}

pub fn bootstrap_lsp_document_symbol_kind(server_id: i64, uri: &str, index: i64) -> i64 {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return 0;
    }
    let qkind = bootstrap_query_symbol_kind(s, index);
    lsp_symbol_kind_for(qkind)
}

/// 0-based line of the symbol's declaration.
pub fn bootstrap_lsp_document_symbol_line(server_id: i64, uri: &str, index: i64) -> i64 {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return 0;
    }
    let l = bootstrap_query_symbol_line(s, index);
    if l > 0 {
        l - 1
    } else {
        0
    }
}

pub fn bootstrap_lsp_document_symbol_character(server_id: i64, uri: &str, index: i64) -> i64 {
    let s = doc_session(server_id, uri);
    if s <= 0 {
        return 0;
    }
    let c = bootstrap_query_symbol_col(s, index);
    if c > 0 {
        c - 1
    } else {
        0
    }
}

// ── Hover ───────────────────────────────────────────────────────────────

fn extract_word_at(text: &str, line0: i64, char0: i64) -> String {
    if line0 < 0 || char0 < 0 {
        return String::new();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let line_idx = line0 as usize;
    if line_idx >= lines.len() {
        return String::new();
    }
    let line = lines[line_idx];
    let bytes = line.as_bytes();
    let mut start = char0 as usize;
    if start > bytes.len() {
        return String::new();
    }
    // Move start backwards while char is alphanumeric / _
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = char0 as usize;
    while end < bytes.len() && is_ident_byte(bytes[end]) {
        end += 1;
    }
    if start == end {
        return String::new();
    }
    String::from_utf8_lossy(&bytes[start..end]).to_string()
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Markdown-formatted hover content for the position. Returns "" if
/// nothing is hoverable. LSP positions are 0-based.
pub fn bootstrap_lsp_hover(server_id: i64, uri: &str, line0: i64, char0: i64) -> String {
    let (text, session) = with_store(|s| {
        s.get(server_id)
            .and_then(|srv| srv.documents.get(uri))
            .map(|doc| (doc.text.clone(), doc.query_session))
            .unwrap_or_default()
    });
    if session <= 0 {
        return String::new();
    }
    let word = extract_word_at(&text, line0, char0);
    if word.is_empty() {
        // Fall back to top-level symbol covering the position. The query
        // layer is 1-based.
        let ty = bootstrap_query_type_at(session, line0 + 1, char0 + 1);
        if ty.is_empty() {
            return String::new();
        }
        return format!("```gradient\n{}\n```", ty);
    }

    // Try resolving via top-level symbol named `word`.
    let idx = bootstrap_query_find_symbol(session, &word);
    if idx >= 0 {
        let kind = bootstrap_query_symbol_kind(session, idx);
        let ty = bootstrap_query_symbol_type(session, idx);
        let kind_label = match kind {
            k if k == SYMBOL_KIND_FUNCTION => "fn",
            k if k == SYMBOL_KIND_EXTERN_FUNCTION => "extern fn",
            k if k == SYMBOL_KIND_TYPE_ALIAS => "type",
            k if k == SYMBOL_KIND_ACTOR => "actor",
            k if k == SYMBOL_KIND_TRAIT => "trait",
            k if k == SYMBOL_KIND_IMPL => "impl",
            _ => "",
        };
        if kind_label.is_empty() {
            return format!("```gradient\n{}: {}\n```", word, ty);
        }
        return format!("```gradient\n{} {}: {}\n```", kind_label, word, ty);
    }

    // Fallback: builtin?
    for (n, sig) in BUILTINS {
        if *n == word {
            return format!("```gradient\n{}\n```\n*built-in function*", sig);
        }
    }
    // Fallback: keyword?
    if KEYWORDS.contains(&word.as_str()) {
        return format!("```gradient\n{}\n```\n*keyword*", word);
    }
    String::new()
}

// ── Goto Definition ─────────────────────────────────────────────────────

/// Resolve `(line0, char0)` to the LSP range of the symbol's definition.
/// Returns `-1` from each accessor when no definition is resolvable
/// (unknown server, missing document, no identifier at position, builtin
/// or keyword, no top-level symbol matching the word).
///
/// LSP positions are 0-based; `bootstrap_query_symbol_line/col` are
/// 1-based, so the kernel translates internally. End position is the
/// start position offset by the word length on the same line.
fn resolve_definition(
    server_id: i64,
    uri: &str,
    line0: i64,
    char0: i64,
) -> Option<(i64, i64, i64, i64)> {
    let (text, session) = with_store(|s| {
        s.get(server_id)
            .and_then(|srv| srv.documents.get(uri))
            .map(|doc| (doc.text.clone(), doc.query_session))
            .unwrap_or_default()
    });
    if session <= 0 {
        return None;
    }
    let word = extract_word_at(&text, line0, char0);
    if word.is_empty() {
        return None;
    }
    let idx = bootstrap_query_find_symbol(session, &word);
    if idx < 0 {
        return None;
    }
    let line1 = bootstrap_query_symbol_line(session, idx);
    let col1 = bootstrap_query_symbol_col(session, idx);
    if line1 <= 0 || col1 <= 0 {
        return None;
    }
    // The query layer reports the column of the item declaration (e.g. the
    // `fn` keyword for a function), not the identifier itself. Locate the
    // identifier within the source line starting at the reported column so
    // the LSP range highlights the symbol name, not the `fn`/`extern` lead.
    let lines: Vec<&str> = text.split('\n').collect();
    let line_idx = (line1 - 1) as usize;
    let (start_line, start_char) = if let Some(line_text) = lines.get(line_idx) {
        let from = (col1 - 1).max(0) as usize;
        if from <= line_text.len() {
            if let Some(rel) = line_text[from..].find(&word) {
                ((line1 - 1), (from + rel) as i64)
            } else {
                ((line1 - 1), col1 - 1)
            }
        } else {
            ((line1 - 1), col1 - 1)
        }
    } else {
        ((line1 - 1), col1 - 1)
    };
    let end_line = start_line;
    let end_char = start_char + word.chars().count() as i64;
    Some((start_line, start_char, end_line, end_char))
}

pub fn bootstrap_lsp_goto_definition_start_line(
    server_id: i64,
    uri: &str,
    line0: i64,
    char0: i64,
) -> i64 {
    resolve_definition(server_id, uri, line0, char0)
        .map(|(s, _, _, _)| s)
        .unwrap_or(-1)
}

pub fn bootstrap_lsp_goto_definition_start_character(
    server_id: i64,
    uri: &str,
    line0: i64,
    char0: i64,
) -> i64 {
    resolve_definition(server_id, uri, line0, char0)
        .map(|(_, s, _, _)| s)
        .unwrap_or(-1)
}

pub fn bootstrap_lsp_goto_definition_end_line(
    server_id: i64,
    uri: &str,
    line0: i64,
    char0: i64,
) -> i64 {
    resolve_definition(server_id, uri, line0, char0)
        .map(|(_, _, e, _)| e)
        .unwrap_or(-1)
}

pub fn bootstrap_lsp_goto_definition_end_character(
    server_id: i64,
    uri: &str,
    line0: i64,
    char0: i64,
) -> i64 {
    resolve_definition(server_id, uri, line0, char0)
        .map(|(_, _, _, e)| e)
        .unwrap_or(-1)
}

// ── Completion ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CompletionItem {
    label: String,
    kind: i64,
    detail: String,
}

/// Completion at position. For now we return a fixed-ish list:
///   - All top-level symbols defined in the document
///   - Keywords
///   - Builtins
///
/// The LSP server is expected to filter client-side with the prefix.
/// The .gr-side `completion(server, uri, position)` will read the
/// list count + per-index accessors via the externs below.
pub fn bootstrap_lsp_completion_count(server_id: i64, uri: &str) -> i64 {
    build_completion_list(server_id, uri).len() as i64
}

pub fn bootstrap_lsp_completion_label(server_id: i64, uri: &str, index: i64) -> String {
    build_completion_list(server_id, uri)
        .get(index as usize)
        .map(|c| c.label.clone())
        .unwrap_or_default()
}

pub fn bootstrap_lsp_completion_kind(server_id: i64, uri: &str, index: i64) -> i64 {
    build_completion_list(server_id, uri)
        .get(index as usize)
        .map(|c| c.kind)
        .unwrap_or(0)
}

pub fn bootstrap_lsp_completion_detail(server_id: i64, uri: &str, index: i64) -> String {
    build_completion_list(server_id, uri)
        .get(index as usize)
        .map(|c| c.detail.clone())
        .unwrap_or_default()
}

fn build_completion_list(server_id: i64, uri: &str) -> Vec<CompletionItem> {
    let mut out = Vec::new();
    // Bail if the server / document is unknown — no completions.
    let exists = with_store(|s| {
        s.get(server_id)
            .map(|srv| srv.documents.contains_key(uri))
            .unwrap_or(false)
    });
    if !exists {
        return out;
    }
    let session = doc_session(server_id, uri);
    if session > 0 {
        let n = bootstrap_query_symbol_count(session);
        for i in 0..n {
            let name = bootstrap_query_symbol_name(session, i);
            let kind = bootstrap_query_symbol_kind(session, i);
            let ty = bootstrap_query_symbol_type(session, i);
            let lsp_kind = if kind == SYMBOL_KIND_FUNCTION || kind == SYMBOL_KIND_EXTERN_FUNCTION {
                COMPLETION_KIND_FUNCTION
            } else {
                COMPLETION_KIND_VARIABLE
            };
            out.push(CompletionItem {
                label: name,
                kind: lsp_kind,
                detail: ty,
            });
        }
    }
    for kw in KEYWORDS {
        out.push(CompletionItem {
            label: kw.to_string(),
            kind: COMPLETION_KIND_KEYWORD,
            detail: "keyword".to_string(),
        });
    }
    for (n, sig) in BUILTINS {
        out.push(CompletionItem {
            label: n.to_string(),
            kind: COMPLETION_KIND_FUNCTION,
            detail: sig.to_string(),
        });
    }
    out
}

// ── Vocabulary helpers (called from lsp.gr::is_keyword / is_builtin) ────

pub fn bootstrap_lsp_is_keyword(word: &str) -> i64 {
    if KEYWORDS.contains(&word) {
        1
    } else {
        0
    }
}

pub fn bootstrap_lsp_is_builtin(word: &str) -> i64 {
    if BUILTINS.iter().any(|(n, _)| *n == word) {
        1
    } else {
        0
    }
}

pub fn bootstrap_lsp_keyword_count() -> i64 {
    KEYWORDS.len() as i64
}

pub fn bootstrap_lsp_keyword_at(index: i64) -> String {
    KEYWORDS
        .get(index as usize)
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub fn bootstrap_lsp_builtin_count() -> i64 {
    BUILTINS.len() as i64
}

pub fn bootstrap_lsp_builtin_name_at(index: i64) -> String {
    BUILTINS
        .get(index as usize)
        .map(|(n, _)| n.to_string())
        .unwrap_or_default()
}

pub fn bootstrap_lsp_builtin_signature_at(index: i64) -> String {
    BUILTINS
        .get(index as usize)
        .map(|(_, s)| s.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_ir_bridge::shared_test_lock;
    use crate::bootstrap_query::reset_query_store;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        shared_test_lock()
    }

    fn reset() {
        reset_lsp_store();
        reset_query_store();
    }

    #[test]
    fn server_init_flow() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        assert!(s > 0);
        assert_eq!(bootstrap_lsp_is_initialized(s), 0);
        assert_eq!(bootstrap_lsp_initialize(s), 1);
        assert_eq!(bootstrap_lsp_is_initialized(s), 1);
    }

    #[test]
    fn open_change_close_lifecycle() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///x.gr";
        assert_eq!(
            bootstrap_lsp_did_open(s, uri, "gradient", 1, "fn f() -> Int:\n    ret 0\n"),
            1
        );
        assert_eq!(bootstrap_lsp_document_count(s), 1);
        assert_eq!(bootstrap_lsp_document_version(s, uri), 1);
        assert_eq!(
            bootstrap_lsp_did_change(s, uri, 2, "fn g() -> Int:\n    ret 1\n"),
            1
        );
        assert_eq!(bootstrap_lsp_document_version(s, uri), 2);
        let txt = bootstrap_lsp_document_text(s, uri);
        assert!(txt.contains("fn g"));
        assert_eq!(bootstrap_lsp_did_close(s, uri), 1);
        assert_eq!(bootstrap_lsp_document_count(s), 0);
    }

    #[test]
    fn diagnostics_reflect_real_parser_errors() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///broken.gr";
        bootstrap_lsp_did_open(
            s,
            uri,
            "gradient",
            1,
            "fn broken(x: Int) -> Int:\n    ret x +\n",
        );
        assert!(bootstrap_lsp_diagnostic_count(s, uri) > 0);
        assert_eq!(
            bootstrap_lsp_diagnostic_severity(s, uri, 0),
            LSP_SEVERITY_ERROR
        );
        let msg = bootstrap_lsp_diagnostic_message(s, uri, 0);
        assert!(!msg.is_empty());
    }

    #[test]
    fn document_symbols_track_real_functions() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///mod.gr";
        let src = "fn add(x: Int, y: Int) -> Int:\n    ret x + y\nfn negate(x: Int) -> Int:\n    ret 0 - x\n";
        bootstrap_lsp_did_open(s, uri, "gradient", 1, src);
        assert_eq!(bootstrap_lsp_document_symbol_count(s, uri), 2);
        assert_eq!(bootstrap_lsp_document_symbol_name(s, uri, 0), "add");
        assert_eq!(
            bootstrap_lsp_document_symbol_kind(s, uri, 0),
            LSP_SYMBOL_FUNCTION
        );
        // 0-based line for first definition is line 0.
        assert_eq!(bootstrap_lsp_document_symbol_line(s, uri, 0), 0);
    }

    #[test]
    fn hover_returns_function_signature() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///hov.gr";
        bootstrap_lsp_did_open(
            s,
            uri,
            "gradient",
            1,
            "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n",
        );
        // Hover on `add` (line 0, char 3 lands inside the identifier).
        let h = bootstrap_lsp_hover(s, uri, 0, 3);
        assert!(h.contains("add"), "hover content: {}", h);
        assert!(h.contains("Int"), "hover should mention Int: {}", h);
    }

    #[test]
    fn hover_on_keyword_returns_keyword_label() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///kw.gr";
        bootstrap_lsp_did_open(s, uri, "gradient", 1, "fn f() -> Int:\n    ret 0\n");
        // Hover on `fn` at line 0 char 0.
        let h = bootstrap_lsp_hover(s, uri, 0, 0);
        assert!(h.contains("fn"));
        assert!(h.contains("keyword"));
    }

    #[test]
    fn completion_lists_symbols_keywords_builtins() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///c.gr";
        bootstrap_lsp_did_open(
            s,
            uri,
            "gradient",
            1,
            "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n",
        );
        let n = bootstrap_lsp_completion_count(s, uri);
        assert!(
            n >= (1 + KEYWORDS.len() + BUILTINS.len()) as i64,
            "expected at least 1 symbol + all keywords + all builtins, got {}",
            n
        );
        // Must include `add` as a function-kind completion somewhere.
        let mut found_add = false;
        let mut found_fn = false;
        for i in 0..n {
            let label = bootstrap_lsp_completion_label(s, uri, i);
            if label == "add"
                && bootstrap_lsp_completion_kind(s, uri, i) == COMPLETION_KIND_FUNCTION
            {
                found_add = true;
            }
            if label == "fn" && bootstrap_lsp_completion_kind(s, uri, i) == COMPLETION_KIND_KEYWORD
            {
                found_fn = true;
            }
        }
        assert!(found_add);
        assert!(found_fn);
    }

    #[test]
    fn unknown_server_returns_safe_defaults() {
        let _g = lock();
        reset();
        assert_eq!(bootstrap_lsp_document_count(99999), 0);
        assert_eq!(bootstrap_lsp_diagnostic_count(99999, "x"), 0);
        assert_eq!(bootstrap_lsp_hover(99999, "x", 0, 0), "");
        assert_eq!(bootstrap_lsp_completion_count(99999, "x"), 0);
    }

    #[test]
    fn vocabulary_helpers_are_correct() {
        assert_eq!(bootstrap_lsp_is_keyword("fn"), 1);
        assert_eq!(bootstrap_lsp_is_keyword("not_a_keyword"), 0);
        assert_eq!(bootstrap_lsp_is_builtin("print"), 1);
        assert_eq!(bootstrap_lsp_is_builtin("add"), 0);
        assert!(bootstrap_lsp_keyword_count() > 0);
        assert!(bootstrap_lsp_builtin_count() > 0);
        let kw0 = bootstrap_lsp_keyword_at(0);
        assert!(KEYWORDS.contains(&kw0.as_str()));
    }

    #[test]
    fn goto_definition_resolves_to_declaration() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///gd.gr";
        // Line 0: fn add(x: Int, y: Int) -> Int:
        // Line 1:     ret x + y
        // Line 2: fn caller() -> Int:
        // Line 3:     ret add(1, 2)
        let src = "fn add(x: Int, y: Int) -> Int:\n    ret x + y\nfn caller() -> Int:\n    ret add(1, 2)\n";
        bootstrap_lsp_did_open(s, uri, "gradient", 1, src);
        // Cursor on the call site `add` at line 3, char 8 (inside identifier).
        let sl = bootstrap_lsp_goto_definition_start_line(s, uri, 3, 8);
        let sc = bootstrap_lsp_goto_definition_start_character(s, uri, 3, 8);
        let el = bootstrap_lsp_goto_definition_end_line(s, uri, 3, 8);
        let ec = bootstrap_lsp_goto_definition_end_character(s, uri, 3, 8);
        // Declaration site: `fn add(...)` — `add` starts at line 0 col 4 (1-based)
        // → 0-based line 0 char 3.
        assert_eq!(sl, 0, "start line");
        assert_eq!(sc, 3, "start char");
        assert_eq!(el, 0, "end line");
        assert_eq!(ec, 6, "end char (3 + len('add'))");
    }

    #[test]
    fn goto_definition_unknown_server_returns_minus_one() {
        let _g = lock();
        reset();
        assert_eq!(
            bootstrap_lsp_goto_definition_start_line(99999, "file:///x.gr", 0, 0),
            -1
        );
        assert_eq!(
            bootstrap_lsp_goto_definition_start_character(99999, "file:///x.gr", 0, 0),
            -1
        );
        assert_eq!(
            bootstrap_lsp_goto_definition_end_line(99999, "file:///x.gr", 0, 0),
            -1
        );
        assert_eq!(
            bootstrap_lsp_goto_definition_end_character(99999, "file:///x.gr", 0, 0),
            -1
        );
    }

    #[test]
    fn goto_definition_no_word_at_position_returns_minus_one() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///nw.gr";
        bootstrap_lsp_did_open(s, uri, "gradient", 1, "fn f() -> Int:\n    ret 0\n");
        // Position past end-of-line on line 1 (whitespace area)
        let sl = bootstrap_lsp_goto_definition_start_line(s, uri, 1, 100);
        assert_eq!(sl, -1);
    }

    #[test]
    fn goto_definition_builtin_or_keyword_returns_minus_one() {
        let _g = lock();
        reset();
        let s = bootstrap_lsp_new_server();
        let uri = "file:///bk.gr";
        // `print` is a builtin (no .gr-side declaration); `fn` is a keyword.
        let src = "fn f() -> Int:\n    ret 0\n";
        bootstrap_lsp_did_open(s, uri, "gradient", 1, src);
        // Hover on `fn` keyword (line 0 char 0) — find_symbol returns -1.
        let sl_kw = bootstrap_lsp_goto_definition_start_line(s, uri, 0, 0);
        assert_eq!(sl_kw, -1, "keyword should yield -1");
    }
}
