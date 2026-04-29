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

/// Issue #222: parser.gr must allocate real AST nodes / lists through the
/// runtime-backed bootstrap AST store rather than collapsing children into
/// structural-fingerprint integers. The body of every `*_bootstrap_handle`
/// builder must call into the new `bootstrap_*_alloc*` externs, the list
/// helpers must drive `bootstrap_<kind>_list_alloc` + `bootstrap_node_list_append`,
/// and the normalized export must walk via `bootstrap_*_get_*` accessors.
#[test]
fn parser_gr_stores_real_ast_nodes_and_lists() {
    let parser_src =
        std::fs::read_to_string(compiler_path("parser.gr")).expect("Failed to read parser.gr");

    // The new AST externs must be declared at the top of the module so the
    // host typechecker accepts them as Phase 0 builtins.
    for extern_decl in [
        "fn bootstrap_expr_alloc_int_lit(value: Int) -> Int",
        "fn bootstrap_expr_alloc_ident(name: String) -> Int",
        "fn bootstrap_expr_alloc_binary(op_tag: Int, left: Int, right: Int) -> Int",
        "fn bootstrap_expr_alloc_call(callee: Int, args_handle: Int) -> Int",
        "fn bootstrap_expr_alloc_if(cond: Int, then_branch: Int, else_branch: Int) -> Int",
        "fn bootstrap_expr_alloc_block(stmts_handle: Int, final_expr: Int) -> Int",
        "fn bootstrap_expr_get_tag(id: Int) -> Int",
        "fn bootstrap_expr_get_child_a(id: Int) -> Int",
        "fn bootstrap_stmt_alloc(node_tag: Int, int_value: Int, child_a: Int, child_b: Int, child_c: Int, text: String) -> Int",
        "fn bootstrap_stmt_get_tag(id: Int) -> Int",
        "fn bootstrap_param_alloc(name: String, type_tag: Int, type_name: String, default_id: Int) -> Int",
        "fn bootstrap_function_alloc(name: String, params_handle: Int, ret_type_tag: Int, ret_type_name: String, body_handle: Int, is_pub: Int, is_extern: Int) -> Int",
        "fn bootstrap_module_item_alloc_function(function_id: Int) -> Int",
        "fn bootstrap_expr_list_alloc() -> Int",
        "fn bootstrap_stmt_list_alloc() -> Int",
        "fn bootstrap_param_list_alloc() -> Int",
        "fn bootstrap_module_item_list_alloc() -> Int",
        "fn bootstrap_node_list_append(handle: Int, id: Int) -> Int",
        "fn bootstrap_node_list_len(handle: Int) -> Int",
        "fn bootstrap_node_list_get(handle: Int, index: Int) -> Int",
    ] {
        assert!(
            parser_src.contains(extern_decl),
            "parser.gr must declare bootstrap AST extern `{extern_decl}`"
        );
    }

    // The expression / statement handle builders must hit the new
    // alloc externs instead of returning hard-coded `3100 + value` style
    // structural fingerprints.
    let expr_handle_body =
        parser_gr_function_body(&parser_src, "fn expr_bootstrap_handle(expr: Expr) -> Int:")
            .expect("parser.gr must define fn expr_bootstrap_handle");
    for required in [
        "bootstrap_expr_alloc_int_lit(",
        "bootstrap_expr_alloc_ident(",
        "bootstrap_expr_alloc_binary(",
        "bootstrap_expr_alloc_unary(",
        "bootstrap_expr_alloc_call(",
        "bootstrap_expr_alloc_if(",
        "bootstrap_expr_alloc_block(",
    ] {
        assert!(
            expr_handle_body.contains(required),
            "expr_bootstrap_handle must call `{required}` to store real AST nodes"
        );
    }

    // The legacy fingerprint encoding must be gone: no arithmetic on integer
    // literals like `3100`/`3700` to derive expr handles.
    for forbidden in ["3100 + value", "3700 +", "3800 +", "3900 +", "4000 +"] {
        assert!(
            !expr_handle_body.contains(forbidden),
            "expr_bootstrap_handle still uses fingerprint encoding `{forbidden}`"
        );
    }

    let stmt_handle_body =
        parser_gr_function_body(&parser_src, "fn stmt_bootstrap_handle(stmt: Stmt) -> Int:")
            .expect("parser.gr must define fn stmt_bootstrap_handle");
    for required in [
        "bootstrap_stmt_alloc(stmt_tag_let()",
        "bootstrap_stmt_alloc(stmt_tag_expr()",
        "bootstrap_stmt_alloc(stmt_tag_ret()",
    ] {
        assert!(
            stmt_handle_body.contains(required),
            "stmt_bootstrap_handle must call `{required}` to store real Stmt nodes"
        );
    }
    for forbidden in ["5100 +", "5200 +", "5300 +", "5400 +"] {
        assert!(
            !stmt_handle_body.contains(forbidden),
            "stmt_bootstrap_handle still uses fingerprint encoding `{forbidden}`"
        );
    }

    // Param / Function / ModuleItem builders must allocate real nodes.
    let param_body = parser_gr_function_body(
        &parser_src,
        "fn param_bootstrap_handle(param: Param) -> Int:",
    )
    .expect("parser.gr must define fn param_bootstrap_handle");
    assert!(
        param_body.contains("bootstrap_param_alloc(param.name"),
        "param_bootstrap_handle must allocate via bootstrap_param_alloc"
    );
    assert!(
        !param_body.contains("7100 +"),
        "param_bootstrap_handle still uses fingerprint encoding"
    );

    let function_body = parser_gr_function_body(
        &parser_src,
        "fn function_bootstrap_handle(fn_def: Function) -> Int:",
    )
    .expect("parser.gr must define fn function_bootstrap_handle");
    assert!(
        function_body.contains("bootstrap_function_alloc(fn_def.name"),
        "function_bootstrap_handle must allocate via bootstrap_function_alloc"
    );
    assert!(
        !function_body.contains("8100 +"),
        "function_bootstrap_handle still uses fingerprint encoding"
    );

    // List helpers must drive runtime list handles, not accumulate count
    // integers via `count + *_bootstrap_handle(...)`.
    let stmt_list_body = parser_gr_function_body(
        &parser_src,
        "fn parse_stmt_list(p: Parser) -> (Parser, StmtList):",
    )
    .expect("parser.gr must define fn parse_stmt_list");
    assert!(
        stmt_list_body.contains("bootstrap_stmt_list_alloc()"),
        "parse_stmt_list must allocate a runtime stmt-id list via bootstrap_stmt_list_alloc"
    );
    let stmt_list_helper = parser_gr_function_body(
        &parser_src,
        "fn parse_stmt_list_helper(p: Parser, list_handle: Int) -> (Parser, StmtList):",
    )
    .expect("parser.gr must define fn parse_stmt_list_helper with list_handle");
    assert!(
        stmt_list_helper.contains("bootstrap_node_list_append(list_handle"),
        "parse_stmt_list_helper must append stmt ids via bootstrap_node_list_append"
    );
    assert!(
        !stmt_list_helper.contains("count + stmt_bootstrap_handle"),
        "parse_stmt_list_helper still folds counts instead of appending node ids"
    );

    let param_list_helper = parser_gr_function_body(
        &parser_src,
        "fn parse_param_list_helper(p: Parser, list_handle: Int) -> (Parser, ParamList):",
    )
    .expect("parser.gr must define fn parse_param_list_helper with list_handle");
    assert!(
        param_list_helper.contains("bootstrap_node_list_append(list_handle"),
        "parse_param_list_helper must append param ids via bootstrap_node_list_append"
    );

    // Normalized export must walk via the new accessors. The legacy
    // `*_handle` JSON form is replaced with tree-shaped `left` / `right` /
    // `operand` / `cond` / `then` / `else` / `value` / `pattern` / `body` /
    // `params` / `items` payloads.
    let export_body = parser_gr_function_body(
        &parser_src,
        "fn normalized_expr_to_json_by_id(id: Int) -> String:",
    )
    .expect("parser.gr must define fn normalized_expr_to_json_by_id");
    for required in [
        "bootstrap_expr_get_tag(id)",
        "bootstrap_expr_get_int_value(id)",
        "bootstrap_expr_get_text(id)",
        "bootstrap_expr_get_child_a(id)",
        "bootstrap_expr_get_child_b(id)",
    ] {
        assert!(
            export_body.contains(required),
            "normalized_expr_to_json_by_id must call `{required}`"
        );
    }

    let function_export_body = parser_gr_function_body(
        &parser_src,
        "fn normalized_function_to_json_by_id(id: Int) -> String:",
    )
    .expect("parser.gr must define fn normalized_function_to_json_by_id");
    assert!(
        function_export_body.contains("bootstrap_function_get_name(id)"),
        "normalized_function_to_json_by_id must walk via bootstrap_function_get_name"
    );
    assert!(
        function_export_body.contains("normalized_param_list_to_json"),
        "normalized_function_to_json_by_id must walk params via normalized_param_list_to_json"
    );
    assert!(
        function_export_body.contains("normalized_stmt_list_to_json"),
        "normalized_function_to_json_by_id must walk body via normalized_stmt_list_to_json"
    );

    // The legacy `*_handle` keys must be gone from the canonical JSON form.
    // A few legacy strings are still used by the comment header / readiness
    // doc, so anchor the check on the JSON keys themselves.
    for forbidden in [
        "\\\"left_handle\\\":",
        "\\\"right_handle\\\":",
        "\\\"operand_handle\\\":",
        "\\\"callee_handle\\\":",
        "\\\"args_handle\\\":",
        "\\\"cond_handle\\\":",
        "\\\"then_handle\\\":",
        "\\\"else_handle\\\":",
        "\\\"stmts_handle\\\":",
        "\\\"final_expr_handle\\\":",
        "\\\"pattern_handle\\\":",
        "\\\"type_handle\\\":",
        "\\\"value_handle\\\":",
        "\\\"params_handle\\\":",
        "\\\"body_handle\\\":",
        "\\\"items_handle\\\":",
    ] {
        assert!(
            !parser_src.contains(forbidden),
            "parser.gr normalized export still emits legacy `{forbidden}` key"
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

/// Issue #225: checker.gr must drive a runtime-backed type environment
/// instead of stub `lookup_*`/`insert_*` placeholders, and must dispatch
/// on real parser AST handles rather than hardcoded `IntLitKind(0)` /
/// `ExprKind(0)` placeholders. The body of every env / dispatch helper
/// must call into the new `bootstrap_checker_env_*` and `bootstrap_*_get_*`
/// externs.
#[test]
fn checker_gr_uses_runtime_backed_env_and_ast_dispatch() {
    let checker_src =
        std::fs::read_to_string(compiler_path("checker.gr")).expect("Failed to read checker.gr");

    // Required extern declarations at module scope.
    for extern_decl in [
        "fn bootstrap_checker_env_alloc(parent: Int, scope_level: Int) -> Int",
        "fn bootstrap_checker_env_insert_var(env_id: Int, name: String, type_tag: Int, type_name: String, is_mut: Int, scope_level: Int) -> Int",
        "fn bootstrap_checker_env_insert_fn(env_id: Int, name: String, params_handle: Int, ret_type_tag: Int, ret_type_name: String, effects_handle: Int, is_extern: Int) -> Int",
        "fn bootstrap_checker_env_lookup_var(env_id: Int, name: String) -> Int",
        "fn bootstrap_checker_env_lookup_fn(env_id: Int, name: String) -> Int",
        "fn bootstrap_checker_env_get_parent(env_id: Int) -> Int",
        "fn bootstrap_checker_env_get_scope_level(env_id: Int) -> Int",
        "fn bootstrap_checker_var_get_name(var_id: Int) -> String",
        "fn bootstrap_checker_var_get_type_tag(var_id: Int) -> Int",
        "fn bootstrap_checker_fn_get_name(fn_id: Int) -> String",
        "fn bootstrap_checker_fn_get_ret_type_tag(fn_id: Int) -> Int",
        "fn bootstrap_expr_get_tag(id: Int) -> Int",
        "fn bootstrap_stmt_get_tag(id: Int) -> Int",
        "fn bootstrap_node_list_len(handle: Int) -> Int",
        "fn bootstrap_node_list_get(handle: Int, index: Int) -> Int",
    ] {
        assert!(
            checker_src.contains(extern_decl),
            "checker.gr must declare bootstrap extern `{extern_decl}`"
        );
    }

    // The four core env helpers must hit the runtime store rather than
    // returning constant placeholders.
    let lookup_var_body = parser_gr_function_body(
        &checker_src,
        "fn lookup_var(env: TypeEnv, name: String) -> VarInfo:",
    )
    .expect("checker.gr must define fn lookup_var");
    assert!(
        lookup_var_body.contains("bootstrap_checker_env_lookup_var(env.env_id, name)"),
        "lookup_var must consult the runtime env store"
    );

    let insert_var_body = parser_gr_function_body(
        &checker_src,
        "fn insert_var(env: TypeEnv, info: VarInfo) -> TypeEnv:",
    )
    .expect("checker.gr must define fn insert_var");
    assert!(
        insert_var_body.contains("bootstrap_checker_env_insert_var("),
        "insert_var must allocate a new runtime env frame"
    );

    let lookup_fn_body = parser_gr_function_body(
        &checker_src,
        "fn lookup_fn(env: TypeEnv, name: String) -> FnInfo:",
    )
    .expect("checker.gr must define fn lookup_fn");
    assert!(
        lookup_fn_body.contains("bootstrap_checker_env_lookup_fn(env.env_id, name)"),
        "lookup_fn must consult the runtime env store"
    );

    let insert_fn_body = parser_gr_function_body(
        &checker_src,
        "fn insert_fn(env: TypeEnv, info: FnInfo) -> TypeEnv:",
    )
    .expect("checker.gr must define fn insert_fn");
    assert!(
        insert_fn_body.contains("bootstrap_checker_env_insert_fn("),
        "insert_fn must allocate a new runtime env frame"
    );

    // Expression dispatch must read the real tag instead of the legacy
    // `IntLitKind(0)` stub.
    let get_expr_kind_body =
        parser_gr_function_body(&checker_src, "fn get_expr_kind(expr_id: Int) -> ExprKind:")
            .expect("checker.gr must define fn get_expr_kind");
    assert!(
        get_expr_kind_body.contains("bootstrap_expr_get_tag(expr_id)"),
        "get_expr_kind must dispatch on the real bootstrap AST tag"
    );
    for expected in [
        "IntLitKind(",
        "BoolLitKind(",
        "StringLitKind(",
        "IdentKind(",
        "BinaryKind(",
        "UnaryKind(",
        "CallKind(",
        "IfKind(",
        "BlockKind(",
    ] {
        assert!(
            get_expr_kind_body.contains(expected),
            "get_expr_kind must surface variant `{expected}` from the AST store"
        );
    }

    // Statement dispatch must mirror parser stmt tags.
    let get_stmt_kind_body =
        parser_gr_function_body(&checker_src, "fn get_stmt_kind(stmt_id: Int) -> StmtKind:")
            .expect("checker.gr must define fn get_stmt_kind");
    assert!(
        get_stmt_kind_body.contains("bootstrap_stmt_get_tag(stmt_id)"),
        "get_stmt_kind must dispatch on the real bootstrap AST tag"
    );
    for expected in ["StmtLet(", "StmtExpr(", "StmtRet(", "StmtIf(", "StmtWhile("] {
        assert!(
            get_stmt_kind_body.contains(expected),
            "get_stmt_kind must surface variant `{expected}` from the AST store"
        );
    }

    // The statement list walker must iterate via the runtime list helpers.
    let stmt_list_body = parser_gr_function_body(
        &checker_src,
        "fn check_stmt_list(c: Checker, stmts_id: Int) -> Checker:",
    )
    .expect("checker.gr must define fn check_stmt_list");
    assert!(
        stmt_list_body.contains("bootstrap_node_list_len(stmts_id)"),
        "check_stmt_list must walk the runtime stmt list, not return c"
    );

    // Old placeholder shapes must be gone.
    let forbidden_pairs: &[(&str, &str)] = &[
        (
            "ret IntLitKind(0)",
            "checker.gr still returns the placeholder IntLitKind(0) without dispatching",
        ),
        (
            "ret ExprKind(0)",
            "checker.gr still returns the placeholder ExprKind(0) without dispatching",
        ),
    ];
    for (forbidden, msg) in forbidden_pairs {
        // The fallback at the very bottom of get_expr_kind / get_stmt_kind
        // is allowed (it returns a single safe variant for unknown
        // tags). What we forbid is *the function body being only that
        // line*. Detect by counting occurrences: a single occurrence is
        // ok (the fallback); two or more means the stub is still there.
        let count = checker_src.matches(forbidden).count();
        assert!(count <= 1, "{msg} (found `{forbidden}` {count} times)");
    }
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
        parser_content.contains("ret \"canonical-json-v2\""),
        "parser.gr normalized export contract version should be canonical-json-v2 (#222)"
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
