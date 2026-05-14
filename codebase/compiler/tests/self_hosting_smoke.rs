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

use gradient_compiler::lexer::{Lexer, TokenKind};
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

/// Lexical milestone for self-hosting: capability keywords are real keywords,
/// so self-hosted compiler sources must not rely on reserved words as ordinary
/// identifiers. This behavior check guards the `val` keyword regression that
/// previously kept the codegen self-hosting smoke ignored.
#[test]
fn codegen_gr_lexes_without_reserved_val_identifier_regression() {
    let codegen_src =
        std::fs::read_to_string(compiler_path("codegen.gr")).expect("failed to read codegen.gr");
    let tokens = Lexer::new(&codegen_src, 0).tokenize();

    let errors: Vec<_> = tokens
        .iter()
        .filter_map(|tok| match &tok.kind {
            TokenKind::Error(msg) => Some(format!(
                "{} at line {}, col {}",
                msg, tok.span.start.line, tok.span.start.col
            )),
            _ => None,
        })
        .collect();
    assert!(
        errors.is_empty(),
        "codegen.gr should lex cleanly: {errors:?}"
    );

    assert!(
        tokens
            .iter()
            .any(|tok| matches!(&tok.kind, TokenKind::Ident(name) if name == "value")),
        "codegen.gr should use `value` as the store/return value identifier"
    );
    assert!(
        !tokens.iter().any(|tok| matches!(tok.kind, TokenKind::Val)),
        "codegen.gr must not use reserved keyword `val` as an identifier"
    );

    let keyword_tokens = Lexer::new("val value", 0).tokenize();
    assert!(matches!(keyword_tokens[0].kind, TokenKind::Val));
    assert!(matches!(&keyword_tokens[1].kind, TokenKind::Ident(name) if name == "value"));
}

/// `compiler/parser.gr` references types from `compiler/token.gr` and
/// `compiler/lexer.gr` (`Token`, `TokenKind`, `Position`, `Span`, `Lexer`).
/// Until a module system lands, we concatenate all three files for validation.
/// This test pins the parser component so regressions are caught in CI.
#[test]
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
        "parse_expr",
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

/// `compiler/checker.gr` references types from `compiler/parser.gr` and
/// previous modules. Until a module system lands, we concatenate all
/// four files for validation. This test pins the checker component.
#[test]
fn token_plus_lexer_plus_parser_plus_checker_concatenated_parses_and_typechecks_clean() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("failed to read checker.gr");

    // Concatenate: token + lexer + parser + checker
    let combined = format!("{}\n\n{}\n\n{}\n\n{}", token_src, lexer_src, parser_src, checker_src);

    let session = Session::from_source(&combined);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "token.gr + lexer.gr + parser.gr + checker.gr (concatenated) should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// The checker module exposes key type checking functions.
#[test]
fn checker_gr_concatenated_exposes_expected_symbols() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("failed to read checker.gr");

    let combined = format!("{}\n\n{}\n\n{}\n\n{}", token_src, lexer_src, parser_src, checker_src);

    let session = Session::from_source(&combined);
    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        "new_checker",
        "check_expr",
        "check_stmt",
        "check_fn",
        "check_module",
        "int_type",
        "bool_type",
        "string_type",
        "float_type",
        "unit_type",
        "types_equal",
        "is_int",
        "is_bool",
        "type_to_string",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected concatenated checker to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// `compiler/query.gr` references types from previous modules.
/// Until a module system lands, we concatenate all five files for validation.
#[test]
fn all_modules_plus_query_concatenated_parses_and_typechecks_clean() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("failed to read checker.gr");
    let query_src =
        std::fs::read_to_string(compiler_path("query.gr")).expect("failed to read query.gr");

    // Concatenate all modules
    let combined = format!(
        "{}\n\n{}\n\n{}\n\n{}\n\n{}",
        token_src, lexer_src, parser_src, checker_src, query_src
    );

    let session = Session::from_source(&combined);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "all modules + query.gr (concatenated) should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// The query module exposes key query functions.
#[test]
fn query_gr_concatenated_exposes_expected_symbols() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("failed to read checker.gr");
    let query_src =
        std::fs::read_to_string(compiler_path("query.gr")).expect("failed to read query.gr");

    let combined = format!(
        "{}\n\n{}\n\n{}\n\n{}\n\n{}",
        token_src, lexer_src, parser_src, checker_src, query_src
    );

    let session = Session::from_source(&combined);
    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        "new_query_engine",
        "new_session",
        "new_session_from_file",
        "check",
        "get_symbols",
        "type_at",
        "symbol_at",
        "severity_to_string",
        "phase_to_string",
        "symbol_kind_to_string",
        "has_errors",
        "error_count",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected concatenated query to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// `compiler/lsp.gr` references types from previous modules.
/// Until a module system lands, we concatenate all six files for validation.
#[test]
fn all_modules_plus_lsp_concatenated_parses_and_typechecks_clean() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("failed to read checker.gr");
    let query_src =
        std::fs::read_to_string(compiler_path("query.gr")).expect("failed to read query.gr");
    let lsp_src =
        std::fs::read_to_string(compiler_path("lsp.gr")).expect("failed to read lsp.gr");

    // Concatenate all modules
    let combined = format!(
        "{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
        token_src, lexer_src, parser_src, checker_src, query_src, lsp_src
    );

    let session = Session::from_source(&combined);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "all modules + lsp.gr (concatenated) should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// The lsp module exposes key LSP functions.
#[test]
fn lsp_gr_concatenated_exposes_expected_symbols() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("failed to read checker.gr");
    let query_src =
        std::fs::read_to_string(compiler_path("query.gr")).expect("failed to read query.gr");
    let lsp_src =
        std::fs::read_to_string(compiler_path("lsp.gr")).expect("failed to read lsp.gr");

    let combined = format!(
        "{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
        token_src, lexer_src, parser_src, checker_src, query_src, lsp_src
    );

    let session = Session::from_source(&combined);
    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        "new_lsp_server",
        "initialize",
        "did_open",
        "did_change",
        "did_close",
        "did_save",
        "hover",
        "completion",
        "document_symbol",
        "goto_definition",
        "run_diagnostics",
        "new_document_store",
        "store_document",
        "get_document",
        "remove_document",
        "update_document",
        "word_at_position",
        "get_builtin_functions",
        "get_keywords",
        "is_builtin",
        "is_keyword",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected concatenated lsp to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// `compiler/codegen.gr` references types from previous modules.
/// Until a module system lands, we concatenate all seven files for validation.
///
#[test]
fn all_modules_plus_codegen_concatenated_parses_and_typechecks_clean() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("failed to read checker.gr");
    let query_src =
        std::fs::read_to_string(compiler_path("query.gr")).expect("failed to read query.gr");
    let lsp_src =
        std::fs::read_to_string(compiler_path("lsp.gr")).expect("failed to read lsp.gr");
    let codegen_src =
        std::fs::read_to_string(compiler_path("codegen.gr")).expect("failed to read codegen.gr");

    let combined = format!(
        "{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
        token_src, lexer_src, parser_src, checker_src, query_src, lsp_src, codegen_src
    );

    let session = Session::from_source(&combined);
    let result = session.check();

    assert!(
        result.ok && result.error_count == 0,
        "all modules + codegen.gr (concatenated) should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// The codegen module exposes key IR and codegen functions.
#[test]
fn codegen_gr_concatenated_exposes_expected_symbols() {
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("failed to read checker.gr");
    let query_src =
        std::fs::read_to_string(compiler_path("query.gr")).expect("failed to read query.gr");
    let lsp_src =
        std::fs::read_to_string(compiler_path("lsp.gr")).expect("failed to read lsp.gr");
    let codegen_src =
        std::fs::read_to_string(compiler_path("codegen.gr")).expect("failed to read codegen.gr");

    let combined = format!(
        "{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
        token_src, lexer_src, parser_src, checker_src, query_src, lsp_src, codegen_src
    );

    let session = Session::from_source(&combined);
    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        "new_ir_module",
        "add_function",
        "add_basic_block",
        "new_ir_builder",
        "new_code_generator",
        "generate_code",
        "generate_function",
        "generate_basic_block",
        "type_to_ir_type",
        "ir_type_to_string",
        "ir_type_size",
        "build_const",
        "build_call",
        "build_ret_void",
        "build_ret",
        "build_add",
        "build_sub",
        "build_mul",
        "build_div",
        "build_cmp",
        "build_or",
        "build_branch",
        "build_jump",
        "build_phi",
        "build_alloca",
        "build_load",
        "build_store",
        "build_ptr_to_int",
        "build_int_to_ptr",
        "next_value",
        "next_block",
        "set_current_function",
        "set_current_block",
        "is_terminator",
        "has_result",
        // #263: emit_module is the first delegating self-hosted body in
        // codegen.gr — it must remain present and discoverable as a
        // top-level symbol after concatenation. Locking it here keeps
        // the SelfHostedDefault classification on the `emit` row honest.
        "emit_module",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected concatenated codegen to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// `compiler/compiler.gr` is self-contained inside `mod compiler:` (it
/// re-declares its own `TokenKind`, `AstModule`, `IrModule`, etc. so it can
/// be type-checked alone without dragging in the other self-hosted modules).
/// This test loads it standalone and asserts it parses + type-checks cleanly,
/// then locks `compile_string` and `compile_file` as expected top-level
/// symbols. Per #267, `compile_string`'s body is now a delegating chain
/// through `bootstrap_pipeline_*` — keeping it as an expected concatenated
/// symbol makes the SelfHostedDefault classification on the `pipeline` row
/// honest the same way #263 did for `emit_module` on the `emit` row.
#[test]
fn compiler_gr_parses_and_typechecks_clean() {
    let path = compiler_path("compiler.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "compiler.gr should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// Lock `compile_string` (and its sibling entry points) as expected top-level
/// symbols of `compiler/compiler.gr`. If a future refactor renames or removes
/// `compile_string` the SelfHostedDefault `pipeline` row in
/// `kernel_boundary.rs` would silently drift; this test fails fast.
#[test]
fn compiler_gr_exposes_pipeline_entry_points() {
    let path = compiler_path("compiler.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        // #267: compile_string is the first delegating self-hosted pipeline
        // entry point — body chains bootstrap_pipeline_* externs end-to-end.
        // Locking it here keeps the SelfHostedDefault classification on the
        // `pipeline` row honest.
        "compile_string",
        "compile_file",
        "default_options",
        "stage_to_string",
        "format_result",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected compiler.gr to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// `compiler/query.gr` is self-contained inside `mod query:` (it
/// re-declares its own `Severity`, `Phase`, `SymbolKind`, and `Session`
/// types so it can be type-checked alone). Per #269, the public query
/// entry points (`new_session`, `check`, `has_errors`, `error_count`)
/// now delegate to the `bootstrap_query_*` kernel surface; locking the
/// standalone clean-typecheck + symbol presence here keeps the
/// SelfHostedDefault classification on the `query` row honest.
#[test]
fn query_gr_standalone_parses_and_typechecks_clean() {
    let path = compiler_path("query.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "query.gr should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// Lock the public query entry points as expected top-level symbols of
/// `compiler/query.gr`. After #269 these all delegate to
/// `bootstrap_query_*`; if a future refactor renames or removes one of
/// them the SelfHostedDefault `query` row in `kernel_boundary.rs`
/// would silently drift.
#[test]
fn query_gr_standalone_exposes_session_entry_points() {
    let path = compiler_path("query.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        // #269: these now delegate to bootstrap_query_* kernel externs.
        "new_session",
        "new_session_from_file",
        "check",
        "has_errors",
        "error_count",
        // #291: the three richer-record query handlers now delegate to
        // bootstrap_query_symbol_count / _at / _type_at / per-index
        // accessors. Locking the symbol names so a regression that drops
        // or renames them fails CI.
        "get_symbols",
        "type_at",
        "symbol_at",
        "wire_code_to_symbol_kind",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected query.gr to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// `compiler/lsp.gr` is self-contained inside `mod lsp:`. Per #271 the
/// LSP server bootstrap and `is_builtin` / `is_keyword` predicates now
/// delegate to `bootstrap_lsp_*` kernel externs; locking the standalone
/// clean-typecheck + symbol presence here keeps the SelfHostedDefault
/// classification on the `lsp` row honest.
#[test]
fn lsp_gr_standalone_parses_and_typechecks_clean() {
    let path = compiler_path("lsp.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "lsp.gr should type-check cleanly:\n{}",
        render_errors(&session),
    );
}

/// Lock the LSP entry points that now delegate to `bootstrap_lsp_*` as
/// expected top-level symbols of `compiler/lsp.gr`.
#[test]
fn lsp_gr_standalone_exposes_server_entry_points() {
    let path = compiler_path("lsp.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        // #271: these now delegate to bootstrap_lsp_* kernel externs.
        "new_lsp_server",
        "is_builtin",
        "is_keyword",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected lsp.gr to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// Lock the LSP lifecycle handlers that now delegate to `bootstrap_lsp_*`
/// per #275 as expected top-level symbols of `compiler/lsp.gr`.
#[test]
fn lsp_gr_standalone_exposes_lifecycle_handlers() {
    let path = compiler_path("lsp.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        // #275 / #277: these now delegate to bootstrap_lsp_* kernel externs.
        "initialize",
        "did_open",
        "did_change",
        "did_close",
        "did_save",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected lsp.gr to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// `compiler/lsp.gr` exports the richer-record LSP request handlers
/// (`hover` so far) per #283 as expected top-level symbols. Locks
/// `hover` so a regression that drops the symbol or renames it fails CI.
#[test]
fn lsp_gr_standalone_exposes_richer_handlers() {
    let path = compiler_path("lsp.gr");
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let names: Vec<String> = session.symbols().into_iter().map(|s| s.name).collect();

    let expected = [
        // #283: hover delegates to bootstrap_lsp_hover; first richer-record
        // handler in lsp.gr.
        "hover",
        // #285: document_symbol delegates to
        // bootstrap_lsp_document_symbol_count; second richer-LSP handler.
        "document_symbol",
        // #287: completion delegates to bootstrap_lsp_completion_count;
        // third richer-LSP handler.
        "completion",
        // #289: run_diagnostics chains bootstrap_lsp_did_save +
        // bootstrap_lsp_diagnostic_count; fourth richer-LSP handler.
        "run_diagnostics",
        // #378: goto_definition delegates to four
        // bootstrap_lsp_goto_definition_* externs (#293 kernel surface);
        // fifth richer-LSP handler — completes the LSP handler set.
        "goto_definition",
    ];

    for sym in expected {
        assert!(
            names.iter().any(|n| n == sym),
            "expected lsp.gr to export `{}`, but symbols() returned: {:?}",
            sym,
            names,
        );
    }
}

/// Lock the new per-index accessor extern declarations inside `mod lsp:`
/// (#404). These extend the kernel surface visible to `.gr`-side handlers
/// from 15 to 26 externs without changing any catalog row. The `.gr`
/// parser does NOT surface ExternFn entries through `session.symbols()`,
/// so the lock asserts on the source text (the externs MUST be inside the
/// `mod lsp:` block to count) plus a clean type-check of `lsp.gr`.
#[test]
fn lsp_gr_declares_per_index_accessor_externs() {
    let path = compiler_path("lsp.gr");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    let expected = [
        // Diagnostic per-index accessors.
        "fn bootstrap_lsp_diagnostic_severity(server_id: Int, uri: String, index: Int) -> Int",
        "fn bootstrap_lsp_diagnostic_message(server_id: Int, uri: String, index: Int) -> String",
        "fn bootstrap_lsp_diagnostic_line(server_id: Int, uri: String, index: Int) -> Int",
        "fn bootstrap_lsp_diagnostic_character(server_id: Int, uri: String, index: Int) -> Int",
        // Document-symbol per-index accessors.
        "fn bootstrap_lsp_document_symbol_name(server_id: Int, uri: String, index: Int) -> String",
        "fn bootstrap_lsp_document_symbol_kind(server_id: Int, uri: String, index: Int) -> Int",
        "fn bootstrap_lsp_document_symbol_line(server_id: Int, uri: String, index: Int) -> Int",
        "fn bootstrap_lsp_document_symbol_character(server_id: Int, uri: String, index: Int) -> Int",
        // Completion per-index accessors.
        "fn bootstrap_lsp_completion_label(server_id: Int, uri: String, index: Int) -> String",
        "fn bootstrap_lsp_completion_kind(server_id: Int, uri: String, index: Int) -> Int",
        "fn bootstrap_lsp_completion_detail(server_id: Int, uri: String, index: Int) -> String",
    ];

    for decl in expected {
        assert!(
            src.contains(decl),
            "expected lsp.gr to declare extern `{}` inside `mod lsp:` block",
            decl,
        );
    }

    // And the file must still parse + type-check cleanly with the new
    // declarations in place.
    let session = Session::from_file(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "lsp.gr should type-check cleanly after #404:\n{}",
        render_errors(&session),
    );
}

// =============================================================================
// E2 effect-tier dogfood (#382): self-hosted compiler/*.gr modules use the
// new effect-tier surface (`!{Heap}` from #313/#455 + `!{Throws(E)}` from
// #317) extensively. These regression tests lock in:
//
//   1. Each of lexer.gr / parser.gr / checker.gr declares at least one
//      function whose effect row contains `Throws(<E>)`. The parameterized
//      effect surface from #317 must be reachable end-to-end from the
//      self-hosted modules; a future refactor that drops it should be
//      flagged by CI.
//
//   2. Each of lexer.gr / parser.gr / checker.gr uses `!{Heap}` on at
//      least N representative function signatures. The heap-allocation-
//      site enforcement from #313/#455 was migrated into compiler/*.gr
//      ahead of this issue; these counts pin the migration so future
//      refactors that quietly drop `!{Heap}` annotations get caught.
//
//   3. Each of those three modules still parses + type-checks cleanly
//      after the dogfood adds.
//
// Pinned counts are conservative lower bounds (the modules currently use
// 17 / 59 / 38 `!{Heap}` rows respectively). They exist to catch wholesale
// regressions, not to micromanage every signature change.
// =============================================================================

#[test]
fn lexer_gr_dogfoods_e2_effects() {
    let path = compiler_path("lexer.gr");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    // (1) At least one Throws(E) signature is reachable in lexer.gr.
    assert!(
        src.contains("!{Throws(LexError)}"),
        "lexer.gr should declare at least one `!{{Throws(LexError)}}` function (#382 dogfood)"
    );

    // (2) Heap migration is in place — pin a conservative lower bound
    //     (current count is 17 to avoid false positives on small refactors).
    let heap_count = src.matches("!{Heap").count() + src.matches(", Heap").count();
    assert!(
        heap_count >= 10,
        "lexer.gr should use `!{{Heap}}` on at least 10 signatures (E2 dogfood); found {}",
        heap_count
    );

    // (3) File still parses + type-checks cleanly when concatenated against
    //     token.gr (lexer.gr depends on token's types).
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let combined = format!("{}\n\n{}", token_src, src);
    let session = Session::from_source(&combined);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "lexer.gr (concat with token.gr) should type-check cleanly after dogfood:\n{}",
        render_errors(&session),
    );
}

#[test]
fn parser_gr_dogfoods_e2_effects() {
    let path = compiler_path("parser.gr");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    // (1) At least one Throws(E) signature is reachable in parser.gr.
    assert!(
        src.contains("!{Throws(ParseError)}"),
        "parser.gr should declare at least one `!{{Throws(ParseError)}}` function (#382 dogfood)"
    );

    // (2) Heap migration is in place. Conservative lower bound: 30 (real count ~59).
    let heap_count = src.matches("!{Heap").count() + src.matches(", Heap").count();
    assert!(
        heap_count >= 30,
        "parser.gr should use `!{{Heap}}` on at least 30 signatures (E2 dogfood); found {}",
        heap_count
    );

    // (3) File still parses + type-checks cleanly when concatenated against
    //     token.gr + lexer.gr (parser.gr depends on both).
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let combined = format!("{}\n\n{}\n\n{}", token_src, lexer_src, src);
    let session = Session::from_source(&combined);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "parser.gr (concat with token.gr + lexer.gr) should type-check cleanly after dogfood:\n{}",
        render_errors(&session),
    );
}

#[test]
fn checker_gr_dogfoods_e2_effects() {
    let path = compiler_path("checker.gr");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    // (1) At least one Throws(E) signature is reachable in checker.gr.
    assert!(
        src.contains("!{Throws(TypeError)}"),
        "checker.gr should declare at least one `!{{Throws(TypeError)}}` function (#382 dogfood)"
    );

    // (2) Heap migration is in place. Conservative lower bound: 20 (real count ~38).
    let heap_count = src.matches("!{Heap").count() + src.matches(", Heap").count();
    assert!(
        heap_count >= 20,
        "checker.gr should use `!{{Heap}}` on at least 20 signatures (E2 dogfood); found {}",
        heap_count
    );

    // (3) checker.gr typechecks via the existing concatenation path
    //     (token + lexer + parser + checker). The existing
    //     `token_plus_lexer_plus_parser_plus_checker_concatenated_*` test
    //     already locks in the full-concat path; this lighter test asserts
    //     just standalone parse + type-check behavior of checker.gr alone
    //     (it depends on token/parser AST; we use the same concat path).
    let token_src =
        std::fs::read_to_string(compiler_path("token.gr")).expect("failed to read token.gr");
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("failed to read lexer.gr");
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("failed to read parser.gr");
    let combined = format!(
        "{}\n\n{}\n\n{}\n\n{}",
        token_src, lexer_src, parser_src, src
    );
    let session = Session::from_source(&combined);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "checker.gr (concat with token+lexer+parser) should type-check cleanly after dogfood:\n{}",
        render_errors(&session),
    );
}

// ============================================================================
// `@system` mod-block dogfood (#619)
//
// `types_positional.gr` is the first kernel module to flip to `@system`
// because every fn in it already carries an explicit return type AND an
// explicit `!{Heap}` effect set. The flip is the launch-tier dogfood for
// acceptance bullet 4 of #352 ("Self-hosted compiler chooses `@system`
// for kernel modules"). Larger modules need a sweeping annotation pass
// before they can flip — deferred to follow-on PRs.
//
// This test pins both halves: the file carries the `@system` attribute
// AND the module still type-checks cleanly under the post-#619 checker
// (which descends into mod-block items to enforce `@system` rules).
// ============================================================================

#[test]
fn types_positional_gr_dogfoods_system_mode() {
    let path = compiler_path("types_positional.gr");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));

    // (1) File carries the `@system` attribute.
    assert!(
        src.contains("@system"),
        "types_positional.gr should declare `@system` at file scope (#619 dogfood)"
    );

    // (2) Module type-checks cleanly under the post-#619 checker.
    //     The `@system` enforcement now descends into the `mod
    //     types_positional:` block; if any fn loses its return type or
    //     effect annotation, this test will fail.
    let session = Session::from_source(&src);
    let result = session.check();
    assert!(
        result.ok && result.error_count == 0,
        "types_positional.gr should type-check cleanly under @system:\n{}",
        render_errors(&session),
    );
}
