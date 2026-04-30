//! Issue #231: bootstrap compiler driver gate.
//!
//! With #230 wiring the lex / parse / check / lower / emit phases together,
//! this test exercises the user-facing driver kernel introduced in
//! `bootstrap_driver.rs`. The driver is what `compiler/main.gr` will call
//! once cross-module extern resolution lands; this gate validates the
//! kernel directly so we know the contract works end-to-end before .gr
//! wiring.
//!
//! Acceptance criteria from #231:
//!
//!   1. Running the self-hosted driver on a simple file executes the
//!      real pipeline (exit 0, real captured/written output).
//!   2. Invalid input path returns a diagnostic and non-zero status.
//!   3. Syntax/type errors are printed and return non-zero status with
//!      stable, distinguishable exit codes.
//!   4. Successful bootstrap fixture returns zero and writes/prints
//!      expected output.
//!
//! Exit-code wire format (must match `compiler/main.gr` once it lands):
//!
//!   0 = ok, 1 = read error, 2 = parse error, 3 = type error,
//!   4 = lower error, 5 = write error, 6 = internal error
//!
//! Companion gates: self_hosted_pipeline (#230), self_hosted_codegen_text
//! (#229), ir_differential_tests (#228).

#![allow(clippy::uninlined_format_args)]

use std::sync::{Mutex, MutexGuard, OnceLock};

use gradient_compiler::bootstrap_ast_bridge::reset_ast_store;
use gradient_compiler::bootstrap_driver::{
    bootstrap_driver_get_captured_output, bootstrap_driver_get_diagnostic_at,
    bootstrap_driver_get_diagnostic_count, bootstrap_driver_get_exit_code,
    bootstrap_driver_get_module_name, bootstrap_driver_get_written_path, bootstrap_driver_run_file,
    bootstrap_driver_run_source, reset_driver_store, DRIVER_OK, DRIVER_PARSE_ERROR,
    DRIVER_READ_ERROR, DRIVER_TYPE_ERROR,
};
use gradient_compiler::bootstrap_ir_bridge::reset_ir_store;
use gradient_compiler::bootstrap_pipeline::reset_pipeline_store;

fn driver_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn reset_all() {
    reset_driver_store();
    reset_pipeline_store();
    reset_ast_store();
    reset_ir_store();
}

/// Acceptance: simple bootstrap fixture flows through every phase, exits
/// 0, and produces non-empty captured output containing the expected
/// function and terminator.
#[test]
fn driver_happy_path_in_memory() {
    let _g = driver_lock();
    reset_all();
    let src = "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n";
    let run = bootstrap_driver_run_source(src, "");
    assert_eq!(
        bootstrap_driver_get_exit_code(run),
        DRIVER_OK,
        "happy-path source must return DRIVER_OK"
    );
    let out = bootstrap_driver_get_captured_output(run);
    assert!(out.contains("module main"), "module header missing");
    assert!(out.contains("fn add"), "function missing from emitted text");
    assert!(out.contains("ret"), "Ret terminator missing");
    assert_eq!(
        bootstrap_driver_get_module_name(run),
        "main",
        "in-memory runs use `main` as module name"
    );
}

/// Acceptance: the same fixture writes to disk when an output path is
/// provided, and the on-disk text matches what the driver captured.
#[test]
fn driver_writes_output_file_when_path_provided() {
    let _g = driver_lock();
    reset_all();
    let src = "fn answer() -> Int:\n    ret 42\n";
    let tmp = std::env::temp_dir().join("self_hosted_driver_test_writes.txt");
    let _ = std::fs::remove_file(&tmp);
    let run = bootstrap_driver_run_source(src, tmp.to_str().unwrap());
    assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_OK);
    assert_eq!(
        bootstrap_driver_get_written_path(run),
        tmp.to_str().unwrap(),
        "written path must round-trip through accessor"
    );
    let on_disk = std::fs::read_to_string(&tmp).expect("driver wrote file");
    assert!(
        on_disk.contains("fn answer"),
        "on-disk emission must contain the function"
    );
    let _ = std::fs::remove_file(&tmp);
}

/// Acceptance: invalid input path returns DRIVER_READ_ERROR with at
/// least one diagnostic explaining the failure.
#[test]
fn driver_missing_input_file_reports_read_error() {
    let _g = driver_lock();
    reset_all();
    let run = bootstrap_driver_run_file("/definitely/not/a/real/path-12345.gr", "");
    assert_eq!(
        bootstrap_driver_get_exit_code(run),
        DRIVER_READ_ERROR,
        "missing input must return DRIVER_READ_ERROR"
    );
    assert!(
        bootstrap_driver_get_diagnostic_count(run) >= 1,
        "must emit at least one read-error diagnostic"
    );
    let first = bootstrap_driver_get_diagnostic_at(run, 0);
    assert!(
        first.contains("cannot read") || first.contains("not found") || first.contains("error"),
        "diagnostic must describe the failure, got: {:?}",
        first
    );
}

/// Acceptance: empty source is treated as a read error so .gr `main`
/// can short-circuit without hitting parse.
#[test]
fn driver_empty_source_reports_read_error() {
    let _g = driver_lock();
    reset_all();
    let run = bootstrap_driver_run_source("", "");
    assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_READ_ERROR);
}

/// Acceptance: parse errors return DRIVER_PARSE_ERROR with at least
/// one parser diagnostic. Test a trailing-binop fixture.
#[test]
fn driver_parse_error_returns_parse_exit_code() {
    let _g = driver_lock();
    reset_all();
    let bad = "fn broken(x: Int) -> Int:\n    ret x +\n";
    let run = bootstrap_driver_run_source(bad, "");
    assert_eq!(
        bootstrap_driver_get_exit_code(run),
        DRIVER_PARSE_ERROR,
        "parse error must return DRIVER_PARSE_ERROR"
    );
    let count = bootstrap_driver_get_diagnostic_count(run);
    assert!(count > 0, "parse error must emit diagnostics");
    // No captured output for failed runs.
    assert_eq!(
        bootstrap_driver_get_captured_output(run),
        "",
        "failed runs must not emit captured output"
    );
}

/// Acceptance: type errors return DRIVER_TYPE_ERROR with at least one
/// type-check diagnostic mentioning the offending identifier.
#[test]
fn driver_type_error_returns_type_exit_code() {
    let _g = driver_lock();
    reset_all();
    let bad = "fn f(x: Int) -> Int:\n    ret bogus\n";
    let run = bootstrap_driver_run_source(bad, "");
    assert_eq!(
        bootstrap_driver_get_exit_code(run),
        DRIVER_TYPE_ERROR,
        "type error must return DRIVER_TYPE_ERROR"
    );
    let count = bootstrap_driver_get_diagnostic_count(run);
    assert!(count > 0, "type error must emit diagnostics");
    let any_mentions = (0..count)
        .map(|i| bootstrap_driver_get_diagnostic_at(run, i))
        .any(|d| d.contains("bogus") || d.contains("undefined"));
    assert!(
        any_mentions,
        "at least one diagnostic must mention the offending identifier"
    );
}

/// Acceptance: module name is derived from the input file's stem so
/// `compiler/foo.gr` becomes module `foo` end-to-end.
#[test]
fn driver_module_name_from_path_stem() {
    let _g = driver_lock();
    reset_all();
    let tmp = std::env::temp_dir().join("driver_module_name_demo.gr");
    std::fs::write(&tmp, "fn answer() -> Int:\n    ret 42\n").unwrap();
    let run = bootstrap_driver_run_file(tmp.to_str().unwrap(), "");
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_OK);
    assert_eq!(
        bootstrap_driver_get_module_name(run),
        "driver_module_name_demo",
        "module name must come from path stem"
    );
    let out = bootstrap_driver_get_captured_output(run);
    assert!(
        out.starts_with("module driver_module_name_demo"),
        "module header in emission must use derived name, got: {:?}",
        out.lines().next()
    );
}

/// Acceptance: deterministic across reset+rerun on the same source.
/// Catches any hidden randomness in the driver / pipeline / emission.
#[test]
fn driver_emission_is_deterministic_across_runs() {
    let _g = driver_lock();
    let src = "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n";

    reset_all();
    let r1 = bootstrap_driver_run_source(src, "");
    let t1 = bootstrap_driver_get_captured_output(r1);

    reset_all();
    let r2 = bootstrap_driver_run_source(src, "");
    let t2 = bootstrap_driver_get_captured_output(r2);

    assert_eq!(
        bootstrap_driver_get_exit_code(r1),
        DRIVER_OK,
        "first run must succeed"
    );
    assert_eq!(t1, t2, "driver emission must be byte-identical across runs");
    assert!(!t1.is_empty(), "emission must be non-empty");
}

/// Acceptance: distinct exit codes for distinct error classes — the
/// .gr `main` driver depends on this to render different messages and
/// (eventually) different process exit statuses.
#[test]
fn driver_distinguishes_exit_codes_by_error_class() {
    let _g = driver_lock();

    reset_all();
    let r_ok = bootstrap_driver_run_source("fn f() -> Int:\n    ret 1\n", "");
    let ok = bootstrap_driver_get_exit_code(r_ok);

    reset_all();
    let r_parse = bootstrap_driver_run_source("fn f() -> Int:\n    ret 1 +\n", "");
    let parse = bootstrap_driver_get_exit_code(r_parse);

    reset_all();
    let r_type = bootstrap_driver_run_source("fn f() -> Int:\n    ret bogus\n", "");
    let ty = bootstrap_driver_get_exit_code(r_type);

    reset_all();
    let r_read = bootstrap_driver_run_source("", "");
    let read = bootstrap_driver_get_exit_code(r_read);

    assert_eq!(ok, DRIVER_OK);
    assert_eq!(parse, DRIVER_PARSE_ERROR);
    assert_eq!(ty, DRIVER_TYPE_ERROR);
    assert_eq!(read, DRIVER_READ_ERROR);
    // Distinct values:
    let codes = [ok, parse, ty, read];
    let mut sorted = codes.to_vec();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        codes.len(),
        "all four exit codes must be distinct, got {:?}",
        codes
    );
}
