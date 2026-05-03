//! Integration gate for #259: ModBlock-ExternFn unblocker.
//!
//! Verifies every `bootstrap_*` extern surface that the .gr-side compiler
//! delegates to is registered in `TypeEnv::new()` so the typechecker can
//! resolve calls like `bootstrap_query_new_session(source)` from inside
//! `compiler/query.gr`, `compiler/lsp.gr`, `compiler/compiler.gr`,
//! `compiler/main.gr`, and `compiler/codegen.gr` once those modules flip
//! from documentation-only stubs to real delegating bodies.
//!
//! Without this registration, the typechecker's `ModBlock` first-pass at
//! `checker.rs:472` only registers `TypeDecl` / `EnumDecl` / `FnDef` from
//! inside `mod` blocks — bare `fn name(args) -> Ret` extern-style
//! declarations inside a `mod` block are not surfaced as functions.
//! See handoff entry #347.

use gradient_compiler::typechecker::env::TypeEnv;
use gradient_compiler::typechecker::types::Ty;

fn assert_registered(env: &TypeEnv, name: &str, expected_arity: usize, expected_ret: Ty) {
    let sig = env
        .lookup_fn(name)
        .unwrap_or_else(|| panic!("expected `{}` to be registered in TypeEnv::new()", name));
    assert_eq!(
        sig.params.len(),
        expected_arity,
        "{} arity mismatch: expected {} params, got {}",
        name,
        expected_arity,
        sig.params.len()
    );
    assert_eq!(
        sig.ret, expected_ret,
        "{} return type mismatch: expected {:?}, got {:?}",
        name, expected_ret, sig.ret
    );
}

/// Smoke check that the ir-bridge surface (#227 / #239) is reachable.
#[test]
fn bootstrap_ir_bridge_externs_registered() {
    let env = TypeEnv::new();
    assert_registered(&env, "bootstrap_ir_type_alloc_primitive", 1, Ty::Int);
    assert_registered(&env, "bootstrap_ir_type_alloc_named", 1, Ty::Int);
    assert_registered(&env, "bootstrap_ir_type_get_name", 1, Ty::String);
    assert_registered(&env, "bootstrap_ir_value_alloc_const_int", 2, Ty::Int);
    assert_registered(&env, "bootstrap_ir_value_alloc_const_float", 2, Ty::Int);
    // 9-arg generic instruction allocator
    assert_registered(&env, "bootstrap_ir_instr_alloc", 9, Ty::Int);
    assert_registered(&env, "bootstrap_ir_block_alloc", 1, Ty::Int);
    assert_registered(&env, "bootstrap_ir_function_alloc", 2, Ty::Int);
    assert_registered(&env, "bootstrap_ir_module_alloc", 1, Ty::Int);
    assert_registered(&env, "bootstrap_ir_module_get_function_at", 2, Ty::Int);
    // List helpers
    assert_registered(&env, "bootstrap_ir_value_list_alloc", 0, Ty::Int);
    assert_registered(&env, "bootstrap_ir_int_list_alloc", 0, Ty::Int);
    assert_registered(&env, "bootstrap_ir_list_append", 2, Ty::Int);
    assert_registered(&env, "bootstrap_ir_list_len", 1, Ty::Int);
    assert_registered(&env, "bootstrap_ir_list_get", 2, Ty::Int);
}

/// Codegen text emission (#229).
#[test]
fn bootstrap_ir_emit_extern_registered() {
    let env = TypeEnv::new();
    assert_registered(&env, "bootstrap_ir_emit_text", 1, Ty::String);
}

/// Pipeline session surface (#230).
#[test]
fn bootstrap_pipeline_externs_registered() {
    let env = TypeEnv::new();
    assert_registered(&env, "bootstrap_pipeline_lex", 2, Ty::Int);
    assert_registered(&env, "bootstrap_pipeline_token_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_pipeline_parse", 1, Ty::Int);
    assert_registered(&env, "bootstrap_pipeline_parse_error_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_pipeline_check", 1, Ty::Int);
    assert_registered(&env, "bootstrap_pipeline_lower", 2, Ty::Int);
    assert_registered(&env, "bootstrap_pipeline_emit", 1, Ty::String);
}

/// Driver run surface (#231).
#[test]
fn bootstrap_driver_externs_registered() {
    let env = TypeEnv::new();
    assert_registered(&env, "bootstrap_driver_run_source", 2, Ty::Int);
    assert_registered(&env, "bootstrap_driver_run_file", 2, Ty::Int);
    assert_registered(&env, "bootstrap_driver_get_exit_code", 1, Ty::Int);
    assert_registered(&env, "bootstrap_driver_get_diagnostic_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_driver_get_diagnostic_at", 2, Ty::String);
    assert_registered(&env, "bootstrap_driver_get_captured_output", 1, Ty::String);
    assert_registered(&env, "bootstrap_driver_get_written_path", 1, Ty::String);
    assert_registered(&env, "bootstrap_driver_get_module_name", 1, Ty::String);
}

/// Query session surface (#232).
#[test]
fn bootstrap_query_externs_registered() {
    let env = TypeEnv::new();
    // Session lifecycle.
    assert_registered(&env, "bootstrap_query_new_session", 1, Ty::Int);
    assert_registered(&env, "bootstrap_query_session_count", 0, Ty::Int);
    assert_registered(&env, "bootstrap_query_session_source", 1, Ty::String);
    // Status.
    assert_registered(&env, "bootstrap_query_check_ok", 1, Ty::Int);
    assert_registered(&env, "bootstrap_query_error_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_query_parse_error_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_query_type_error_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_query_is_type_checked", 1, Ty::Int);
    // Diagnostics.
    assert_registered(&env, "bootstrap_query_diagnostic_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_query_diagnostic_phase", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_diagnostic_severity", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_diagnostic_message", 2, Ty::String);
    assert_registered(&env, "bootstrap_query_diagnostic_line", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_diagnostic_col", 2, Ty::Int);
    // Symbols.
    assert_registered(&env, "bootstrap_query_symbol_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_name", 2, Ty::String);
    assert_registered(&env, "bootstrap_query_symbol_kind", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_type", 2, Ty::String);
    assert_registered(&env, "bootstrap_query_symbol_is_pure", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_is_extern", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_is_export", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_is_test", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_line", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_col", 2, Ty::Int);
    // Symbol detail.
    assert_registered(&env, "bootstrap_query_symbol_param_count", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_param_name", 3, Ty::String);
    assert_registered(&env, "bootstrap_query_symbol_param_type", 3, Ty::String);
    assert_registered(&env, "bootstrap_query_symbol_effect_count", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_effect_at", 3, Ty::String);
    // Find / position queries.
    assert_registered(&env, "bootstrap_query_find_symbol", 2, Ty::Int);
    assert_registered(&env, "bootstrap_query_symbol_at", 3, Ty::Int);
    assert_registered(&env, "bootstrap_query_type_at", 3, Ty::String);
}

/// LSP server surface (#233).
#[test]
fn bootstrap_lsp_externs_registered() {
    let env = TypeEnv::new();
    // Server lifecycle.
    assert_registered(&env, "bootstrap_lsp_new_server", 0, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_initialize", 1, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_is_initialized", 1, Ty::Int);
    // Document sync.
    assert_registered(&env, "bootstrap_lsp_did_open", 5, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_did_change", 4, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_did_close", 2, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_did_save", 3, Ty::Int);
    // Document state.
    assert_registered(&env, "bootstrap_lsp_document_count", 1, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_document_text", 2, Ty::String);
    assert_registered(&env, "bootstrap_lsp_document_version", 2, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_document_session", 2, Ty::Int);
    // Diagnostics.
    assert_registered(&env, "bootstrap_lsp_diagnostic_count", 2, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_diagnostic_severity", 3, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_diagnostic_message", 3, Ty::String);
    assert_registered(&env, "bootstrap_lsp_diagnostic_line", 3, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_diagnostic_character", 3, Ty::Int);
    // Document symbols.
    assert_registered(&env, "bootstrap_lsp_document_symbol_count", 2, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_document_symbol_name", 3, Ty::String);
    assert_registered(&env, "bootstrap_lsp_document_symbol_kind", 3, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_document_symbol_line", 3, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_document_symbol_character", 3, Ty::Int);
    // Hover / completion.
    assert_registered(&env, "bootstrap_lsp_hover", 4, Ty::String);
    assert_registered(&env, "bootstrap_lsp_completion_count", 2, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_completion_label", 3, Ty::String);
    assert_registered(&env, "bootstrap_lsp_completion_kind", 3, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_completion_detail", 3, Ty::String);
    // Goto definition (#293).
    assert_registered(&env, "bootstrap_lsp_goto_definition_start_line", 4, Ty::Int);
    assert_registered(
        &env,
        "bootstrap_lsp_goto_definition_start_character",
        4,
        Ty::Int,
    );
    assert_registered(&env, "bootstrap_lsp_goto_definition_end_line", 4, Ty::Int);
    assert_registered(
        &env,
        "bootstrap_lsp_goto_definition_end_character",
        4,
        Ty::Int,
    );
    // Static keyword/builtin tables.
    assert_registered(&env, "bootstrap_lsp_is_keyword", 1, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_is_builtin", 1, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_keyword_count", 0, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_keyword_at", 1, Ty::String);
    assert_registered(&env, "bootstrap_lsp_builtin_count", 0, Ty::Int);
    assert_registered(&env, "bootstrap_lsp_builtin_name_at", 1, Ty::String);
    assert_registered(&env, "bootstrap_lsp_builtin_signature_at", 1, Ty::String);
}

/// Aggregate sanity check: every previously-registered bootstrap surface
/// (lexer/parser/AST/checker) plus the new ir/pipeline/driver/query/lsp
/// surfaces are all visible. Catches accidental removal of an entire
/// section if someone refactors `TypeEnv::new()`.
#[test]
fn bootstrap_total_extern_count_meets_expected_floor() {
    let env = TypeEnv::new();
    let bootstrap_fn_count = env
        .all_functions()
        .keys()
        .filter(|k| k.starts_with("bootstrap_"))
        .count();
    // Pre-#259 baseline: ~30 bootstrap externs registered (lexer, parser,
    // AST, checker). #259 adds ir_bridge (65) + ir_emit (1) + pipeline (7)
    // + driver (8) + query (32) + lsp (33) = 146 new entries, total ≥ 170.
    // #293 adds 4 lsp goto_definition externs → total ≥ 174.
    assert!(
        bootstrap_fn_count >= 174,
        "expected at least 174 bootstrap_* externs registered, got {}",
        bootstrap_fn_count
    );
}
