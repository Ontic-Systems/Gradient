//! Compiler-to-LSP diagnostic conversion.
//!
//! This module runs the Gradient compiler pipeline (lex -> parse -> typecheck)
//! on a source string and converts the resulting errors into LSP [`Diagnostic`]
//! structs suitable for publishing to the client.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use gradient_compiler::ast::module::TrustMode;
use gradient_compiler::ast::span::Span;
use gradient_compiler::lexer::token::TokenKind;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser::error::ParseError;
use gradient_compiler::parser::Parser;
use gradient_compiler::typechecker;
use gradient_compiler::typechecker::TypeError;

/// Parse timeout in seconds. The parser should always terminate, but we
/// keep a timeout as a safety net against unforeseen edge cases.
const PARSE_TIMEOUT_SECS: u64 = 5;

/// Knobs that the LSP backend passes into the diagnostic pipeline.
///
/// The most important one is [`Self::default_untrusted`], which mirrors
/// the `gradient build --untrusted` source-mode flag for unsaved
/// editor buffers (#359, companion to #360 / PR #508).
#[derive(Debug, Clone, Copy, Default)]
pub struct DiagnosticOptions {
    /// When true, the LSP applies `@untrusted` mode by default to any
    /// document that did NOT explicitly set its trust posture via a
    /// top-of-file `@trusted` / `@untrusted` attribute. Documents that
    /// DO set the attribute always win — the default only applies in
    /// the unannotated case.
    pub default_untrusted: bool,
}

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
    /// The trust mode actually used to type-check this run. Useful for
    /// the LSP's `gradient/batchDiagnostics` consumer to confirm the
    /// default kicked in.
    pub trust_mode: TrustMode,
}

/// Run the Gradient compiler pipeline on `source` and collect all diagnostics.
///
/// The pipeline runs as far as possible:
///   1. Lexing always completes (error tokens are embedded in the token stream).
///   2. Parsing always completes (the parser recovers from errors).
///   3. Type checking runs only if there are no parse errors, since a broken
///      AST would produce misleading type errors.
///
/// When `opts.default_untrusted` is true and the source did NOT carry an
/// explicit `@trusted` or `@untrusted` annotation, the parsed module is
/// promoted to [`TrustMode::Untrusted`] before type checking. This is how
/// the LSP closes adversarial finding F4 (input-surface workspace default).
#[allow(dead_code)]
pub fn run_diagnostics(source: &str) -> DiagnosticResult {
    run_diagnostics_with(source, DiagnosticOptions::default())
}

/// Variant of [`run_diagnostics`] that takes explicit options. Most callers
/// should use [`run_diagnostics`]; the LSP backend uses this directly so it
/// can wire in the workspace `untrusted` config.
pub fn run_diagnostics_with(source: &str, opts: DiagnosticOptions) -> DiagnosticResult {
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
    // We run parsing in a background thread with a timeout as a safety
    // net, keeping the LSP responsive even if an unforeseen edge case
    // causes the parser to stall.
    let (tx, rx) = mpsc::channel();
    let tokens_for_thread = tokens;
    thread::spawn(move || {
        let result = Parser::parse(tokens_for_thread, 0);
        let _ = tx.send(result);
    });

    let parse_result = rx.recv_timeout(Duration::from_secs(PARSE_TIMEOUT_SECS));

    match parse_result {
        Ok((mut module, parse_errors)) => {
            let parse_error_count = parse_errors.len();
            for err in &parse_errors {
                diagnostics.push(parse_error_to_diagnostic(err));
            }

            // Apply the workspace default trust posture if the source
            // did not pin one explicitly. Documents that opt in via a
            // top-of-file `@trusted` / `@untrusted` attribute always win
            // — the default fills the gap for unannotated buffers.
            if opts.default_untrusted
                && module.trust == TrustMode::Trusted
                && !source_has_explicit_trust_annotation(source)
            {
                module.trust = TrustMode::Untrusted;
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
                trust_mode: module.trust,
            }
        }
        Err(_) => {
            // Parser timed out — likely hit an infinite loop. Report a
            // single diagnostic indicating the failure.
            diagnostics.push(Diagnostic {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 0,
                        character: 0,
                    },
                },
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("gradient-parser".to_string()),
                message: "parser timed out (possible syntax error causing infinite loop)"
                    .to_string(),
                ..Default::default()
            });

            DiagnosticResult {
                diagnostics,
                lex_errors: lex_error_count,
                parse_errors: 1,
                type_errors: 0,
                // We never got a module; report what we'd have used so
                // the consumer can still tell what budget was applied.
                trust_mode: if opts.default_untrusted {
                    TrustMode::Untrusted
                } else {
                    TrustMode::Trusted
                },
            }
        }
    }
}

/// Returns true iff `source` carries a top-of-file `@trusted` /
/// `@untrusted` attribute. Used to decide whether the LSP's
/// workspace-default `@untrusted` posture applies.
///
/// We intentionally do this with a small token-aware textual scan
/// rather than re-parsing: the parser already promoted `module.trust`
/// when it saw the attribute, but we cannot distinguish "explicit
/// `@trusted`" from "default `Trusted`" from the AST alone. Scanning
/// the source for the attribute literal lets us preserve user intent
/// without changing the AST shape.
///
/// The scan tolerates leading whitespace, comments, and blank lines —
/// matching the parser's own `skip_newlines()` / file-scope-attr loop
/// in `parse_program`.
fn source_has_explicit_trust_annotation(source: &str) -> bool {
    let mut chars = source.chars().peekable();
    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                chars.next();
            }
            '#' => {
                // Line comment — skip to end-of-line. Gradient also has
                // `//` comments, but `#` is the conservative skip
                // pattern for top-of-file workflow markers and is safe
                // to ignore. Falling through to '/' below covers the
                // real comment syntax.
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' => {
                // Look ahead for `//` line comment or `/*` block comment.
                let mut clone = chars.clone();
                clone.next(); // consume the first '/'
                match clone.peek() {
                    Some('/') => {
                        // Line comment — skip to EOL.
                        for c in chars.by_ref() {
                            if c == '\n' {
                                break;
                            }
                        }
                    }
                    Some('*') => {
                        // Block comment — skip until `*/`.
                        chars.next(); // '/'
                        chars.next(); // '*'
                        let mut prev = '\0';
                        for c in chars.by_ref() {
                            if prev == '*' && c == '/' {
                                break;
                            }
                            prev = c;
                        }
                    }
                    _ => return false, // an unrelated `/` — not a leading attribute.
                }
            }
            '@' => {
                // Found a top-of-file attribute. Read the identifier.
                chars.next(); // consume '@'
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                return name == "trusted" || name == "untrusted";
            }
            _ => return false,
        }
    }
    false
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

    let severity = if err.is_warning {
        DiagnosticSeverity::WARNING
    } else {
        DiagnosticSeverity::ERROR
    };

    Diagnostic {
        range: span_to_range(&err.span),
        severity: Some(severity),
        source: Some("gradient-typechecker".to_string()),
        message,
        ..Default::default()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_trust_detection_no_attribute() {
        assert!(!source_has_explicit_trust_annotation(""));
        assert!(!source_has_explicit_trust_annotation(
            "fn main() -> Int:\n    ret 0\n"
        ));
    }

    #[test]
    fn explicit_trust_detection_trusted() {
        assert!(source_has_explicit_trust_annotation(
            "@trusted\nfn main() -> Int:\n    ret 0\n"
        ));
    }

    #[test]
    fn explicit_trust_detection_untrusted() {
        assert!(source_has_explicit_trust_annotation(
            "@untrusted\nfn main() -> Int:\n    ret 0\n"
        ));
    }

    #[test]
    fn explicit_trust_detection_unrelated_attribute() {
        // `@panic` / `@no_std` / `@verified` etc. should NOT count as
        // an explicit trust posture — the LSP default should still
        // apply.
        assert!(!source_has_explicit_trust_annotation(
            "@panic(abort)\nfn main() -> Int:\n    ret 0\n"
        ));
        assert!(!source_has_explicit_trust_annotation(
            "@no_std\nfn main() -> Int:\n    ret 0\n"
        ));
    }

    #[test]
    fn explicit_trust_detection_through_blank_lines_and_comments() {
        let source = "// header comment\n\n@untrusted\nfn main() -> Int:\n    ret 0\n";
        assert!(source_has_explicit_trust_annotation(source));
        let source = "/* block\n   comment */\n@trusted\nfn main():\n    ret 0\n";
        assert!(source_has_explicit_trust_annotation(source));
    }

    #[test]
    fn default_untrusted_off_keeps_module_trusted() {
        // A trivial program with FFI would normally be rejected under
        // `@untrusted`, but with the default off it should pass.
        let source =
            "@extern(\"libc\")\nfn puts(s: String) -> Int\n\nfn main() -> Int:\n    ret 0\n";
        let result = run_diagnostics_with(
            source,
            DiagnosticOptions {
                default_untrusted: false,
            },
        );
        assert_eq!(result.trust_mode, TrustMode::Trusted);
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.message.contains("untrusted")),
            "no untrusted diagnostics expected, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn default_untrusted_on_promotes_unannotated_module() {
        // Same FFI program — with default_untrusted ON, the LSP should
        // surface an `@untrusted` rejection for the `@extern` declaration.
        let source =
            "@extern(\"libc\")\nfn puts(s: String) -> Int\n\nfn main() -> Int:\n    ret 0\n";
        let result = run_diagnostics_with(
            source,
            DiagnosticOptions {
                default_untrusted: true,
            },
        );
        assert_eq!(
            result.trust_mode,
            TrustMode::Untrusted,
            "default_untrusted=true must promote unannotated modules"
        );
        let has_untrusted_diag = result
            .diagnostics
            .iter()
            .any(|d| d.message.to_lowercase().contains("untrusted"));
        assert!(
            has_untrusted_diag,
            "expected an `@untrusted` rejection diagnostic, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn explicit_trusted_overrides_default_untrusted() {
        // `@trusted` at the top of file must beat the workspace default.
        let source = "@trusted\n@extern(\"libc\")\nfn puts(s: String) -> Int\n\nfn main() -> Int:\n    ret 0\n";
        let result = run_diagnostics_with(
            source,
            DiagnosticOptions {
                default_untrusted: true,
            },
        );
        assert_eq!(
            result.trust_mode,
            TrustMode::Trusted,
            "explicit @trusted must beat the workspace default"
        );
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|d| d.message.to_lowercase().contains("untrusted")),
            "explicit @trusted should suppress untrusted diagnostics, got {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn explicit_untrusted_with_default_untrusted_is_idempotent() {
        let source = "@untrusted\nfn main() -> Int:\n    ret 0\n";
        let result = run_diagnostics_with(
            source,
            DiagnosticOptions {
                default_untrusted: true,
            },
        );
        assert_eq!(result.trust_mode, TrustMode::Untrusted);
    }

    #[test]
    fn run_diagnostics_uses_default_options_with_untrusted_on() {
        // Sanity check: the public no-arg entry point is the LSP's
        // production path. Defaults to `default_untrusted: false` per
        // the `Default` impl, matching the previous behavior.
        let source =
            "@extern(\"libc\")\nfn puts(s: String) -> Int\n\nfn main() -> Int:\n    ret 0\n";
        let result = run_diagnostics(source);
        assert_eq!(
            result.trust_mode,
            TrustMode::Trusted,
            "default DiagnosticOptions must preserve previous behavior"
        );
    }
}
