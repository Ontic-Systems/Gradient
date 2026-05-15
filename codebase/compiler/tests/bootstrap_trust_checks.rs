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
fn trust_call_compare_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("46_call_compare_pipeline.gr");
    assert_full_compile_trust(
        "46_call_compare_pipeline.gr",
        &src,
        &[
            "fn double",
            "fn triple",
            "fn add3",
            "fn cmp_call_chain",
            "fn nested_call_compare",
            "fn pipeline_driver",
        ],
    );
}

#[test]
fn trust_four_chain_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("47_four_chain_mutual_rec.gr");
    assert_full_compile_trust(
        "47_four_chain_mutual_rec.gr",
        &src,
        &[
            "fn alpha",
            "fn beta",
            "fn gamma",
            "fn delta",
            "fn helper_a",
            "fn helper_b",
            "fn four_chain_driver",
        ],
    );
}

#[test]
fn trust_boolean_truth_matrix() {
    let _g = lock();
    reset_all();
    let src = fixture("48_boolean_truth_matrix.gr");
    assert_full_compile_trust(
        "48_boolean_truth_matrix.gr",
        &src,
        &[
            "fn truth_row",
            "fn truth_from_compare",
            "fn truth_from_logic",
            "fn truth_matrix_driver",
        ],
    );
}

#[test]
fn trust_pipeline_let_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("49_pipeline_let_chain.gr");
    assert_full_compile_trust(
        "49_pipeline_let_chain.gr",
        &src,
        &[
            "fn add_two",
            "fn mul_two",
            "fn long_pipeline",
            "fn deep_compare_pipeline",
            "fn pipeline_let_driver",
        ],
    );
}

#[test]
fn trust_signed_branch_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("50_signed_branch_dispatch.gr");
    assert_full_compile_trust(
        "50_signed_branch_dispatch.gr",
        &src,
        &[
            "fn signed_dispatch",
            "fn signed_pair_dispatch",
            "fn signed_descent",
            "fn signed_branch_dispatch_driver",
        ],
    );
}

#[test]
fn trust_arith_with_division() {
    let _g = lock();
    reset_all();
    let src = fixture("51_arith_with_division.gr");
    assert_full_compile_trust(
        "51_arith_with_division.gr",
        &src,
        &[
            "fn halve",
            "fn quotient_chain",
            "fn divide_then_combine",
            "fn quotient_descent",
            "fn arith_division_driver",
        ],
    );
}

#[test]
fn trust_compare_let_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("52_compare_let_dispatch.gr");
    assert_full_compile_trust(
        "52_compare_let_dispatch.gr",
        &src,
        &[
            "fn compare_dispatch",
            "fn equal_dispatch",
            "fn compare_let_dispatch_driver",
        ],
    );
}

#[test]
fn trust_calls_inside_if_arms() {
    let _g = lock();
    reset_all();
    let src = fixture("53_calls_inside_if_arms.gr");
    assert_full_compile_trust(
        "53_calls_inside_if_arms.gr",
        &src,
        &[
            "fn add_pair",
            "fn add_triple",
            "fn add_five",
            "fn calls_inside_branches",
            "fn recursive_branch_calls",
            "fn calls_in_branches_driver",
        ],
    );
}

#[test]
fn trust_unary_minus_combos() {
    let _g = lock();
    reset_all();
    let src = fixture("54_unary_minus_combos.gr");
    assert_full_compile_trust(
        "54_unary_minus_combos.gr",
        &src,
        &[
            "fn neg_chain",
            "fn unary_in_compares",
            "fn unary_in_arith_chain",
            "fn unary_in_recursion",
            "fn unary_minus_combos_driver",
        ],
    );
}

#[test]
fn trust_recursion_deep_branching() {
    let _g = lock();
    reset_all();
    let src = fixture("55_recursion_deep_branching.gr");
    assert_full_compile_trust(
        "55_recursion_deep_branching.gr",
        &src,
        &[
            "fn three_way_descent",
            "fn branch_recursive_sum",
            "fn deep_branching_recursion_driver",
        ],
    );
}

#[test]
fn trust_modulo_arith() {
    let _g = lock();
    reset_all();
    let src = fixture("56_modulo_arith.gr");
    assert_full_compile_trust(
        "56_modulo_arith.gr",
        &src,
        &[
            "fn parity",
            "fn mod_chain",
            "fn parity_dispatch",
            "fn mod_descent",
            "fn modulo_arith_driver",
        ],
    );
}

#[test]
fn trust_compare_chain_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("57_compare_chain_recursion.gr");
    assert_full_compile_trust(
        "57_compare_chain_recursion.gr",
        &src,
        &[
            "fn cmp_chain_descent",
            "fn bool_let_descent",
            "fn compare_chain_recursion_driver",
        ],
    );
}

#[test]
fn trust_calls_in_let_chains() {
    let _g = lock();
    reset_all();
    let src = fixture("58_calls_in_let_chains.gr");
    assert_full_compile_trust(
        "58_calls_in_let_chains.gr",
        &src,
        &[
            "fn pair_sum",
            "fn pair_diff",
            "fn triple_sum",
            "fn alt_let_calls",
            "fn alt_let_with_compare",
            "fn calls_in_let_chains_driver",
        ],
    );
}

#[test]
fn trust_negation_truth_logic() {
    let _g = lock();
    reset_all();
    let src = fixture("59_negation_truth_logic.gr");
    assert_full_compile_trust(
        "59_negation_truth_logic.gr",
        &src,
        &[
            "fn neg_compare_logic",
            "fn negation_truth_dispatch",
            "fn compare_negate_chain",
            "fn negation_truth_logic_driver",
        ],
    );
}

#[test]
fn trust_div_mod_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("60_div_mod_dispatch.gr");
    assert_full_compile_trust(
        "60_div_mod_dispatch.gr",
        &src,
        &[
            "fn divmod_classify",
            "fn divmod_signed",
            "fn divmod_chain",
            "fn divmod_dispatch_driver",
        ],
    );
}

#[test]
fn trust_nested_let_consumes_call() {
    let _g = lock();
    reset_all();
    let src = fixture("61_nested_let_consumes_call.gr");
    assert_full_compile_trust(
        "61_nested_let_consumes_call.gr",
        &src,
        &[
            "fn outer_calc",
            "fn inner_calc",
            "fn nested_let_consume",
            "fn nested_let_branched",
            "fn nested_let_recursion",
            "fn nested_call_let_driver",
        ],
    );
}

#[test]
fn trust_compare_modulo_mix() {
    let _g = lock();
    reset_all();
    let src = fixture("62_compare_modulo_mix.gr");
    assert_full_compile_trust(
        "62_compare_modulo_mix.gr",
        &src,
        &[
            "fn mod_eq_zero",
            "fn mod_lt_three",
            "fn mod_compare_chain",
            "fn mod_compare_and",
            "fn mod_compare_dispatch",
            "fn compare_modulo_mix_driver",
        ],
    );
}

#[test]
fn trust_five_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("63_five_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "63_five_fn_mutual_rec.gr",
        &src,
        &[
            "fn alpha5",
            "fn beta5",
            "fn gamma5",
            "fn delta5",
            "fn epsilon5",
            "fn five_fn_mutual_rec_driver",
        ],
    );
}

#[test]
fn trust_deep_let_15_binders() {
    let _g = lock();
    reset_all();
    let src = fixture("64_deep_let_15_binders.gr");
    assert_full_compile_trust(
        "64_deep_let_15_binders.gr",
        &src,
        &[
            "fn double15",
            "fn triple15",
            "fn deep_let_15_binders",
            "fn deep_let_compares",
            "fn deep_let_15_driver",
        ],
    );
}

#[test]
fn trust_signed_modulo_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("65_signed_modulo_dispatch.gr");
    assert_full_compile_trust(
        "65_signed_modulo_dispatch.gr",
        &src,
        &[
            "fn signed_mod",
            "fn signed_mod_dispatch",
            "fn signed_mod_band",
            "fn signed_mod_descent",
            "fn signed_modulo_dispatch_driver",
        ],
    );
}

#[test]
fn trust_div_compare_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("66_div_compare_chain.gr");
    assert_full_compile_trust(
        "66_div_compare_chain.gr",
        &src,
        &[
            "fn div_eq_zero",
            "fn div_gt_one",
            "fn div_compare_or",
            "fn div_compare_and",
            "fn div_compare_dispatch",
            "fn div_compare_chain_driver",
        ],
    );
}

#[test]
fn trust_six_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("67_six_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "67_six_fn_mutual_rec.gr",
        &src,
        &[
            "fn six_a",
            "fn six_b",
            "fn six_c",
            "fn six_d",
            "fn six_e",
            "fn six_f",
            "fn six_fn_mutual_rec_driver",
        ],
    );
}

#[test]
fn trust_let_chain_with_compare_branches() {
    let _g = lock();
    reset_all();
    let src = fixture("68_let_chain_with_compare_branches.gr");
    assert_full_compile_trust(
        "68_let_chain_with_compare_branches.gr",
        &src,
        &[
            "fn lc_double",
            "fn lc_triple",
            "fn let_chain_compare_branches",
            "fn let_chain_recursive",
            "fn let_chain_compare_driver",
        ],
    );
}

#[test]
fn trust_modulo_recursion_branches() {
    let _g = lock();
    reset_all();
    let src = fixture("69_modulo_recursion_branches.gr");
    assert_full_compile_trust(
        "69_modulo_recursion_branches.gr",
        &src,
        &[
            "fn mod_recurse_even",
            "fn mod_recurse_three",
            "fn mod_recurse_combined",
            "fn modulo_recursion_branches_driver",
        ],
    );
}

#[test]
fn trust_signed_arith_truth_table() {
    let _g = lock();
    reset_all();
    let src = fixture("70_signed_arith_truth_table.gr");
    assert_full_compile_trust(
        "70_signed_arith_truth_table.gr",
        &src,
        &[
            "fn sat_pos",
            "fn sat_neg",
            "fn sat_zero",
            "fn signed_truth_p",
            "fn signed_truth_full",
            "fn signed_arith_truth_table_driver",
        ],
    );
}

#[test]
fn trust_mixed_bool_arith_compare() {
    let _g = lock();
    reset_all();
    let src = fixture("71_mixed_bool_arith_compare.gr");
    assert_full_compile_trust(
        "71_mixed_bool_arith_compare.gr",
        &src,
        &[
            "fn mba_classify",
            "fn mba_score",
            "fn mixed_bool_arith_compare_driver",
        ],
    );
}

#[test]
fn trust_three_way_mutual_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("72_three_way_mutual_recursion.gr");
    assert_full_compile_trust(
        "72_three_way_mutual_recursion.gr",
        &src,
        &[
            "fn is_div2",
            "fn is_div3",
            "fn step_down",
            "fn three_way_mutual_recursion_driver",
        ],
    );
}

#[test]
fn trust_deep_let_chain_pipeline_v2() {
    let _g = lock();
    reset_all();
    let src = fixture("73_deep_let_chain_pipeline.gr");
    assert_full_compile_trust(
        "73_deep_let_chain_pipeline.gr",
        &src,
        &[
            "fn dlc_add3",
            "fn dlc_pair",
            "fn deep_let_chain_pipeline",
            "fn deep_let_chain_driver",
        ],
    );
}

#[test]
fn trust_arith_identities_v2() {
    let _g = lock();
    reset_all();
    let src = fixture("74_arith_identities_v2.gr");
    assert_full_compile_trust(
        "74_arith_identities_v2.gr",
        &src,
        &[
            "fn id_add_zero",
            "fn id_sub_zero",
            "fn id_mul_one",
            "fn id_self_diff",
            "fn id_double_neg",
            "fn id_add_self_zero",
            "fn arith_identities_driver",
        ],
    );
}

#[test]
fn trust_compare_transitivity() {
    let _g = lock();
    reset_all();
    let src = fixture("75_compare_transitivity.gr");
    assert_full_compile_trust(
        "75_compare_transitivity.gr",
        &src,
        &[
            "fn ct_lt",
            "fn ct_le",
            "fn ct_eq",
            "fn compare_transitivity",
            "fn equality_chain_check",
            "fn compare_transitivity_driver",
        ],
    );
}

#[test]
fn trust_nested_not_over_compare() {
    let _g = lock();
    reset_all();
    let src = fixture("76_nested_not_over_compare.gr");
    assert_full_compile_trust(
        "76_nested_not_over_compare.gr",
        &src,
        &[
            "fn cmp_lt",
            "fn cmp_eq",
            "fn nested_not_over_compare",
            "fn nested_not_driver",
        ],
    );
}

#[test]
fn trust_four_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("77_four_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "77_four_fn_mutual_rec.gr",
        &src,
        &[
            "fn ring_a",
            "fn ring_b",
            "fn ring_c",
            "fn ring_d",
            "fn four_fn_mutual_rec_driver",
        ],
    );
}

#[test]
fn trust_div_mod_let_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("78_div_mod_let_chain.gr");
    assert_full_compile_trust(
        "78_div_mod_let_chain.gr",
        &src,
        &[
            "fn div_mod_combine",
            "fn div_mod_let_chain",
            "fn div_mod_let_chain_driver",
        ],
    );
}

#[test]
fn trust_guarded_single_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("79_guarded_single_recursion.gr");
    assert_full_compile_trust(
        "79_guarded_single_recursion.gr",
        &src,
        &[
            "fn fact_like",
            "fn triangle_like",
            "fn power_two_like",
            "fn guarded_single_recursion_driver",
        ],
    );
}

#[test]
fn trust_comparison_ladder_truth() {
    let _g = lock();
    reset_all();
    let src = fixture("80_comparison_ladder_truth.gr");
    assert_full_compile_trust(
        "80_comparison_ladder_truth.gr",
        &src,
        &[
            "fn ladder_step",
            "fn ladder_eq",
            "fn comparison_ladder_truth",
            "fn comparison_ladder_truth_driver",
        ],
    );
}

#[test]
fn trust_calls_with_unary_args() {
    let _g = lock();
    reset_all();
    let src = fixture("81_calls_with_unary_args.gr");
    assert_full_compile_trust(
        "81_calls_with_unary_args.gr",
        &src,
        &[
            "fn dispatch_int",
            "fn dispatch_bool",
            "fn combine",
            "fn main",
        ],
    );
}

#[test]
fn trust_modulo_truth_table() {
    let _g = lock();
    reset_all();
    let src = fixture("82_modulo_truth_table.gr");
    assert_full_compile_trust(
        "82_modulo_truth_table.gr",
        &src,
        &["fn classify", "fn quadrant_match", "fn main"],
    );
}

#[test]
fn trust_let_call_alternation() {
    let _g = lock();
    reset_all();
    let src = fixture("83_let_call_alternation.gr");
    assert_full_compile_trust(
        "83_let_call_alternation.gr",
        &src,
        &["fn double_it", "fn add_pair", "fn main"],
    );
}

#[test]
fn trust_signed_compare_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("84_signed_compare_pipeline.gr");
    assert_full_compile_trust(
        "84_signed_compare_pipeline.gr",
        &src,
        &[
            "fn positive_signed_sum",
            "fn signed_pair_classify",
            "fn evaluate",
            "fn main",
        ],
    );
}

#[test]
fn trust_compare_negation_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("85_compare_negation_recursion.gr");
    assert_full_compile_trust(
        "85_compare_negation_recursion.gr",
        &src,
        &[
            "fn count_down",
            "fn sum_to",
            "fn power_two_log",
            "fn main",
        ],
    );
}

#[test]
fn trust_arith_call_bool_reduce() {
    let _g = lock();
    reset_all();
    let src = fixture("86_arith_call_bool_reduce.gr");
    assert_full_compile_trust(
        "86_arith_call_bool_reduce.gr",
        &src,
        &[
            "fn doubled",
            "fn is_pos",
            "fn check_chain",
            "fn main",
        ],
    );
}

#[test]
fn trust_nested_if_call_returns() {
    let _g = lock();
    reset_all();
    let src = fixture("87_nested_if_call_returns.gr");
    assert_full_compile_trust(
        "87_nested_if_call_returns.gr",
        &src,
        &[
            "fn one",
            "fn two",
            "fn three",
            "fn four",
            "fn five",
            "fn pick",
            "fn main",
        ],
    );
}

#[test]
fn trust_five_arg_compare_args() {
    let _g = lock();
    reset_all();
    let src = fixture("88_five_arg_compare_args.gr");
    assert_full_compile_trust(
        "88_five_arg_compare_args.gr",
        &src,
        &[
            "fn b2i",
            "fn sum5",
            "fn count_truths",
            "fn main",
        ],
    );
}

#[test]
fn trust_five_fn_recursion_ladder() {
    let _g = lock();
    reset_all();
    let src = fixture("89_five_fn_recursion_ladder.gr");
    assert_full_compile_trust(
        "89_five_fn_recursion_ladder.gr",
        &src,
        &[
            "fn step1",
            "fn step2",
            "fn step3",
            "fn step4",
            "fn step5",
            "fn main",
        ],
    );
}

#[test]
fn trust_arith_identities_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("100_arith_identities_chain.gr");
    assert_full_compile_trust(
        "100_arith_identities_chain.gr",
        &src,
        &["fn add4", "fn ident_pipeline", "fn main"],
    );
}

#[test]
fn trust_negation_double_unwind() {
    let _g = lock();
    reset_all();
    let src = fixture("101_negation_double_unwind.gr");
    assert_full_compile_trust(
        "101_negation_double_unwind.gr",
        &src,
        &["fn classify", "fn flip", "fn route", "fn main"],
    );
}

#[test]
fn trust_ladder_recursion_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("102_ladder_recursion_pipeline.gr");
    assert_full_compile_trust(
        "102_ladder_recursion_pipeline.gr",
        &src,
        &["fn step", "fn pipe", "fn main"],
    );
}

#[test]
fn trust_compare_neg_arith_blend() {
    let _g = lock();
    reset_all();
    let src = fixture("103_compare_neg_arith_blend.gr");
    assert_full_compile_trust(
        "103_compare_neg_arith_blend.gr",
        &src,
        &["fn polarity", "fn distance", "fn main"],
    );
}

#[test]
fn trust_seven_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("104_seven_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "104_seven_fn_mutual_rec.gr",
        &src,
        &[
            "fn r1", "fn r2", "fn r3", "fn r4", "fn r5", "fn r6", "fn r7", "fn main",
        ],
    );
}

#[test]
fn trust_long_bool_chain_negations() {
    let _g = lock();
    reset_all();
    let src = fixture("105_long_bool_chain_negations.gr");
    assert_full_compile_trust(
        "105_long_bool_chain_negations.gr",
        &src,
        &["fn classify", "fn alt_classify", "fn main"],
    );
}

#[test]
fn trust_eight_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("106_eight_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "106_eight_fn_mutual_rec.gr",
        &src,
        &[
            "fn s1", "fn s2", "fn s3", "fn s4", "fn s5", "fn s6", "fn s7", "fn s8", "fn main",
        ],
    );
}

#[test]
fn trust_div_mod_identity_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("107_div_mod_identity_pipeline.gr");
    assert_full_compile_trust(
        "107_div_mod_identity_pipeline.gr",
        &src,
        &[
            "fn combine",
            "fn rebuild",
            "fn rebuild5",
            "fn pair_eq",
            "fn main",
        ],
    );
}

#[test]
fn trust_compare_let_and_reduce() {
    let _g = lock();
    reset_all();
    let src = fixture("108_compare_let_and_reduce.gr");
    assert_full_compile_trust(
        "108_compare_let_and_reduce.gr",
        &src,
        &["fn ordered6", "fn any_strict", "fn main"],
    );
}

#[test]
fn trust_nested_call_arith_identities() {
    let _g = lock();
    reset_all();
    let src = fixture("109_nested_call_arith_identities.gr");
    assert_full_compile_trust(
        "109_nested_call_arith_identities.gr",
        &src,
        &["fn add3", "fn double", "fn drive", "fn main"],
    );
}

#[test]
fn trust_arith_or_bool_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("110_arith_or_bool_chain.gr");
    assert_full_compile_trust(
        "110_arith_or_bool_chain.gr",
        &src,
        &["fn classify", "fn invert", "fn main"],
    );
}

#[test]
fn trust_nine_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("111_nine_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "111_nine_fn_mutual_rec.gr",
        &src,
        &[
            "fn t1", "fn t2", "fn t3", "fn t4", "fn t5", "fn t6", "fn t7", "fn t8", "fn t9",
            "fn main",
        ],
    );
}

#[test]
fn trust_recursion_with_nested_let() {
    let _g = lock();
    reset_all();
    let src = fixture("112_recursion_with_nested_let.gr");
    assert_full_compile_trust(
        "112_recursion_with_nested_let.gr",
        &src,
        &["fn collatz_like", "fn step_through", "fn main"],
    );
}

#[test]
fn trust_signed_branch_truth() {
    let _g = lock();
    reset_all();
    let src = fixture("113_signed_branch_truth.gr");
    assert_full_compile_trust(
        "113_signed_branch_truth.gr",
        &src,
        &["fn bucket", "fn equals_bucket", "fn main"],
    );
}

#[test]
fn trust_compare_let_pipeline_v2() {
    let _g = lock();
    reset_all();
    let src = fixture("114_compare_let_pipeline_v2.gr");
    assert_full_compile_trust(
        "114_compare_let_pipeline_v2.gr",
        &src,
        &["fn pos_step", "fn neg_step", "fn drive", "fn main"],
    );
}

#[test]
fn trust_ten_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("115_ten_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "115_ten_fn_mutual_rec.gr",
        &src,
        &[
            "fn r1", "fn r2", "fn r3", "fn r4", "fn r5", "fn r6", "fn r7", "fn r8", "fn r9",
            "fn r10", "fn main",
        ],
    );
}

#[test]
fn trust_deep_nested_if_arith_tree() {
    let _g = lock();
    reset_all();
    let src = fixture("116_deep_nested_if_arith_tree.gr");
    assert_full_compile_trust(
        "116_deep_nested_if_arith_tree.gr",
        &src,
        &["fn classify", "fn main"],
    );
}

#[test]
fn trust_bool_chain_into_multi_arg_call() {
    let _g = lock();
    reset_all();
    let src = fixture("117_bool_chain_into_multi_arg_call.gr");
    assert_full_compile_trust(
        "117_bool_chain_into_multi_arg_call.gr",
        &src,
        &["fn pick", "fn main"],
    );
}

#[test]
fn trust_arith_id_mod_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("118_arith_id_mod_pipeline.gr");
    assert_full_compile_trust(
        "118_arith_id_mod_pipeline.gr",
        &src,
        &["fn step", "fn pipeline", "fn main"],
    );
}

#[test]
fn trust_compare_let_recursive_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("119_compare_let_recursive_chain.gr");
    assert_full_compile_trust(
        "119_compare_let_recursive_chain.gr",
        &src,
        &["fn descend", "fn main"],
    );
}

#[test]
fn trust_eleven_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("120_eleven_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "120_eleven_fn_mutual_rec.gr",
        &src,
        &[
            "fn m1", "fn m2", "fn m3", "fn m4", "fn m5", "fn m6", "fn m7", "fn m8", "fn m9",
            "fn m10", "fn m11", "fn main",
        ],
    );
}

#[test]
fn trust_double_call_arith() {
    let _g = lock();
    reset_all();
    let src = fixture("121_double_call_arith.gr");
    assert_full_compile_trust(
        "121_double_call_arith.gr",
        &src,
        &["fn double", "fn triple", "fn quad", "fn combine", "fn main"],
    );
}

#[test]
fn trust_compare_and_or_truth() {
    let _g = lock();
    reset_all();
    let src = fixture("122_compare_and_or_truth.gr");
    assert_full_compile_trust(
        "122_compare_and_or_truth.gr",
        &src,
        &["fn truth", "fn main"],
    );
}

#[test]
fn trust_let_chain_with_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("123_let_chain_with_recursion.gr");
    assert_full_compile_trust(
        "123_let_chain_with_recursion.gr",
        &src,
        &["fn descend", "fn main"],
    );
}

#[test]
fn trust_signed_arith_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("124_signed_arith_pipeline.gr");
    assert_full_compile_trust(
        "124_signed_arith_pipeline.gr",
        &src,
        &["fn alt_sum", "fn nonneg_chain", "fn main"],
    );
}

#[test]
fn trust_twelve_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("125_twelve_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "125_twelve_fn_mutual_rec.gr",
        &src,
        &[
            "fn r1", "fn r2", "fn r3", "fn r4", "fn r5", "fn r6", "fn r7", "fn r8", "fn r9",
            "fn r10", "fn r11", "fn r12", "fn main",
        ],
    );
}

#[test]
fn trust_four_input_truth_table() {
    let _g = lock();
    reset_all();
    let src = fixture("126_four_input_truth_table.gr");
    assert_full_compile_trust(
        "126_four_input_truth_table.gr",
        &src,
        &["fn dispatch4", "fn main"],
    );
}

#[test]
fn trust_arith_associativity_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("127_arith_associativity_chain.gr");
    assert_full_compile_trust(
        "127_arith_associativity_chain.gr",
        &src,
        &["fn flat_chain", "fn paren_chain", "fn diff_chain", "fn main"],
    );
}

#[test]
fn trust_triple_nested_let_mix() {
    let _g = lock();
    reset_all();
    let src = fixture("128_triple_nested_let_mix.gr");
    assert_full_compile_trust(
        "128_triple_nested_let_mix.gr",
        &src,
        &["fn descend", "fn double_descend", "fn main"],
    );
}

#[test]
fn trust_wide_compare_with_negations() {
    let _g = lock();
    reset_all();
    let src = fixture("129_wide_compare_with_negations.gr");
    assert_full_compile_trust(
        "129_wide_compare_with_negations.gr",
        &src,
        &["fn all_band", "fn or_band", "fn main"],
    );
}

#[test]
fn trust_thirteen_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("130_thirteen_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "130_thirteen_fn_mutual_rec.gr",
        &src,
        &[
            "fn s1", "fn s2", "fn s3", "fn s4", "fn s5", "fn s6", "fn s7", "fn s8", "fn s9",
            "fn s10", "fn s11", "fn s12", "fn s13", "fn main",
        ],
    );
}

#[test]
fn trust_arith_let_pyramid() {
    let _g = lock();
    reset_all();
    let src = fixture("131_arith_let_pyramid.gr");
    assert_full_compile_trust(
        "131_arith_let_pyramid.gr",
        &src,
        &["fn pyramid", "fn main"],
    );
}

#[test]
fn trust_compare_pair_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("132_compare_pair_dispatch.gr");
    assert_full_compile_trust(
        "132_compare_pair_dispatch.gr",
        &src,
        &["fn pair_class", "fn main"],
    );
}

#[test]
fn trust_signed_recursion_with_modulo() {
    let _g = lock();
    reset_all();
    let src = fixture("133_signed_recursion_with_modulo.gr");
    assert_full_compile_trust(
        "133_signed_recursion_with_modulo.gr",
        &src,
        &["fn alt_descend", "fn main"],
    );
}

#[test]
fn trust_call_in_let_in_call() {
    let _g = lock();
    reset_all();
    let src = fixture("134_call_in_let_in_call.gr");
    assert_full_compile_trust(
        "134_call_in_let_in_call.gr",
        &src,
        &["fn helper", "fn doubler", "fn quad", "fn main"],
    );
}

#[test]
fn trust_fourteen_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("135_fourteen_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "135_fourteen_fn_mutual_rec.gr",
        &src,
        &[
            "fn r1", "fn r2", "fn r3", "fn r4", "fn r5", "fn r6", "fn r7", "fn r8",
            "fn r9", "fn r10", "fn r11", "fn r12", "fn r13", "fn r14", "fn main",
        ],
    );
}

#[test]
fn trust_guard_ladder_arith_payloads() {
    let _g = lock();
    reset_all();
    let src = fixture("136_guard_ladder_arith_payloads.gr");
    assert_full_compile_trust(
        "136_guard_ladder_arith_payloads.gr",
        &src,
        &[
            "fn payload_low",
            "fn payload_mid",
            "fn payload_high",
            "fn classify",
            "fn main",
        ],
    );
}

#[test]
fn trust_modulo_dispatch_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("137_modulo_dispatch_pipeline.gr");
    assert_full_compile_trust(
        "137_modulo_dispatch_pipeline.gr",
        &src,
        &["fn advance", "fn normalize", "fn main"],
    );
}

#[test]
fn trust_call_chain_diamond() {
    let _g = lock();
    reset_all();
    let src = fixture("138_call_chain_diamond.gr");
    assert_full_compile_trust(
        "138_call_chain_diamond.gr",
        &src,
        &[
            "fn seed",
            "fn branch_left",
            "fn branch_right",
            "fn join_paths",
            "fn diamond",
            "fn main",
        ],
    );
}

#[test]
fn trust_let_call_compare_braid() {
    let _g = lock();
    reset_all();
    let src = fixture("139_let_call_compare_braid.gr");
    assert_full_compile_trust(
        "139_let_call_compare_braid.gr",
        &src,
        &["fn weave", "fn descend", "fn braided", "fn main"],
    );
}

#[test]
fn trust_fifteen_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("140_fifteen_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "140_fifteen_fn_mutual_rec.gr",
        &src,
        &[
            "fn t1", "fn t2", "fn t3", "fn t4", "fn t5", "fn t6", "fn t7", "fn t8",
            "fn t9", "fn t10", "fn t11", "fn t12", "fn t13", "fn t14", "fn t15",
            "fn main",
        ],
    );
}

#[test]
fn trust_nested_bool_compare_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("141_nested_bool_compare_dispatch.gr");
    assert_full_compile_trust(
        "141_nested_bool_compare_dispatch.gr",
        &src,
        &["fn score", "fn adjust", "fn main"],
    );
}

#[test]
fn trust_div_mod_accumulator_braid() {
    let _g = lock();
    reset_all();
    let src = fixture("142_div_mod_accumulator_braid.gr");
    assert_full_compile_trust(
        "142_div_mod_accumulator_braid.gr",
        &src,
        &["fn fold_parts", "fn mix", "fn main"],
    );
}

#[test]
fn trust_multi_arg_call_fanout() {
    let _g = lock();
    reset_all();
    let src = fixture("143_multi_arg_call_fanout.gr");
    assert_full_compile_trust(
        "143_multi_arg_call_fanout.gr",
        &src,
        &["fn combine6", "fn fanout", "fn main"],
    );
}

#[test]
fn trust_signed_negation_recursion_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("144_signed_negation_recursion_chain.gr");
    assert_full_compile_trust(
        "144_signed_negation_recursion_chain.gr",
        &src,
        &["fn descend", "fn choose", "fn main"],
    );
}

#[test]
fn trust_sixteen_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("145_sixteen_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "145_sixteen_fn_mutual_rec.gr",
        &src,
        &[
            "fn u1", "fn u2", "fn u3", "fn u4", "fn u5", "fn u6", "fn u7", "fn u8",
            "fn u9", "fn u10", "fn u11", "fn u12", "fn u13", "fn u14", "fn u15",
            "fn u16", "fn main",
        ],
    );
}

#[test]
fn trust_compare_driven_arith_selector() {
    let _g = lock();
    reset_all();
    let src = fixture("146_compare_driven_arith_selector.gr");
    assert_full_compile_trust(
        "146_compare_driven_arith_selector.gr",
        &src,
        &["fn add_path", "fn sub_path", "fn mix_path", "fn select", "fn main"],
    );
}

#[test]
fn trust_nested_let_div_mod_fanout() {
    let _g = lock();
    reset_all();
    let src = fixture("147_nested_let_div_mod_fanout.gr");
    assert_full_compile_trust(
        "147_nested_let_div_mod_fanout.gr",
        &src,
        &["fn pack", "fn fan", "fn main"],
    );
}

#[test]
fn trust_recursive_branch_call_mixer() {
    let _g = lock();
    reset_all();
    let src = fixture("148_recursive_branch_call_mixer.gr");
    assert_full_compile_trust(
        "148_recursive_branch_call_mixer.gr",
        &src,
        &["fn step", "fn descend", "fn main"],
    );
}

#[test]
fn trust_boolean_normalization_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("149_boolean_normalization_pipeline.gr");
    assert_full_compile_trust(
        "149_boolean_normalization_pipeline.gr",
        &src,
        &["fn bool_to_int", "fn normalize", "fn main"],
    );
}

#[test]
fn trust_seventeen_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("150_seventeen_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "150_seventeen_fn_mutual_rec.gr",
        &src,
        &[
            "fn v1", "fn v2", "fn v3", "fn v4", "fn v5", "fn v6", "fn v7", "fn v8",
            "fn v9", "fn v10", "fn v11", "fn v12", "fn v13", "fn v14", "fn v15",
            "fn v16", "fn v17", "fn main",
        ],
    );
}

#[test]
fn trust_guard_matrix_arith_payloads() {
    let _g = lock();
    reset_all();
    let src = fixture("151_guard_matrix_arith_payloads.gr");
    assert_full_compile_trust(
        "151_guard_matrix_arith_payloads.gr",
        &src,
        &["fn low_path", "fn mid_path", "fn high_path", "fn guard_matrix", "fn main"],
    );
}

#[test]
fn trust_div_mod_stage_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("152_div_mod_stage_dispatch.gr");
    assert_full_compile_trust(
        "152_div_mod_stage_dispatch.gr",
        &src,
        &["fn combine5", "fn choose_stage", "fn div_mod_stage", "fn main"],
    );
}

#[test]
fn trust_call_let_recursive_weave() {
    let _g = lock();
    reset_all();
    let src = fixture("153_call_let_recursive_weave.gr");
    assert_full_compile_trust(
        "153_call_let_recursive_weave.gr",
        &src,
        &["fn bump", "fn twist", "fn weave", "fn main"],
    );
}

#[test]
fn trust_bool_compare_reducer_mix() {
    let _g = lock();
    reset_all();
    let src = fixture("154_bool_compare_reducer_mix.gr");
    assert_full_compile_trust(
        "154_bool_compare_reducer_mix.gr",
        &src,
        &["fn flag_to_int", "fn normalize_bool", "fn reduce_compare", "fn main"],
    );
}

#[test]
fn trust_eighteen_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("155_eighteen_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "155_eighteen_fn_mutual_rec.gr",
        &src,
        &[
            "fn w1", "fn w2", "fn w3", "fn w4", "fn w5", "fn w6", "fn w7", "fn w8",
            "fn w9", "fn w10", "fn w11", "fn w12", "fn w13", "fn w14", "fn w15",
            "fn w16", "fn w17", "fn w18", "fn main",
        ],
    );
}

#[test]
fn trust_compare_chain_truth_fanout() {
    let _g = lock();
    reset_all();
    let src = fixture("156_compare_chain_truth_fanout.gr");
    assert_full_compile_trust(
        "156_compare_chain_truth_fanout.gr",
        &src,
        &["fn payload_a", "fn payload_b", "fn payload_c", "fn payload_d", "fn dispatch", "fn main"],
    );
}

#[test]
fn trust_let_call_modulo_braid() {
    let _g = lock();
    reset_all();
    let src = fixture("157_let_call_modulo_braid.gr");
    assert_full_compile_trust(
        "157_let_call_modulo_braid.gr",
        &src,
        &["fn add_two", "fn sub_two", "fn braid", "fn main"],
    );
}

#[test]
fn trust_arith_or_bool_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("158_arith_or_bool_dispatch.gr");
    assert_full_compile_trust(
        "158_arith_or_bool_dispatch.gr",
        &src,
        &["fn id_add", "fn id_sub", "fn id_double_undo", "fn id_negate_back", "fn pick", "fn main"],
    );
}

#[test]
fn trust_recursive_negation_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("159_recursive_negation_pipeline.gr");
    assert_full_compile_trust(
        "159_recursive_negation_pipeline.gr",
        &src,
        &["fn step", "fn pipeline", "fn main"],
    );
}

#[test]
fn trust_nineteen_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("160_nineteen_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "160_nineteen_fn_mutual_rec.gr",
        &src,
        &[
            "fn x1", "fn x2", "fn x3", "fn x4", "fn x5", "fn x6", "fn x7", "fn x8",
            "fn x9", "fn x10", "fn x11", "fn x12", "fn x13", "fn x14", "fn x15",
            "fn x16", "fn x17", "fn x18", "fn x19", "fn main",
        ],
    );
}

#[test]
fn trust_arith_chain_then_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("161_arith_chain_then_dispatch.gr");
    assert_full_compile_trust(
        "161_arith_chain_then_dispatch.gr",
        &src,
        &["fn arith_chain", "fn dispatch", "fn main"],
    );
}

#[test]
fn trust_compare_and_or_truth_full() {
    let _g = lock();
    reset_all();
    let src = fixture("162_compare_and_or_truth_full.gr");
    assert_full_compile_trust(
        "162_compare_and_or_truth_full.gr",
        &src,
        &["fn check", "fn main"],
    );
}

#[test]
fn trust_let_pyramid_with_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("163_let_pyramid_with_recursion.gr");
    assert_full_compile_trust(
        "163_let_pyramid_with_recursion.gr",
        &src,
        &["fn descend", "fn main"],
    );
}

#[test]
fn trust_nested_call_modulo_braid() {
    let _g = lock();
    reset_all();
    let src = fixture("164_nested_call_modulo_braid.gr");
    assert_full_compile_trust(
        "164_nested_call_modulo_braid.gr",
        &src,
        &["fn inner", "fn middle", "fn outer", "fn braid", "fn main"],
    );
}

#[test]
fn trust_twenty_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("165_twenty_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "165_twenty_fn_mutual_rec.gr",
        &src,
        &[
            "fn y1", "fn y2", "fn y3", "fn y4", "fn y5", "fn y6", "fn y7", "fn y8",
            "fn y9", "fn y10", "fn y11", "fn y12", "fn y13", "fn y14", "fn y15",
            "fn y16", "fn y17", "fn y18", "fn y19", "fn y20", "fn main",
        ],
    );
}

#[test]
fn trust_arith_chain_let_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("166_arith_chain_let_dispatch.gr");
    assert_full_compile_trust(
        "166_arith_chain_let_dispatch.gr",
        &src,
        &["fn arith_grow", "fn dispatch", "fn main"],
    );
}

#[test]
fn trust_compare_truth_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("167_compare_truth_recursion.gr");
    assert_full_compile_trust(
        "167_compare_truth_recursion.gr",
        &src,
        &["fn descend", "fn main"],
    );
}

#[test]
fn trust_modulo_call_braid() {
    let _g = lock();
    reset_all();
    let src = fixture("168_modulo_call_braid.gr");
    assert_full_compile_trust(
        "168_modulo_call_braid.gr",
        &src,
        &["fn mod2", "fn mod3", "fn mod5", "fn combine", "fn braid", "fn main"],
    );
}

#[test]
fn trust_signed_let_pipeline() {
    let _g = lock();
    reset_all();
    let src = fixture("169_signed_let_pipeline.gr");
    assert_full_compile_trust(
        "169_signed_let_pipeline.gr",
        &src,
        &["fn step", "fn pipeline", "fn main"],
    );
}

#[test]
fn trust_twentyone_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("170_twentyone_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "170_twentyone_fn_mutual_rec.gr",
        &src,
        &[
            "fn z1", "fn z2", "fn z3", "fn z4", "fn z5", "fn z6", "fn z7", "fn z8", "fn z9",
            "fn z10", "fn z11", "fn z12", "fn z13", "fn z14", "fn z15", "fn z16", "fn z17",
            "fn z18", "fn z19", "fn z20", "fn z21", "fn main",
        ],
    );
}

#[test]
fn trust_nested_if_compare_cascade() {
    let _g = lock();
    reset_all();
    let src = fixture("171_nested_if_compare_cascade.gr");
    assert_full_compile_trust(
        "171_nested_if_compare_cascade.gr",
        &src,
        &["fn classify", "fn either_extreme", "fn cascade", "fn main"],
    );
}

#[test]
fn trust_arith_call_arg_shuffle() {
    let _g = lock();
    reset_all();
    let src = fixture("172_arith_call_arg_shuffle.gr");
    assert_full_compile_trust(
        "172_arith_call_arg_shuffle.gr",
        &src,
        &["fn add3", "fn diff3", "fn mix", "fn shuffle", "fn main"],
    );
}

#[test]
fn trust_triple_modulo_intersect() {
    let _g = lock();
    reset_all();
    let src = fixture("173_triple_modulo_intersect.gr");
    assert_full_compile_trust(
        "173_triple_modulo_intersect.gr",
        &src,
        &[
            "fn divisible_by_2",
            "fn divisible_by_3",
            "fn divisible_by_5",
            "fn intersect_three",
            "fn any_three",
            "fn filter_step",
            "fn main",
        ],
    );
}

#[test]
fn trust_let_call_chain_negation() {
    let _g = lock();
    reset_all();
    let src = fixture("174_let_call_chain_negation.gr");
    assert_full_compile_trust(
        "174_let_call_chain_negation.gr",
        &src,
        &[
            "fn bump",
            "fn drop",
            "fn flip",
            "fn chain",
            "fn pipeline",
            "fn main",
        ],
    );
}

#[test]
fn trust_twentytwo_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("175_twentytwo_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "175_twentytwo_fn_mutual_rec.gr",
        &src,
        &[
            "fn q1", "fn q2", "fn q3", "fn q4", "fn q5", "fn q6", "fn q7", "fn q8", "fn q9",
            "fn q10", "fn q11", "fn q12", "fn q13", "fn q14", "fn q15", "fn q16", "fn q17",
            "fn q18", "fn q19", "fn q20", "fn q21", "fn q22", "fn main",
        ],
    );
}

#[test]
fn trust_let_lattice_cross_deps() {
    let _g = lock();
    reset_all();
    let src = fixture("176_let_lattice_cross_deps.gr");
    assert_full_compile_trust(
        "176_let_lattice_cross_deps.gr",
        &src,
        &["fn lattice", "fn step", "fn main"],
    );
}

#[test]
fn trust_bool_asymmetric_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("177_bool_asymmetric_dispatch.gr");
    assert_full_compile_trust(
        "177_bool_asymmetric_dispatch.gr",
        &src,
        &[
            "fn classify_bool_pair",
            "fn negation_swap",
            "fn dispatch",
            "fn main",
        ],
    );
}

#[test]
fn trust_serial_bool_guard_chain() {
    let _g = lock();
    reset_all();
    let src = fixture("178_serial_bool_guard_chain.gr");
    assert_full_compile_trust(
        "178_serial_bool_guard_chain.gr",
        &src,
        &["fn guarded_step", "fn main"],
    );
}

#[test]
fn trust_paired_accumulator_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("179_paired_accumulator_recursion.gr");
    assert_full_compile_trust(
        "179_paired_accumulator_recursion.gr",
        &src,
        &["fn paired_recurse", "fn paired_via_let", "fn main"],
    );
}

#[test]
fn trust_twentythree_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("180_twentythree_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "180_twentythree_fn_mutual_rec.gr",
        &src,
        &[
            "fn r1", "fn r2", "fn r3", "fn r4", "fn r5", "fn r6", "fn r7", "fn r8",
            "fn r9", "fn r10", "fn r11", "fn r12", "fn r13", "fn r14", "fn r15",
            "fn r16", "fn r17", "fn r18", "fn r19", "fn r20", "fn r21", "fn r22",
            "fn r23", "fn main",
        ],
    );
}

#[test]
fn trust_compare_chain_multi_arg_call() {
    let _g = lock();
    reset_all();
    let src = fixture("181_compare_chain_multi_arg_call.gr");
    assert_full_compile_trust(
        "181_compare_chain_multi_arg_call.gr",
        &src,
        &[
            "fn bool_to_payload",
            "fn weighted_sum",
            "fn classify_via_compares",
            "fn main",
        ],
    );
}

#[test]
fn trust_dual_modulo_classify() {
    let _g = lock();
    reset_all();
    let src = fixture("182_dual_modulo_classify.gr");
    assert_full_compile_trust(
        "182_dual_modulo_classify.gr",
        &src,
        &["fn dual_classify", "fn sum_classified", "fn main"],
    );
}

#[test]
fn trust_let_pyramid_cross_modulo() {
    let _g = lock();
    reset_all();
    let src = fixture("183_let_pyramid_cross_modulo.gr");
    assert_full_compile_trust(
        "183_let_pyramid_cross_modulo.gr",
        &src,
        &["fn pyramid", "fn pyramid_pair", "fn main"],
    );
}

#[test]
fn trust_triple_accumulator_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("184_triple_accumulator_recursion.gr");
    assert_full_compile_trust(
        "184_triple_accumulator_recursion.gr",
        &src,
        &["fn triple_recurse", "fn triple_via_let", "fn main"],
    );
}

#[test]
fn trust_twentyfour_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("185_twentyfour_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "185_twentyfour_fn_mutual_rec.gr",
        &src,
        &[
            "fn s1", "fn s2", "fn s3", "fn s4", "fn s5", "fn s6", "fn s7", "fn s8",
            "fn s9", "fn s10", "fn s11", "fn s12", "fn s13", "fn s14", "fn s15",
            "fn s16", "fn s17", "fn s18", "fn s19", "fn s20", "fn s21", "fn s22",
            "fn s23", "fn s24", "fn main",
        ],
    );
}

#[test]
fn trust_signed_compare_truth_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("186_signed_compare_truth_dispatch.gr");
    assert_full_compile_trust(
        "186_signed_compare_truth_dispatch.gr",
        &src,
        &[
            "fn payload_a",
            "fn payload_b",
            "fn payload_c",
            "fn payload_d",
            "fn signed_dispatch",
            "fn main",
        ],
    );
}

#[test]
fn trust_let_modulo_pyramid() {
    let _g = lock();
    reset_all();
    let src = fixture("187_let_modulo_pyramid.gr");
    assert_full_compile_trust(
        "187_let_modulo_pyramid.gr",
        &src,
        &["fn step", "fn modulo_pyramid", "fn main"],
    );
}

#[test]
fn trust_compare_negation_call_braid() {
    let _g = lock();
    reset_all();
    let src = fixture("188_compare_negation_call_braid.gr");
    assert_full_compile_trust(
        "188_compare_negation_call_braid.gr",
        &src,
        &["fn bool_pick", "fn recurse_three", "fn braid", "fn main"],
    );
}

#[test]
fn trust_quad_accumulator_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("189_quad_accumulator_recursion.gr");
    assert_full_compile_trust(
        "189_quad_accumulator_recursion.gr",
        &src,
        &["fn quad_recurse", "fn quad_via_let", "fn main"],
    );
}

#[test]
fn trust_twentyfive_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("190_twentyfive_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "190_twentyfive_fn_mutual_rec.gr",
        &src,
        &[
            "fn step25_a", "fn step25_b", "fn step25_c", "fn step25_d",
            "fn step25_e", "fn step25_f", "fn step25_g", "fn step25_h",
            "fn step25_i", "fn step25_j", "fn step25_k", "fn step25_l",
            "fn step25_m", "fn step25_n", "fn step25_o", "fn step25_p",
            "fn step25_q", "fn step25_r", "fn step25_s", "fn step25_t",
            "fn step25_u", "fn step25_v", "fn step25_w", "fn step25_x",
            "fn step25_y", "fn main",
        ],
    );
}

#[test]
fn trust_quintuple_accumulator_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("191_quintuple_accumulator_recursion.gr");
    assert_full_compile_trust(
        "191_quintuple_accumulator_recursion.gr",
        &src,
        &["fn pick_max", "fn quint_recurse", "fn quint_via_let", "fn main"],
    );
}

#[test]
fn trust_let_pyramid_eight_with_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("192_let_pyramid_eight_with_recursion.gr");
    assert_full_compile_trust(
        "192_let_pyramid_eight_with_recursion.gr",
        &src,
        &["fn rec_even", "fn rec_odd", "fn pyramid_eight", "fn main"],
    );
}

#[test]
fn trust_five_input_truth_table_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("193_five_input_truth_table_recursion.gr");
    assert_full_compile_trust(
        "193_five_input_truth_table_recursion.gr",
        &src,
        &["fn to_int", "fn fold3", "fn truth_five", "fn main"],
    );
}

#[test]
fn trust_modulo_dispatch_mutual_cycle() {
    let _g = lock();
    reset_all();
    let src = fixture("194_modulo_dispatch_mutual_cycle.gr");
    assert_full_compile_trust(
        "194_modulo_dispatch_mutual_cycle.gr",
        &src,
        &["fn dispatch_a", "fn dispatch_b", "fn dispatch_c", "fn main"],
    );
}

#[test]
fn trust_twentysix_fn_mutual_rec() {
    let _g = lock();
    reset_all();
    let src = fixture("195_twentysix_fn_mutual_rec.gr");
    assert_full_compile_trust(
        "195_twentysix_fn_mutual_rec.gr",
        &src,
        &[
            "fn step26_a", "fn step26_b", "fn step26_c", "fn step26_d",
            "fn step26_e", "fn step26_f", "fn step26_g", "fn step26_h",
            "fn step26_i", "fn step26_j", "fn step26_k", "fn step26_l",
            "fn step26_m", "fn step26_n", "fn step26_o", "fn step26_p",
            "fn step26_q", "fn step26_r", "fn step26_s", "fn step26_t",
            "fn step26_u", "fn step26_v", "fn step26_w", "fn step26_x",
            "fn step26_y", "fn step26_z", "fn main",
        ],
    );
}

#[test]
fn trust_sextuple_accumulator_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("196_sextuple_accumulator_recursion.gr");
    assert_full_compile_trust(
        "196_sextuple_accumulator_recursion.gr",
        &src,
        &[
            "fn pick_max6",
            "fn pick_min6",
            "fn sext_recurse",
            "fn sext_via_let",
            "fn main",
        ],
    );
}

#[test]
fn trust_let_pyramid_nine_with_dispatch() {
    let _g = lock();
    reset_all();
    let src = fixture("197_let_pyramid_nine_with_dispatch.gr");
    assert_full_compile_trust(
        "197_let_pyramid_nine_with_dispatch.gr",
        &src,
        &[
            "fn rec_low",
            "fn rec_mid",
            "fn rec_high",
            "fn pyramid_nine",
            "fn main",
        ],
    );
}

#[test]
fn trust_six_input_truth_table_recursion() {
    let _g = lock();
    reset_all();
    let src = fixture("198_six_input_truth_table_recursion.gr");
    assert_full_compile_trust(
        "198_six_input_truth_table_recursion.gr",
        &src,
        &["fn to_int6", "fn fold4", "fn truth_six", "fn main"],
    );
}

#[test]
fn trust_modulo_five_dispatch_cycle() {
    let _g = lock();
    reset_all();
    let src = fixture("199_modulo_five_dispatch_cycle.gr");
    assert_full_compile_trust(
        "199_modulo_five_dispatch_cycle.gr",
        &src,
        &[
            "fn five_a",
            "fn five_b",
            "fn five_c",
            "fn five_d",
            "fn five_e",
            "fn main",
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
fn trust_parse_error_unclosed_paren() {
    let _g = lock();
    reset_all();
    let src = fixture("92_parse_error_unclosed_paren.gr");
    assert_parse_error_trust("92_parse_error_unclosed_paren.gr", &src);
}

#[test]
fn trust_parse_error_stray_token() {
    let _g = lock();
    reset_all();
    let src = fixture("93_parse_error_stray_token.gr");
    assert_parse_error_trust("93_parse_error_stray_token.gr", &src);
}

#[test]
fn trust_type_error_arity_mismatch() {
    let _g = lock();
    reset_all();
    let src = fixture("94_type_error_arity_mismatch.gr");
    assert_type_error_trust("94_type_error_arity_mismatch.gr", &src);
}

#[test]
fn trust_type_error_arg_type() {
    let _g = lock();
    reset_all();
    let src = fixture("95_type_error_arg_type.gr");
    assert_type_error_trust("95_type_error_arg_type.gr", &src);
}

#[test]
fn trust_type_error_return_mismatch() {
    let _g = lock();
    reset_all();
    let src = fixture("96_type_error_return_mismatch.gr");
    assert_type_error_trust("96_type_error_return_mismatch.gr", &src);
}

#[test]
fn trust_parse_error_missing_colon() {
    let _g = lock();
    reset_all();
    let src = fixture("97_parse_error_missing_colon.gr");
    assert_parse_error_trust("97_parse_error_missing_colon.gr", &src);
}

#[test]
fn trust_type_error_if_condition_int() {
    let _g = lock();
    reset_all();
    let src = fixture("98_type_error_if_condition_int.gr");
    assert_type_error_trust("98_type_error_if_condition_int.gr", &src);
}

#[test]
fn trust_type_error_unknown_identifier() {
    let _g = lock();
    reset_all();
    let src = fixture("99_type_error_unknown_identifier.gr");
    assert_type_error_trust("99_type_error_unknown_identifier.gr", &src);
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
        "46_call_compare_pipeline.gr",
        "47_four_chain_mutual_rec.gr",
        "48_boolean_truth_matrix.gr",
        "49_pipeline_let_chain.gr",
        "50_signed_branch_dispatch.gr",
        "51_arith_with_division.gr",
        "52_compare_let_dispatch.gr",
        "53_calls_inside_if_arms.gr",
        "54_unary_minus_combos.gr",
        "55_recursion_deep_branching.gr",
        "56_modulo_arith.gr",
        "57_compare_chain_recursion.gr",
        "58_calls_in_let_chains.gr",
        "59_negation_truth_logic.gr",
        "60_div_mod_dispatch.gr",
        "61_nested_let_consumes_call.gr",
        "62_compare_modulo_mix.gr",
        "63_five_fn_mutual_rec.gr",
        "64_deep_let_15_binders.gr",
        "65_signed_modulo_dispatch.gr",
        "66_div_compare_chain.gr",
        "67_six_fn_mutual_rec.gr",
        "68_let_chain_with_compare_branches.gr",
        "69_modulo_recursion_branches.gr",
        "70_signed_arith_truth_table.gr",
        "71_mixed_bool_arith_compare.gr",
        "72_three_way_mutual_recursion.gr",
        "73_deep_let_chain_pipeline.gr",
        "74_arith_identities_v2.gr",
        "75_compare_transitivity.gr",
        "76_nested_not_over_compare.gr",
        "77_four_fn_mutual_rec.gr",
        "78_div_mod_let_chain.gr",
        "79_guarded_single_recursion.gr",
        "80_comparison_ladder_truth.gr",
        "81_calls_with_unary_args.gr",
        "82_modulo_truth_table.gr",
        "83_let_call_alternation.gr",
        "84_signed_compare_pipeline.gr",
        "85_compare_negation_recursion.gr",
        "86_arith_call_bool_reduce.gr",
        "87_nested_if_call_returns.gr",
        "88_five_arg_compare_args.gr",
        "89_five_fn_recursion_ladder.gr",
        "100_arith_identities_chain.gr",
        "101_negation_double_unwind.gr",
        "102_ladder_recursion_pipeline.gr",
        "103_compare_neg_arith_blend.gr",
        "104_seven_fn_mutual_rec.gr",
        "105_long_bool_chain_negations.gr",
        "106_eight_fn_mutual_rec.gr",
        "107_div_mod_identity_pipeline.gr",
        "108_compare_let_and_reduce.gr",
        "109_nested_call_arith_identities.gr",
        "110_arith_or_bool_chain.gr",
        "111_nine_fn_mutual_rec.gr",
        "112_recursion_with_nested_let.gr",
        "113_signed_branch_truth.gr",
        "114_compare_let_pipeline_v2.gr",
        "115_ten_fn_mutual_rec.gr",
        "116_deep_nested_if_arith_tree.gr",
        "117_bool_chain_into_multi_arg_call.gr",
        "118_arith_id_mod_pipeline.gr",
        "119_compare_let_recursive_chain.gr",
        "120_eleven_fn_mutual_rec.gr",
        "121_double_call_arith.gr",
        "122_compare_and_or_truth.gr",
        "123_let_chain_with_recursion.gr",
        "124_signed_arith_pipeline.gr",
        "125_twelve_fn_mutual_rec.gr",
        "126_four_input_truth_table.gr",
        "127_arith_associativity_chain.gr",
        "128_triple_nested_let_mix.gr",
        "129_wide_compare_with_negations.gr",
        "130_thirteen_fn_mutual_rec.gr",
        "131_arith_let_pyramid.gr",
        "132_compare_pair_dispatch.gr",
        "133_signed_recursion_with_modulo.gr",
        "134_call_in_let_in_call.gr",
        "135_fourteen_fn_mutual_rec.gr",
        "136_guard_ladder_arith_payloads.gr",
        "137_modulo_dispatch_pipeline.gr",
        "138_call_chain_diamond.gr",
        "139_let_call_compare_braid.gr",
        "140_fifteen_fn_mutual_rec.gr",
        "141_nested_bool_compare_dispatch.gr",
        "142_div_mod_accumulator_braid.gr",
        "143_multi_arg_call_fanout.gr",
        "144_signed_negation_recursion_chain.gr",
        "145_sixteen_fn_mutual_rec.gr",
        "146_compare_driven_arith_selector.gr",
        "147_nested_let_div_mod_fanout.gr",
        "148_recursive_branch_call_mixer.gr",
        "149_boolean_normalization_pipeline.gr",
        "150_seventeen_fn_mutual_rec.gr",
        "151_guard_matrix_arith_payloads.gr",
        "152_div_mod_stage_dispatch.gr",
        "153_call_let_recursive_weave.gr",
        "154_bool_compare_reducer_mix.gr",
        "155_eighteen_fn_mutual_rec.gr",
        "156_compare_chain_truth_fanout.gr",
        "157_let_call_modulo_braid.gr",
        "158_arith_or_bool_dispatch.gr",
        "159_recursive_negation_pipeline.gr",
        "160_nineteen_fn_mutual_rec.gr",
        "161_arith_chain_then_dispatch.gr",
        "162_compare_and_or_truth_full.gr",
        "163_let_pyramid_with_recursion.gr",
        "164_nested_call_modulo_braid.gr",
        "165_twenty_fn_mutual_rec.gr",
        "166_arith_chain_let_dispatch.gr",
        "167_compare_truth_recursion.gr",
        "168_modulo_call_braid.gr",
        "169_signed_let_pipeline.gr",
        "170_twentyone_fn_mutual_rec.gr",
        "171_nested_if_compare_cascade.gr",
        "172_arith_call_arg_shuffle.gr",
        "173_triple_modulo_intersect.gr",
        "174_let_call_chain_negation.gr",
        "175_twentytwo_fn_mutual_rec.gr",
        "176_let_lattice_cross_deps.gr",
        "177_bool_asymmetric_dispatch.gr",
        "178_serial_bool_guard_chain.gr",
        "179_paired_accumulator_recursion.gr",
        "180_twentythree_fn_mutual_rec.gr",
        "181_compare_chain_multi_arg_call.gr",
        "182_dual_modulo_classify.gr",
        "183_let_pyramid_cross_modulo.gr",
        "184_triple_accumulator_recursion.gr",
        "185_twentyfour_fn_mutual_rec.gr",
        "186_signed_compare_truth_dispatch.gr",
        "187_let_modulo_pyramid.gr",
        "188_compare_negation_call_braid.gr",
        "189_quad_accumulator_recursion.gr",
        "190_twentyfive_fn_mutual_rec.gr",
        "191_quintuple_accumulator_recursion.gr",
        "192_let_pyramid_eight_with_recursion.gr",
        "193_five_input_truth_table_recursion.gr",
        "194_modulo_dispatch_mutual_cycle.gr",
        "195_twentysix_fn_mutual_rec.gr",
        "196_sextuple_accumulator_recursion.gr",
        "197_let_pyramid_nine_with_dispatch.gr",
        "198_six_input_truth_table_recursion.gr",
        "199_modulo_five_dispatch_cycle.gr",
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
