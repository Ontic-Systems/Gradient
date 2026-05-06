//! Integration tests for Phase MM: Standard I/O builtins.
//!
//! These tests compile Gradient source through the full pipeline
//! (Lexer → Parser → TypeChecker → IR → Cranelift Codegen → object file),
//! link with `cc`, and run the resulting binary to verify output.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;

/// Full pipeline: compile Gradient source to a binary and return the output.
///
/// Returns `(stdout, exit_code)`.
fn compile_and_run(src: &str, stdin_input: Option<&[u8]>) -> (String, i32) {
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

    // ── 5. Codegen ─────────────────────────────────────────────────────────
    let mut cg = CraneliftCodegen::new().expect("CraneliftCodegen::new");
    cg.compile_module(&ir_module).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");

    // ── 6. Write object file ───────────────────────────────────────────────
    let obj_path = tmp.path().join("out.o");
    let bin_path = tmp.path().join("out");
    fs::write(&obj_path, &obj_bytes).expect("write .o");

    // ── 7. Link (with C runtime) ─────────────────────────────────────────
    // Locate the C runtime relative to the compiler crate root.
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
    let output = if let Some(input_bytes) = stdin_input {
        let mut child = Command::new(&bin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn binary");
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input_bytes)
            .expect("write stdin");
        child.wait_with_output().expect("wait_with_output")
    } else {
        Command::new(&bin_path)
            .stdout(Stdio::piped())
            .output()
            .expect("run binary")
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}

// ---------------------------------------------------------------------------
// parse_int tests
// ---------------------------------------------------------------------------

#[test]
fn test_parse_int_valid() {
    let src = "\
mod test
fn main() -> !{IO} ():
    let n: Int = parse_int(\"123\")
    print_int(n)
";
    let (out, code) = compile_and_run(src, None);
    assert_eq!(code, 0);
    assert_eq!(out, "123");
}

#[test]
fn test_parse_int_negative() {
    let src = "\
mod test
fn main() -> !{IO} ():
    let n: Int = parse_int(\"-42\")
    print_int(n)
";
    let (out, code) = compile_and_run(src, None);
    assert_eq!(code, 0);
    assert_eq!(out, "-42");
}

#[test]
fn test_parse_int_invalid_returns_zero() {
    let src = "\
mod test
fn main() -> !{IO} ():
    let n: Int = parse_int(\"not_a_number\")
    print_int(n)
";
    let (out, code) = compile_and_run(src, None);
    assert_eq!(code, 0);
    assert_eq!(out, "0");
}

// ---------------------------------------------------------------------------
// parse_float tests
// ---------------------------------------------------------------------------

#[test]
fn test_parse_float_valid() {
    // Use float_to_string to avoid the pre-existing print_float variadic-ABI
    // issue when the float value comes from a runtime function call.
    let src = "\
mod test
fn main() -> !{IO} ():
    let f: Float = parse_float(\"3.14\")
    let s: String = float_to_string(f)
    print(s)
";
    let (out, code) = compile_and_run(src, None);
    assert_eq!(code, 0);
    // print() uses printf("%s") without a newline.
    assert_eq!(out, "3.14");
}

#[test]
fn test_parse_float_invalid_returns_zero() {
    let src = "\
mod test
fn main() -> !{IO} ():
    let f: Float = parse_float(\"not_a_number\")
    let s: String = float_to_string(f)
    print(s)
";
    let (out, code) = compile_and_run(src, None);
    assert_eq!(code, 0);
    // print() uses printf("%s") without a newline.
    assert_eq!(out, "0");
}

// ---------------------------------------------------------------------------
// exit tests
// ---------------------------------------------------------------------------

#[test]
fn test_exit_zero_terminates_immediately() {
    let src = "\
mod test
fn main() -> !{IO} ():
    print(\"before\")
    exit(0)
    print(\"after\")
";
    let (out, code) = compile_and_run(src, None);
    // "before" is printed (without newline, print uses printf), then exit(0) is called.
    assert_eq!(out, "before", "should print 'before' then stop");
    assert_eq!(code, 0, "exit(0) should produce exit code 0");
}

#[test]
fn test_exit_nonzero() {
    let src = "\
mod test
fn main() -> !{IO} ():
    exit(42)
";
    let (out, code) = compile_and_run(src, None);
    assert_eq!(out, "");
    assert_eq!(code, 42, "exit(42) should produce exit code 42");
}

// ---------------------------------------------------------------------------
// Multi-field enum variant destructuring tests
// ---------------------------------------------------------------------------

#[test]
fn test_multifield_enum_variant_destructuring() {
    let src = "\
mod multifield_test

type Task = Task(Int, String, Bool)

fn task_id(t: Task) -> Int:
    match t:
        Task(id, title, done):
            id

fn task_title(t: Task) -> String:
    match t:
        Task(id, title, done):
            title

fn task_done(t: Task) -> Bool:
    match t:
        Task(id, title, done):
            done

fn main() -> !{IO} ():
    let t: Task = Task(42, \"hello world\", true)
    print_int(task_id(t))
    print(task_title(t))
    print_bool(task_done(t))
";
    let (out, code) = compile_and_run(src, None);
    assert_eq!(code, 0);
    // All print functions use printf without newlines.
    assert_eq!(out, "42hello worldtrue");
}

#[test]
fn test_multifield_enum_filter_pattern() {
    let src = "\
mod filter_test

type Task = Task(Int, String, Bool)

fn remove_done(tasks: List[Task]) -> !{Heap} List[Task]:
    let mut result: List[Task] = []
    for t in tasks:
        match t:
            Task(id, title, done):
                if done == false:
                    result = list_push(result, t)
    ret result

fn main() -> !{IO, Heap} ():
    let t1: Task = Task(1, \"foo\", false)
    let t2: Task = Task(2, \"bar\", true)
    let t3: Task = Task(3, \"baz\", false)
    let tasks: List[Task] = [t1, t2, t3]
    let active: List[Task] = remove_done(tasks)
    print_int(list_length(active))
";
    let (out, code) = compile_and_run(src, None);
    assert_eq!(code, 0);
    assert_eq!(out, "2");
}
