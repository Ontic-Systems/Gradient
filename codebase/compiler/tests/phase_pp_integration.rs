//! Integration tests for Phase PP: JSON builtins.
//!
//! These tests compile Gradient source through the full pipeline,
//! link with the C runtime, and run the resulting binary.

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
fn test_json_parse_stringify() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let input: String = "{\"name\":\"gradient\",\"version\":1}"
    match json_parse(input):
        Ok(val):
            let output: String = json_stringify(val)
            println(output)
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let trimmed = out.trim();
    assert!(trimmed.contains("\"name\":\"gradient\""));
    assert!(trimmed.contains("\"version\":1"));
}

#[test]
fn test_json_type_and_get() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("{\"x\":42}"):
        Ok(val):
            match json_get(val, "x"):
                Some(xval):
                    println(json_type(xval))
                None:
                    println("not found")
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "int");
}

#[test]
fn test_json_array_roundtrip() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("[1,2,3]"):
        Ok(val):
            println(json_type(val))
            println(json_stringify(val))
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "array");
    assert_eq!(lines[1], "[1,2,3]");
}

#[test]
fn test_json_is_null() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("null"):
        Ok(val):
            print_bool(json_is_null(val))
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "true");
}

#[test]
fn test_json_has_and_keys() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("{\"a\":1,\"b\":2}"):
        Ok(val):
            print_bool(json_has(val, "a"))
            let ks = json_keys(val)
            println(int_to_string(list_length(ks)))
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "true2");
}

#[test]
fn test_json_len_object_and_array() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("[10,20,30]"):
        Ok(arr):
            println(int_to_string(json_len(arr)))
        Err(msg):
            println(msg)
    match json_parse("{\"x\":1,\"y\":2}"):
        Ok(obj):
            println(int_to_string(json_len(obj)))
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "3");
    assert_eq!(lines[1], "2");
}

#[test]
fn test_json_array_get() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("[10,20,30]"):
        Ok(arr):
            match json_array_get(arr, 1):
                Some(v):
                    println(json_type(v))
                None:
                    println("missing")
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "int");
}
