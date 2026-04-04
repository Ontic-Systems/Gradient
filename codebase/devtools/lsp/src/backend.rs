//! LSP backend for the Gradient programming language.
//!
//! Implements [`LanguageServer`] from `tower-lsp`, providing:
//! - **Diagnostics** on open / change / save (lex + parse + typecheck)
//! - **Hover** showing the type or signature of the identifier under the cursor
//! - **Completion** offering keywords and builtin function names
//! - **Custom `gradient/batchDiagnostics`** notification for agent consumption

use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::diagnostics;

// ── Builtin function signatures ──────────────────────────────────────────
// Kept in sync with `TypeEnv::preload_builtins` in the compiler.

const BUILTIN_FUNCTIONS: &[(&str, &str)] = &[
    ("print", "print(value: String) -> !{IO} ()"),
    ("println", "println(value: String) -> !{IO} ()"),
    ("print_int", "print_int(value: Int) -> !{IO} ()"),
    ("print_float", "print_float(value: Float) -> !{IO} ()"),
    ("print_bool", "print_bool(value: Bool) -> !{IO} ()"),
    ("range", "range(n: Int) -> Iterable"),
    ("to_string", "to_string(value: Int) -> String"),
    ("int_to_string", "int_to_string(value: Int) -> String"),
    ("abs", "abs(n: Int) -> Int"),
    ("min", "min(a: Int, b: Int) -> Int"),
    ("max", "max(a: Int, b: Int) -> Int"),
    ("mod_int", "mod_int(a: Int, b: Int) -> Int"),
];

/// All Gradient keywords in source-code form.
const KEYWORDS: &[&str] = &[
    "fn", "let", "if", "else", "for", "in", "ret", "type", "mod", "use", "match", "impl", "true",
    "false", "and", "or", "not",
];

// ── Custom notification types ────────────────────────────────────────────

/// Payload for the `gradient/batchDiagnostics` custom notification.
///
/// Provides all diagnostics for a file in a single message, along with counts
/// broken down by compiler phase. Useful for AI agents that prefer to receive
/// the full diagnostic picture in one shot.
#[derive(serde::Serialize, serde::Deserialize)]
struct BatchDiagnosticsResult {
    uri: String,
    diagnostics: Vec<Diagnostic>,
    lex_errors: usize,
    parse_errors: usize,
    type_errors: usize,
}

// ── Backend ──────────────────────────────────────────────────────────────

/// The Gradient LSP backend.
///
/// Holds the LSP client handle (for sending notifications back) and an
/// in-memory document store that caches the latest content of each open file.
pub struct Backend {
    /// The LSP client handle, used to publish diagnostics and log messages.
    client: Client,
    /// In-memory document store: maps document URIs to their latest content.
    documents: Mutex<HashMap<Url, String>>,
}

impl Backend {
    /// Create a new backend with the given LSP client.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Mutex::new(HashMap::new()),
        }
    }

    /// Run the compiler pipeline on a document and publish diagnostics.
    ///
    /// This is the core diagnostic loop: it lexes, parses, and type-checks
    /// the document, then converts all errors into LSP diagnostics and
    /// publishes them. It also sends the custom `gradient/batchDiagnostics`
    /// notification.
    async fn diagnose(&self, uri: Url, text: &str) {
        let result = diagnostics::run_diagnostics(text);

        // Publish standard LSP diagnostics.
        self.client
            .publish_diagnostics(uri.clone(), result.diagnostics.clone(), None)
            .await;

        // Send custom batch notification for agent consumers.
        let batch = BatchDiagnosticsResult {
            uri: uri.to_string(),
            diagnostics: result.diagnostics,
            lex_errors: result.lex_errors,
            parse_errors: result.parse_errors,
            type_errors: result.type_errors,
        };
        self.client
            .send_notification::<BatchDiagnosticsNotification>(batch)
            .await;
    }

    /// Find the type / signature of an identifier at a given position.
    ///
    /// For v0.1 this uses a simple word-extraction approach: extract the word
    /// under the cursor, then check if it matches a builtin function name or
    /// a keyword. A more sophisticated version would walk the typed AST.
    fn hover_info(&self, text: &str, position: Position) -> Option<String> {
        let word = word_at_position(text, position)?;

        // Check builtins first.
        for &(name, sig) in BUILTIN_FUNCTIONS {
            if word == name {
                return Some(format!("(builtin) {}", sig));
            }
        }

        // Check keywords.
        for &kw in KEYWORDS {
            if word == kw {
                return Some(format!("(keyword) {}", kw));
            }
        }

        // Try a lightweight parse + typecheck to resolve user-defined symbols.
        self.resolve_user_symbol(text, &word)
    }

    /// Attempt to resolve a user-defined symbol by parsing and type-checking
    /// the document, then searching for the name in the resulting AST.
    ///
    /// For v0.1 this handles top-level function definitions. It returns the
    /// reconstructed signature string if the name matches a defined function.
    fn resolve_user_symbol(&self, text: &str, name: &str) -> Option<String> {
        use gradient_compiler::ast::item::ItemKind;
        use gradient_compiler::lexer::Lexer;
        use gradient_compiler::parser::Parser;

        let mut lexer = Lexer::new(text, 0);
        let tokens = lexer.tokenize();
        let (module, _) = Parser::parse(tokens, 0);

        for item in &module.items {
            match &item.node {
                ItemKind::FnDef(fn_def) if fn_def.name == name => {
                    let params: Vec<String> = fn_def
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, format_type_expr(&p.type_ann.node)))
                        .collect();
                    let ret = fn_def
                        .return_type
                        .as_ref()
                        .map(|t| format!(" -> {}", format_type_expr(&t.node)))
                        .unwrap_or_default();
                    let effects = fn_def
                        .effects
                        .as_ref()
                        .map(|e| {
                            if e.effects.is_empty() {
                                String::new()
                            } else {
                                format!(" !{{{}}}", e.effects.join(", "))
                            }
                        })
                        .unwrap_or_default();
                    return Some(format!(
                        "fn {}({}){}{}",
                        fn_def.name,
                        params.join(", "),
                        effects,
                        ret
                    ));
                }
                ItemKind::ExternFn(decl) if decl.name == name => {
                    let params: Vec<String> = decl
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, format_type_expr(&p.type_ann.node)))
                        .collect();
                    let ret = decl
                        .return_type
                        .as_ref()
                        .map(|t| format!(" -> {}", format_type_expr(&t.node)))
                        .unwrap_or_default();
                    return Some(format!(
                        "(extern) fn {}({}){}",
                        decl.name,
                        params.join(", "),
                        ret
                    ));
                }
                ItemKind::Let {
                    name: let_name,
                    type_ann,
                    ..
                } if let_name == name => {
                    let ty = type_ann
                        .as_ref()
                        .map(|t| format!(": {}", format_type_expr(&t.node)))
                        .unwrap_or_else(|| ": <inferred>".to_string());
                    return Some(format!("let {}{}", let_name, ty));
                }
                _ => {}
            }
        }

        None
    }
}

// ── LanguageServer trait implementation ───────────────────────────────────

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string(), "!".to_string()]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "gradient-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Gradient LSP v0.1 initialized")
            .await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();
        self.documents
            .lock()
            .unwrap()
            .insert(uri.clone(), text.clone());
        self.diagnose(uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // We use TextDocumentSyncKind::FULL, so there is always exactly one
        // change event containing the full document text.
        if let Some(change) = params.content_changes.into_iter().last() {
            let uri = params.text_document.uri.clone();
            let text = change.text.clone();
            self.documents
                .lock()
                .unwrap()
                .insert(uri.clone(), text.clone());
            self.diagnose(uri, &text).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            self.diagnose(params.text_document.uri, &text).await;
        } else {
            // If the client didn't include the text, use our cached copy.
            let text = self
                .documents
                .lock()
                .unwrap()
                .get(&params.text_document.uri)
                .cloned();
            if let Some(text) = text {
                self.diagnose(params.text_document.uri, &text).await;
            }
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let text = self.documents.lock().unwrap().get(uri).cloned();

        let hover = text.and_then(|text| {
            self.hover_info(&text, position).map(|info| Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("```gradient\n{}\n```", info),
                }),
                range: None,
            })
        });

        Ok(hover)
    }

    async fn completion(&self, _: CompletionParams) -> Result<Option<CompletionResponse>> {
        let mut items = Vec::new();

        // Add keywords.
        for &kw in KEYWORDS {
            items.push(CompletionItem {
                label: kw.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some("keyword".to_string()),
                insert_text: Some(kw.to_string()),
                ..Default::default()
            });
        }

        // Add builtin functions with their signatures.
        for &(name, sig) in BUILTIN_FUNCTIONS {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(sig.to_string()),
                insert_text: Some(name.to_string()),
                ..Default::default()
            });
        }

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

// ── Custom notification type registration ────────────────────────────────

/// Custom LSP notification: `gradient/batchDiagnostics`.
///
/// Sends all diagnostics for a file in a single notification, including
/// per-phase error counts. Designed for AI agent consumers that want the
/// complete diagnostic picture without streaming.
struct BatchDiagnosticsNotification;

impl tower_lsp::lsp_types::notification::Notification for BatchDiagnosticsNotification {
    type Params = BatchDiagnosticsResult;
    const METHOD: &'static str = "gradient/batchDiagnostics";
}

// ── Text utilities ───────────────────────────────────────────────────────

/// Format a `TypeExpr` as a human-readable string.
///
/// The compiler's `TypeExpr` does not implement `Display`, so we provide
/// our own formatting for hover information.
fn format_type_expr(te: &gradient_compiler::ast::types::TypeExpr) -> String {
    use gradient_compiler::ast::types::TypeExpr;
    match te {
        TypeExpr::Named { name, cap } => {
            let cap_str = match cap {
                Some(c) => format!(" {}", c),
                None => String::new(),
            };
            format!("{}{}", name, cap_str)
        }
        TypeExpr::Unit => "()".to_string(),
        TypeExpr::Fn {
            params,
            ret,
            effects,
        } => {
            let param_strs: Vec<String> =
                params.iter().map(|p| format_type_expr(&p.node)).collect();
            let eff_str = match effects {
                Some(eff) if !eff.effects.is_empty() => {
                    format!(" !{{{}}}", eff.effects.join(", "))
                }
                _ => String::new(),
            };
            format!(
                "({}) ->{} {}",
                param_strs.join(", "),
                eff_str,
                format_type_expr(&ret.node)
            )
        }
        TypeExpr::Generic { name, args, cap } => {
            let arg_strs: Vec<String> = args.iter().map(|a| format_type_expr(&a.node)).collect();
            let cap_str = match cap {
                Some(c) => format!(" {}", c),
                None => String::new(),
            };
            format!("{}[{}]{}", name, arg_strs.join(", "), cap_str)
        }
        TypeExpr::Tuple(elems) => {
            let elem_strs: Vec<String> = elems.iter().map(|e| format_type_expr(&e.node)).collect();
            format!("({})", elem_strs.join(", "))
        }
        TypeExpr::Linear(_) => "Linear".to_string(),
        TypeExpr::Type => "type".to_string(),
    }
}

/// Extract the word (identifier-like sequence) at the given LSP position.
///
/// Returns `None` if the position is not within a word.
fn word_at_position(text: &str, position: Position) -> Option<String> {
    let line_idx = position.line as usize;
    let col_idx = position.character as usize;

    let line = text.lines().nth(line_idx)?;

    if col_idx > line.len() {
        return None;
    }

    // Find the start of the word (scan backwards from the cursor).
    let mut start = col_idx;
    while start > 0 {
        let ch = line.as_bytes().get(start - 1).copied()?;
        if ch.is_ascii_alphanumeric() || ch == b'_' {
            start -= 1;
        } else {
            break;
        }
    }

    // Find the end of the word (scan forwards from the cursor).
    let mut end = col_idx;
    while end < line.len() {
        let ch = line.as_bytes().get(end).copied()?;
        if ch.is_ascii_alphanumeric() || ch == b'_' {
            end += 1;
        } else {
            break;
        }
    }

    if start == end {
        return None;
    }

    Some(line[start..end].to_string())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_extraction_mid_word() {
        let text = "let foo = bar + baz\nprint(foo)";
        // "foo" starts at col 4, cursor at col 5 (inside "foo")
        let word = word_at_position(
            text,
            Position {
                line: 0,
                character: 5,
            },
        );
        assert_eq!(word, Some("foo".to_string()));
    }

    #[test]
    fn word_extraction_start_of_word() {
        let text = "let foo = bar";
        let word = word_at_position(
            text,
            Position {
                line: 0,
                character: 4,
            },
        );
        assert_eq!(word, Some("foo".to_string()));
    }

    #[test]
    fn word_extraction_on_operator() {
        let text = "a + b";
        let word = word_at_position(
            text,
            Position {
                line: 0,
                character: 2,
            },
        );
        assert_eq!(word, None);
    }

    #[test]
    fn word_extraction_second_line() {
        let text = "let x = 1\nprint_int(x)";
        let word = word_at_position(
            text,
            Position {
                line: 1,
                character: 3,
            },
        );
        assert_eq!(word, Some("print_int".to_string()));
    }

    #[test]
    fn word_extraction_empty_line() {
        let text = "let x = 1\n\nlet y = 2";
        let word = word_at_position(
            text,
            Position {
                line: 1,
                character: 0,
            },
        );
        assert_eq!(word, None);
    }
}
