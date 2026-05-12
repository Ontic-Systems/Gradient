//! Cranelift backend regression test for issue #600.
//!
//! Regression: previously, consecutive `f64`-returning extern calls
//! (`pi()`, `e()`, `random_float()`, etc.) interleaved with `print` /
//! `float_to_string` / `print_float` produced garbage f64 output
//! (subnormals like `5e-310`) on the Cranelift backend.
//!
//! Root cause: Cranelift's `Signature` type cannot mark a callee as
//! SysV-AMD64 variadic, so the backend never emitted `mov $1, %al`
//! before the `call_indirect` to `printf`/`snprintf`. The first call
//! in a function happened to work because `%al` was non-zero by
//! coincidence; later calls failed because intervening libc calls
//! left `%al = 0`, and glibc then read the f64 va_arg from the
//! integer overflow area instead of `xmm0`.
//!
//! Fix: route f64-printf through non-variadic C wrappers
//! `__gradient_format_float(buf, size, val)` and
//! `__gradient_print_float(val)`. These take a fixed-shape signature
//! Cranelift can model directly — no `call_indirect`, no `%al` setup
//! required.
//!
//! This file pins the canonical multi-call repros so a regression on
//! Cranelift's f64-extern dispatch path is visible.

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;

fn compile_and_run_cranelift(src: &str) -> (String, i32) {
    let tmp = TempDir::new().expect("temp dir");

    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(real_errors.is_empty(), "type errors: {:?}", real_errors);

    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    assert!(ir_errors.is_empty(), "IR errors: {:?}", ir_errors);

    let mut cg = CraneliftCodegen::new().expect("CraneliftCodegen::new");
    cg.compile_module(&ir_module).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");

    let obj_path = tmp.path().join("out.o");
    let bin_path = tmp.path().join("out");
    fs::write(&obj_path, &obj_bytes).expect("write .o");

    let runtime_src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("gradient_runtime.c");
    let runtime_obj = tmp.path().join("gradient_runtime.o");
    let cc_compile = Command::new("cc")
        .arg("-c")
        .arg(&runtime_src)
        .arg("-o")
        .arg(&runtime_obj)
        .status()
        .expect("cc compile runtime");
    assert!(
        cc_compile.success(),
        "runtime compile failed: {:?}",
        cc_compile
    );

    let link_status = Command::new("cc")
        .arg(&obj_path)
        .arg(&runtime_obj)
        .arg("-o")
        .arg(&bin_path)
        .arg("-lcurl")
        .arg("-lm")
        .status()
        .expect("cc link");
    assert!(link_status.success(), "link failed: {:?}", link_status);

    let output = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}

/// Two-call repro from issue #600. Pre-fix: `3.14159|5.20103e-310`.
/// Post-fix: `3.14159|3.14159`.
#[test]
fn cranelift_two_calls_to_pi_in_sequence_return_correct_values() {
    let src = "\
mod test
fn main() -> !{IO, Heap} ():
    print(float_to_string(pi()))
    print(\"|\")
    print(float_to_string(pi()))
";
    let (out, code) = compile_and_run_cranelift(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "3.14159|3.14159", "stdout was {:?}", out);
}

/// `pi()` then `e()` in the same function — previously worked by
/// coincidence (first call lucked into non-zero `%al`, second call had
/// `%al` non-zero from a preceding printf). Pin it explicitly so a
/// future refactor cannot regress the easy case.
#[test]
fn cranelift_pi_then_e_returns_distinct_constants() {
    let src = "\
mod test
fn main() -> !{IO, Heap} ():
    print(float_to_string(pi()))
    print(\"|\")
    print(float_to_string(e()))
";
    let (out, code) = compile_and_run_cranelift(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "3.14159|2.71828", "stdout was {:?}", out);
}

/// Four-call alternation: this is the original #600 repro extended to
/// show the bug is positional, not specific to any single builtin. Pre-fix
/// output was `3.14159|3.14159|3.14159|2.71828` (only the LAST `e()`
/// returned the correct value; all earlier calls returned `pi()`'s
/// value because `%al = 0` made glibc read the wrong va_arg slot).
#[test]
fn cranelift_pi_e_pi_e_alternation_returns_correct_constants() {
    let src = "\
mod test
fn main() -> !{IO, Heap} ():
    print(float_to_string(pi()))
    print(\"|\")
    print(float_to_string(e()))
    print(\"|\")
    print(float_to_string(pi()))
    print(\"|\")
    print(float_to_string(e()))
";
    let (out, code) = compile_and_run_cranelift(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(
        out, "3.14159|2.71828|3.14159|2.71828",
        "stdout was {:?}",
        out
    );
}

/// `pi()` sandwiching a `random_float()` call — confirms the fix is
/// not specific to math constants. Pre-fix, the middle `random_float()`
/// returned a subnormal (`5e-310`-ish). Post-fix, both outer pi()s
/// return 3.14159 and the middle value is a real number in [0, 1).
#[test]
fn cranelift_pi_sandwiches_random_float_call() {
    let src = "\
mod test
fn main() -> !{IO, Heap} ():
    print(float_to_string(pi()))
    print(\"|\")
    print(float_to_string(random_float()))
    print(\"|\")
    print(float_to_string(pi()))
";
    let (out, code) = compile_and_run_cranelift(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // Random middle value varies; assert structure: pi|<some-value>|pi
    let parts: Vec<&str> = out.split('|').collect();
    assert_eq!(
        parts.len(),
        3,
        "expected three |-separated parts: {:?}",
        out
    );
    assert_eq!(parts[0], "3.14159", "first part should be pi: {:?}", out);
    assert_eq!(parts[2], "3.14159", "third part should be pi: {:?}", out);
    // Middle value must NOT be a subnormal. A correct random_float
    // returns a value in [0, 1); pre-fix produced ~5e-310 which would
    // format as `5.X e-310`.
    let mid_str = parts[1];
    assert!(
        !mid_str.contains("e-31") && !mid_str.contains("e-30"),
        "middle value looks like a subnormal (bug regression): {:?}",
        mid_str
    );
}

/// `print_float` arm regression: three consecutive print_float calls
/// with intervening `print` separators. Pre-fix, the middle print_float
/// occasionally printed garbage. Post-fix, all three print correctly
/// via the non-variadic `__gradient_print_float` wrapper.
#[test]
fn cranelift_print_float_three_calls_render_correctly() {
    let src = "\
mod test
fn main() -> !{IO, Heap} ():
    print_float(pi())
    print(\"|\")
    print_float(e())
    print(\"|\")
    print_float(pi())
";
    let (out, code) = compile_and_run_cranelift(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    // print_float uses "%.6f", so output has 6 decimal places.
    assert_eq!(out, "3.141593|2.718282|3.141593", "stdout was {:?}", out);
}

/// Composition test: nested f64-returning calls. `gcd(24, 36) == 12`
/// works on Cranelift (i64 has no variadic-call issue). Confirms the
/// i64 path is unaffected by the fix.
#[test]
fn cranelift_gcd_unaffected_by_f64_wrapper_fix() {
    let src = "\
mod test
fn main() -> !{IO, Heap} ():
    print_int(gcd(24, 36))
    print(\"|\")
    print_int(gcd(17, 13))
    print(\"|\")
    print_int(gcd(0, 7))
";
    let (out, code) = compile_and_run_cranelift(src);
    assert_eq!(code, 0, "binary exited non-zero; stdout was {:?}", out);
    assert_eq!(out, "12|1|7", "stdout was {:?}", out);
}
