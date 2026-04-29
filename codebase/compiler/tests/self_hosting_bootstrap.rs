//! Self-Hosting Bootstrap Test
//!
//! Validates that the self-hosted compiler can be loaded and its
//! structure is correct. This is the first step toward full bootstrap.
//!
//! Note: The self-hosted compiler implementations are currently stubs.
//! This test validates the structure (types, function signatures)
//! is correct. Full compilation will be tested once implementations
//! are complete.

use std::path::PathBuf;

/// Get the path to a compiler source file
fn compiler_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compiler")
        .join(rel)
}

/// Read all self-hosted compiler modules
fn read_all_compiler_modules() -> Vec<(String, String)> {
    let modules = [
        ("token", compiler_path("token.gr")),
        ("types", compiler_path("types.gr")),
        ("types_positional", compiler_path("types_positional.gr")),
        ("lexer", compiler_path("lexer.gr")),
        ("parser", compiler_path("parser.gr")),
        ("checker", compiler_path("checker.gr")),
        ("ir", compiler_path("ir.gr")),
        ("ir_builder", compiler_path("ir_builder.gr")),
        ("compiler", compiler_path("compiler.gr")),
        ("bootstrap", compiler_path("bootstrap.gr")),
        ("query", compiler_path("query.gr")),
        ("lsp", compiler_path("lsp.gr")),
        ("codegen", compiler_path("codegen.gr")),
        ("main", compiler_path("main.gr")),
    ];

    modules
        .iter()
        .map(|(name, path)| {
            let content = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("Failed to read {}: {}", name, e));
            (name.to_string(), content)
        })
        .collect()
}

/// Count lines in all self-hosted modules
#[test]
fn self_hosted_compiler_line_count() {
    let modules = read_all_compiler_modules();
    let total_lines: usize = modules
        .iter()
        .map(|(_, content)| content.lines().count())
        .sum();

    println!("Self-hosted compiler modules:");
    for (name, content) in &modules {
        let lines = content.lines().count();
        println!("  {}: {} lines", name, lines);
    }
    println!("Total: {} lines", total_lines);

    // We expect at least 6000 lines
    assert!(
        total_lines >= 6000,
        "Expected at least 6000 lines of self-hosted code, got {}",
        total_lines
    );
}

/// Verify all expected modules exist
#[test]
fn all_compiler_modules_exist() {
    let expected = [
        "token.gr",
        "types.gr",
        "types_positional.gr",
        "lexer.gr",
        "parser.gr",
        "checker.gr",
        "ir.gr",
        "ir_builder.gr",
        "compiler.gr",
        "bootstrap.gr",
        "query.gr",
        "lsp.gr",
        "codegen.gr",
        "main.gr",
    ];

    for module in &expected {
        let path = compiler_path(module);
        assert!(
            path.exists(),
            "Compiler module {} should exist at {:?}",
            module,
            path
        );
    }
}

/// Verify main.gr contains the entry point
#[test]
fn main_gr_has_entry_point() {
    let main_content =
        std::fs::read_to_string(compiler_path("main.gr")).expect("Failed to read main.gr");

    // Check for key components
    assert!(
        main_content.contains("fn main("),
        "main.gr should have a main function"
    );
    assert!(
        main_content.contains("CompileResult"),
        "main.gr should define CompileResult"
    );
    assert!(
        main_content.contains("CompilerConfig"),
        "main.gr should define CompilerConfig"
    );
    assert!(
        main_content.contains("file_read"),
        "main.gr should use file_read for I/O"
    );
}

/// Verify the self-hosted compiler exports expected query API
#[test]
fn query_gr_has_api_definitions() {
    let query_content =
        std::fs::read_to_string(compiler_path("query.gr")).expect("Failed to read query.gr");

    // Check for key query API types (defined with 'type' keyword in query.gr)
    let expected_types = ["QueryEngine", "SymbolInfo", "TypeAtResult"];

    for ty in &expected_types {
        assert!(
            query_content.contains(&format!("type {}", ty)),
            "query.gr should define type {}",
            ty
        );
    }

    // Check for key methods (in impl QueryEngine blocks)
    let expected_methods = ["symbol_at", "type_at"];

    for method in &expected_methods {
        assert!(
            query_content.contains(&format!("fn {}", method)),
            "query.gr should have method {}",
            method
        );
    }
}

/// Ensure bootstrap compiler collections use explicit runtime-backed handles
/// instead of ad hoc placeholder records or zero handles.
#[test]
fn lexer_parser_query_have_no_dummy_collection_fields() {
    let modules = read_all_compiler_modules();

    for (name, content) in &modules {
        assert!(
            !content.contains("dummy: Int"),
            "{} should not define dummy Int collection placeholders",
            name
        );
        assert!(
            !content.contains("{ dummy:"),
            "{} should not construct dummy collection placeholders",
            name
        );
        assert!(
            !content.contains("handle: 0"),
            "{} should not construct zero bootstrap collection handles",
            name
        );
    }

    let lexer_content =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("Failed to read lexer.gr");
    let parser_content =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("Failed to read parser.gr");
    let query_content =
        std::fs::read_to_string(compiler_path("query.gr")).expect("Failed to read query.gr");

    assert!(lexer_content.contains("type TokenList"));
    assert!(parser_content.contains("type ExprList"));
    assert!(parser_content.contains("type StmtList"));
    assert!(parser_content.contains("type ModuleItemList"));
    assert!(query_content.contains("type DiagnosticList"));
    assert!(query_content.contains("type SymbolList"));

    for (name, content) in [
        ("lexer.gr", lexer_content),
        ("parser.gr", parser_content),
        ("query.gr", query_content),
    ] {
        assert!(
            content.contains("handle: Int"),
            "{} should expose explicit bootstrap collection handles",
            name
        );
    }
}

/// Issue #220: lexer.gr::tokenize must accumulate real tokens through the
/// runtime-backed bootstrap collection API, not return a placeholder handle.
#[test]
fn lexer_gr_tokenize_emits_real_token_list() {
    let lexer_src =
        std::fs::read_to_string(compiler_path("lexer.gr")).expect("Failed to read lexer.gr");

    // Bootstrap collection externs must be declared so that tokenize can
    // allocate and append against the host store. The append extern is
    // FFI-primitive-only because the runtime cannot pass record values
    // across the boundary yet (#220).
    for extern_decl in [
        "fn bootstrap_token_list_alloc() -> Int",
        "fn bootstrap_token_list_append(handle: Int, kind_tag: Int, file_id: Int, start_offset: Int, end_offset: Int) -> Int",
        "fn bootstrap_token_list_len(handle: Int) -> Int",
    ] {
        assert!(
            lexer_src.contains(extern_decl),
            "lexer.gr must declare bootstrap token list extern `{extern_decl}`"
        );
    }

    // Locate the body of `tokenize` and assert it (a) does not use the old
    // placeholder handle, (b) allocates a real handle, and (c) appends
    // tokens via the bootstrap API rather than returning a static record.
    let signature = "fn tokenize(source: String, file_id: Int) -> TokenList:";
    let start = lexer_src
        .find(signature)
        .expect("lexer.gr must define fn tokenize");
    let after_signature = &lexer_src[start + signature.len()..];
    let end = after_signature
        .find("\n\n    fn ")
        .unwrap_or(after_signature.len());
    let tokenize_body = &after_signature[..end];

    for forbidden in [
        "TokenList { handle: 0 }",
        "TokenList { handle: 1 }",
        "TokenList { handle: 2 }",
    ] {
        assert!(
            !tokenize_body.contains(forbidden),
            "lexer.gr::tokenize must not return placeholder `{forbidden}`"
        );
    }

    assert!(
        tokenize_body.contains("bootstrap_token_list_alloc()"),
        "lexer.gr::tokenize must allocate a runtime-backed token list handle"
    );
    assert!(
        tokenize_body.contains("bootstrap_token_list_append(handle"),
        "lexer.gr::tokenize must append tokens to the runtime-backed handle"
    );
    assert!(
        tokenize_body.contains("next_token(lex)"),
        "lexer.gr::tokenize must drive the next_token scanner"
    );
    assert!(
        tokenize_body.contains("token_kind_tag"),
        "lexer.gr::tokenize must encode token kinds via token_kind_tag"
    );
}

/// Issue #221: parser.gr token access must read through the runtime-backed
/// TokenList store via the `bootstrap_token_list_get_*` extern primitives,
/// not return a hard-coded Eof token.
#[test]
fn parser_gr_token_access_reads_real_token_list() {
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("Failed to read parser.gr");

    // Reader-side externs must be declared so parser execution can recover
    // a token's kind and span from a list handle + index. Signatures stay
    // FFI-primitive (Int only) until the runtime can carry token payloads.
    for extern_decl in [
        "fn bootstrap_token_list_get_kind(handle: Int, index: Int) -> Int",
        "fn bootstrap_token_list_get_file_id(handle: Int, index: Int) -> Int",
        "fn bootstrap_token_list_get_start_offset(handle: Int, index: Int) -> Int",
        "fn bootstrap_token_list_get_end_offset(handle: Int, index: Int) -> Int",
    ] {
        assert!(
            parser_src.contains(extern_decl),
            "parser.gr must declare bootstrap token access extern `{extern_decl}`"
        );
    }

    // current_token / peek_token must drive the new accessors instead of
    // returning a static Eof. Keep the assertions structural so cosmetic
    // refactors stay free.
    let current_body =
        parser_gr_function_body(&parser_src, "fn current_token(p: Parser) -> Token:")
            .expect("parser.gr must define fn current_token");
    let peek_body = parser_gr_function_body(
        &parser_src,
        "fn peek_token(p: Parser, offset: Int) -> Token:",
    )
    .expect("parser.gr must define fn peek_token");

    for (name, body) in [("current_token", current_body), ("peek_token", peek_body)] {
        assert!(
            !body.contains("Token { kind: Eof, span: Span { file_id: p.file_id"),
            "parser.gr::{name} must not hard-code an Eof token"
        );
    }

    // The shared lookup helper must hit all four reader externs so token
    // identity (kind + span) round-trips through the runtime store.
    let token_at_body =
        parser_gr_function_body(&parser_src, "fn token_at(p: Parser, index: Int) -> Token:")
            .expect("parser.gr must define a runtime-backed token_at lookup helper");
    for required in [
        "bootstrap_token_list_get_kind(p.tokens.handle",
        "bootstrap_token_list_get_file_id(p.tokens.handle",
        "bootstrap_token_list_get_start_offset(p.tokens.handle",
        "bootstrap_token_list_get_end_offset(p.tokens.handle",
        "kind_tag_to_token_kind(",
    ] {
        assert!(
            token_at_body.contains(required),
            "parser.gr::token_at must invoke `{required}` to materialize a runtime-backed token"
        );
    }

    // peek_token must apply its offset relative to the parser cursor; a
    // regression that drops `offset` and reads `p.pos` directly would silently
    // alias current_token.
    assert!(
        peek_body.contains("p.pos + offset"),
        "parser.gr::peek_token must read at p.pos + offset"
    );
}

fn parser_gr_function_body<'a>(src: &'a str, signature: &str) -> Option<&'a str> {
    let start = src.find(signature)?;
    let after_signature = &src[start + signature.len()..];
    let end = after_signature
        .find("\n\n    fn ")
        .unwrap_or(after_signature.len());
    Some(&after_signature[..end])
}

#[test]
fn parser_gr_exposes_direct_execution_readiness_metadata() {
    let parser_content =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("Failed to read parser.gr");

    assert!(
        parser_content.contains("fn normalized_export_contract_version() -> String:"),
        "parser.gr should expose a normalized export contract version"
    );
    assert!(
        parser_content.contains("ret \"canonical-json-v1\""),
        "parser.gr normalized export contract version should be canonical-json-v1"
    );

    let readiness_body = parser_gr_function_body(
        &parser_content,
        "fn parser_direct_execution_ready() -> Bool:",
    )
    .expect("parser.gr should expose explicit direct-execution readiness metadata");
    assert!(
        readiness_body.contains("ret false") && !readiness_body.contains("ret true"),
        "parser.gr direct execution must remain false until TokenList/list storage is real"
    );
}

/// Count functions defined in self-hosted code
#[test]
fn self_hosted_function_count() {
    let modules = read_all_compiler_modules();
    let mut total_functions = 0;

    for (name, content) in &modules {
        let count = content.matches("fn ").count();
        println!("{}: ~{} functions", name, count);
        total_functions += count;
    }

    println!("Total functions: ~{}", total_functions);

    // We expect at least 150 functions
    assert!(
        total_functions >= 150,
        "Expected at least 150 functions, got {}",
        total_functions
    );
}

/// Verify self-hosted compiler has proper type definitions
#[test]
fn self_hosted_has_core_types() {
    let token_content =
        std::fs::read_to_string(compiler_path("token.gr")).expect("Failed to read token.gr");

    // Check for Token type
    assert!(
        token_content.contains("type Token"),
        "token.gr should define Token type"
    );

    // Check for TokenKind enum
    assert!(
        token_content.contains("enum TokenKind"),
        "token.gr should define TokenKind enum"
    );
}
