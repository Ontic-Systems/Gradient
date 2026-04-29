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

/// Ensure the lexer/parser/query bootstrap boundary uses explicit collection
/// handles instead of ad hoc dummy Int placeholder records.
#[test]
fn lexer_parser_query_have_no_dummy_collection_fields() {
    let targets = ["lexer.gr", "parser.gr", "query.gr"];

    for target in &targets {
        let content = std::fs::read_to_string(compiler_path(target))
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", target, e));

        assert!(
            !content.contains("dummy: Int"),
            "{} should not define dummy Int collection placeholders",
            target
        );
        assert!(
            !content.contains("{ dummy:"),
            "{} should not construct dummy collection placeholders",
            target
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
