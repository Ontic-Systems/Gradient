//! Integration tests for Map data structure (Stream O).
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

#[test]
fn test_map_new_and_insert() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let m: Map[String, Int] = map_new()
    let m2 = map_set(m, "key", 42)
    println("created map")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.contains("created map"));
}

#[test]
fn test_map_contains_existing() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let m: Map[String, Int] = map_new()
    let m2 = map_set(m, "key", 42)
    if map_contains(m2, "key"):
        println("found key")
    else:
        println("not found")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.contains("found key"));
}

#[test]
fn test_map_contains_missing() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let m: Map[String, Int] = map_new()
    let m2 = map_set(m, "key", 42)
    if map_contains(m2, "missing"):
        println("found missing")
    else:
        println("not found")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.contains("not found"));
}

#[test]
fn test_map_size() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let m: Map[String, Int] = map_new()
    print_int(map_size(m))
    println("")
    let m2 = map_set(m, "a", 1)
    print_int(map_size(m2))
    println("")
    let m3 = map_set(m2, "b", 2)
    print_int(map_size(m3))
    println("")
    let m4 = map_remove(m3, "a")
    print_int(map_size(m4))
    println("")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines[0], "0");
    assert_eq!(lines[1], "1");
    assert_eq!(lines[2], "2");
    assert_eq!(lines[3], "1");
}

#[test]
fn test_map_remove() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let m: Map[String, Int] = map_new()
    let m2 = map_set(m, "a", 1)
    let m3 = map_set(m2, "b", 2)
    let m4 = map_remove(m3, "a")
    if map_contains(m4, "a"):
        println("a still exists")
    else:
        println("a removed")
    if map_contains(m4, "b"):
        println("b still exists")
    else:
        println("b missing")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.contains("a removed"));
    assert!(out.contains("b still exists"));
}

#[test]
fn test_map_string_values() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let m: Map[String, String] = map_new()
    let m2 = map_set(m, "name", "gradient")
    if map_contains(m2, "name"):
        println("name key exists")
    else:
        println("name not found")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.contains("name key exists"));
}

#[test]
fn test_map_multiple_inserts() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let m: Map[String, Int] = map_new()
    let m = map_set(m, "one", 1)
    let m = map_set(m, "two", 2)
    let m = map_set(m, "three", 3)
    print_int(map_size(m))
    println("")
    
    // Test overwriting
    let m = map_set(m, "two", 22)
    print_int(map_size(m))
    println("")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines[0], "3");
    assert_eq!(lines[1], "3"); // size unchanged after overwrite
}

#[test]
fn test_map_empty_operations() {
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    let m: Map[String, Int] = map_new()
    if map_contains(m, "anything"):
        println("unexpected found")
    else:
        println("empty map correct")
    print_int(map_size(m))
    println("")
    let m2 = map_remove(m, "nonexistent")
    print_int(map_size(m2))
    println("")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.contains("empty map correct"));
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines[1], "0");
    assert_eq!(lines[2], "0");
}

#[test]
fn test_map_chained_operations() {
    // Test chaining map operations
    let src = r#"
mod test
fn main() -> !{IO, Heap} ():
    // Create a map and chain operations
    let m: Map[String, Int] = map_new()
    let m = map_set(m, "a", 1)
    let m = map_set(m, "b", 2)
    let m = map_set(m, "c", 3)
    
    // Check size
    print_int(map_size(m))
    println("")
    
    // Remove one
    let m = map_remove(m, "b")
    print_int(map_size(m))
    println("")
    
    // Check what's left
    if map_contains(m, "a"):
        println("has a")
    if map_contains(m, "b"):
        println("has b")
    if map_contains(m, "c"):
        println("has c")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.trim().lines().collect();
    assert_eq!(lines[0], "3");
    assert_eq!(lines[1], "2");
    assert!(out.contains("has a"));
    assert!(!out.contains("has b"));
    assert!(out.contains("has c"));
}
