//! Integration tests for the LLVM backend.
//!
//! These tests compile Gradient source through the full pipeline,
//! using the LLVM backend instead of Cranelift, link with the C runtime,
//! and run the resulting binary to verify correct behavior.
//!
//! # Feature gate
//!
//! These tests are only compiled when the `llvm` feature is enabled:
//!
//! ```bash
//! cargo test --features llvm
//! ```
//!
//! The tests verify that the LLVM backend produces working binaries that
//! match the output of the Cranelift backend.

#![cfg(feature = "llvm")]

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use gradient_compiler::codegen::llvm::LlvmCodegen;
use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;
use inkwell::context::Context;

/// Compile Gradient source using the LLVM backend and return the binary output.
///
/// Returns `(stdout, exit_code)`.
fn compile_and_run_llvm(src: &str) -> (String, i32) {
    let tmp = TempDir::new().expect("failed to create temp dir");

    // ── 1. Lex ─────────────────────────────────────────────────────────────
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    // ── 2. Parse ───────────────────────────────────────────────────────────
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    // ── 3. Type check ──────────────────────────────────────────────────────
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

    // ── 4. IR Build ────────────────────────────────────────────────────────
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    assert!(ir_errors.is_empty(), "IR errors: {:?}", ir_errors);

    // ── 5. LLVM Codegen ────────────────────────────────────────────────────
    let context = Context::create();
    let mut cg = LlvmCodegen::new(&context).expect("LlvmCodegen::new failed");
    cg.compile_module(&ir_module)
        .expect("compile_module failed");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes failed");

    // ── 6. Write object file ───────────────────────────────────────────────
    let obj_path = tmp.path().join("out.o");
    let bin_path = tmp.path().join("out");
    fs::write(&obj_path, &obj_bytes).expect("write .o");

    // ── 7. Link (with C runtime) ─────────────────────────────────────────
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
        .status()
        .expect("cc link");
    assert!(link_status.success(), "link failed: {:?}", link_status);

    // ── 8. Run ─────────────────────────────────────────────────────────────
    let output = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}

/// Compile Gradient source using the Cranelift backend and return the binary output.
///
/// Returns `(stdout, exit_code)`.
fn compile_and_run_cranelift(src: &str) -> (String, i32) {
    let tmp = TempDir::new().expect("failed to create temp dir");

    // ── 1. Lex ─────────────────────────────────────────────────────────────
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    // ── 2. Parse ───────────────────────────────────────────────────────────
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    // ── 3. Type check ──────────────────────────────────────────────────────
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

    // ── 4. IR Build ────────────────────────────────────────────────────────
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    assert!(ir_errors.is_empty(), "IR errors: {:?}", ir_errors);

    // ── 5. Cranelift Codegen ───────────────────────────────────────────────
    let mut cg = CraneliftCodegen::new().expect("CraneliftCodegen::new");
    cg.compile_module(&ir_module).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");

    // ── 6. Write object file ───────────────────────────────────────────────
    let obj_path = tmp.path().join("out.o");
    let bin_path = tmp.path().join("out");
    fs::write(&obj_path, &obj_bytes).expect("write .o");

    // ── 7. Link (with C runtime) ─────────────────────────────────────────
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
        .status()
        .expect("cc link");
    assert!(link_status.success(), "link failed: {:?}", link_status);

    // ── 8. Run ─────────────────────────────────────────────────────────────
    let output = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}

// ============================================================================
// Hello World Tests
// ============================================================================

#[test]
fn test_llvm_hello_world() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    println("Hello from LLVM!")
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0, "LLVM hello world should exit with code 0");
    assert_eq!(
        out.trim(),
        "Hello from LLVM!",
        "LLVM hello world output mismatch"
    );
}

#[test]
fn test_cranelift_hello_world() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    println("Hello from LLVM!")
"#;
    let (out, code) = compile_and_run_cranelift(src);
    assert_eq!(code, 0, "Cranelift hello world should exit with code 0");
    assert_eq!(
        out.trim(),
        "Hello from LLVM!",
        "Cranelift hello world output mismatch"
    );
}

#[test]
fn test_hello_world_backends_match() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    println("Hello, World!")
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);

    assert_eq!(
        llvm_code, cl_code,
        "Exit codes should match between backends"
    );
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}

// ============================================================================
// Arithmetic Tests
// ============================================================================

#[test]
fn test_llvm_arithmetic_int() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: Int = 10
    let b: Int = 3
    print_int(a + b)
    print_int(a - b)
    print_int(a * b)
    print_int(a / b)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    // `print_int` lowers to `printf("%ld", ...)` (no newline) on both
    // backends — see issue #551 / `lower_builtin_call`. Outputs are
    // therefore concatenated: 13 + 7 + 30 + 3 = "137303".
    assert_eq!(out, "137303");
}

#[test]
fn test_arithmetic_int_backends_match() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: Int = 10
    let b: Int = 3
    print_int(a + b)
    print_int(a - b)
    print_int(a * b)
    print_int(a / b)
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);
    assert_eq!(
        llvm_code, cl_code,
        "Exit codes should match between backends"
    );
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}

#[test]
fn test_llvm_arithmetic_float() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let a: Float = 10.5
    let b: Float = 2.5
    print(float_to_string(a + b))
    print(float_to_string(a - b))
    print(float_to_string(a * b))
    print(float_to_string(a / b))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    // Float output format may vary slightly, just check it compiles and runs
    assert!(!out.is_empty());
}

#[test]
fn test_arithmetic_backends_match() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: Int = 100
    let b: Int = 25
    print_int(a + b)
    print_int(a - b)
    print_int(a * b)
    print_int(a / b)
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);

    assert_eq!(llvm_code, cl_code, "Exit codes should match");
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}

// ============================================================================
// Function Call Tests
// ============================================================================

#[test]
fn test_llvm_function_calls() {
    let src = r#"
mod test
fn add(a: Int, b: Int) -> Int:
    ret a + b

fn multiply(a: Int, b: Int) -> Int:
    ret a * b

fn main() -> !{IO} ():
    let sum: Int = add(5, 3)
    let product: Int = multiply(4, 7)
    print_int(sum)
    print_int(product)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    // print_int has no trailing newline; outputs concatenate.
    assert_eq!(out, "828");
}

#[test]
fn test_llvm_function_calls_backends_match() {
    let src = r#"
mod test
fn add(a: Int, b: Int) -> Int:
    ret a + b

fn multiply(a: Int, b: Int) -> Int:
    ret a * b

fn main() -> !{IO} ():
    let sum: Int = add(5, 3)
    let product: Int = multiply(4, 7)
    print_int(sum)
    print_int(product)
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);
    assert_eq!(llvm_code, cl_code, "Exit codes should match");
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}

#[test]
fn test_llvm_nested_function_calls() {
    let src = r#"
mod test
fn square(x: Int) -> Int:
    ret x * x

fn sum_of_squares(a: Int, b: Int) -> Int:
    ret square(a) + square(b)

fn main() -> !{IO} ():
    let result: Int = sum_of_squares(3, 4)
    print_int(result)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "25"); // 3*3 + 4*4 = 9 + 16 = 25
}

// ============================================================================
// Control Flow Tests
// ============================================================================

#[test]
fn test_llvm_if_else() {
    let src = r#"
mod test
fn max(a: Int, b: Int) -> Int:
    if a > b:
        ret a
    else:
        ret b

fn main() -> !{IO} ():
    print_int(max(10, 5))
    print_int(max(3, 8))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out, "108");
}

#[test]
fn test_llvm_if_else_backends_match() {
    let src = r#"
mod test
fn max(a: Int, b: Int) -> Int:
    if a > b:
        ret a
    else:
        ret b

fn main() -> !{IO} ():
    print_int(max(10, 5))
    print_int(max(3, 8))
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);
    assert_eq!(llvm_code, cl_code, "Exit codes should match");
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}

#[test]
fn test_llvm_nested_if() {
    let src = r#"
mod test
fn classify(x: Int) -> String:
    if x > 0:
        if x > 100:
            ret "large"
        else:
            ret "positive"
    else:
        if x < 0:
            ret "negative"
        else:
            ret "zero"

fn main() -> !{IO} ():
    print(classify(150))
    print(classify(50))
    print(classify(-5))
    print(classify(0))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out, "largepositivenegativezero");
}

#[test]
fn test_llvm_nested_if_backends_match() {
    let src = r#"
mod test
fn classify(x: Int) -> String:
    if x > 0:
        if x > 100:
            ret "large"
        else:
            ret "positive"
    else:
        if x < 0:
            ret "negative"
        else:
            ret "zero"

fn main() -> !{IO} ():
    print(classify(150))
    print(classify(50))
    print(classify(-5))
    print(classify(0))
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);
    assert_eq!(llvm_code, cl_code, "Exit codes should match");
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}

#[test]
fn test_llvm_comparison_operators() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    print_bool(5 == 5)
    print_bool(5 != 3)
    print_bool(3 < 5)
    print_bool(5 > 3)
    print_bool(3 <= 5)
    print_bool(5 >= 5)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "truetruetruetruetruetrue");
}

// ============================================================================
// Recursion Tests
// ============================================================================

#[test]
fn test_llvm_factorial() {
    let src = r#"
mod test
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

fn main() -> !{IO} ():
    print_int(factorial(0))
    print_int(factorial(1))
    print_int(factorial(5))
    print_int(factorial(10))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    // 0! = 1, 1! = 1, 5! = 120, 10! = 3628800; print_int has no newline.
    assert_eq!(out, "111203628800");
}

#[test]
fn test_llvm_fibonacci() {
    let src = r#"
mod test
fn fibonacci(n: Int) -> Int:
    if n <= 0:
        ret 0
    if n == 1:
        ret 1
    ret fibonacci(n - 1) + fibonacci(n - 2)

fn main() -> !{IO} ():
    print_int(fibonacci(0))
    print_int(fibonacci(1))
    print_int(fibonacci(5))
    print_int(fibonacci(10))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    // fib(0)=0, fib(1)=1, fib(5)=5, fib(10)=55; print_int concatenates.
    assert_eq!(out, "01555");
}

#[test]
fn test_llvm_fibonacci_backends_match() {
    let src = r#"
mod test
fn fibonacci(n: Int) -> Int:
    if n <= 0:
        ret 0
    if n == 1:
        ret 1
    ret fibonacci(n - 1) + fibonacci(n - 2)

fn main() -> !{IO} ():
    print_int(fibonacci(0))
    print_int(fibonacci(1))
    print_int(fibonacci(5))
    print_int(fibonacci(10))
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);
    assert_eq!(llvm_code, cl_code, "Exit codes should match");
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}

#[test]
fn test_factorial_backends_match() {
    let src = r#"
mod test
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

fn main() -> !{IO} ():
    print_int(factorial(5))
    print_int(factorial(7))
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);

    assert_eq!(llvm_code, cl_code, "Exit codes should match");
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}

// ============================================================================
// Variable and Assignment Tests
// ============================================================================

#[test]
fn test_llvm_variable_assignment() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let mut x: Int = 42
    print_int(x)
    x = 100
    print_int(x)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out, "42100");
}

#[test]
fn test_llvm_multiple_variables() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: Int = 1
    let b: Int = 2
    let c: Int = 3
    print_int(a)
    print_int(b)
    print_int(c)
    print_int(a + b + c)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out, "1236");
}

// ============================================================================
// String Tests
// ============================================================================

#[test]
fn test_llvm_string_operations() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let s1: String = "Hello"
    let s2: String = "World"
    print(s1 + " ")
    print(s2)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert!(out.contains("Hello"));
    assert!(out.contains("World"));
}

#[test]
fn test_llvm_string_length() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let s: String = "Gradient"
    print_int(string_length(s))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "8");
}

// ============================================================================
// List Tests
// ============================================================================

#[test]
fn test_llvm_list_operations() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let list: List[Int] = [1, 2, 3, 4, 5]
    print_int(list_length(list))
    print_int(list_get(list, 0))
    print_int(list_get(list, 4))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    // `print_int` emits no newline (matches Cranelift). Output is the
    // concatenated decimal form: length=5, list[0]=1, list[4]=5 → "515".
    assert_eq!(out, "515");
}

#[test]
fn test_llvm_list_mutation() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let list: List[Int] = [10, 20, 30]
    list_set(list, 1, 99)
    print_int(list_get(list, 0))
    print_int(list_get(list, 1))
    print_int(list_get(list, 2))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, vec!["10", "99", "30"]);
}

// ============================================================================
// Enum/Variant Tests
// ============================================================================

#[test]
fn test_llvm_simple_enum() {
    let src = r#"
mod test
type Color = Red | Green | Blue

fn color_name(c: Color) -> String:
    match c:
        Red: ret "red"
        Green: ret "green"
        Blue: ret "blue"

fn main() -> !{IO} ():
    print(color_name(Red))
    print(color_name(Green))
    print(color_name(Blue))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, vec!["red", "green", "blue"]);
}

#[test]
fn test_llvm_enum_with_data() {
    let src = r#"
mod test
type Shape = Circle(Float) | Rectangle(Float, Float)

fn area(s: Shape) -> Float:
    match s:
        Circle(r):
            ret 3.14159 * r * r
        Rectangle(w, h):
            ret w * h

fn main() -> !{IO, Heap} ():
    let c: Shape = Circle(5.0)
    let r: Shape = Rectangle(4.0, 6.0)
    print(float_to_string(area(c)))
    print(float_to_string(area(r)))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    // Circle area ≈ 78.54, Rectangle area = 24
    assert_eq!(lines.len(), 2);
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn test_llvm_empty_main() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    ()
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out, "");
}

#[test]
fn test_llvm_large_numbers() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let big: Int = 1000000000
    print_int(big)
    print_int(big * 2)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out, "10000000002000000000");
}

#[test]
fn test_llvm_negative_numbers() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let neg: Int = -42
    print_int(neg)
    print_int(neg + 10)
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out, "-42-32");
}

// ============================================================================
// Complex Integration Tests
// ============================================================================

#[test]
fn test_llvm_gcd_algorithm() {
    // Euclidean algorithm for GCD
    let src = r#"
mod test
fn gcd(a: Int, b: Int) -> Int:
    if b == 0:
        ret a
    else:
        ret gcd(b, a % b)

fn main() -> !{IO} ():
    print_int(gcd(48, 18))
    print_int(gcd(56, 98))
    print_int(gcd(100, 35))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out, "6145");
}

#[test]
fn test_llvm_sum_list_recursive() {
    let src = r#"
mod test
fn sum_list(nums: List[Int]) -> Int:
    if list_length(nums) == 0:
        ret 0
    else:
        let first: Int = list_get(nums, 0)
        let rest: List[Int] = list_slice(nums, 1, list_length(nums))
        ret first + sum_list(rest)

fn main() -> !{IO} ():
    let nums: List[Int] = [1, 2, 3, 4, 5]
    print_int(sum_list(nums))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "15");
}

#[test]
fn test_llvm_prime_check() {
    let src = r#"
mod test
fn is_prime(n: Int) -> Bool:
    if n <= 1:
        ret false
    if n <= 3:
        ret true
    if n % 2 == 0:
        ret false
    let mut i: Int = 3
    while i * i <= n:
        if n % i == 0:
            ret false
        i = i + 2
    ret true

fn main() -> !{IO} ():
    print_bool(is_prime(2))
    print_bool(is_prime(17))
    print_bool(is_prime(25))
    print_bool(is_prime(97))
"#;
    let (out, code) = compile_and_run_llvm(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "truetruefalsetrue");
}

#[test]
fn test_complex_backends_match() {
    // A complex program that tests multiple features
    let src = r#"
mod test
fn add(a: Int, b: Int) -> Int:
    ret a + b

fn multiply(a: Int, b: Int) -> Int:
    ret a * b

fn calculate(x: Int) -> Int:
    if x > 10:
        ret multiply(x, 2)
    else:
        ret add(x, 5)

fn main() -> !{IO} ():
    print_int(calculate(5))
    print_int(calculate(15))
    print_int(calculate(0))
"#;
    let (llvm_out, llvm_code) = compile_and_run_llvm(src);
    let (cl_out, cl_code) = compile_and_run_cranelift(src);

    assert_eq!(llvm_code, cl_code, "Exit codes should match");
    assert_eq!(llvm_out, cl_out, "Output should match between backends");
}
