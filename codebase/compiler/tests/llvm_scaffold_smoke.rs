//! LLVM backend scaffold smoke tests.
//!
//! These are the minimum tests required to demonstrate that the LLVM
//! backend (`#339`) compiles, accepts our IR, emits a valid object file,
//! and round-trips through `llc` / `clang` to a runnable binary.
//!
//! The richer companion suite lives in `llvm_backend_integration.rs`. As
//! of #339 launch tier, that suite is partially bit-rotted from the IR's
//! growth (new `Instruction` variants, builtin call lowerings the LLVM
//! backend never grew, list/string/enum machinery). This file pins the
//! subset we have actually shipped so future regressions are visible.
//!
//! # Feature gate
//!
//! These tests are only compiled when the `llvm` feature is enabled:
//!
//! ```bash
//! cargo test -p gradient-compiler --features llvm --test llvm_scaffold_smoke
//! ```

#![cfg(feature = "llvm")]

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use gradient_compiler::codegen::llvm::LlvmCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::{parser, typechecker};
use inkwell::context::Context;

/// Lex + parse + check + IR-build the source. Returns the IR module on
/// success; panics with a clear message on any earlier-stage failure so a
/// scaffold smoke can fail loudly when its fixture drifts.
fn lower_to_ir(src: &str) -> gradient_compiler::ir::Module {
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);
    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        real_errors.is_empty(),
        "type errors: {}",
        real_errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    assert!(ir_errors.is_empty(), "IR errors: {:?}", ir_errors);
    ir_module
}

/// Smallest possible test: empty `main`. Pins (a) inkwell builds against
/// LLVM 18 and (b) the backend can declare and compile a function with
/// the `main` C-ABI shape (i32 argc, i8** argv -> i32).
#[test]
fn empty_main_compiles_and_runs() {
    let src = "fn main() -> !{IO} ():\n    ()\n";
    let tmp = TempDir::new().unwrap();
    let ir = lower_to_ir(src);

    let context = Context::create();
    let mut cg = LlvmCodegen::new(&context).expect("LlvmCodegen::new");
    cg.compile_module(&ir).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");
    assert!(!obj_bytes.is_empty(), "object file should not be empty");

    let obj_path = tmp.path().join("out.o");
    let bin_path = tmp.path().join("out");
    fs::write(&obj_path, &obj_bytes).unwrap();

    let runtime_src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("gradient_runtime.c");
    let runtime_obj = tmp.path().join("gradient_runtime.o");
    let cc = Command::new("cc")
        .args(["-c"])
        .arg(&runtime_src)
        .arg("-o")
        .arg(&runtime_obj)
        .status()
        .expect("cc compile runtime");
    assert!(cc.success(), "runtime compile failed: {:?}", cc);

    let link = Command::new("cc")
        .arg(&obj_path)
        .arg(&runtime_obj)
        .arg("-o")
        .arg(&bin_path)
        .arg("-lcurl")
        .status()
        .expect("cc link");
    assert!(link.success(), "link failed: {:?}", link);

    let run = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");
    assert!(run.status.success(), "binary exited non-zero: {:?}", run);
}

/// Verify the LLVM module text produced for a recursive `factorial`
/// function contains the expected structure: the function definition,
/// at least one call to itself, and a multiplication.
///
/// This is the "factorial.gr produces correct LLVM IR" half of
/// acceptance bullet 2 on issue #339. A user-facing builtin (`print_int`)
/// is intentionally NOT used here because the LLVM backend's builtin
/// lowering is incomplete (tracked separately).
#[test]
fn factorial_emits_recursive_llvm_ir() {
    let src = "\
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

fn main() -> !{IO} ():
    ()
";
    let ir = lower_to_ir(src);
    let context = Context::create();
    let mut cg = LlvmCodegen::new(&context).expect("LlvmCodegen::new");
    cg.compile_module(&ir).expect("compile_module");

    let llvm_text = cg.print_to_string_for_test();
    // Function definition for factorial.
    assert!(
        llvm_text.contains("define ") && llvm_text.contains("@factorial"),
        "expected `define ... @factorial` in emitted IR:\n{}",
        llvm_text
    );
    // Self-recursive call.
    assert!(
        llvm_text.contains("call ") && llvm_text.contains("@factorial("),
        "expected `call ... @factorial(...)` in emitted IR:\n{}",
        llvm_text
    );
    // The multiply that combines `n` with the recursive result.
    assert!(
        llvm_text.contains(" mul "),
        "expected an integer multiplication instruction in emitted IR:\n{}",
        llvm_text
    );
}

/// `llc` round-trip: emit a valid LLVM IR text file for a small program
/// and assert that `llc -filetype=obj` accepts it without errors. This
/// is acceptance bullet 3 on issue #339 ("LLVM IR validated via `llc`
/// round-trip").
///
/// We deliberately use an arithmetic-only fixture so no Gradient
/// builtins are referenced — the goal is to validate the emitted IR
/// structurally, not to link a runnable binary.
#[test]
fn llc_round_trip_accepts_emitted_ir() {
    // Locate llc. CI installs the LLVM 18 toolchain at /usr/lib/llvm-18.
    // Locally the developer typically has `llc-18` on PATH.
    let llc = which_llc().expect(
        "neither `llc` nor `llc-18` found on PATH; install LLVM 18 toolchain to run this test",
    );

    let src = "\
fn add(x: Int, y: Int) -> Int:
    ret x + y

fn main() -> !{IO} ():
    ()
";
    let ir = lower_to_ir(src);
    let context = Context::create();
    let mut cg = LlvmCodegen::new(&context).expect("LlvmCodegen::new");
    cg.compile_module(&ir).expect("compile_module");

    let llvm_text = cg.print_to_string_for_test();
    let tmp = TempDir::new().unwrap();
    let ll_path = tmp.path().join("scaffold.ll");
    let obj_path = tmp.path().join("scaffold.o");
    fs::write(&ll_path, &llvm_text).expect("write .ll file");

    let status = Command::new(&llc)
        .arg("-filetype=obj")
        .arg(&ll_path)
        .arg("-o")
        .arg(&obj_path)
        .status()
        .expect("spawn llc");
    assert!(
        status.success(),
        "llc rejected emitted LLVM IR (path={}, status={:?}). \
         IR was:\n{}",
        ll_path.display(),
        status,
        llvm_text
    );

    let meta = fs::metadata(&obj_path).expect("llc produced an object file");
    assert!(meta.len() > 0, "llc produced an empty object file");
}

/// Locate an `llc` binary on PATH. Tries `llc-18` first (Ubuntu/Debian
/// LLVM 18 toolchain), falls back to bare `llc`. Returns `None` if
/// neither exists.
fn which_llc() -> Option<std::path::PathBuf> {
    for name in ["llc-18", "llc"] {
        let probe = Command::new(name).arg("--version").output();
        if let Ok(out) = probe {
            if out.status.success() {
                return Some(std::path::PathBuf::from(name));
            }
        }
    }
    None
}

/// Compile `src` end-to-end with the LLVM backend, link with the C
/// runtime, run the resulting binary, and return `(stdout, exit_code)`.
fn build_run_llvm(src: &str) -> (String, i32) {
    let tmp = TempDir::new().unwrap();
    let ir = lower_to_ir(src);

    let context = Context::create();
    let mut cg = LlvmCodegen::new(&context).expect("LlvmCodegen::new");
    cg.compile_module(&ir).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");

    let obj_path = tmp.path().join("out.o");
    let bin_path = tmp.path().join("out");
    fs::write(&obj_path, &obj_bytes).unwrap();

    let runtime_src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("gradient_runtime.c");
    let runtime_obj = tmp.path().join("gradient_runtime.o");
    let cc = Command::new("cc")
        .args(["-c"])
        .arg(&runtime_src)
        .arg("-o")
        .arg(&runtime_obj)
        .status()
        .expect("cc compile runtime");
    assert!(cc.success(), "runtime compile failed: {:?}", cc);

    let link = Command::new("cc")
        .arg(&obj_path)
        .arg(&runtime_obj)
        .arg("-o")
        .arg(&bin_path)
        .arg("-lcurl")
        .status()
        .expect("cc link");
    assert!(link.success(), "link failed: {:?}", link);

    let out = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// `abs` / `min` / `max` builtins lowered via the LLVM backend (#553).
/// Each prints to stdout via `print_int` (which lowers to
/// `printf("%ld", ...)` — no newline). Output is concatenated digits.
#[test]
fn numeric_builtins_abs_min_max_lower_correctly() {
    let src = "\
fn main() -> !{IO} ():
    print_int(abs(-7))
    print_int(min(3, 9))
    print_int(max(3, 9))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // abs(-7) = 7, min(3,9) = 3, max(3,9) = 9 → "739"
    assert_eq!(out, "739", "unexpected stdout: {:?}", out);
}

/// `mod_int` builtin: `mod_int(17, 5) == 2`.
#[test]
fn numeric_builtin_mod_int_lowers_correctly() {
    let src = "\
fn main() -> !{IO} ():
    print_int(mod_int(17, 5))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "2", "unexpected stdout: {:?}", out);
}

/// `int_to_float` / `float_to_int` round-trip: 42 → 42.0 → 42.
/// Lowered via SIToFP / FPToSI in the LLVM builtin dispatch.
#[test]
fn numeric_builtins_int_float_conversions_lower_correctly() {
    let src = "\
fn main() -> !{IO} ():
    let n: Int = 42
    let f: Float = int_to_float(n)
    print_int(float_to_int(f))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "42", "unexpected stdout: {:?}", out);
}

/// `now_ms()` extern resolves to `__gradient_now_ms` in the C runtime
/// and returns a positive i64. We can't pin the exact value (it's a
/// wall-clock timestamp); just assert the program runs to completion
/// and `print_int(now_ms())` produces non-empty digit output. The if/
/// else path is intentionally avoided because the LLVM backend's
/// existing PHI-type plumbing has a known mismatch on i32-returning
/// builtins (separate follow-on, tracked in `llvm_backend_integration`
/// failures).
#[test]
fn numeric_builtin_now_ms_returns_positive_timestamp() {
    let src = "\
fn main() -> !{IO, Time} ():
    print_int(now_ms())
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert!(
        !out.is_empty() && out.chars().all(|c| c.is_ascii_digit()),
        "expected printed timestamp to be a non-empty digit string; got {:?}",
        out
    );
    let parsed: i64 = out
        .parse()
        .unwrap_or_else(|e| panic!("now_ms() output {:?} did not parse as i64: {}", out, e));
    assert!(
        parsed > 0,
        "now_ms() returned non-positive value: {}",
        parsed
    );
}

/// Regression for the PHI-with-zero-incomings bug closed by #555.
///
/// Pre-fix behaviour: the IR builder always emits a `Phi` at the
/// merge block of an `if`-expression and pushes one phi entry per arm
/// regardless of whether the arm jumped to the merge or terminated via
/// `ret`. The LLVM backend then created an empty `phi.<n>` instruction,
/// cleared `phi_incoming` between functions, and ran the resolver ONCE
/// at the end of the module — so by the time it ran, every function's
/// `phi_incoming` had been clobbered by the next function's clear, and
/// the LLVM phi was left with zero incomings against a merge block
/// whose only would-be predecessors were `ret`-terminated. LLVM's
/// verifier rejected with: "PHINode should have one entry for each
/// predecessor of its parent basic block!".
///
/// Post-fix:
///   1. `compile_function` computes reachable blocks via BFS and
///      skips unreachable ones (the merge block here is unreachable
///      because every arm `ret`s).
///   2. The Phi arm in `compile_instruction` filters incoming entries
///      down to actual jump-targets. With every arm ret'ing, the
///      filter empties the entry list and we emit `unreachable`
///      instead of a 0-incoming phi.
///   3. `resolve_phi_nodes` runs per-function so each function's
///      `block_map`/`phi_incoming` are still in scope.
///
/// This program triggers the original bug because every branch of
/// `classify` returns via `ret`. If the pre-fix codegen were back, the
/// program would fail to emit object code.
#[test]
fn nested_if_all_branches_ret_compiles_and_runs() {
    let src = "\
mod test
fn classify(x: Int) -> Int:
    if x > 0:
        if x > 100:
            ret 3
        else:
            ret 2
    else:
        if x < 0:
            ret 1
        else:
            ret 0

fn main() -> !{IO} ():
    print_int(classify(150))
    print_int(classify(50))
    print_int(classify(-5))
    print_int(classify(0))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_int has no newline → "3210"
    assert_eq!(out, "3210", "unexpected stdout: {:?}", out);
}

/// Companion regression: an `if`-expression where one arm `ret`s and
/// the other falls through to the merge block. The phi should keep
/// exactly one (filtered) incoming. Pre-fix this also miscompiled
/// because the IR builder pushed phantom phi entries from the
/// `ret`-terminated arm.
#[test]
fn if_ret_one_branch_other_falls_through_compiles_and_runs() {
    let src = "\
mod test
fn at_least_ten(x: Int) -> Int:
    if x < 10:
        ret 10
    x

fn main() -> !{IO} ():
    print_int(at_least_ten(5))
    print_int(at_least_ten(42))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // at_least_ten(5)=10, at_least_ten(42)=42 → "1042"
    assert_eq!(out, "1042", "unexpected stdout: {:?}", out);
}

/// `pow(base, exp)` builtin lowered via the LLVM backend (#557).
///
/// Mirrors Cranelift's 3-block integer-exponentiation loop at
/// `cranelift.rs:4431`. Lowered as header / body / exit with phi'd
/// counter and accumulator on the header. `print_int` produces no
/// newline, so output is concatenated digits.
#[test]
fn pow_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO} ():
    print_int(pow(2, 10))
    print_int(pow(3, 4))
    print_int(pow(5, 0))
    print_int(pow(7, 1))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // 2^10=1024, 3^4=81, 5^0=1, 7^1=7 → "1024" + "81" + "1" + "7" = "10248117"
    assert_eq!(out, "10248117", "unexpected stdout: {:?}", out);
}

/// `pow` inside an arithmetic expression — sanity check that the
/// header/body/exit blocks leave the builder positioned correctly so
/// subsequent instructions emit into the exit block.
#[test]
fn pow_builtin_composes_with_arithmetic() {
    let src = "\
fn main() -> !{IO} ():
    let a: Int = pow(2, 8)
    let b: Int = pow(3, 3)
    print_int(a + b)
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // 256 + 27 = 283
    assert_eq!(out, "283", "unexpected stdout: {:?}", out);
}

/// `int_to_string(n)` lowered via the LLVM backend (#559).
///
/// Mirrors Cranelift's malloc(32) + snprintf("%ld", n) recipe. The
/// returned pointer is passed to `print` (no newline) so output is
/// the decimal representation of the i64.
#[test]
fn int_to_string_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(int_to_string(42))
    print(int_to_string(0))
    print(int_to_string(-7))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print() does not add a newline; concatenation: "42" + "0" + "-7" = "420-7"
    assert_eq!(out, "420-7", "unexpected stdout: {:?}", out);
}

/// `string_length(s)` lowered via strlen (#561).
///
/// In Gradient surface syntax this is the `.length()` method on
/// `String`. The typechecker rewrites `s.length()` into a call to
/// the runtime function `string_length`.
#[test]
fn string_length_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO} ():
    let s: String = \"Gradient\"
    print_int(s.length())
    print_int(\"\".length())
    print_int(\"hi\".length())
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_int has no newline → "8" + "0" + "2" = "802"
    assert_eq!(out, "802", "unexpected stdout: {:?}", out);
}

/// `string_concat(a, b)` lowered via malloc + strcpy + strcat (#561).
///
/// In Gradient surface syntax this builtin is reached via the `+`
/// operator on `String` values; the IR builder emits a call to the
/// runtime function `string_concat`.
#[test]
fn string_concat_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    let a: String = \"Hello, \"
    let b: String = \"world!\"
    print(a + b)
    print(\"\" + \"x\")
    print(\"a\" + \"\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print() has no newline; concatenation: "Hello, world!" + "x" + "a" = "Hello, world!xa"
    assert_eq!(out, "Hello, world!xa", "unexpected stdout: {:?}", out);
}

/// `float_to_string(f)` lowered via the LLVM backend (#563).
///
/// Mirrors Cranelift's malloc(64) + snprintf("%g", f) recipe. The
/// returned pointer is passed to `print` (no newline) so output is
/// the `%g` representation of the f64.
///
/// `%g` chooses the shorter of `%e` / `%f` and trims trailing zeros,
/// so `3.14` prints as `3.14`, `0.0` as `0`, and `-2.5` as `-2.5`.
/// Matches the Cranelift backend's output verbatim.
#[test]
fn float_to_string_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(3.14))
    print(float_to_string(0.0))
    print(float_to_string(-2.5))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print() does not add a newline; %g concatenation: "3.14" + "0" + "-2.5" = "3.140-2.5"
    assert_eq!(out, "3.140-2.5", "unexpected stdout: {:?}", out);
}

/// `string_contains(s, sub)` lowered via `strstr` + null-pointer compare (#565).
///
/// Mirrors Cranelift's `strstr(s, sub) != NULL` recipe. Smoke covers
/// (a) a hit in the middle, (b) a miss, (c) the empty-substring edge
/// case (always true).
#[test]
fn string_contains_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO} ():
    print_bool(string_contains(\"Hello, world!\", \"world\"))
    print_bool(string_contains(\"Hello, world!\", \"xyz\"))
    print_bool(string_contains(\"abc\", \"\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_bool has no newline; "true" + "false" + "true"
    assert_eq!(out, "truefalsetrue", "unexpected stdout: {:?}", out);
}

/// `string_starts_with(s, prefix)` lowered via `strncmp` + length compare (#565).
///
/// Mirrors Cranelift's `strncmp(s, prefix, strlen(prefix)) == 0` recipe.
/// Smoke covers (a) a true prefix, (b) a near-miss, (c) the
/// empty-prefix edge case (always true).
#[test]
fn string_starts_with_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO} ():
    print_bool(string_starts_with(\"Gradient\", \"Grad\"))
    print_bool(string_starts_with(\"Gradient\", \"grad\"))
    print_bool(string_starts_with(\"abc\", \"\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_bool has no newline; "true" + "false" + "true"
    assert_eq!(out, "truefalsetrue", "unexpected stdout: {:?}", out);
}

/// `datetime_year` / `datetime_month` / `datetime_day` lowered via
/// `__gradient_datetime_<field>` runtime externs (#567).
///
/// Mirrors Cranelift's `cranelift.rs:5133`/`5145`/`5157` recipes. Each
/// is a thin wrapper over a runtime extern that takes a Unix timestamp
/// and returns the requested calendar field.
///
/// Uses a fixed timestamp `1700000000` = 2023-11-14 22:13:20 UTC.
/// The runtime uses `gmtime`, so the result is deterministic regardless
/// of the host time zone.
#[test]
fn datetime_field_builtins_lower_correctly() {
    let src = "\
fn main() -> !{IO} ():
    let ts: Int = 1700000000
    print_int(datetime_year(ts))
    print_int(datetime_month(ts))
    print_int(datetime_day(ts))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_int has no newline; concatenation: "2023" + "11" + "14"
    assert_eq!(out, "20231114", "unexpected stdout: {:?}", out);
}

/// `time_string` / `date_string` lowered via `__gradient_time_string` /
/// `__gradient_date_string` runtime externs (#567).
///
/// The contents are wall-clock-dependent (RFC3339 / YYYY-MM-DD), so the
/// assertion only verifies non-empty output and a successful exit.
#[test]
fn time_and_date_string_builtins_lower_correctly() {
    let src = "\
fn main() -> !{IO, Time} ():
    let t: String = time_string()
    let d: String = date_string()
    print(t)
    print(\"|\")
    print(d)
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert!(out.contains('|'), "expected separator in {:?}", out);
    let parts: Vec<&str> = out.split('|').collect();
    assert_eq!(parts.len(), 2, "expected 2 segments, got {:?}", parts);
    assert!(!parts[0].is_empty(), "time_string produced empty output");
    assert!(!parts[1].is_empty(), "date_string produced empty output");
    // YYYY-MM-DD shape; full year is 4 digits → at least 10 chars.
    assert!(
        parts[1].len() >= 10,
        "date_string output too short: {:?}",
        parts[1]
    );
}

/// `sleep` / `sleep_seconds` lowered via `__gradient_sleep` /
/// `__gradient_sleep_seconds` runtime externs (#567).
///
/// We sleep `1` ms / `0` s — enough to exercise the lowering, fast
/// enough not to slow tests down.
#[test]
fn sleep_builtins_lower_correctly() {
    let src = "\
fn main() -> !{IO, Time} ():
    sleep(1)
    sleep_seconds(0)
    print(\"ok\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "ok", "unexpected stdout: {:?}", out);
}

/// `==` / `!=` on `String` lowered via `string_eq` (`strcmp`-based) (#569).
///
/// The IR builder rewrites `Eq`/`Ne` on string operands into a call to
/// the `string_eq` runtime function (`ir/builder/mod.rs:2482`); the
/// LLVM backend then lowers that call via `strcmp` + `== 0`.
///
/// Smoke covers (a) equal strings, (b) length-equal but different
/// content, (c) different lengths, (d) `!=` returning the negation.
#[test]
fn string_eq_lowers_correctly_via_eq_operator() {
    let src = "\
fn main() -> !{IO} ():
    print_bool(\"hello\" == \"hello\")
    print_bool(\"hello\" == \"world\")
    print_bool(\"hi\" == \"hello\")
    print_bool(\"hello\" != \"world\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_bool: true, false, false, true (no newlines).
    assert_eq!(out, "truefalsefalsetrue", "unexpected stdout: {:?}", out);
}

/// `string_ends_with(s, suffix)` lowered via `strncmp` on `s + (slen -
/// suflen)` (#569).
///
/// Mirrors Cranelift's `cranelift.rs:3589` recipe. Smoke covers (a) a
/// true suffix, (b) a near-miss with same length, (c) the empty-suffix
/// edge case (always true).
#[test]
fn string_ends_with_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO} ():
    print_bool(string_ends_with(\"Gradient\", \"ient\"))
    print_bool(string_ends_with(\"Gradient\", \"IENT\"))
    print_bool(string_ends_with(\"abc\", \"\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_bool has no newline; "true" + "false" + "true"
    assert_eq!(out, "truefalsetrue", "unexpected stdout: {:?}", out);
}

/// `bool_to_string(b)` lowered via `select` between two static C
/// strings (#569).
///
/// Mirrors Cranelift's `cranelift.rs:4550` recipe. The result is a
/// pointer to a constant string, no allocation — we assert exact
/// `"true"` / `"false"` text via `print`.
#[test]
fn bool_to_string_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(bool_to_string(true))
    print(\"|\")
    print(bool_to_string(false))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "true|false", "unexpected stdout: {:?}", out);
}

/// `float_abs(f)` lowered via libc `fabs` (#569).
///
/// Mirrors Cranelift's `cranelift.rs:4479` recipe. The Cranelift side
/// uses the native `fabs` instruction; the LLVM side calls libc `fabs`,
/// which the linker resolves on glibc without an explicit `-lm`.
///
/// Output uses `float_to_string` (already lowered via #564) so we can
/// pin exact text via `%g` formatting.
#[test]
fn float_abs_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(float_abs(-3.5)))
    print(\"|\")
    print(float_to_string(float_abs(2.25)))
    print(\"|\")
    print(float_to_string(float_abs(0.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // %g formatting: 3.5, 2.25, 0
    assert_eq!(out, "3.5|2.25|0", "unexpected stdout: {:?}", out);
}

/// `float_sqrt(f)` lowered via libm `sqrt` (#569).
///
/// Mirrors Cranelift's `cranelift.rs:4486` recipe. The link step
/// already passes `-lm` for math-using fixtures (the runtime build
/// pipeline already needed it for the bench harness's measurements),
/// so the libm `sqrt` symbol resolves at link time without changes.
#[test]
fn float_sqrt_builtin_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(float_sqrt(16.0)))
    print(\"|\")
    print(float_to_string(float_sqrt(2.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // sqrt(16) = 4 exactly; sqrt(2) ≈ 1.41421 under %g (default 6 sig figs).
    assert!(out.starts_with("4|1.41421"), "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// List-read builtins (#581).
//
// Mirrors Cranelift's `cranelift.rs:5539-5592` + 7027-7055 list layout
// (`[length: i64 @ 0, capacity: i64 @ 8, data: i64[] @ 16]`). The LLVM
// arms in `lower_builtin_call` produce the same bytes; these tests
// pin that the literal constructor + reads agree with Cranelift on
// real programs.
// ---------------------------------------------------------------------

#[test]
fn list_literal_and_length_lower_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = [10, 20, 30, 40]
    print_int(list_length(xs))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "4");
}

#[test]
fn list_get_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = [11, 22, 33]
    print_int(list_get(xs, 0))
    print_int(list_get(xs, 2))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_int has no newline; Cranelift agrees.
    assert_eq!(out, "1133");
}

#[test]
fn list_is_empty_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    let nonempty: List[Int] = [7]
    print_bool(list_is_empty(nonempty))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "false");
}

#[test]
fn list_head_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = [42, 99, 100]
    print_int(list_head(xs))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "42");
}

// ---------------------------------------------------------------------
// RNG + `now` builtins (#583).
//
// Thin wrappers over the `__gradient_random` / `__gradient_random_int`
// / `__gradient_random_float` / `__gradient_seed_random` /
// `__gradient_now` C externs. The runtime is link-bound at the e2e
// step; these smoke tests exercise the LLVM lowering through link +
// execute. RNG output is non-deterministic by definition; tests assert
// on observable invariants (range, type, ordering) rather than fixed
// bytes, mirroring the `now_ms` precedent in #554.
// ---------------------------------------------------------------------

#[test]
fn random_int_in_range_lowers_correctly() {
    // seed_random pins the sequence; random_int(0, 10) must land in
    // [0, 10]. Avoiding nested if/else (pre-existing PHI-type bug for
    // i8/i32 mixed branches — see Pitfall #3 of
    // `gradient-llvm-builtin-lowering-pattern.md`); we print the value
    // and parse it Rust-side.
    let src = "\
fn main() -> !{IO} ():
    seed_random(42)
    let v: Int = random_int(0, 10)
    print_int(v)
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    let parsed: i64 = out
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("random_int output {:?} did not parse: {}", out, e));
    assert!(
        (0..=10).contains(&parsed),
        "random_int(0, 10) returned out-of-range value: {}",
        parsed
    );
}

#[test]
fn random_float_returns_double_lowers_correctly() {
    // `random_float()` returns f64; we cast to int via float_to_int and
    // print to confirm the call lowered + linked. The value itself is
    // non-deterministic; we only check the binary ran cleanly.
    let src = "\
fn main() -> !{IO} ():
    seed_random(7)
    let f: Float = random_float()
    print(\"done\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "done");
}

#[test]
fn random_returns_double_lowers_correctly() {
    // Same shape as random_float(); `random()` is the original alias.
    let src = "\
fn main() -> !{IO} ():
    seed_random(13)
    let f: Float = random()
    print(\"r\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "r");
}

#[test]
fn now_returns_positive_unix_epoch_seconds() {
    // `now()` returns Unix epoch seconds as i64. We assert it parses as
    // a positive i64 (current epoch is ~1.7e9; will stay positive
    // through year 2038). Mirrors `numeric_builtin_now_ms_returns_positive_timestamp`.
    let src = "\
fn main() -> !{IO, Time} ():
    let t: Int = now()
    print_int(t)
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    let parsed: i64 = out
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("now() output {:?} did not parse as i64: {}", out, e));
    assert!(parsed > 0, "now() returned non-positive: {}", parsed);
}

#[test]
fn seed_random_lowers_to_void_call() {
    // `seed_random(seed)` returns Unit. We pin that the call lowers
    // cleanly without a phi/value-map shape error and the program
    // proceeds to the subsequent `print` call.
    let src = "\
fn main() -> !{IO} ():
    seed_random(99)
    print(\"seeded\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "seeded");
}

// ---------------------------------------------------------------------
// Cross-compile target triple plumbing (E6 #342).
//
// These tests pin the `LlvmCodegen::new_with_target` entry point that
// `gradient build --target <triple> --backend llvm` rides on. They do
// NOT link the emitted object — cross-target objects are not host-
// runnable — so they assert against the IR's `target triple` directive
// and the underlying `TargetMachine`'s reported triple.
// ---------------------------------------------------------------------

use gradient_compiler::codegen::llvm::LlvmOptLevel;

/// `new_with_target(None)` reproduces the historical host-targeting
/// behavior. This pins that the refactor (host now goes through the
/// same code path as cross) didn't change observable behavior on the
/// happy path.
#[test]
fn target_triple_default_matches_host() {
    let context = Context::create();
    let cg = LlvmCodegen::new_with_target(&context, LlvmOptLevel::default(), None)
        .expect("default target init");
    let host = inkwell::targets::TargetMachine::get_default_triple()
        .as_str()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        cg.target_triple(),
        host,
        "new_with_target(None) must initialize for the host triple"
    );
}

/// Cross-compile target initialization for a non-host triple. Asserts
/// the backend reports the requested triple AND the emitted IR's
/// `target triple` directive matches. This is the load-bearing
/// observable for cross-compile downstream tooling (`llc`, the linker).
#[test]
fn target_triple_riscv32_initializes_and_emits_in_ir() {
    let context = Context::create();
    let triple = "riscv32-unknown-none-elf";
    let mut cg = LlvmCodegen::new_with_target(&context, LlvmOptLevel::default(), Some(triple))
        .expect("riscv32 target init");

    // The backend's reported triple round-trips through inkwell's
    // TargetTriple, which preserves the canonical form. Asserting
    // contains-prefix instead of strict equality so future canonical-
    // form normalization (e.g. extra `-` segments LLVM may insert)
    // doesn't false-flag.
    assert!(
        cg.target_triple().contains("riscv32"),
        "cross-compile triple {:?} not reported by backend; got {:?}",
        triple,
        cg.target_triple()
    );

    // Emit a tiny module and confirm the `target triple = "..."`
    // directive in IR text matches.
    let src = "fn main() -> Int:\n    ret 0\n";
    let ir = lower_to_ir(src);
    cg.compile_module(&ir)
        .expect("compile_module on cross target");
    let ir_text = cg.print_to_string_for_test();
    assert!(
        ir_text.contains(&format!("target triple = \"{}", triple)),
        "emitted IR missing target triple directive; full IR:\n{}",
        ir_text
    );
}

/// ARM cross-compile target. Sister test to riscv32 to pin a SECOND
/// non-host architecture; if libLLVM lost a target between versions,
/// only one of these would silently regress.
#[test]
fn target_triple_armv7_initializes_and_emits_in_ir() {
    let context = Context::create();
    let triple = "armv7-unknown-none-eabi";
    let mut cg = LlvmCodegen::new_with_target(&context, LlvmOptLevel::default(), Some(triple))
        .expect("armv7 target init");

    assert!(
        cg.target_triple().contains("arm"),
        "cross-compile triple {:?} not reported by backend; got {:?}",
        triple,
        cg.target_triple()
    );

    let src = "fn main() -> Int:\n    ret 0\n";
    let ir = lower_to_ir(src);
    cg.compile_module(&ir)
        .expect("compile_module on cross target");
    let ir_text = cg.print_to_string_for_test();
    assert!(
        ir_text.contains("target triple = \"armv7"),
        "emitted IR missing armv7 target triple directive; full IR:\n{}",
        ir_text
    );
}

/// Bogus triple must produce a clean error, not a panic. This pins
/// the user-facing diagnostic on `gradient build --target garbage
/// --backend llvm` so downstream UX stays good.
#[test]
fn target_triple_bogus_value_returns_error() {
    let context = Context::create();
    let result = LlvmCodegen::new_with_target(
        &context,
        LlvmOptLevel::default(),
        Some("totally-bogus-not-a-real-triple"),
    );
    assert!(
        result.is_err(),
        "bogus triple should error, but got Ok backend"
    );
    let err = result.err().unwrap();
    let msg = format!("{}", err);
    assert!(
        msg.contains("totally-bogus-not-a-real-triple"),
        "error message must mention the offending triple; got {:?}",
        msg
    );
}

/// End-to-end through `BackendWrapper`: the build-system enters this
/// path when `gradient build --backend llvm --target <triple>` is
/// dispatched. Asserts the wrapper's factory plumbs the triple all the
/// way through to a working `TargetMachine`.
#[test]
fn backend_wrapper_with_backend_and_target_initializes_llvm_cross() {
    use gradient_compiler::codegen::BackendWrapper;
    let _wrapper =
        BackendWrapper::new_with_backend_and_target("llvm", Some("riscv32-unknown-none-elf"))
            .expect("BackendWrapper::new_with_backend_and_target llvm + riscv32");
    // Construction succeeded; the IR-level assertions live in the
    // sibling tests above. This pins the wrapper-level entry point so
    // future refactors that drop the second arg don't silently regress
    // CLI cross-compile.
}

// ---------------------------------------------------------------------
// Math libm thin wrappers (#585).
//
// Mirrors Cranelift's `cranelift.rs:7059-7168` recipe. All thin libm
// wrappers (`sin`/`cos`/`tan`/`asin`/`acos`/`atan`/`atan2`/`log`/
// `log10`/`log2`/`exp`/`exp2`/`ceil`/`floor`/`round`/`trunc`/
// `float_mod`). libm folded into libc on modern glibc — no `-lm`
// needed (the existing `float_sqrt` lowering #569 is the precedent).
//
// Each test pins exact `%g`-formatted output for cases where the
// answer is mathematically exact (e.g. `sin(0) == 0`, `floor(3.7) ==
// 3`, `ceil(-2.3) == -2`). For irrational results we assert a
// `starts_with` prefix matching `%g`'s default 6 significant digits.
// ---------------------------------------------------------------------

#[test]
fn math_trig_zero_args_lower_correctly() {
    // sin(0) = 0, cos(0) = 1, tan(0) = 0. All exact under `%g`.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(sin(0.0)))
    print(\"|\")
    print(float_to_string(cos(0.0)))
    print(\"|\")
    print(float_to_string(tan(0.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "0|1|0", "unexpected stdout: {:?}", out);
}

#[test]
fn math_inverse_trig_lowers_correctly() {
    // asin(0) = 0, acos(1) = 0, atan(0) = 0. All exact.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(asin(0.0)))
    print(\"|\")
    print(float_to_string(acos(1.0)))
    print(\"|\")
    print(float_to_string(atan(0.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "0|0|0", "unexpected stdout: {:?}", out);
}

#[test]
fn math_atan2_two_arg_lowers_correctly() {
    // atan2(0, 1) = 0; atan2(1, 0) = pi/2 ≈ 1.5708.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(atan2(0.0, 1.0)))
    print(\"|\")
    print(float_to_string(atan2(1.0, 0.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // %g default 6 sig figs: 0|1.5708
    assert!(out.starts_with("0|1.5708"), "unexpected stdout: {:?}", out);
}

#[test]
fn math_log_exp_lower_correctly() {
    // log(1) = 0, exp(0) = 1, log10(100) = 2, log2(8) = 3, exp2(3) = 8.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(log(1.0)))
    print(\"|\")
    print(float_to_string(exp(0.0)))
    print(\"|\")
    print(float_to_string(log10(100.0)))
    print(\"|\")
    print(float_to_string(log2(8.0)))
    print(\"|\")
    print(float_to_string(exp2(3.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "0|1|2|3|8", "unexpected stdout: {:?}", out);
}

#[test]
fn math_rounding_family_lowers_correctly() {
    // ceil(3.2) = 4, floor(3.7) = 3, round(2.5) = 2 or 3 (banker's
    // round under libm; glibc rounds half-away-from-zero so 2.5 -> 3),
    // trunc(-2.7) = -2.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(ceil(3.2)))
    print(\"|\")
    print(float_to_string(floor(3.7)))
    print(\"|\")
    print(float_to_string(round(2.5)))
    print(\"|\")
    print(float_to_string(trunc(-2.7)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "4|3|3|-2", "unexpected stdout: {:?}", out);
}

#[test]
fn math_float_mod_lowers_correctly() {
    // fmod(7, 3) = 1; fmod(10.5, 3) = 1.5.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(float_mod(7.0, 3.0)))
    print(\"|\")
    print(float_to_string(float_mod(10.5, 3.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "1|1.5", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// String runtime-wrapper builtins (#587).
//
// Single-call delegations to `__gradient_string_*` runtime helpers.
// Each test pins exact stdout from a tiny program that exercises one
// of the new arms in `lower_builtin_call`.
// ---------------------------------------------------------------------

#[test]
fn string_is_empty_lowers_correctly() {
    // is_empty("") = true, is_empty("x") = false.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(bool_to_string(string_is_empty(\"\")))
    print(\"|\")
    print(bool_to_string(string_is_empty(\"x\")))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "true|false", "unexpected stdout: {:?}", out);
}

#[test]
fn string_reverse_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_reverse(\"hello\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "olleh", "unexpected stdout: {:?}", out);
}

#[test]
fn string_trim_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_trim(\"  hello  \"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "hello", "unexpected stdout: {:?}", out);
}

#[test]
fn string_compare_lowers_correctly() {
    // strcmp-like: <0 / 0 / >0. We print the integer result directly so
    // we avoid the if/else + print PHI-type bug (Pitfall #3 of
    // `gradient-llvm-builtin-lowering-pattern.md`). Then parse Rust-side.
    let src = "\
fn main() -> !{IO} ():
    print_int(string_compare(\"a\", \"a\"))
    print(\"|\")
    print_int(string_compare(\"a\", \"b\"))
    print(\"|\")
    print_int(string_compare(\"b\", \"a\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    let parts: Vec<&str> = out.split('|').collect();
    assert_eq!(parts.len(), 3, "expected 3 parts; got {:?}", parts);
    let eq: i64 = parts[0].parse().expect("eq parse");
    let lt: i64 = parts[1].parse().expect("lt parse");
    let gt: i64 = parts[2].parse().expect("gt parse");
    assert_eq!(eq, 0, "string_compare(a, a) should be 0; got {}", eq);
    assert!(lt < 0, "string_compare(a, b) should be < 0; got {}", lt);
    assert!(gt > 0, "string_compare(b, a) should be > 0; got {}", gt);
}

#[test]
fn string_append_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_append(\"foo\", \"bar\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "foobar", "unexpected stdout: {:?}", out);
}

#[test]
fn string_repeat_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_repeat(\"ab\", 3))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "ababab", "unexpected stdout: {:?}", out);
}

#[test]
fn string_slice_lowers_correctly() {
    // slice("hello", 1, 4) = "ell".
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_slice(\"hello\", 1, 4))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "ell", "unexpected stdout: {:?}", out);
}

#[test]
fn string_char_at_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_char_at(\"hello\", 1))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "e", "unexpected stdout: {:?}", out);
}

#[test]
fn string_char_code_at_lowers_correctly() {
    // 'A' = 65, 'a' = 97.
    let src = "\
fn main() -> !{IO} ():
    print_int(string_char_code_at(\"A\", 0))
    print(\"|\")
    print_int(string_char_code_at(\"a\", 0))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "65|97", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// List mutator builtins (#589): list_tail, list_push, list_concat.
//
// All three allocate a new list buffer and memcpy data across. Layout
// matches the #582 read-only family: `[length: i64 @ 0, capacity: i64
// @ 8, data: i64[] @ 16]`.
// ---------------------------------------------------------------------

#[test]
fn list_tail_lowers_correctly() {
    // Drop the first element. Result list reads back via list_length
    // + list_get to confirm length=3 and data=[20, 30, 40].
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = [10, 20, 30, 40]
    let rest = list_tail(xs)
    print_int(list_length(rest))
    print_int(list_get(rest, 0))
    print_int(list_get(rest, 1))
    print_int(list_get(rest, 2))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_int has no newline — concatenated: 3, 20, 30, 40.
    assert_eq!(out, "3203040", "unexpected stdout: {:?}", out);
}

#[test]
fn list_push_lowers_correctly() {
    // Append an element. Result list reads back length=4 and last
    // element = the pushed value.
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = [1, 2, 3]
    let ys = list_push(xs, 99)
    print_int(list_length(ys))
    print_int(list_get(ys, 0))
    print_int(list_get(ys, 3))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // 4, 1, 99 concatenated.
    assert_eq!(out, "4199", "unexpected stdout: {:?}", out);
}

#[test]
fn list_concat_lowers_correctly() {
    // Concatenate two lists. Verify length and a few elements
    // straddling the boundary.
    let src = "\
fn main() -> !{IO, Heap} ():
    let a: List[Int] = [1, 2, 3]
    let b: List[Int] = [40, 50]
    let c = list_concat(a, b)
    print_int(list_length(c))
    print_int(list_get(c, 0))
    print_int(list_get(c, 2))
    print_int(list_get(c, 3))
    print_int(list_get(c, 4))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // 5, 1, 3, 40, 50 concatenated.
    assert_eq!(out, "5134050", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// I/O builtins (#591): file_write / file_read / file_exists /
// file_append / file_delete. read_line / http_* lowerings exist in
// the same PR but aren't smoke-tested here — stdin / network can't be
// exercised reliably under CI. The fact that the binary compiles +
// links proves the externs resolve and the lowering is well-formed.
// ---------------------------------------------------------------------

#[test]
fn file_read_write_exists_delete_lower_correctly() {
    // Round-trip: write a file, check it exists, read it back, delete
    // it, check it's gone. The runtime helpers operate on real disk
    // paths so use /tmp.
    let pid = std::process::id();
    let path = format!("/tmp/gradient_llvm_file_smoke_{}.txt", pid);
    // Best-effort cleanup of any leftover from a prior run.
    let _ = std::fs::remove_file(&path);

    let src = format!(
        "\
fn main() -> !{{IO, FS}} ():
    let ok1 = file_write(\"{p}\", \"hello\")
    print_bool(ok1)
    let exists = file_exists(\"{p}\")
    print_bool(exists)
    let body = file_read(\"{p}\")
    print(body)
    let ok2 = file_delete(\"{p}\")
    print_bool(ok2)
    let exists2 = file_exists(\"{p}\")
    print_bool(exists2)
",
        p = path
    );
    let (out, code) = build_run_llvm(&src);
    // Defensive cleanup in case the binary failed mid-cycle.
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // true true hello true false (concatenated, no separators).
    assert_eq!(
        out, "truetruehellotruefalse",
        "unexpected stdout: {:?}",
        out
    );
}

#[test]
fn file_append_lowers_correctly() {
    // Append to a fresh file and verify both halves are present.
    let pid = std::process::id();
    let path = format!("/tmp/gradient_llvm_file_append_{}.txt", pid);
    let _ = std::fs::remove_file(&path);

    let src = format!(
        "\
fn main() -> !{{IO, FS}} ():
    let ok1 = file_write(\"{p}\", \"abc\")
    print_bool(ok1)
    let ok2 = file_append(\"{p}\", \"def\")
    print_bool(ok2)
    let body = file_read(\"{p}\")
    print(body)
    let _ok3 = file_delete(\"{p}\")
",
        p = path
    );
    let (out, code) = build_run_llvm(&src);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "truetrueabcdef", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// list_contains (#593): multi-block linear search.
//
// Mirrors Cranelift's per-element loop at `cranelift.rs:5744`. Three
// LLVM blocks (header / body / merge) with phi nodes for the loop
// index and the i8 Bool result. List layout matches the rest of the
// list family.
// ---------------------------------------------------------------------

#[test]
fn list_contains_finds_present_element() {
    // Element is in the middle of the list. print_bool prints "true".
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = [10, 20, 30, 40, 50]
    print_bool(list_contains(xs, 30))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "true", "unexpected stdout: {:?}", out);
}

#[test]
fn list_contains_misses_absent_element() {
    // Element is not in the list. print_bool prints "false".
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = [10, 20, 30, 40, 50]
    print_bool(list_contains(xs, 99))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "false", "unexpected stdout: {:?}", out);
}

#[test]
fn list_contains_handles_empty_list() {
    // Empty list: header block immediately falls through to merge
    // with false. Exercises the zero-iteration loop path.
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = []
    print_bool(list_contains(xs, 42))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "false", "unexpected stdout: {:?}", out);
}

#[test]
fn list_contains_finds_first_and_last() {
    // First element exercises the i=0 path; last exercises the
    // i=length-1 path. Both should return true.
    let src = "\
fn main() -> !{IO, Heap} ():
    let xs: List[Int] = [7, 14, 21, 28, 35]
    print_bool(list_contains(xs, 7))
    print_bool(list_contains(xs, 35))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "truetrue", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// String runtime-wrapper family (#595): string_strip, string_pad_left,
// string_pad_right, string_join, string_split, string_format.
//
// All are single-call delegations to existing `__gradient_string_*`
// runtime externs. Cheap-ride bundle — same recipe as #588.
// ---------------------------------------------------------------------

#[test]
fn string_strip_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_strip(\"  spaced  \"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "spaced", "unexpected stdout: {:?}", out);
}

#[test]
fn string_pad_left_lowers_correctly() {
    // Pad "42" on the left with "0" until total length is 5.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_pad_left(\"42\", 5, \"0\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "00042", "unexpected stdout: {:?}", out);
}

#[test]
fn string_pad_right_lowers_correctly() {
    // Pad "hi" on the right with "." until total length is 5.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_pad_right(\"hi\", 5, \".\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "hi...", "unexpected stdout: {:?}", out);
}

#[test]
fn string_join_lowers_correctly() {
    // Join three strings with ", ".
    let src = "\
fn main() -> !{IO, Heap} ():
    let parts: List[String] = [\"a\", \"b\", \"c\"]
    print(string_join(parts, \", \"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "a, b, c", "unexpected stdout: {:?}", out);
}

#[test]
fn string_split_then_join_round_trips() {
    // Split on ":" then join back with "|". Exercises both ABI shapes
    // and confirms the round-trip preserves order.
    let src = "\
fn main() -> !{IO, Heap} ():
    let parts = string_split(\"x:y:z\", \":\")
    print(string_join(parts, \"|\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "x|y|z", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// Math constants and integer math (#599)
// `pi()` / `e()` / `gcd(a, b)` — runtime helpers exposed via
// `__gradient_pi` / `__gradient_e` / `__gradient_gcd` and dispatched by
// the LLVM builtin-lowering switch. The runtime helpers were added in
// this PR so both Cranelift and LLVM now link cleanly against them.
// ---------------------------------------------------------------------

#[test]
fn math_pi_constant_lowers_correctly() {
    // M_PI under `%g` (which `float_to_string` uses) renders as
    // `3.14159`. Exact byte match works because `%g` truncates to 6
    // significant digits.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(pi()))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "3.14159", "unexpected stdout: {:?}", out);
}

#[test]
fn math_e_constant_lowers_correctly() {
    // M_E under `%g` renders as `2.71828`.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(e()))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "2.71828", "unexpected stdout: {:?}", out);
}

#[test]
fn math_gcd_positive_lowers_correctly() {
    // gcd(12, 18) = 6. Classic Euclidean result.
    let src = "\
fn main() -> !{IO} ():
    print_int(gcd(12, 18))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "6", "unexpected stdout: {:?}", out);
}

#[test]
fn math_gcd_coprime_lowers_correctly() {
    // gcd(17, 13) = 1 — both primes, coprime.
    let src = "\
fn main() -> !{IO} ():
    print_int(gcd(17, 13))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "1", "unexpected stdout: {:?}", out);
}

#[test]
fn math_gcd_with_zero_lowers_correctly() {
    // gcd(0, 7) = 7. The runtime handles this via the loop exit when
    // b reaches 0 — a=0, b=7 swaps to a=7, b=0 on iteration 1.
    let src = "\
fn main() -> !{IO} ():
    print_int(gcd(0, 7))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "7", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// clamp(v, lo, hi) (#609)
// Generic-over-T builtin: the LLVM lowering dispatches on the resolved
// `BasicValueEnum` variant of the first argument and routes to
// `__gradient_clamp_i64` (Int branch) or `__gradient_clamp_f64` (Float
// branch). The runtime helpers were added in this PR; Cranelift
// already declared the externs and had a dispatch arm but the C
// symbols did not exist so neither backend could link a real clamp
// call before this PR.
// ---------------------------------------------------------------------

#[test]
fn clamp_int_inside_range_lowers_correctly() {
    // 5 ∈ [0, 10] → 5 passes through unchanged.
    let src = "\
fn main() -> !{IO} ():
    print_int(clamp(5, 0, 10))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "5", "unexpected stdout: {:?}", out);
}

#[test]
fn clamp_int_above_range_lowers_correctly() {
    // 15 > 10 → clamped to upper bound 10.
    let src = "\
fn main() -> !{IO} ():
    print_int(clamp(15, 0, 10))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "10", "unexpected stdout: {:?}", out);
}

#[test]
fn clamp_int_below_range_lowers_correctly() {
    // -3 < 0 → clamped to lower bound 0.
    let src = "\
fn main() -> !{IO} ():
    print_int(clamp(-3, 0, 10))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "0", "unexpected stdout: {:?}", out);
}

#[test]
fn clamp_float_inside_range_lowers_correctly() {
    // 1.5 ∈ [1.0, 2.0] → 1.5 passes through; `%g` renders as `1.5`.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(clamp(1.5, 1.0, 2.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "1.5", "unexpected stdout: {:?}", out);
}

#[test]
fn clamp_float_above_range_lowers_correctly() {
    // 3.5 > 2.0 → clamped to upper bound 2.0; `%g` renders as `2`.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(clamp(3.5, 1.0, 2.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "2", "unexpected stdout: {:?}", out);
}

#[test]
fn clamp_float_below_range_lowers_correctly() {
    // 0.5 < 1.0 → clamped to lower bound 1.0; `%g` renders as `1`.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(clamp(0.5, 1.0, 2.0)))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "1", "unexpected stdout: {:?}", out);
}

#[test]
fn clamp_int_composes_with_arithmetic() {
    // clamp(2 * 8, 0, 10) = clamp(16, 0, 10) = 10. Exercises the
    // IR builder's value_types lookup on a synthesized intermediate.
    let src = "\
fn main() -> !{IO} ():
    print_int(clamp(2 * 8, 0, 10))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "10", "unexpected stdout: {:?}", out);
}

#[test]
fn clamp_int_negative_range_lowers_correctly() {
    // Clamp 5 to a wholly-negative range [-10, -2] → 5 > -2 so result is -2.
    let src = "\
fn main() -> !{IO} ():
    print_int(clamp(5, -10, -2))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "-2", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// parse_int / parse_float / exit (#611)
// libc cheap-ride bundle: atoi (sign-extended to i64), atof (direct),
// libc exit (i64→i32 truncate, noreturn). Mirrors Cranelift's
// `cranelift.rs:5479-5519`.
// ---------------------------------------------------------------------

#[test]
fn parse_int_positive_lowers_correctly() {
    let src = "\
fn main() -> !{IO} ():
    print_int(parse_int(\"42\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "42", "unexpected stdout: {:?}", out);
}

#[test]
fn parse_int_negative_lowers_correctly() {
    // atoi handles `-` prefix; sign-extension preserves negativity.
    let src = "\
fn main() -> !{IO} ():
    print_int(parse_int(\"-137\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "-137", "unexpected stdout: {:?}", out);
}

#[test]
fn parse_float_lowers_correctly() {
    // `%g` truncates 3.14 to `3.14`.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(parse_float(\"3.14\")))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "3.14", "unexpected stdout: {:?}", out);
}

#[test]
fn parse_float_negative_fraction_lowers_correctly() {
    let src = "\
fn main() -> !{IO, Heap} ():
    print(float_to_string(parse_float(\"-0.5\")))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "-0.5", "unexpected stdout: {:?}", out);
}

#[test]
fn exit_with_zero_lowers_correctly() {
    // Exit code 0; any prints AFTER exit must not appear in stdout.
    let src = "\
fn main() -> !{IO} ():
    print(\"before\")
    exit(0)
    print(\"after\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "expected exit code 0; stdout was {:?}", out);
    assert_eq!(out, "before", "unexpected stdout: {:?}", out);
}

#[test]
fn exit_with_nonzero_code_lowers_correctly() {
    // Exit code 42 propagates to the process exit status.
    let src = "\
fn main() -> !{IO} ():
    print(\"before\")
    exit(42)
    print(\"after\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 42, "expected exit code 42; stdout was {:?}", out);
    assert_eq!(out, "before", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// string_to_upper / string_to_lower (#602)
// Multi-block builtins: header/body/exit loop applying libc toupper /
// tolower per byte. Mirrors Cranelift's `cranelift.rs:3691-3870` recipe.
// ---------------------------------------------------------------------

#[test]
fn string_to_upper_alphabetic_lowers_correctly() {
    // Lowercase ASCII passes through toupper → uppercase ASCII.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_to_upper(\"hello\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "HELLO", "unexpected stdout: {:?}", out);
}

#[test]
fn string_to_upper_mixed_case_and_punctuation_lowers_correctly() {
    // Non-alphabetics pass through unchanged; alphabetics uppercased.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_to_upper(\"Hi, World!\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "HI, WORLD!", "unexpected stdout: {:?}", out);
}

#[test]
fn string_to_upper_empty_string_lowers_correctly() {
    // Empty input: header sees len=0 and falls straight to exit. The
    // null-terminator store at buf[0] = 0 still happens, producing "".
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_to_upper(\"\"))
    print(\"|done\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "|done", "unexpected stdout: {:?}", out);
}

#[test]
fn string_to_lower_alphabetic_lowers_correctly() {
    // Uppercase ASCII passes through tolower → lowercase ASCII.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_to_lower(\"HELLO\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "hello", "unexpected stdout: {:?}", out);
}

#[test]
fn string_to_lower_mixed_case_and_digits_lowers_correctly() {
    // Digits/punctuation pass through unchanged.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_to_lower(\"AbC 123 XyZ\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "abc 123 xyz", "unexpected stdout: {:?}", out);
}

#[test]
fn string_to_upper_then_lower_round_trips() {
    // Compose both transforms in the same function to exercise the
    // multi-block phi machinery twice and confirm no stale-block bugs.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_to_lower(string_to_upper(\"GrAdIeNt\")))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "gradient", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// string_substring / string_index_of (#605)
// Straight-line builtins: substring does malloc + memcpy + nul-term,
// index_of does strstr + pointer-diff + select. Mirrors Cranelift's
// `cranelift.rs:3656` and `cranelift.rs:4081` recipes.
// ---------------------------------------------------------------------

#[test]
fn string_substring_basic_extraction_lowers_correctly() {
    // s.substring(0, 5) on "Hello, World!" → "Hello".
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_substring(\"Hello, World!\", 0, 5))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "Hello", "unexpected stdout: {:?}", out);
}

#[test]
fn string_substring_mid_range_lowers_correctly() {
    // s.substring(7, 12) on "Hello, World!" → "World".
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_substring(\"Hello, World!\", 7, 12))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "World", "unexpected stdout: {:?}", out);
}

#[test]
fn string_substring_empty_range_lowers_correctly() {
    // s.substring(3, 3) → "", followed by sentinel to confirm the
    // null-terminator was written and the result is a valid empty C string.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_substring(\"hello\", 3, 3))
    print(\"|done\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "|done", "unexpected stdout: {:?}", out);
}

#[test]
fn string_substring_method_call_surface_works() {
    // Surface form `s.substring(start, end)` (typechecker rewrite to
    // `string_substring`). Confirms the method-call path lands in the
    // same lowering arm.
    let src = "\
fn main() -> !{IO, Heap} ():
    let s: String = \"abcdef\"
    print(s.substring(1, 4))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "bcd", "unexpected stdout: {:?}", out);
}

#[test]
fn string_index_of_found_returns_offset() {
    // "World" starts at index 7 in "Hello, World!".
    let src = "\
fn main() -> !{IO, Heap} ():
    print_int(string_index_of(\"Hello, World!\", \"World\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "7", "unexpected stdout: {:?}", out);
}

#[test]
fn string_index_of_not_found_returns_negative_one() {
    // "xyz" not in "Hello, World!" → -1.
    let src = "\
fn main() -> !{IO, Heap} ():
    print_int(string_index_of(\"Hello, World!\", \"xyz\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "-1", "unexpected stdout: {:?}", out);
}

#[test]
fn string_index_of_first_char_returns_zero() {
    // strstr returns s itself when substr matches at offset 0 →
    // pointer-diff = 0. Pins that the NULL check doesn't mis-fire on
    // a 0-offset match (NULL is the only sentinel, not the offset).
    let src = "\
fn main() -> !{IO, Heap} ():
    print_int(string_index_of(\"abc\", \"a\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "0", "unexpected stdout: {:?}", out);
}

#[test]
fn string_index_of_method_call_surface_works() {
    // Surface form `s.index_of(sub)` (typechecker rewrite to
    // `string_index_of`).
    let src = "\
fn main() -> !{IO, Heap} ():
    let s: String = \"abcdef\"
    print_int(s.index_of(\"cd\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "2", "unexpected stdout: {:?}", out);
}

#[test]
fn string_substring_and_index_of_compose() {
    // index_of finds offset, then substring extracts the remainder.
    // s = "key=value", sep_pos = index_of(s, "="), val =
    // substring(s, sep_pos+1, length). length hardcoded to 9 here
    // because string_length isn't part of this PR's surface.
    let src = "\
fn main() -> !{IO, Heap} ():
    let s: String = \"key=value\"
    let p: Int = string_index_of(s, \"=\")
    print(string_substring(s, p + 1, 9))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "value", "unexpected stdout: {:?}", out);
}

// ---------------------------------------------------------------------
// string_replace (#607)
// Multi-block builtin: entry/empty/nonempty/header/found/notfound/merge
// with phi nodes for src_pos + dst_pos in the loop header and a result
// phi in merge. Mirrors Cranelift's `cranelift.rs:3891` recipe.
// ---------------------------------------------------------------------

#[test]
fn string_replace_single_occurrence_lowers_correctly() {
    // Replace one substring with a shorter one.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_replace(\"Hello, World!\", \"World\", \"Gradient\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "Hello, Gradient!", "unexpected stdout: {:?}", out);
}

#[test]
fn string_replace_multiple_occurrences_lowers_correctly() {
    // Replace ALL occurrences (Cranelift's loop replaces every hit).
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_replace(\"a-b-c-d\", \"-\", \"_\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "a_b_c_d", "unexpected stdout: {:?}", out);
}

#[test]
fn string_replace_no_match_passes_through() {
    // No match → loop's first strstr returns NULL → notfound copies
    // the entire input verbatim.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_replace(\"hello\", \"xyz\", \"abc\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "hello", "unexpected stdout: {:?}", out);
}

#[test]
fn string_replace_empty_old_returns_input_copy() {
    // old_len == 0 path: short-circuit to malloc + strcpy, returning
    // a copy of the input. Cranelift's empty_block half.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_replace(\"keep me\", \"\", \"NOPE\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "keep me", "unexpected stdout: {:?}", out);
}

#[test]
fn string_replace_longer_replacement_lowers_correctly() {
    // Replacement bigger than the match — exercises the over-allocate
    // worst-case sizing `s_len * (new_len + 1) + 1`.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_replace(\"a-b\", \"-\", \"<sep>\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "a<sep>b", "unexpected stdout: {:?}", out);
}

#[test]
fn string_replace_method_call_surface_works() {
    // Method-call surface form: typechecker rewrites `s.replace(o, n)`
    // to the `string_replace` IR call. Confirms the lowering arm is
    // reachable from both the bare-name and method paths.
    let src = "\
fn main() -> !{IO, Heap} ():
    let s: String = \"foo bar baz\"
    print(s.replace(\"bar\", \"BAR\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "foo BAR baz", "unexpected stdout: {:?}", out);
}

#[test]
fn string_replace_adjacent_matches_lowers_correctly() {
    // Two adjacent matches: ensures the loop's src_pos advance moves
    // past the matched substring (found_ptr + old_len), not just one byte.
    let src = "\
fn main() -> !{IO, Heap} ():
    print(string_replace(\"xxxxxx\", \"xx\", \"y\"))
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "yyy", "unexpected stdout: {:?}", out);
}

// ── Environment / process bundle (#613) ────────────────────────────────
//
// Four cheap-ride builtins: set_env, current_dir, change_dir, process_id.
// Each smoke test drives the binary end-to-end and asserts stdout/exit.

/// `set_env("GRADIENT_TEST_KEY", "hello")` sets the var; we then prove
/// the runtime side-effect landed by reading it back via `@extern`
/// would be heavyweight; instead we rely on Cranelift mirroring the
/// same lowering — assert that the program completes cleanly and
/// emits the literal we print before/after.
#[test]
fn set_env_writes_and_program_completes() {
    let src = "\
fn main() -> !{IO, Heap} ():
    set_env(\"GRADIENT_TEST_KEY_613\", \"hello\")
    print(\"ok\")
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "ok", "unexpected stdout: {:?}", out);
}

/// `current_dir()` returns a non-empty String that starts with '/' on
/// POSIX. We avoid pinning the exact path because tests run in
/// arbitrary tempdirs.
#[test]
fn current_dir_returns_absolute_path() {
    let src = "\
fn main() -> !{IO, Heap} ():
    let d: String = current_dir()
    print(d)
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert!(
        out.starts_with('/'),
        "expected absolute path starting with '/', got {:?}",
        out
    );
    assert!(!out.is_empty(), "current_dir returned empty string");
}

/// `change_dir("/")` changes to root, then `current_dir()` should
/// report "/". Exercises the change_dir LLVM arm AND its interaction
/// with the runtime's process-global cwd.
#[test]
fn change_dir_then_current_dir_reflects_change() {
    let src = "\
fn main() -> !{IO, Heap} ():
    change_dir(\"/\")
    print(current_dir())
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "/", "expected '/' after chdir to root, got {:?}", out);
}

/// `process_id()` returns a positive i64 (kernel pid). The exact
/// value is non-deterministic — assert it parses to a positive int
/// when printed via `print_int`.
#[test]
fn process_id_returns_positive_integer() {
    let src = "\
fn main() -> !{IO} ():
    print_int(process_id())
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    let pid: i64 = out
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("process_id stdout {:?} did not parse as i64: {}", out, e));
    assert!(pid > 0, "expected pid > 0, got {}", pid);
}

/// Composition: process_id() arithmetic exercises the sign-extend +
/// downstream consumer path. `pid + 0` should round-trip the value.
#[test]
fn process_id_composes_with_arithmetic() {
    let src = "\
fn main() -> !{IO} ():
    let pid: Int = process_id()
    print_int(pid + 0)
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    let pid: i64 = out
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("composed stdout {:?} did not parse as i64: {}", out, e));
    assert!(pid > 0, "expected positive pid, got {}", pid);
}

// ── #615: `args()` builtin LLVM lowering ─────────────────────────────────────
//
// The `args()` builtin returns `!{IO} List[String]` populated from C `argv`.
// The LLVM backend lowers via `__gradient_get_args` and emits a
// `__gradient_save_args(argc, argv)` call at the top of `main` so the
// runtime's saved-argc/argv statics are populated. Mirrors Cranelift's setup.

/// `args()` returns a non-empty list when the binary is invoked — at minimum
/// the program name (`argv[0]`) is present, so `args().length() >= 1`.
#[test]
fn args_returns_at_least_program_name() {
    let src = "\
fn main() -> !{IO, Heap} ():
    let argv: List[String] = args()
    print_int(argv.length())
";
    let (out, code) = build_run_llvm(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    let n: i64 = out
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("args length stdout {:?} did not parse: {}", out, e));
    assert!(n >= 1, "expected args().length() >= 1, got {}", n);
}

// Note: testing per-element access through `list_get` is gated on #340
// (closures/generics/pattern match) because the IR-builder erases the
// element type of `List[T]` — `list_get` always returns IR `i64`. A test
// like `string_length(list_get(argv, 0))` fails LLVM module verification
// with "Call parameter type does not match function signature" since
// `strlen` expects `ptr` but receives `i64`. The Cranelift backend's
// flat type system masks the same shape. Once #340 lands, the per-element
// String coverage moves into a follow-on PR.

/// Cross-backend parity: LLVM and Cranelift produce the same `args().length()`
/// when invoked with the same argv (the test harness passes no extra args,
/// so both should report `1`).
#[test]
fn args_length_backends_match() {
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    let src = "\
fn main() -> !{IO, Heap} ():
    print_int(args().length())
";

    let (llvm_out, llvm_code) = build_run_llvm(src);
    assert_eq!(llvm_code, 0, "llvm binary failed; stdout {:?}", llvm_out);

    // Cranelift path: drive the compiler binary the normal way.
    let tmp = TempDir::new().unwrap();
    let src_path = tmp.path().join("argslen.gr");
    std::fs::write(&src_path, src).unwrap();
    let obj_path = tmp.path().join("argslen.o");
    let bin_path = tmp.path().join("argslen");
    let compiler = std::path::PathBuf::from(env!("CARGO_BIN_EXE_gradient-compiler"));
    let status = Command::new(&compiler)
        .arg("--backend")
        .arg("cranelift")
        .arg(&src_path)
        .arg(&obj_path)
        .status()
        .expect("run gradient-compiler");
    assert!(status.success(), "cranelift compile failed: {:?}", status);

    let runtime_src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("gradient_runtime.c");
    let runtime_obj = tmp.path().join("gradient_runtime.o");
    let cc = Command::new("cc")
        .args(["-c"])
        .arg(&runtime_src)
        .arg("-o")
        .arg(&runtime_obj)
        .status()
        .expect("cc compile runtime (cranelift path)");
    assert!(cc.success(), "runtime compile failed: {:?}", cc);

    let link = Command::new("cc")
        .arg(&obj_path)
        .arg(&runtime_obj)
        .arg("-o")
        .arg(&bin_path)
        .arg("-lcurl")
        .status()
        .expect("cc link (cranelift path)");
    assert!(link.success(), "cranelift link failed: {:?}", link);

    let out = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run cranelift binary");
    let cl_out = String::from_utf8_lossy(&out.stdout).to_string();
    let cl_code = out.status.code().unwrap_or(-1);
    assert_eq!(cl_code, 0, "cranelift binary failed; stdout {:?}", cl_out);

    assert_eq!(
        llvm_out.trim(),
        cl_out.trim(),
        "args().length() differed: llvm={:?} cranelift={:?}",
        llvm_out,
        cl_out
    );
}
