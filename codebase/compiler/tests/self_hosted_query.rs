//! Integration gate for #232: self-hosted query service kernel.
//!
//! Drives `bootstrap_query_*` entry points through the same fixture
//! shapes the `.gr`-side `query.gr` will eventually delegate to. The
//! kernel is exercised directly here because the typechecker's
//! `ModBlock` first-pass doesn't yet register `ExternFn` declarations,
//! mirroring the strategy used by `self_hosted_pipeline.rs` (#230) and
//! `self_hosted_driver.rs` (#231).

use gradient_compiler::bootstrap_ir_bridge::shared_test_lock;
use gradient_compiler::bootstrap_query::{
    bootstrap_query_check_ok, bootstrap_query_diagnostic_count, bootstrap_query_diagnostic_message,
    bootstrap_query_diagnostic_phase, bootstrap_query_diagnostic_severity,
    bootstrap_query_error_count, bootstrap_query_find_symbol, bootstrap_query_new_session,
    bootstrap_query_parse_error_count, bootstrap_query_symbol_count,
    bootstrap_query_symbol_effect_at, bootstrap_query_symbol_effect_count,
    bootstrap_query_symbol_is_extern, bootstrap_query_symbol_is_pure,
    bootstrap_query_symbol_kind, bootstrap_query_symbol_name, bootstrap_query_symbol_param_count,
    bootstrap_query_symbol_param_name, bootstrap_query_symbol_param_type,
    bootstrap_query_symbol_type, bootstrap_query_type_at, bootstrap_query_type_error_count,
    reset_query_store, PHASE_PARSER, PHASE_TYPECHECKER, SEVERITY_ERROR, SYMBOL_KIND_EXTERN_FUNCTION,
    SYMBOL_KIND_FUNCTION,
};

fn lock() -> std::sync::MutexGuard<'static, ()> {
    shared_test_lock()
}

fn reset() {
    reset_query_store();
}

#[test]
fn happy_path_module_reports_real_symbols() {
    let _g = lock();
    reset();
    let src = "\
fn add(x: Int, y: Int) -> Int:
    ret x + y

fn negate(x: Int) -> Int:
    ret 0 - x
";
    let id = bootstrap_query_new_session(src);
    assert!(id > 0);
    assert_eq!(bootstrap_query_check_ok(id), 1, "valid module must check OK");
    assert_eq!(bootstrap_query_error_count(id), 0);
    assert_eq!(bootstrap_query_symbol_count(id), 2);

    let add_idx = bootstrap_query_find_symbol(id, "add");
    assert!(add_idx >= 0);
    assert_eq!(bootstrap_query_symbol_name(id, add_idx), "add");
    assert_eq!(bootstrap_query_symbol_kind(id, add_idx), SYMBOL_KIND_FUNCTION);
    assert_eq!(bootstrap_query_symbol_param_count(id, add_idx), 2);
    assert_eq!(bootstrap_query_symbol_param_name(id, add_idx, 0), "x");
    assert_eq!(bootstrap_query_symbol_param_type(id, add_idx, 1), "Int");
    let add_ty = bootstrap_query_symbol_type(id, add_idx);
    assert!(add_ty.contains("Int"), "add type: {}", add_ty);

    let negate_idx = bootstrap_query_find_symbol(id, "negate");
    assert!(negate_idx >= 0);
    assert_eq!(bootstrap_query_symbol_param_count(id, negate_idx), 1);
}

#[test]
fn parse_error_diagnostics_visible_via_query() {
    let _g = lock();
    reset();
    let bad = "fn broken(x: Int) -> Int:\n    ret x +\n";
    let id = bootstrap_query_new_session(bad);
    assert!(bootstrap_query_parse_error_count(id) > 0);
    assert_eq!(bootstrap_query_check_ok(id), 0);
    assert!(bootstrap_query_diagnostic_count(id) > 0);
    assert_eq!(bootstrap_query_diagnostic_phase(id, 0), PHASE_PARSER);
    assert_eq!(bootstrap_query_diagnostic_severity(id, 0), SEVERITY_ERROR);
    let msg = bootstrap_query_diagnostic_message(id, 0);
    assert!(!msg.is_empty(), "parse error must carry a message");
}

#[test]
fn type_error_diagnostics_visible_via_query() {
    let _g = lock();
    reset();
    let bad = "fn f(x: Int) -> Int:\n    ret bogus\n";
    let id = bootstrap_query_new_session(bad);
    assert_eq!(bootstrap_query_parse_error_count(id), 0);
    assert!(bootstrap_query_type_error_count(id) > 0);
    assert!(bootstrap_query_diagnostic_count(id) > 0);
    assert_eq!(bootstrap_query_diagnostic_phase(id, 0), PHASE_TYPECHECKER);
    let msg = bootstrap_query_diagnostic_message(id, 0);
    assert!(!msg.is_empty());
}

#[test]
fn extern_function_reports_extern_kind() {
    let _g = lock();
    reset();
    // `print` declared as extern via Gradient `extern fn` syntax.
    let src = "\
extern fn print(s: String)
fn main():
    print(\"hi\")
";
    let id = bootstrap_query_new_session(src);
    let print_idx = bootstrap_query_find_symbol(id, "print");
    assert!(print_idx >= 0, "expected to find print");
    assert_eq!(bootstrap_query_symbol_is_extern(id, print_idx), 1);
    assert_eq!(
        bootstrap_query_symbol_kind(id, print_idx),
        SYMBOL_KIND_EXTERN_FUNCTION
    );
    let main_idx = bootstrap_query_find_symbol(id, "main");
    assert!(main_idx >= 0);
    assert_eq!(bootstrap_query_symbol_is_extern(id, main_idx), 0);
}

#[test]
fn type_at_returns_function_type_for_top_level_symbol() {
    let _g = lock();
    reset();
    let src = "\
fn add(x: Int, y: Int) -> Int:
    ret x + y
";
    let id = bootstrap_query_new_session(src);
    // Position cursor on the `fn` keyword on line 1.
    let ty = bootstrap_query_type_at(id, 1, 1);
    assert!(
        ty.contains("Int"),
        "type_at on add() should return its function type, got {:?}",
        ty
    );
}

#[test]
fn pure_function_marked_pure() {
    let _g = lock();
    reset();
    let src = "\
fn pure_add(x: Int, y: Int) -> Int:
    ret x + y
";
    let id = bootstrap_query_new_session(src);
    let idx = bootstrap_query_find_symbol(id, "pure_add");
    assert!(idx >= 0);
    assert_eq!(
        bootstrap_query_symbol_is_pure(id, idx),
        1,
        "pure function should report is_pure = 1"
    );
    assert_eq!(bootstrap_query_symbol_effect_count(id, idx), 0);
}

#[test]
fn function_with_effect_set_reports_effects() {
    let _g = lock();
    reset();
    // Function with explicit IO effect declaration.
    let src = "\
extern fn print(s: String)
fn greet(name: String) -> !{IO} ():
    print(name)
";
    let id = bootstrap_query_new_session(src);
    let idx = bootstrap_query_find_symbol(id, "greet");
    assert!(idx >= 0, "expected greet symbol");
    let effect_count = bootstrap_query_symbol_effect_count(id, idx);
    assert!(
        effect_count > 0,
        "function with !{{IO}} should report at least one declared effect"
    );
    let first_effect = bootstrap_query_symbol_effect_at(id, idx, 0);
    assert_eq!(first_effect, "IO");
    assert_eq!(
        bootstrap_query_symbol_is_pure(id, idx),
        0,
        "function with !{{IO}} cannot be marked pure"
    );
}

#[test]
fn empty_source_session_yields_no_symbols() {
    let _g = lock();
    reset();
    let id = bootstrap_query_new_session("");
    assert_eq!(bootstrap_query_symbol_count(id), 0);
    assert_eq!(bootstrap_query_diagnostic_count(id), 0);
    assert_eq!(bootstrap_query_check_ok(id), 0);
    assert_eq!(bootstrap_query_find_symbol(id, "anything"), -1);
}

#[test]
fn unknown_session_id_returns_safe_defaults() {
    let _g = lock();
    reset();
    let phantom = 9999;
    assert_eq!(bootstrap_query_symbol_count(phantom), 0);
    assert_eq!(bootstrap_query_diagnostic_count(phantom), 0);
    assert_eq!(bootstrap_query_check_ok(phantom), 0);
    assert_eq!(bootstrap_query_symbol_name(phantom, 0), "");
    assert_eq!(bootstrap_query_symbol_kind(phantom, 0), 0);
    assert_eq!(bootstrap_query_type_at(phantom, 1, 1), "");
}

#[test]
fn multiple_sessions_are_independent() {
    let _g = lock();
    reset();
    let a = bootstrap_query_new_session("fn one() -> Int:\n    ret 1\n");
    let b = bootstrap_query_new_session("fn two() -> Int:\n    ret 2\n");
    assert_ne!(a, b);
    assert_eq!(bootstrap_query_symbol_name(a, 0), "one");
    assert_eq!(bootstrap_query_symbol_name(b, 0), "two");
    assert_eq!(bootstrap_query_check_ok(a), 1);
    assert_eq!(bootstrap_query_check_ok(b), 1);
}

#[test]
fn type_decl_appears_as_symbol() {
    let _g = lock();
    reset();
    let src = "\
type Meters = Int

fn hike(d: Meters) -> Meters:
    ret d
";
    let id = bootstrap_query_new_session(src);
    let meters_idx = bootstrap_query_find_symbol(id, "Meters");
    assert!(meters_idx >= 0, "expected Meters type-alias symbol");
    let hike_idx = bootstrap_query_find_symbol(id, "hike");
    assert!(hike_idx >= 0);
}
