//! File I/O Builtins Integration Tests
//!
//! Tests for the file system operations under the !{FS} effect:
//! - file_read(path: String) -> String
//! - file_write(path: String, content: String) -> Bool
//! - file_exists(path: String) -> Bool
//! - file_delete(path: String) -> Bool
//! - file_append(path: String, content: String) -> Bool

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
        .status()
        .expect("cc link");
    assert!(link_status.success(), "link failed: {:?}", link_status);

    let output = Command::new(&bin_path)
        .current_dir(&tmp)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}

#[test]
fn test_file_write_and_read() {
    let src = r#"
mod test
fn print_result(label: String, ok: Bool) -> !{IO} ():
    if ok:
        println(label ++ "OK")
    else:
        println(label ++ "FAIL")

fn main() -> !{IO, FS} ():
    let path = "/tmp/gradient_test_io.txt"
    let content = "Hello, File I/O!"
    
    // Write to file
    let write_result = file_write(path, content)
    print_result("Write: ", write_result)
    
    // Read from file
    let read_content = file_read(path)
    println("Read: " ++ read_content)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0, "Exit code should be 0, got stdout: {}", out);
    assert!(out.contains("Write: OK"), "Expected write to succeed, got: {}", out);
    assert!(out.contains("Read: Hello, File I/O!"), "Expected correct content, got: {}", out);
}

#[test]
fn test_file_exists() {
    let src = r#"
mod test
fn print_status(label: String, exists: Bool) -> !{IO} ():
    if exists:
        println(label ++ "EXISTS")
    else:
        println(label ++ "NOTFOUND")

fn main() -> !{IO, FS} ():
    let path = "/tmp/gradient_exists_test.txt"
    
    // Ensure file doesn't exist first
    file_delete(path)
    
    // Check before creating
    print_status("Before: ", file_exists(path))
    
    // Create file
    file_write(path, "test content")
    
    // Check after creating
    print_status("After: ", file_exists(path))
    
    // Cleanup
    file_delete(path)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0, "Exit code should be 0, got: {}", out);
    assert!(out.contains("Before: NOTFOUND"), "File should not exist initially, got: {}", out);
    assert!(out.contains("After: EXISTS"), "File should exist after creation, got: {}", out);
}

#[test]
fn test_file_append() {
    let src = r#"
mod test
fn print_result(label: String, ok: Bool) -> !{IO} ():
    if ok:
        println(label ++ "OK")
    else:
        println(label ++ "FAIL")

fn main() -> !{IO, FS} ():
    let path = "/tmp/gradient_append_test.txt"
    
    // Write initial content
    file_write(path, "First")
    
    // Append more content
    print_result("Append: ", file_append(path, "Second"))
    
    // Read and print full content
    println("Content: " ++ file_read(path))
    
    // Cleanup
    file_delete(path)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0, "Exit code should be 0, got: {}", out);
    assert!(out.contains("Append: OK"), "Append should succeed, got: {}", out);
    assert!(out.contains("First"), "Should contain first part, got: {}", out);
    assert!(out.contains("Second"), "Should contain second part, got: {}", out);
}

#[test]
fn test_file_delete() {
    let src = r#"
mod test
fn print_status(label: String, exists: Bool) -> !{IO} ():
    if exists:
        println(label ++ "EXISTS")
    else:
        println(label ++ "NOTFOUND")

fn print_result(label: String, ok: Bool) -> !{IO} ():
    if ok:
        println(label ++ "OK")
    else:
        println(label ++ "FAIL")

fn main() -> !{IO, FS} ():
    let path = "/tmp/gradient_delete_test.txt"
    
    // Create file
    file_write(path, "to be deleted")
    print_status("Before: ", file_exists(path))
    
    // Delete file
    print_result("Delete: ", file_delete(path))
    
    // Check file no longer exists
    print_status("After: ", file_exists(path))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0, "Exit code should be 0, got: {}", out);
    assert!(out.contains("Before: EXISTS"), "File should exist before delete, got: {}", out);
    assert!(out.contains("Delete: OK"), "Delete should succeed, got: {}", out);
    assert!(out.contains("After: NOTFOUND"), "File should not exist after delete, got: {}", out);
}

#[test]
fn test_file_delete_nonexistent() {
    let src = r#"
mod test
fn print_result(label: String, ok: Bool) -> !{IO} ():
    if ok:
        println(label ++ "OK")
    else:
        println(label ++ "FAIL")

fn main() -> !{IO, FS} ():
    let path = "/tmp/nonexistent_gradient_file.txt"
    
    // Ensure file doesn't exist
    file_delete(path)
    
    // Try to delete file that doesn't exist
    print_result("Delete nonexistent: ", file_delete(path))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0, "Exit code should be 0, got: {}", out);
    assert!(out.contains("Delete nonexistent: FAIL"), "Deleting nonexistent file should return false, got: {}", out);
}

#[test]
fn test_file_read_empty_for_nonexistent() {
    let src = r#"
mod test
fn main() -> !{IO, FS} ():
    let path = "/tmp/nonexistent_gradient_read.txt"
    
    // Ensure file doesn't exist
    file_delete(path)
    
    // Try to read file that doesn't exist
    let content = file_read(path)
    print("START[")
    print(content)
    println("]END")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0, "Exit code should be 0, got: {}", out);
    // file_read returns empty string for nonexistent files
    assert!(out.contains("START[]END"), "Should return empty string for nonexistent file, got: {}", out);
}

#[test]
fn test_file_operations_workflow() {
    let src = r#"
mod test
fn print_status(label: String, exists: Bool) -> !{IO} ():
    if exists:
        println(label ++ "EXISTS")
    else:
        println(label ++ "NOTFOUND")

fn print_result(label: String, ok: Bool) -> !{IO} ():
    if ok:
        println(label ++ "OK")
    else:
        println(label ++ "FAIL")

fn main() -> !{IO, FS} ():
    let path = "/tmp/gradient_workflow_test.txt"
    
    // Cleanup any previous run
    file_delete(path)
    
    // Step 1: File shouldn't exist
    print_status("Step1: ", file_exists(path))
    
    // Step 2: Write initial content
    file_write(path, "Line1")
    println("Step2: WRITE")
    
    // Step 3: Append more content
    file_append(path, "Line2")
    println("Step3: APPEND")
    
    // Step 4: Read and verify
    println("Step4: " ++ file_read(path))
    
    // Step 5: Delete file
    print_result("Step5: ", file_delete(path))
    
    // Step 6: Verify deletion
    print_status("Step6: ", file_exists(path))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0, "Exit code should be 0, got: {}", out);
    assert!(out.contains("Step1: NOTFOUND"), "Step 1 failed, got: {}", out);
    assert!(out.contains("Step2: WRITE"), "Step 2 failed, got: {}", out);
    assert!(out.contains("Step3: APPEND"), "Step 3 failed, got: {}", out);
    assert!(out.contains("Line1"), "Should contain Line1, got: {}", out);
    assert!(out.contains("Line2"), "Should contain Line2, got: {}", out);
    assert!(out.contains("Step5: OK"), "Step 5 failed, got: {}", out);
    assert!(out.contains("Step6: NOTFOUND"), "Step 6 failed, got: {}", out);
}
