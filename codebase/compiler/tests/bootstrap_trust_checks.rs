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
fn trust_boolean_combinations() {
    let _g = lock();
    reset_all();
    let src = fixture("16_boolean_combinations.gr");
    assert_full_compile_trust(
        "16_boolean_combinations.gr",
        &src,
        &["fn at_least_two_true", "fn exactly_one_true", "fn xor3"],
    );
}

#[test]
fn trust_deep_nested_if_else() {
    let _g = lock();
    reset_all();
    let src = fixture("17_deep_nested_if_else.gr");
    assert_full_compile_trust(
        "17_deep_nested_if_else.gr",
        &src,
        &["fn classify", "fn sign_band"],
    );
}

#[test]
fn trust_arith_chains() {
    let _g = lock();
    reset_all();
    let src = fixture("18_arith_chains.gr");
    assert_full_compile_trust(
        "18_arith_chains.gr",
        &src,
        &["fn poly2", "fn dot4", "fn fma_chain"],
    );
}

#[test]
fn trust_compare_and_bool() {
    let _g = lock();
    reset_all();
    let src = fixture("19_compare_and_bool.gr");
    assert_full_compile_trust(
        "19_compare_and_bool.gr",
        &src,
        &[
            "fn in_range_strict",
            "fn in_range_inclusive",
            "fn outside_range",
            "fn equal_or_zero",
        ],
    );
}

#[test]
fn trust_recursion_with_conditionals() {
    let _g = lock();
    reset_all();
    let src = fixture("20_recursion_with_conditionals.gr");
    assert_full_compile_trust(
        "20_recursion_with_conditionals.gr",
        &src,
        &["fn is_even", "fn is_odd", "fn safe_div"],
    );
}

#[test]
fn trust_chained_function_calls() {
    let _g = lock();
    reset_all();
    let src = fixture("21_chained_function_calls.gr");
    assert_full_compile_trust(
        "21_chained_function_calls.gr",
        &src,
        &["fn add3", "fn double", "fn deep_chain", "fn computed_args"],
    );
}

#[test]
fn trust_nested_boolean_expr() {
    let _g = lock();
    reset_all();
    let src = fixture("22_nested_boolean_expr.gr");
    assert_full_compile_trust(
        "22_nested_boolean_expr.gr",
        &src,
        &[
            "fn precedence_demo",
            "fn paren_group",
            "fn negated_disjunction",
            "fn mixed_chain",
        ],
    );
}

#[test]
fn trust_signed_arithmetic() {
    let _g = lock();
    reset_all();
    let src = fixture("23_signed_arithmetic.gr");
    assert_full_compile_trust(
        "23_signed_arithmetic.gr",
        &src,
        &[
            "fn neg_then_add",
            "fn alternating_signs",
            "fn neg_product",
            "fn negate_sum",
            "fn signed_compare",
        ],
    );
}

#[test]
fn trust_let_then_call() {
    let _g = lock();
    reset_all();
    let src = fixture("24_let_then_call.gr");
    assert_full_compile_trust(
        "24_let_then_call.gr",
        &src,
        &["fn weighted_sum", "fn compute_then_call", "fn pipeline"],
    );
}

#[test]
fn trust_branch_heavy_ops() {
    let _g = lock();
    reset_all();
    let src = fixture("25_branch_heavy_ops.gr");
    assert_full_compile_trust(
        "25_branch_heavy_ops.gr",
        &src,
        &[
            "fn truth_band",
            "fn any_negative",
            "fn all_equal",
            "fn dense_branch",
        ],
    );
}

#[test]
fn trust_mixed_bool_compare_arith() {
    let _g = lock();
    reset_all();
    let src = fixture("26_mixed_bool_compare_arith.gr");
    assert_full_compile_trust(
        "26_mixed_bool_compare_arith.gr",
        &src,
        &["fn weighted_check", "fn band_score", "fn mix_reduce"],
    );
}

#[test]
fn trust_wide_mutual_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("27_wide_mutual_recursion.gr");
    assert_full_compile_trust(
        "27_wide_mutual_recursion.gr",
        &src,
        &["fn step_a", "fn step_b", "fn step_c", "fn cycle_three"],
    );
}

#[test]
fn trust_deep_let_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("28_deep_let_pipeline.gr");
    assert_full_compile_trust(
        "28_deep_let_pipeline.gr",
        &src,
        &["fn deep_pipeline", "fn fold_pipeline", "fn mixed_pipeline"],
    );
}

#[test]
fn trust_arith_identities() {
    let _g = lock();
    reset_all();
    let src = fixture("29_arith_identities.gr");
    assert_full_compile_trust(
        "29_arith_identities.gr",
        &src,
        &[
            "fn add_zero",
            "fn mul_one",
            "fn sub_self",
            "fn zero_product",
            "fn identity_chain",
            "fn double_negate",
            "fn id_with_branch",
        ],
    );
}

#[test]
fn trust_truth_table_if() {
    let _g = lock();
    reset_all();
    let src = fixture("30_truth_table_if.gr");
    assert_full_compile_trust(
        "30_truth_table_if.gr",
        &src,
        &[
            "fn xor_table",
            "fn three_input_majority",
            "fn implies",
            "fn nand",
            "fn truth_pick",
        ],
    );
}

#[test]
fn trust_comparison_transitivity() {
    let _g = lock();
    reset_all();
    let src = fixture("31_comparison_transitivity.gr");
    assert_full_compile_trust(
        "31_comparison_transitivity.gr",
        &src,
        &[
            "fn lt_chain",
            "fn le_chain",
            "fn eq_chain",
            "fn ordering_branch",
        ],
    );
}

#[test]
fn trust_deep_recursion_stack() {
    let _g = lock();
    reset_all();
    let src = fixture("32_deep_recursion_stack.gr");
    assert_full_compile_trust(
        "32_deep_recursion_stack.gr",
        &src,
        &[
            "fn countdown",
            "fn ackermann_lite",
            "fn deep_call",
            "fn driver",
        ],
    );
}

#[test]
fn trust_accumulator_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("33_accumulator_recursion.gr");
    assert_full_compile_trust(
        "33_accumulator_recursion.gr",
        &src,
        &[
            "fn sum_acc",
            "fn product_acc",
            "fn count_positive",
            "fn fold_driver",
        ],
    );
}

#[test]
fn trust_four_input_bool_reduce() {
    let _g = lock();
    reset_all();
    let src = fixture("34_four_input_bool_reduce.gr");
    assert_full_compile_trust(
        "34_four_input_bool_reduce.gr",
        &src,
        &[
            "fn all_four",
            "fn any_four",
            "fn exactly_two",
            "fn balanced_four",
        ],
    );
}

#[test]
fn trust_signed_chain_mixed() {
    let _g = lock();
    reset_all();
    let src = fixture("35_signed_chain_mixed.gr");
    assert_full_compile_trust(
        "35_signed_chain_mixed.gr",
        &src,
        &[
            "fn signed_balance",
            "fn signed_compare",
            "fn alternating",
            "fn signed_branch",
            "fn deep_signed_let",
        ],
    );
}

#[test]
fn trust_recursion_with_let_args() {
    let _g = lock();
    reset_all();
    let src = fixture("36_recursion_with_let_args.gr");
    assert_full_compile_trust(
        "36_recursion_with_let_args.gr",
        &src,
        &[
            "fn step_descent",
            "fn weighted_descent",
            "fn paired_descent",
            "fn let_arg_driver",
        ],
    );
}

#[test]
fn trust_conditional_accumulator() {
    let _g = lock();
    reset_all();
    let src = fixture("37_conditional_accumulator.gr");
    assert_full_compile_trust(
        "37_conditional_accumulator.gr",
        &src,
        &[
            "fn cond_sum",
            "fn parity_sum",
            "fn clamp_acc",
            "fn cond_acc_driver",
        ],
    );
}

#[test]
fn trust_wide_comparison_reduction() {
    let _g = lock();
    reset_all();
    let src = fixture("38_wide_comparison_reduction.gr");
    assert_full_compile_trust(
        "38_wide_comparison_reduction.gr",
        &src,
        &[
            "fn five_increasing",
            "fn five_any_zero",
            "fn five_all_positive",
            "fn six_band_filter",
            "fn wide_compare_driver",
        ],
    );
}

#[test]
fn trust_mutual_recursion_arith() {
    let _g = lock();
    reset_all();
    let src = fixture("39_mutual_recursion_arith.gr");
    assert_full_compile_trust(
        "39_mutual_recursion_arith.gr",
        &src,
        &[
            "fn alpha",
            "fn beta",
            "fn gamma",
            "fn dual_alpha",
            "fn dual_beta",
            "fn mutual_arith_driver",
        ],
    );
}

#[test]
fn trust_signed_recursive() {
    let _g = lock();
    reset_all();
    let src = fixture("40_signed_recursive.gr");
    assert_full_compile_trust(
        "40_signed_recursive.gr",
        &src,
        &[
            "fn signed_descent",
            "fn signed_swap_descent",
            "fn alt_sign_recursion",
            "fn signed_pair_descent",
            "fn signed_recursive_driver",
        ],
    );
}

#[test]
fn trust_nested_recursion_branches() {
    let _g = lock();
    reset_all();
    let src = fixture("41_nested_recursion_branches.gr");
    assert_full_compile_trust(
        "41_nested_recursion_branches.gr",
        &src,
        &[
            "fn branch_split",
            "fn dual_branch",
            "fn triple_recur",
            "fn nested_branch_driver",
        ],
    );
}

#[test]
fn trust_compare_chain_arith() {
    let _g = lock();
    reset_all();
    let src = fixture("42_compare_chain_arith.gr");
    assert_full_compile_trust(
        "42_compare_chain_arith.gr",
        &src,
        &[
            "fn cmp_chain_score",
            "fn alternating_compare",
            "fn chained_arith_after_compare",
            "fn compare_chain_driver",
        ],
    );
}

#[test]
fn trust_let_returning_bool() {
    let _g = lock();
    reset_all();
    let src = fixture("43_let_returning_bool.gr");
    assert_full_compile_trust(
        "43_let_returning_bool.gr",
        &src,
        &[
            "fn bool_let_combine",
            "fn bool_let_negation",
            "fn bool_let_dispatch",
            "fn bool_let_driver",
        ],
    );
}

#[test]
fn trust_arithmetic_distribution() {
    let _g = lock();
    reset_all();
    let src = fixture("44_arithmetic_distribution.gr");
    assert_full_compile_trust(
        "44_arithmetic_distribution.gr",
        &src,
        &[
            "fn distribute_left",
            "fn distribute_right",
            "fn factor_chain",
            "fn nested_distribution",
            "fn distribution_driver",
        ],
    );
}

#[test]
fn trust_deep_if_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("45_deep_if_chain.gr");
    assert_full_compile_trust(
        "45_deep_if_chain.gr",
        &src,
        &[
            "fn category_five",
            "fn sign_band",
            "fn pair_classify",
            "fn deep_if_driver",
        ],
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
        "16_boolean_combinations.gr",
        "17_deep_nested_if_else.gr",
        "18_arith_chains.gr",
        "19_compare_and_bool.gr",
        "20_recursion_with_conditionals.gr",
        "21_chained_function_calls.gr",
        "22_nested_boolean_expr.gr",
        "23_signed_arithmetic.gr",
        "24_let_then_call.gr",
        "25_branch_heavy_ops.gr",
        "26_mixed_bool_compare_arith.gr",
        "27_wide_mutual_recursion.gr",
        "28_deep_let_pipeline.gr",
        "29_arith_identities.gr",
        "30_truth_table_if.gr",
        "31_comparison_transitivity.gr",
        "32_deep_recursion_stack.gr",
        "33_accumulator_recursion.gr",
        "34_four_input_bool_reduce.gr",
        "35_signed_chain_mixed.gr",
        "36_recursion_with_let_args.gr",
        "37_conditional_accumulator.gr",
        "38_wide_comparison_reduction.gr",
        "39_mutual_recursion_arith.gr",
        "40_signed_recursive.gr",
        "41_nested_recursion_branches.gr",
        "42_compare_chain_arith.gr",
        "43_let_returning_bool.gr",
        "44_arithmetic_distribution.gr",
        "45_deep_if_chain.gr",
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
