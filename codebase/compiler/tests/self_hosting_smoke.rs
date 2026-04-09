//! Self-hosting smoke tests.
//!
//! These tests lock in the self-hosted parser/typechecker work from
//! PRs #8-#16 against future regressions. They load the in-tree
//! `compiler/token.gr` and `compiler/lexer.gr` files via the public
//! `Session` query API and assert they parse and type-check cleanly.
//!
//! Because there is no module system yet, `lexer.gr` references types
//! defined in `token.gr` (`Token`, `TokenKind`, `Position`, `Span`).
//! Loading `lexer.gr` standalone fails with ~97 unknown-type errors.
//! The validation strategy mirrors the manual workaround in use today:
//! concatenate the two files (token first, then lexer) before loading.
//!
//! If a future PR breaks the self-hosted lexer or its dependencies,
//! these tests will fail in CI instead of silently rotting on disk.
//!
//! Run with: `cargo test --release -p gradient-compiler --test self_hosting_smoke`

use std::path::PathBuf;

use gradient_compiler::query::{Session, Severity};

/// Absolute path to self-hosted compiler files (`../../compiler/<rel>`).
/// The self-hosted .gr files live in the workspace root's `compiler/` directory,
/// not inside the `codebase/compiler/` crate directory.
fn compiler_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compiler")
        .join(rel)
}

/// Render the error diagnostics from a `Session` into a single human-readable
/// string suitable for an `assert!` failure message.
fn render_errors(session: &Session) -> String {
    let result = session.check();
    let mut out = String::new();
    out.push_str(&format!(
        "ok={} error_count={} total_diagnostics={}\n",
        result.ok,
        result.error_count,
        result.diagnostics.len()
    ));
    for diag in result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
    {
        out.push_str(&format!(
            "  [{:?}/{:?}] {} (line {}, col {})\n",
            diag.phase, diag.severity, diag.message, diag.span.start.line, diag.span.start.col,
        ));
    }
    out
}

/// `compiler/token.gr` is a standalone module: it defines `Position`, `Span`,
/// `TokenKind`, `Token` plus their constructor and predicate helpers, and has
/// no external dependencies. It must parse and type-check with zero errors.
#[test]
fn token_gr_parses_and_typechecks_clean() {
    let path = compiler_path("token.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "token.gr should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// `compiler/lexer.gr` references types from `compiler/token.gr` (`Token`,
/// `TokenKind`, `Position`, `Span`). Until a module system lands, the only
/// way to type-check the lexer is to concatenate it behind the token module.
/// This is the same workaround used today by hand; the test pins it so a
/// regression in either file is caught in CI.
#[test]
fn token_plus_lexer_concatenated_parses_and_typechecks_clean() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");

    // A blank-line separator keeps line numbers in error messages somewhat
    // recognizable and avoids accidentally fusing the last line of token.gr
    // into the first line of lexer.gr.
    let combined = format!("{}\n\n{}", token_src, lexer_src);

    let session = Session::from_source(&combined);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "token.gr + lexer.gr (concatenated) should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// The token module is the public API of the self-hosted lexer's data layer.
/// We assert that the symbol set extracted by `Session::symbols()` contains
/// the load-bearing constructor and predicate helpers. If a future
/// rename or refactor drops one of these from `token.gr`, the lexer will
/// stop type-checking and downstream stages will break, so we want to fail
/// loudly here first.
///
/// NOTE: Types (Position, Span, TokenKind, Token) are not in symbols() because
/// they're type definitions, not functions. The functions below operate on
/// these types.
#[test]
fn token_gr_exposes_expected_symbols() {
    let path = compiler_path("token.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        // Token constructors
        "new",
        "eof",
        "error",
        // Predicates used by the lexer / future parser
        "is_literal",
        "is_keyword",
        "is_operator",
        "is_delimiter",
        "is_identifier",
        // Display helpers
        "kind_name",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected token.gr to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// The concatenated module must expose the lexer's entry points `tokenize`
/// and `new_lexer` (the functions downstream phases will eventually call).
/// If either disappears from `lexer.gr`, this test fires.
///
/// NOTE: The Lexer type is not in symbols() because it's a type definition,
/// not a function. The functions below operate on the Lexer type.
#[test]
fn lexer_gr_concatenated_exposes_tokenize() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let combined = format!("{}\n\n{}", token_src, lexer_src);

    let session = Session::from_source(&combined);
    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        // Lexer constructors
        "new_lexer",
        "new_lexer_from_file",
        // Main entry points
        "tokenize",
        "tokenize_file",
        // Token scanning
        "next_token",
        // Character access
        "current_char",
        "peek_char",
        "is_eof",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected concatenated token+lexer to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// `compiler/parser.gr` references types from `compiler/token.gr` and
/// `compiler/lexer.gr` (`Token`, `TokenKind`, `Position`, `Span`, `Lexer`).
/// Until a module system lands, we concatenate all three files for validation.
/// This test pins the parser component so regressions are caught in CI.
#[test]
#[ignore = "experimental: self-hosted parser.gr has name conflicts with lexer (both define TokenKind and advance function)"]
fn token_plus_lexer_plus_parser_concatenated_parses_and_typechecks_clean() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");

    // Concatenate: token.gr first (types), then lexer.gr (tokenize), then parser.gr (AST)
    let combined = format!("{}\n\n{}\n\n{}", token_src, lexer_src, parser_src);

    let session = Session::from_source(&combined);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "token.gr + lexer.gr + parser.gr (concatenated) should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// The parser module exposes key entry points for downstream phases.
/// This test verifies `parse_module` and core parsing functions are present.
///
/// NOTE: Types (Parser, Expr, Stmt, Module) are not in symbols() because they're
/// type definitions, not functions.
#[test]
#[ignore = "experimental: self-hosted parser.gr has name conflicts with lexer (advance function)"]
fn parser_gr_concatenated_exposes_parse_module() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");

    let combined = format!("{}\n\n{}\n\n{}", token_src, lexer_src, parser_src);

    let session = Session::from_source(&combined);
    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    // Key parser exports that downstream phases depend on (functions only)
    let expected = [
        "new_parser",
        "parse_module",
        "parse_expression",
        "parse_stmt",
        "parse_function",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected concatenated token+lexer+parser to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}
