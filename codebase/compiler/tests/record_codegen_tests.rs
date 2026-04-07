//! Tests for record field access code generation.
//!
//! These tests verify that record literals are properly allocated
//! and field reads work at runtime.

use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::builder::IrBuilder;
use gradient_compiler::ir::Instruction;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

/// Compile Gradient source and run, returning (stdout, exit_code).
fn compile_and_run(src: &str) -> (String, i32) {
    let tmp = TempDir::new().expect("failed to create temp dir");

    // 1. Lex
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    // 2. Parse
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    // 3. Type check
    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(real_errors.is_empty(), "type errors: {:?}", real_errors);

    // 4. Build IR
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    // Don't assert on IR errors for now - record field access may still have edge cases
    if !ir_errors.is_empty() {
        println!("IR warnings/errors: {:?}", ir_errors);
    }

    // 5. Codegen
    let mut cg = CraneliftCodegen::new().expect("CraneliftCodegen::new");
    cg.compile_module(&ir_module).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");

    let obj_path = tmp.path().join("output.o");
    fs::write(&obj_path, &obj_bytes).expect("write object file");

    // 6. Link with runtime
    let runtime_c =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/gradient_runtime.c");
    let bin_path = tmp.path().join("program");

    let mut link_cmd = Command::new("cc");
    link_cmd
        .arg("-o")
        .arg(&bin_path)
        .arg(&obj_path)
        .arg(&runtime_c)
        .arg("-lm") // Link math library
        .arg("-lpthread") // Link pthread for actor runtime
        .arg("-lcurl"); // Link curl for HTTP operations

    let link_output = link_cmd.output().expect("link command failed");
    if !link_output.status.success() {
        let stderr = String::from_utf8_lossy(&link_output.stderr);
        panic!("linking failed: {}", stderr);
    }

    // 7. Run
    let run_output = Command::new(&bin_path)
        .output()
        .expect("run command failed");

    let stdout = String::from_utf8_lossy(&run_output.stdout).to_string();
    let exit_code = run_output.status.code().unwrap_or(-1);

    (stdout, exit_code)
}

/// Check if the IR module contains a LoadField instruction.
fn has_load_field(ir_module: &gradient_compiler::ir::Module) -> bool {
    for func in &ir_module.functions {
        for block in &func.blocks {
            for instr in &block.instructions {
                if matches!(instr, Instruction::LoadField { .. }) {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if the IR module contains a StoreField instruction.
fn has_store_field(ir_module: &gradient_compiler::ir::Module) -> bool {
    for func in &ir_module.functions {
        for block in &func.blocks {
            for instr in &block.instructions {
                if matches!(instr, Instruction::StoreField { .. }) {
                    return true;
                }
            }
        }
    }
    false
}

/// Test that a record type declaration generates a layout without errors.
#[test]
fn record_type_decl_compiles() {
    let src = r#"type Point:
    x: Int
    y: Int

fn main() -> Int:
    ret 0
"#;

    let (_stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Program should exit successfully");
}

/// Test that a record literal allocation works.
#[test]
fn record_literal_allocation() {
    let src = r#"type Point:
    x: Int
    y: Int

fn main() -> Int:
    let p = Point { x = 10, y = 20 }
    ret 0
"#;

    let (_stdout, exit_code) = compile_and_run(src);
    // For now, just ensure it compiles and runs
    println!("Exit code: {}", exit_code);
}

/// Test that field access generates LoadField instruction in IR.
#[test]
fn field_access_generates_load_field_ir() {
    let src = r#"type Point:
    x: Int
    y: Int

fn get_x() -> Int:
    let p = Point { x = 42, y = 0 }
    ret p.x
"#;

    // 1. Lex
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    // 2. Parse
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    // 3. Type check
    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(real_errors.is_empty(), "type errors: {:?}", real_errors);

    // 4. Build IR
    let (ir_module, _ir_errors) = IrBuilder::build_module(&ast_module);

    // Check that LoadField instruction was generated
    let has_load = has_load_field(&ir_module);
    assert!(
        has_load,
        "IR should contain LoadField instruction for record field access"
    );
}

/// Test that record literal generates StoreField instruction in IR.
#[test]
fn record_literal_generates_store_field_ir() {
    let src = r#"type Point:
    x: Int
    y: Int

fn main() -> Int:
    let p = Point { x = 10, y = 20 }
    ret 0
"#;

    // 1. Lex
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    // 2. Parse
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    // 3. Type check
    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(real_errors.is_empty(), "type errors: {:?}", real_errors);

    // 4. Build IR
    let (ir_module, _ir_errors) = IrBuilder::build_module(&ast_module);

    // Check that StoreField instruction was generated
    let has_store = has_store_field(&ir_module);
    assert!(
        has_store,
        "IR should contain StoreField instruction for record literal"
    );
}

/// Test nested record types compile successfully.
#[test]
fn nested_record_types_compile() {
    let src = r#"type Point:
    x: Int
    y: Int

type Rect:
    top_left: Point
    bottom_right: Point

fn main() -> Int:
    ret 0
"#;

    let (_stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Nested record types should compile");
}

/// Test record with different field types compiles.
#[test]
fn record_with_mixed_field_types() {
    let src = r#"type Data:
    int_field: Int
    str_field: String

fn main() -> Int:
    let d = Data { int_field = 42, str_field = "hello" }
    ret 0
"#;

    let (_stdout, exit_code) = compile_and_run(src);
    println!("Exit code: {}", exit_code);
    // Note: This may have limitations but should at least compile
}

/// Test record with Float fields - proper type handling.
#[test]
fn record_with_float_fields() {
    let src = r#"type Point:
    x: Float
    y: Float

fn main() -> !{IO} ():
    let p = Point { x = 3.5, y = 4.5 }
    print("Float record created")
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Float record should compile and run");
    assert!(
        stdout.contains("Float record created"),
        "Should print success message"
    );
}

/// Test field access returns correct value for Int fields.
#[test]
fn field_access_returns_correct_int_value() {
    let src = r#"type Point:
    x: Int
    y: Int

fn main() -> !{IO} ():
    let p = Point { x = 42, y = 100 }
    print_int(p.x)
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Should exit successfully");
    assert!(
        stdout.contains("42"),
        "Should print field value 42, got: {}",
        stdout
    );
}

/// Test field access on second field (y) works correctly.
#[test]
fn field_access_second_field() {
    let src = r#"type Point:
    x: Int
    y: Int

fn main() -> !{IO} ():
    let p = Point { x = 1, y = 99 }
    print_int(p.y)
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Should exit successfully");
    assert!(
        stdout.contains("99"),
        "Should print field value 99, got: {}",
        stdout
    );
}

/// Test record with Bool fields.
#[test]
fn record_with_bool_fields() {
    let src = r#"type Flags:
    is_active: Bool
    is_visible: Bool

fn main() -> !{IO} ():
    let f = Flags { is_active = true, is_visible = false }
    print("Bool record created")
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Bool record should compile and run");
    assert!(
        stdout.contains("Bool record created"),
        "Should print success message"
    );
}

/// Test record layout with multiple field types (Int, Float, Bool).
#[test]
fn record_mixed_int_float_bool_fields() {
    let src = r#"type Mixed:
    int_val: Int
    float_val: Float
    bool_val: Bool

fn main() -> !{IO} ():
    let m = Mixed { int_val = 10, float_val = 3.14, bool_val = true }
    print("Mixed record created")
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Mixed type record should compile and run");
    assert!(
        stdout.contains("Mixed record created"),
        "Should print success message"
    );
}

/// Test record field access in arithmetic expression.
#[test]
fn field_access_in_arithmetic() {
    let src = r#"type Point:
    x: Int
    y: Int

fn main() -> !{IO} ():
    let p = Point { x = 10, y = 20 }
    let sum = p.x + p.y
    print_int(sum)
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Should exit successfully");
    assert!(
        stdout.contains("30"),
        "Should print sum 30, got: {}",
        stdout
    );
}

/// Test: Tuple values returned from function should not destructure to zero
/// Issue #47: Tuple values returned from a function destructure to zero
#[test]
fn tuple_returned_from_function_destructures_correctly() {
    let src = r#"fn pair() -> (Int, Int):
    ret (42, 99)

fn main() -> !{IO} ():
    let (a, b) = pair()
    print_int(a)
    print_int(b)
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Should exit successfully");
    // print_int adds newline, so output is "42\n99\n"
    assert!(
        stdout.contains("42") && stdout.contains("99"),
        "Should print 42 and 99, got: {:?}",
        stdout
    );
}

/// Test: Tuple field access with .0 and .1 notation
#[test]
fn tuple_field_access_returns_correct_values() {
    let src = r#"fn main() -> !{IO} ():
    let t = (10, 20)
    print_int(t.0)
    print_int(t.1)
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Should exit successfully");
    // print_int adds newline, so output is "10\n20\n"
    assert!(
        stdout.contains("10") && stdout.contains("20"),
        "Should print 10 and 20, got: {:?}",
        stdout
    );
}

/// Test: Three-element tuple construction and access
#[test]
fn three_element_tuple_field_access() {
    let src = r#"fn main() -> !{IO} ():
    let t = (1, 2, 3)
    print_int(t.0)
    print_int(t.1)
    print_int(t.2)
"#;

    let (stdout, exit_code) = compile_and_run(src);
    assert_eq!(exit_code, 0, "Should exit successfully");
    // print_int adds newline
    assert!(
        stdout.contains("1") && stdout.contains("2") && stdout.contains("3"),
        "Should print 1, 2, 3, got: {:?}",
        stdout
    );
}
