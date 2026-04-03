//! Integration tests for Phase PP: Random Number Generation builtins.
//!
//! These tests verify the 4-layer implementation of random functions.

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;

fn compile_and_run(src: &str) -> (String, i32) {
    let tmp = TempDir::new().expect("failed to create temp dir");

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
    assert!(cc_compile.success(), "runtime compile failed: {:?}", cc_compile);

    let link_status = Command::new("cc")
        .arg(&obj_path)
        .arg(&runtime_obj)
        .arg("-o")
        .arg(&bin_path)
        .arg("-lcurl")
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

#[test]
fn test_random_returns_float() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let r: Float = random()
    println(float_to_string(r))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let val: f64 = out.trim().parse().expect("should parse as float");
    assert!(val >= 0.0 && val < 1.0, "random() should return value in [0.0, 1.0)");
}

#[test]
fn test_random_int_in_range() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let r: Int = random_int(10, 20)
    println(int_to_string(r))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let val: i64 = out.trim().parse().expect("should parse as int");
    assert!(val >= 10 && val <= 20, "random_int(10, 20) should return value in [10, 20]");
}

#[test]
fn test_random_float_returns_float() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let r: Float = random_float()
    println(float_to_string(r))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let val: f64 = out.trim().parse().expect("should parse as float");
    assert!(val >= 0.0 && val < 1.0, "random_float() should return value in [0.0, 1.0)");
}

#[test]
fn test_seed_random_reproducible() {
    // With same seed, we should get reproducible results
    let src = r#"
mod test
fn main() -> !{IO} ():
    seed_random(12345)
    let r1: Float = random()
    println(float_to_string(r1))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let val: f64 = out.trim().parse().expect("should parse as float");
    // Run again with same seed - should produce same value
    let (out2, code2) = compile_and_run(src);
    assert_eq!(code2, 0);
    let val2: f64 = out2.trim().parse().expect("should parse as float");
    assert_eq!(val, val2, "seed_random(12345) should produce reproducible results");
}
