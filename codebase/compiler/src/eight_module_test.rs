//! Test for main.gr module (Phase 7: Bootstrap entry point)
//!
//! Note: The 8-module concatenation test is skipped due to the same parser
//! state issue affecting files >3500 lines. See issue #125 for details.

#[cfg(test)]
mod tests {
    use crate::ast::ItemKind;
    use crate::lexer::Lexer;
    use crate::parser::parse;

    fn compiler_path(rel: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../compiler")
            .join(rel)
    }

    /// Test that main.gr exports expected symbols for the bootstrap.
    /// Uses 7 modules + main.gr concatenated (known to work within size limits).
    #[test]
    fn main_gr_exports_expected_symbols() {
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
        let main_src =
            std::fs::read_to_string(compiler_path("main.gr")).expect("failed to read main.gr");

        let combined = format!(
            "{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
            token_src, lexer_src, parser_src, checker_src, query_src, lsp_src, codegen_src, main_src
        );

        let mut lexer = Lexer::new(&combined, 0);
        let tokens = lexer.tokenize();
        let (module, _errors) = parse(tokens, 0);

        // Extract function names - Item is Spanned<ItemKind>
        let function_names: Vec<String> = module
            .items
            .iter()
            .filter_map(|item| match &item.node {
                ItemKind::FnDef(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect();

        // Expected symbols from main.gr
        let expected = [
            "new_session",
            "read_source_file",
            "extract_module_name",
            "compile_file",
            "tokenize_source",
            "parse_tokens",
            "type_check_module",
            "generate_target_code",
            "add_diagnostics",
            "count_errors",
            "count_warnings",
            "default_config",
            "parse_args",
            "print_result",
            "main",
        ];

        for sym in expected {
            assert!(
                function_names.iter().any(|n| n == sym),
                "expected main.gr to export `{}`, but got: {:?}",
                sym,
                function_names
            );
        }
    }
}
