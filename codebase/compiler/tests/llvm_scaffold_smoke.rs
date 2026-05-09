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
