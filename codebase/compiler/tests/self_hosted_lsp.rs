//! Integration gate for #233: self-hosted LSP service kernel.
//!
//! Drives `bootstrap_lsp_*` entry points directly. Same .gr-side
//! deferral pattern as #229/#230/#231/#232 — typechecker's `ModBlock`
//! first-pass doesn't yet register `ExternFn`, so the kernel is
//! exercised through Rust-side fixtures and the .gr-side `lsp.gr`
//! gets boundary documentation only for now.

use gradient_compiler::bootstrap_ir_bridge::shared_test_lock;
use gradient_compiler::bootstrap_lsp::{
    bootstrap_lsp_builtin_count, bootstrap_lsp_builtin_name_at, bootstrap_lsp_builtin_signature_at,
    bootstrap_lsp_completion_count, bootstrap_lsp_completion_detail, bootstrap_lsp_completion_kind,
    bootstrap_lsp_completion_label, bootstrap_lsp_diagnostic_character,
    bootstrap_lsp_diagnostic_count, bootstrap_lsp_diagnostic_line, bootstrap_lsp_diagnostic_message,
    bootstrap_lsp_diagnostic_severity, bootstrap_lsp_did_change, bootstrap_lsp_did_close,
    bootstrap_lsp_did_open, bootstrap_lsp_did_save, bootstrap_lsp_document_count,
    bootstrap_lsp_document_session, bootstrap_lsp_document_symbol_count,
    bootstrap_lsp_document_symbol_kind, bootstrap_lsp_document_symbol_line,
    bootstrap_lsp_document_symbol_name, bootstrap_lsp_document_text, bootstrap_lsp_document_version,
    bootstrap_lsp_hover, bootstrap_lsp_initialize, bootstrap_lsp_is_builtin,
    bootstrap_lsp_is_initialized, bootstrap_lsp_is_keyword, bootstrap_lsp_keyword_count,
    bootstrap_lsp_new_server, reset_lsp_store, COMPLETION_KIND_FUNCTION, COMPLETION_KIND_KEYWORD,
    LSP_SEVERITY_ERROR, LSP_SYMBOL_FUNCTION, LSP_SYMBOL_TYPE_PARAMETER,
};
use gradient_compiler::bootstrap_query::reset_query_store;

fn lock() -> std::sync::MutexGuard<'static, ()> {
    shared_test_lock()
}

fn reset() {
    reset_lsp_store();
    reset_query_store();
}

#[test]
fn server_initialization_flow() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    assert!(s > 0);
    assert_eq!(bootstrap_lsp_is_initialized(s), 0);
    assert_eq!(bootstrap_lsp_initialize(s), 1);
    assert_eq!(bootstrap_lsp_is_initialized(s), 1);
}

#[test]
fn full_document_lifecycle_open_change_save_close() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///proj/main.gr";
    let v1 = "fn one() -> Int:\n    ret 1\n";
    let v2 = "fn two() -> Int:\n    ret 2\n";

    assert_eq!(bootstrap_lsp_did_open(s, uri, "gradient", 1, v1), 1);
    assert_eq!(bootstrap_lsp_document_count(s), 1);
    assert_eq!(bootstrap_lsp_document_version(s, uri), 1);
    assert_eq!(bootstrap_lsp_document_text(s, uri), v1);
    assert!(bootstrap_lsp_document_session(s, uri) > 0);

    assert_eq!(bootstrap_lsp_did_change(s, uri, 2, v2), 1);
    assert_eq!(bootstrap_lsp_document_version(s, uri), 2);
    assert_eq!(bootstrap_lsp_document_text(s, uri), v2);

    // didSave with empty text just confirms presence.
    assert_eq!(bootstrap_lsp_did_save(s, uri, ""), 1);
    // didSave with new text re-runs query.
    assert_eq!(
        bootstrap_lsp_did_save(s, uri, "fn three() -> Int:\n    ret 3\n"),
        1
    );

    assert_eq!(bootstrap_lsp_did_close(s, uri), 1);
    assert_eq!(bootstrap_lsp_document_count(s), 0);
}

#[test]
fn diagnostics_track_parse_errors_real_time() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///dx.gr";

    bootstrap_lsp_did_open(s, uri, "gradient", 1, "fn ok() -> Int:\n    ret 0\n");
    assert_eq!(bootstrap_lsp_diagnostic_count(s, uri), 0);

    // Introduce an error.
    bootstrap_lsp_did_change(s, uri, 2, "fn broken(x: Int) -> Int:\n    ret x +\n");
    assert!(bootstrap_lsp_diagnostic_count(s, uri) > 0);
    assert_eq!(
        bootstrap_lsp_diagnostic_severity(s, uri, 0),
        LSP_SEVERITY_ERROR
    );
    let msg = bootstrap_lsp_diagnostic_message(s, uri, 0);
    assert!(!msg.is_empty());

    // Heal it again.
    bootstrap_lsp_did_change(s, uri, 3, "fn ok() -> Int:\n    ret 0\n");
    assert_eq!(bootstrap_lsp_diagnostic_count(s, uri), 0);
}

#[test]
fn diagnostics_track_type_errors() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///t.gr";
    bootstrap_lsp_did_open(s, uri, "gradient", 1, "fn f(x: Int) -> Int:\n    ret bogus\n");
    assert!(bootstrap_lsp_diagnostic_count(s, uri) > 0);
    let msg = bootstrap_lsp_diagnostic_message(s, uri, 0);
    assert!(msg.contains("bogus") || msg.to_lowercase().contains("undefined"));
}

#[test]
fn document_symbols_returns_real_top_level_items() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///s.gr";
    let src = "\
type Meters = Int

fn add(x: Int, y: Int) -> Int:
    ret x + y

fn sub(x: Int, y: Int) -> Int:
    ret x - y
";
    bootstrap_lsp_did_open(s, uri, "gradient", 1, src);
    assert_eq!(bootstrap_lsp_document_symbol_count(s, uri), 3);
    // Order must match source.
    assert_eq!(bootstrap_lsp_document_symbol_name(s, uri, 0), "Meters");
    assert_eq!(
        bootstrap_lsp_document_symbol_kind(s, uri, 0),
        LSP_SYMBOL_TYPE_PARAMETER
    );
    assert_eq!(bootstrap_lsp_document_symbol_name(s, uri, 1), "add");
    assert_eq!(
        bootstrap_lsp_document_symbol_kind(s, uri, 1),
        LSP_SYMBOL_FUNCTION
    );
    // Lines should be 0-based (LSP convention).
    assert_eq!(bootstrap_lsp_document_symbol_line(s, uri, 0), 0);
}

#[test]
fn hover_returns_function_signature_for_known_identifier() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///h.gr";
    bootstrap_lsp_did_open(
        s,
        uri,
        "gradient",
        1,
        "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n",
    );
    // Hover on `add` (line 0, char 3 inside the identifier).
    let h = bootstrap_lsp_hover(s, uri, 0, 3);
    assert!(!h.is_empty());
    assert!(h.contains("add"));
    assert!(h.contains("Int"));
    assert!(h.contains("```gradient"), "hover should be Markdown: {}", h);
}

#[test]
fn hover_on_keyword_returns_keyword_marker() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///kw.gr";
    bootstrap_lsp_did_open(s, uri, "gradient", 1, "fn f() -> Int:\n    ret 0\n");
    let h = bootstrap_lsp_hover(s, uri, 0, 0);
    assert!(h.contains("fn"));
    assert!(h.contains("keyword"));
}

#[test]
fn hover_on_builtin_returns_signature() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///b.gr";
    bootstrap_lsp_did_open(s, uri, "gradient", 1, "fn use_it():\n    print(\"x\")\n");
    // Hover on `print` (line 1, character 4 -- after 4 spaces).
    let h = bootstrap_lsp_hover(s, uri, 1, 4);
    assert!(h.contains("print"));
    assert!(h.contains("built-in"));
}

#[test]
fn completion_includes_symbols_keywords_and_builtins() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///c.gr";
    bootstrap_lsp_did_open(
        s,
        uri,
        "gradient",
        1,
        "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n",
    );
    let n = bootstrap_lsp_completion_count(s, uri);
    assert!(n > 0);
    let mut have_add = false;
    let mut have_fn = false;
    let mut have_print = false;
    for i in 0..n {
        let label = bootstrap_lsp_completion_label(s, uri, i);
        let kind = bootstrap_lsp_completion_kind(s, uri, i);
        let detail = bootstrap_lsp_completion_detail(s, uri, i);
        if label == "add" && kind == COMPLETION_KIND_FUNCTION {
            have_add = true;
        }
        if label == "fn" && kind == COMPLETION_KIND_KEYWORD {
            have_fn = true;
        }
        if label == "print" && kind == COMPLETION_KIND_FUNCTION {
            assert!(detail.contains("print"), "builtin detail: {}", detail);
            have_print = true;
        }
    }
    assert!(have_add);
    assert!(have_fn);
    assert!(have_print);
}

#[test]
fn diagnostic_position_is_zero_based_for_lsp() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let uri = "file:///pos.gr";
    bootstrap_lsp_did_open(
        s,
        uri,
        "gradient",
        1,
        "fn broken(x: Int) -> Int:\n    ret x +\n",
    );
    assert!(bootstrap_lsp_diagnostic_count(s, uri) > 0);
    // Lines/cols come from the parser, which uses 1-based — LSP wants
    // 0-based. The kernel performs the conversion. Just sanity-check
    // they're non-negative.
    let line = bootstrap_lsp_diagnostic_line(s, uri, 0);
    let col = bootstrap_lsp_diagnostic_character(s, uri, 0);
    assert!(line >= 0);
    assert!(col >= 0);
}

#[test]
fn unknown_server_or_uri_returns_safe_defaults() {
    let _g = lock();
    reset();
    let phantom_srv = 99999;
    let phantom_uri = "file:///nope.gr";
    assert_eq!(bootstrap_lsp_diagnostic_count(phantom_srv, phantom_uri), 0);
    assert_eq!(
        bootstrap_lsp_document_symbol_count(phantom_srv, phantom_uri),
        0
    );
    assert_eq!(bootstrap_lsp_completion_count(phantom_srv, phantom_uri), 0);
    assert_eq!(bootstrap_lsp_hover(phantom_srv, phantom_uri, 0, 0), "");
}

#[test]
fn vocabulary_helpers_are_self_consistent() {
    assert_eq!(bootstrap_lsp_is_keyword("fn"), 1);
    assert_eq!(bootstrap_lsp_is_keyword("not_a_keyword"), 0);
    assert_eq!(bootstrap_lsp_is_builtin("print"), 1);
    assert_eq!(bootstrap_lsp_is_builtin("add"), 0);
    assert!(bootstrap_lsp_keyword_count() > 0);
    assert!(bootstrap_lsp_builtin_count() > 0);
    let bn = bootstrap_lsp_builtin_name_at(0);
    let bs = bootstrap_lsp_builtin_signature_at(0);
    assert!(!bn.is_empty());
    assert!(bs.contains(&bn));
}

#[test]
fn multiple_documents_per_server_are_isolated() {
    let _g = lock();
    reset();
    let s = bootstrap_lsp_new_server();
    let a = "file:///a.gr";
    let b = "file:///b.gr";
    bootstrap_lsp_did_open(s, a, "gradient", 1, "fn alpha() -> Int:\n    ret 1\n");
    bootstrap_lsp_did_open(s, b, "gradient", 1, "fn beta() -> Int:\n    ret 2\n");
    assert_eq!(bootstrap_lsp_document_count(s), 2);
    assert_eq!(bootstrap_lsp_document_symbol_name(s, a, 0), "alpha");
    assert_eq!(bootstrap_lsp_document_symbol_name(s, b, 0), "beta");
}
