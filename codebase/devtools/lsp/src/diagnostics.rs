//! Compiler-to-LSP diagnostic conversion.
//!
//! This module runs the Gradient compiler pipeline (lex -> parse -> typecheck)
//! on a source string and converts the resulting errors into LSP [`Diagnostic`]
//! structs suitable for publishing to the client.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use gradient_compiler::ast::span::Span;
use gradient_compiler::lexer::token::TokenKind;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser::error::ParseError;
use gradient_compiler::parser::Parser;
use gradient_compiler::typechecker;
use gradient_compiler::typechecker::TypeError;

/// Maximum time (in seconds) to wait for the parser to complete.
/// If the parser hangs (e.g. due to a known infinite-loop bug on certain
/// malformed inputs), we return the lex errors we already collected.
const PARSE_TIMEOUT_SECS: u64 = 5;

/// The result of running the full diagnostic pipeline on a source string.
pub struct DiagnosticResult {
    /// LSP diagnostics ready for publishing.
    pub diagnostics: Vec<Diagnostic>,
    /// How many errors came from the lexer.
    pub lex_errors: usize,
    /// How many errors came from the parser.
    pub parse_errors: usize,
    /// How many errors came from the type checker.
    pub type_errors: usize,
}

/// Run the Gradient compiler pipeline on `source` and collect all diagnostics.
///
/// The pipeline runs as far as possible:
///   1. Lexing always completes (error tokens are embedded in the token stream).
///   2. Parsing always completes (the parser recovers from errors).
///   3. Type checking runs only if there are no parse errors, since a broken
///      AST would produce misleading type errors.
pub fn run_diagnostics(source: &str) -> DiagnosticResult {
    let mut diagnostics = Vec::new();
    let mut lex_error_count = 0;

    // ── Step 1: Lex ──────────────────────────────────────────────────────
    let mut lexer = Lexer::new(source, 0);
    let tokens = lexer.tokenize();

    // Collect lexer errors (tokens with kind `Error`).
    for token in &tokens {
        if let TokenKind::Error(ref msg) = token.kind {
            lex_error_count += 1;
            diagnostics.push(Diagnostic {
                range: span_to_range(&token.span),
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("gradient-lexer".to_string()),
                message: msg.clone(),
                ..Default::default()
            });
        }
    }

    // ── Step 2: Parse (with timeout guard) ─────────────────────────────
    //
    // The parser has a known bug where certain malformed inputs (e.g.
    // `fn main()` without a colon) can cause an infinite loop. We run
    // parsing in a background thread with a timeout to keep the LSP
    // responsive.
    let (tx, rx) = mpsc::channel();
    let tokens_for_thread = tokens;
    thread::spawn(move || {
        let result = Parser::parse(tokens_for_thread, 0);
        let _ = tx.send(result);
    });

    let parse_result = rx.recv_timeout(Duration::from_secs(PARSE_TIMEOUT_SECS));

    match parse_result {
        Ok((module, parse_errors)) => {
            let parse_error_count = parse_errors.len();
            for err in &parse_errors {
                diagnostics.push(parse_error_to_diagnostic(err));
            }

            // ── Step 3: Type check (only if parsing succeeded) ───────
            let mut type_error_count = 0;
            if parse_errors.is_empty() {
                let type_errors = typechecker::check_module(&module, 0);
                type_error_count = type_errors.len();
                for err in &type_errors {
                    diagnostics.push(type_error_to_diagnostic(err));
                }
            }

            DiagnosticResult {
                diagnostics,
                lex_errors: lex_error_count,
                parse_errors: parse_error_count,
                type_errors: type_error_count,
            }
        }
        Err(_) => {
            // Parser timed out — likely hit an infinite loop. Report a
            // single diagnostic indicating the failure.
            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position { line: 0, character: 0 },
                    end: Position { line: 0, character: 0 },
                },
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("gradient-parser".to_string()),
                message: "parser timed out (possible syntax error causing infinite loop)".to_string(),
                ..Default::default()
            });

            DiagnosticResult {
                diagnostics,
                lex_errors: lex_error_count,
                parse_errors: 1,
                type_errors: 0,
            }
        }
    }
}

// ── Span conversion helpers ──────────────────────────────────────────────

/// Convert a compiler `Span` to an LSP `Range`.
///
/// The compiler uses 1-based line and column numbers; LSP uses 0-based.
fn span_to_range(span: &Span) -> Range {
    Range {
        start: Position {
            line: span.start.line.saturating_sub(1),
            character: span.start.col.saturating_sub(1),
        },
        end: Position {
            line: span.end.line.saturating_sub(1),
            character: span.end.col.saturating_sub(1),
        },
    }
}

// ── Error-to-diagnostic converters ───────────────────────────────────────

/// Convert a [`ParseError`] into an LSP [`Diagnostic`].
fn parse_error_to_diagnostic(err: &ParseError) -> Diagnostic {
    Diagnostic {
        range: span_to_range(&err.span),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("gradient-parser".to_string()),
        message: err.message.clone(),
        ..Default::default()
    }
}

/// Convert a [`TypeError`] into an LSP [`Diagnostic`].
fn type_error_to_diagnostic(err: &TypeError) -> Diagnostic {
    let mut message = err.message.clone();
    for note in &err.notes {
        message.push_str("\nnote: ");
        message.push_str(note);
    }

    Diagnostic {
        range: span_to_range(&err.span),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("gradient-typechecker".to_string()),
        message,
        ..Default::default()
    }
}
