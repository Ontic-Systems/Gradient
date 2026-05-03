//! Issue #234: end-to-end bootstrap trust checks.
//!
//! Proves the self-hosted bootstrap pipeline can drive a small but
//! growing suite of non-trivial Gradient programs through the same
//! observable phases as the Rust host pipeline (lex → parse → check
//! → lower → emit) and produces non-placeholder output at each
//! supported phase.
//!
//! Each fixture under `tests/bootstrap_trust_corpus/` is a standalone
//! `.gr` source file. For each fixture we:
//!
//!   1. Run the bootstrap pipeline (`bootstrap_pipeline_*`) and the
//!      Rust host pipeline directly on the same source.
//!   2. Confirm both sides agree on whether the program parses /
//!      type-checks.
//!   3. Confirm the bootstrap pipeline emits non-empty IR text for
//!      programs that reach the lower/emit phase.
//!   4. Confirm `bootstrap_driver_run_source` returns the expected
//!      structured exit code AND captures non-placeholder output for
//!      successful programs.
//!   5. Confirm the bootstrap query kernel returns at least one real
//!      symbol for fixtures that contain top-level `fn` items.
//!
//! Stage-mismatch failures must be loud — empty captured output for a
//! fixture that's supposed to compile is treated as a regression and
//! must fail the trust check.

use std::path::PathBuf;

use gradient_compiler::bootstrap_driver::{
    bootstrap_driver_get_captured_output, bootstrap_driver_get_diagnostic_count,
    bootstrap_driver_get_exit_code, bootstrap_driver_run_source, reset_driver_store, DRIVER_OK,
    DRIVER_PARSE_ERROR, DRIVER_TYPE_ERROR,
};
use gradient_compiler::bootstrap_ir_bridge::{reset_ir_store, shared_test_lock};
use gradient_compiler::bootstrap_pipeline::{
    bootstrap_pipeline_check, bootstrap_pipeline_emit, bootstrap_pipeline_lex,
    bootstrap_pipeline_lower, bootstrap_pipeline_parse, bootstrap_pipeline_parse_error_count,
    bootstrap_pipeline_token_count, reset_pipeline_store,
};
use gradient_compiler::bootstrap_query::{
    bootstrap_query_check_ok, bootstrap_query_diagnostic_count, bootstrap_query_error_count,
    bootstrap_query_new_session, bootstrap_query_symbol_count, reset_query_store,
};
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser as ast_parser;
use gradient_compiler::typechecker;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bootstrap_trust_corpus")
}

fn fixture(name: &str) -> String {
    let path = corpus_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {}", path.display(), e))
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    shared_test_lock()
}

/// Reset every per-process store the trust check exercises.
fn reset_all() {
    reset_driver_store();
    reset_pipeline_store();
    reset_query_store();
    reset_ir_store();
}

/// Outcome the host (Rust) pipeline produced on a given source.
#[derive(Debug)]
struct HostOutcome {
    parse_errors: usize,
    type_errors: usize,
    function_count: usize,
}

fn run_host(src: &str) -> HostOutcome {
    let mut lex = Lexer::new(src, 0);
    let toks = lex.tokenize();
    let (module, parse_errors) = ast_parser::parse(toks, 0);
    let type_errors_full = if parse_errors.is_empty() {
        typechecker::check_module(&module, 0)
    } else {
        Vec::new()
    };
    let type_errors = type_errors_full.iter().filter(|e| !e.is_warning).count();
    use gradient_compiler::ast::item::ItemKind;
    let function_count = module
        .items
        .iter()
        .filter(|i| matches!(i.node, ItemKind::FnDef(_)))
        .count();
    HostOutcome {
        parse_errors: parse_errors.len(),
        type_errors,
        function_count,
    }
}

/// Run the bootstrap pipeline through every phase the fixture should
/// reach. Returns whether the bootstrap pipeline considers the
/// program valid AND non-trivial (i.e. it produced real IR + emitted
/// non-empty text).
fn run_bootstrap_full_compile(src: &str) -> (bool, String) {
    let session = bootstrap_pipeline_lex(src, 0);
    if session == 0 {
        return (false, String::new());
    }
    let token_count = bootstrap_pipeline_token_count(session);
    if token_count == 0 {
        return (false, String::new());
    }
    let items = bootstrap_pipeline_parse(session);
    if items == 0 || bootstrap_pipeline_parse_error_count(session) > 0 {
        return (false, String::new());
    }
    if bootstrap_pipeline_check(session) > 0 {
        return (false, String::new());
    }
    let ir = bootstrap_pipeline_lower(session, "trust");
    if ir == 0 {
        return (false, String::new());
    }
    let text = bootstrap_pipeline_emit(ir);
    if text.is_empty() {
        return (false, String::new());
    }
    (true, text)
}

// ── Trust-check helpers ─────────────────────────────────────────────────

/// Assert the bootstrap pipeline AND host pipeline agree the program
/// is fully valid, the bootstrap driver returns OK with non-empty
/// captured output, and the query layer reports the expected number
/// of top-level functions.
fn assert_full_compile_trust(name: &str, src: &str, must_contain: &[&str]) {
    let host = run_host(src);
    assert_eq!(
        host.parse_errors, 0,
        "host parser must accept fixture {}: {:?}",
        name, host
    );
    assert_eq!(
        host.type_errors, 0,
        "host typechecker must accept fixture {}: {:?}",
        name, host
    );
    assert!(
        host.function_count > 0,
        "fixture {} must contain at least one function",
        name
    );

    let (ok, text) = run_bootstrap_full_compile(src);
    assert!(
        ok,
        "bootstrap pipeline must compile fixture {} cleanly",
        name
    );
    assert!(
        !text.is_empty(),
        "bootstrap pipeline must emit non-empty text for fixture {}",
        name
    );
    for needle in must_contain {
        assert!(
            text.contains(needle),
            "bootstrap emission for {} must contain {:?}, got:\n{}",
            name,
            needle,
            text
        );
    }

    let run = bootstrap_driver_run_source(src, "");
    assert_eq!(
        bootstrap_driver_get_exit_code(run),
        DRIVER_OK,
        "bootstrap driver must return OK for fixture {}",
        name
    );
    assert_eq!(
        bootstrap_driver_get_diagnostic_count(run),
        0,
        "bootstrap driver must record no diagnostics for fixture {}",
        name
    );
    let captured = bootstrap_driver_get_captured_output(run);
    assert!(
        !captured.is_empty(),
        "bootstrap driver must capture non-empty output for fixture {} (placeholder regression)",
        name
    );
    for needle in must_contain {
        assert!(
            captured.contains(needle),
            "driver capture for {} must contain {:?}, got:\n{}",
            name,
            needle,
            captured
        );
    }

    let session = bootstrap_query_new_session(src);
    assert_eq!(
        bootstrap_query_check_ok(session),
        1,
        "query kernel must agree fixture {} is OK",
        name
    );
    assert_eq!(
        bootstrap_query_error_count(session),
        0,
        "query kernel must report zero errors for fixture {}",
        name
    );
    let symbol_count = bootstrap_query_symbol_count(session);
    assert!(
        symbol_count >= host.function_count as i64,
        "query kernel must report at least {} symbols for fixture {} (got {})",
        host.function_count,
        name,
        symbol_count
    );
}

/// Assert the host pipeline reports a parse error, the bootstrap
/// driver maps that to `DRIVER_PARSE_ERROR`, and the query kernel
/// surfaces at least one diagnostic.
fn assert_parse_error_trust(name: &str, src: &str) {
    let host = run_host(src);
    assert!(
        host.parse_errors > 0,
        "fixture {} must produce host parse errors: {:?}",
        name,
        host
    );
    let run = bootstrap_driver_run_source(src, "");
    assert_eq!(
        bootstrap_driver_get_exit_code(run),
        DRIVER_PARSE_ERROR,
        "bootstrap driver must map fixture {} to DRIVER_PARSE_ERROR",
        name
    );
    assert!(
        bootstrap_driver_get_diagnostic_count(run) > 0,
        "bootstrap driver must record diagnostics for fixture {}",
        name
    );

    let session = bootstrap_query_new_session(src);
    assert_eq!(
        bootstrap_query_check_ok(session),
        0,
        "query kernel must reject fixture {}",
        name
    );
    assert!(
        bootstrap_query_diagnostic_count(session) > 0,
        "query kernel must surface diagnostics for fixture {}",
        name
    );
}

/// Assert the host pipeline reports a type error, the bootstrap
/// driver maps that to `DRIVER_TYPE_ERROR`, and the query kernel
/// surfaces at least one diagnostic.
fn assert_type_error_trust(name: &str, src: &str) {
    let host = run_host(src);
    assert_eq!(
        host.parse_errors, 0,
        "fixture {} must parse cleanly: {:?}",
        name, host
    );
    assert!(
        host.type_errors > 0,
        "fixture {} must produce host type errors: {:?}",
        name,
        host
    );
    let run = bootstrap_driver_run_source(src, "");
    assert_eq!(
        bootstrap_driver_get_exit_code(run),
        DRIVER_TYPE_ERROR,
        "bootstrap driver must map fixture {} to DRIVER_TYPE_ERROR",
        name
    );

    let session = bootstrap_query_new_session(src);
    assert_eq!(bootstrap_query_check_ok(session), 0);
    assert!(bootstrap_query_diagnostic_count(session) > 0);
}

// ── Trust checks ────────────────────────────────────────────────────────

#[test]
fn trust_simple_arithmetic() {
    let _g = lock();
    reset_all();
    let src = fixture("01_simple_arithmetic.gr");
    assert_full_compile_trust("01_simple_arithmetic.gr", &src, &["fn add", "ret"]);
}

#[test]
fn trust_multi_function() {
    let _g = lock();
    reset_all();
    let src = fixture("02_multi_function.gr");
    assert_full_compile_trust(
        "02_multi_function.gr",
        &src,
        &["fn add", "fn sub", "fn mul"],
    );
}

#[test]
fn trust_let_bindings() {
    let _g = lock();
    reset_all();
    let src = fixture("03_let_bindings.gr");
    assert_full_compile_trust("03_let_bindings.gr", &src, &["fn compute", "ret"]);
}

#[test]
fn trust_function_calls() {
    let _g = lock();
    reset_all();
    let src = fixture("04_function_calls.gr");
    assert_full_compile_trust(
        "04_function_calls.gr",
        &src,
        &["fn helper", "fn caller"],
    );
}

#[test]
fn trust_boolean_logic() {
    let _g = lock();
    reset_all();
    let src = fixture("05_boolean_logic.gr");
    assert_full_compile_trust("05_boolean_logic.gr", &src, &["fn both", "fn either"]);
}

#[test]
fn trust_comparisons() {
    let _g = lock();
    reset_all();
    let src = fixture("06_comparisons.gr");
    assert_full_compile_trust("06_comparisons.gr", &src, &["fn lt", "fn ge", "fn eq"]);
}

#[test]
fn trust_unary_ops() {
    let _g = lock();
    reset_all();
    let src = fixture("07_unary_ops.gr");
    assert_full_compile_trust("07_unary_ops.gr", &src, &["fn negate", "fn invert"]);
}

#[test]
fn trust_nested_let() {
    let _g = lock();
    reset_all();
    let src = fixture("08_nested_let.gr");
    assert_full_compile_trust("08_nested_let.gr", &src, &["fn polynomial", "ret"]);
}

#[test]
fn trust_if_else() {
    let _g = lock();
    reset_all();
    let src = fixture("09_if_else.gr");
    assert_full_compile_trust("09_if_else.gr", &src, &["fn abs_value", "fn signum"]);
}

#[test]
fn trust_mutual_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("10_mutual_recursion.gr");
    assert_full_compile_trust("10_mutual_recursion.gr", &src, &["fn is_even", "fn is_odd"]);
}

#[test]
fn trust_nested_function_calls() {
    let _g = lock();
    reset_all();
    let src = fixture("11_nested_function_calls.gr");
    assert_full_compile_trust(
        "11_nested_function_calls.gr",
        &src,
        &["fn fma", "fn quad", "fn cube", "fn caller"],
    );
}

#[test]
fn trust_deep_let_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("12_deep_let_chain.gr");
    assert_full_compile_trust("12_deep_let_chain.gr", &src, &["fn deep"]);
}

#[test]
fn trust_comparison_matrix() {
    let _g = lock();
    reset_all();
    let src = fixture("13_comparison_matrix.gr");
    assert_full_compile_trust("13_comparison_matrix.gr", &src, &["fn classify"]);
}

#[test]
fn trust_if_expression_in_let() {
    let _g = lock();
    reset_all();
    let src = fixture("14_if_expression_in_let.gr");
    assert_full_compile_trust(
        "14_if_expression_in_let.gr",
        &src,
        &["fn pick", "fn max_of_three"],
    );
}

#[test]
fn trust_recursive_arithmetic() {
    let _g = lock();
    reset_all();
    let src = fixture("15_recursive_arithmetic.gr");
    assert_full_compile_trust(
        "15_recursive_arithmetic.gr",
        &src,
        &["fn factorial", "fn sum_to", "fn power"],
    );
}

#[test]
fn trust_parse_error_caught() {
    let _g = lock();
    reset_all();
    let src = fixture("90_parse_error.gr");
    assert_parse_error_trust("90_parse_error.gr", &src);
}

#[test]
fn trust_type_error_caught() {
    let _g = lock();
    reset_all();
    let src = fixture("91_type_error.gr");
    assert_type_error_trust("91_type_error.gr", &src);
}

#[test]
fn trust_phase_coverage_report() {
    // Meta-test that documents which phases each successful fixture
    // exercises. If a fixture stops reaching a phase, this test
    // surfaces the regression alongside the per-fixture asserts.
    let _g = lock();
    reset_all();

    let happy_path_fixtures = [
        "01_simple_arithmetic.gr",
        "02_multi_function.gr",
        "03_let_bindings.gr",
        "04_function_calls.gr",
        "05_boolean_logic.gr",
        "06_comparisons.gr",
        "07_unary_ops.gr",
        "08_nested_let.gr",
        "09_if_else.gr",
        "10_mutual_recursion.gr",
        "11_nested_function_calls.gr",
        "12_deep_let_chain.gr",
        "13_comparison_matrix.gr",
        "14_if_expression_in_let.gr",
        "15_recursive_arithmetic.gr",
    ];

    for name in &happy_path_fixtures {
        let src = fixture(name);

        // lex
        let session = bootstrap_pipeline_lex(&src, 0);
        assert!(session > 0, "{} must reach lex phase", name);
        assert!(
            bootstrap_pipeline_token_count(session) > 0,
            "{} must produce tokens",
            name
        );

        // parse
        let items = bootstrap_pipeline_parse(session);
        assert!(items > 0, "{} must reach parse phase with items", name);
        assert_eq!(
            bootstrap_pipeline_parse_error_count(session),
            0,
            "{} must parse cleanly",
            name
        );

        // check
        assert_eq!(
            bootstrap_pipeline_check(session),
            0,
            "{} must type-check cleanly",
            name
        );

        // lower
        let ir = bootstrap_pipeline_lower(session, "phase_coverage");
        assert!(ir > 0, "{} must reach lower phase", name);

        // emit
        let text = bootstrap_pipeline_emit(ir);
        assert!(!text.is_empty(), "{} must reach emit phase", name);
    }
}

#[test]
fn trust_rejects_empty_placeholder_success() {
    // Defensive test: make sure the trust-check infrastructure itself
    // would catch a future regression where the bootstrap driver
    // returns OK but emits nothing. We trigger this by feeding the
    // driver a syntactically valid module with NO bootstrap-subset
    // functions (e.g. only an actor declaration), which the driver
    // should reject with `DRIVER_LOWER_ERROR` rather than fabricate
    // empty success.
    let _g = lock();
    reset_all();
    let src = "actor Empty:\n    state count: Int = 0\n";
    let run = bootstrap_driver_run_source(src, "");
    let exit = bootstrap_driver_get_exit_code(run);
    assert_ne!(
        exit, DRIVER_OK,
        "driver must not return OK for a module with no bootstrap-subset functions; exit was {}",
        exit
    );
    let captured = bootstrap_driver_get_captured_output(run);
    assert!(
        captured.is_empty(),
        "driver must not capture output for a non-OK run; got:\n{}",
        captured
    );
}
