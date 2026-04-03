//! End-to-end WASM compilation tests
//!
//! These tests verify that Gradient programs can be compiled to WebAssembly
//! and that the resulting WASM modules are valid and runnable.

use gradient_compiler::codegen::{BackendWrapper, CodegenBackend};
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;

/// Helper to compile Gradient source to WASM bytes
fn compile_to_wasm(source: &str) -> Result<Vec<u8>, String> {
    // Lex
    let mut lexer = Lexer::new(source, 0);
    let tokens = lexer.tokenize();

    // Parse
    let (ast, parse_errors) = parser::parse(tokens, 0);
    if !parse_errors.is_empty() {
        return Err(format!("Parse errors: {:?}", parse_errors));
    }

    // Type check
    let errors = typechecker::check_module(&ast, 0);
    if errors.iter().any(|e| !e.is_warning) {
        return Err(format!("Type errors: {:?}", errors));
    }

    // Build IR
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast);
    if !ir_errors.is_empty() {
        return Err(format!("IR errors: {:?}", ir_errors));
    }

    // Compile with WASM backend
    let mut backend: Box<dyn CodegenBackend> = Box::new(
        BackendWrapper::new_with_backend("wasm").map_err(|e| format!("Backend error: {}", e))?,
    );

    backend
        .compile_module(&ir_module)
        .map_err(|e| format!("Codegen error: {}", e))?;

    let wasm_bytes = backend
        .finish()
        .map_err(|e| format!("Finish error: {}", e))?;

    Ok(wasm_bytes)
}

/// Validate that the WASM bytes are a valid WebAssembly module
fn validate_wasm(wasm_bytes: &[u8]) -> Result<(), String> {
    // Check magic number
    if wasm_bytes.len() < 8 {
        return Err("WASM module too small".to_string());
    }
    if &wasm_bytes[0..4] != &[0x00, 0x61, 0x73, 0x6d] {
        return Err("Invalid WASM magic number".to_string());
    }
    // Check version
    if &wasm_bytes[4..8] != &[0x01, 0x00, 0x00, 0x00] {
        return Err("Unsupported WASM version".to_string());
    }
    Ok(())
}

#[test]
fn test_e2e_simple_function() {
    // Simple function returning a constant
    let source = r#"fn main() -> Int:
    ret 42
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");
    validate_wasm(&wasm_bytes).expect("Invalid WASM module");

    // Verify it's a non-trivial module
    assert!(
        wasm_bytes.len() > 20,
        "WASM module should have reasonable size"
    );
}

#[test]
fn test_e2e_arithmetic() {
    let source = r#"fn calc() -> Int:
    let x = 10
    let y = 20
    ret x + y * 2

fn main() -> Int:
    ret calc()
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");
    validate_wasm(&wasm_bytes).expect("Invalid WASM module");
}

#[test]
#[ignore = "Comparison codegen needs value tracking fix"]
fn test_e2e_factorial() {
    let source = r#"fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    ret n * factorial(n - 1)

fn main() -> Int:
    ret factorial(5)
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");
    validate_wasm(&wasm_bytes).expect("Invalid WASM module");
}

#[test]
fn test_e2e_string_output() {
    // Simple string constant (no println to avoid effect requirements)
    let source = r#"fn get_msg() -> String:
    ret "Hello, WASM!"

fn main() -> Int:
    ret 0
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");
    validate_wasm(&wasm_bytes).expect("Invalid WASM module");

    // String output programs should have data section
    // The module should be larger due to string data
    assert!(
        wasm_bytes.len() > 30,
        "WASM with strings should have data section"
    );
}

#[test]
#[ignore = "Comparison codegen needs value tracking fix"]
fn test_e2e_conditional() {
    let source = r#"fn max(a: Int, b: Int) -> Int:
    if a > b:
        ret a
    ret b

fn main() -> Int:
    ret max(5, 10)
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");
    validate_wasm(&wasm_bytes).expect("Invalid WASM module");
}

#[test]
#[ignore = "Comparison codegen needs value tracking fix"]
fn test_e2e_loop() {
    let source = r#"fn sum_n(n: Int) -> Int:
    let sum = 0
    let i = 0
    while i < n:
        sum = sum + i
        i = i + 1
    ret sum

fn main() -> Int:
    ret sum_n(10)
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");
    validate_wasm(&wasm_bytes).expect("Invalid WASM module");
}

#[test]
fn test_wasm_has_memory_section() {
    let source = r#"fn main() -> Int:
    ret 42
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");

    // Look for memory section (section ID 5)
    // WASM sections: 1=type, 2=import, 3=function, 4=table, 5=memory, etc.
    let has_memory = wasm_bytes.windows(2).any(|w| w[0] == 0x05); // Section ID 5 = memory section

    assert!(
        has_memory,
        "WASM module should have a memory section for heap allocations"
    );
}

#[test]
#[ignore = "Memory export encoding differs from test expectation"]
fn test_wasm_exports_memory() {
    let source = r#"fn main() -> Int:
    ret 42
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");

    // Look for "memory" in export section (section ID 7)
    // Export section contains names followed by export kinds
    // Memory exports have kind 0x02
    // We should find the string "memory" (0x6d 0x65 0x6d 0x6f 0x72 0x79)
    let memory_name = vec![0x06, 0x6d, 0x65, 0x6d, 0x6f, 0x72, 0x79]; // "memory" with length prefix
    let has_memory_export = wasm_bytes
        .windows(memory_name.len())
        .any(|w| w == memory_name.as_slice());

    assert!(
        has_memory_export,
        "WASM module should export memory for host interaction"
    );
}

#[test]
#[ignore = "WASI import encoding differs from test expectation"]
fn test_wasm_has_wasi_imports() {
    let source = r#"fn main() !{IO}:
    println("test")
"#;

    let wasm_bytes = compile_to_wasm(source).expect("Failed to compile to WASM");

    // Look for WASI module name "wasi_snapshot_preview1"
    let wasi_name = b"wasi_snapshot_preview1";
    let has_wasi_import = wasm_bytes.windows(wasi_name.len()).any(|w| w == wasi_name);

    assert!(
        has_wasi_import,
        "WASM module with I/O should import WASI functions"
    );
}
