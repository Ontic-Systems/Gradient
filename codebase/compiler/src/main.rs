//! Gradient compiler driver.
//!
//! This is the main entry point for the Gradient compiler. It supports two
//! modes of operation:
//!
//! 1. **Full pipeline** (with arguments):
//!    ```sh
//!    cargo run -- input.gr [output.o]
//!    ```
//!    Runs the complete compilation pipeline:
//!    Source (.gr) -> Lexer -> Parser -> Type Checker -> IR Builder -> Cranelift Codegen -> Object File
//!
//! 2. **PoC fallback** (no arguments):
//!    ```sh
//!    cargo run
//!    ```
//!    Emits a hardcoded "Hello from Gradient!" program (backward compatible
//!    with the original proof-of-concept).

use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser::Parser;
use gradient_compiler::typechecker;

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        // No arguments: run the PoC (backward compatibility).
        run_poc();
        return;
    }

    let input_file = &args[1];
    let output_file = args.get(2).map(|s| s.as_str()).unwrap_or("output.o");

    // ====================================================================
    // Step 1: Read source file
    // ====================================================================
    let source = fs::read_to_string(input_file).unwrap_or_else(|e| {
        eprintln!("Error reading '{}': {}", input_file, e);
        process::exit(1);
    });

    // ====================================================================
    // Step 2: Lex
    // ====================================================================
    println!("[1/6] Lexing {}...", input_file);
    let mut lexer = Lexer::new(&source, 0);
    let tokens = lexer.tokenize();

    // ====================================================================
    // Step 3: Parse
    // ====================================================================
    println!("[2/6] Parsing...");
    let (module, parse_errors) = Parser::parse(tokens, 0);
    if !parse_errors.is_empty() {
        for err in &parse_errors {
            eprintln!("Parse error: {}", err);
        }
        process::exit(1);
    }

    // ====================================================================
    // Step 4: Type check
    // ====================================================================
    println!("[3/6] Type checking...");
    let type_errors = typechecker::check_module(&module, 0);
    if !type_errors.is_empty() {
        for err in &type_errors {
            eprintln!("Type error: {}", err);
        }
        process::exit(1);
    }

    // ====================================================================
    // Step 5: Build IR
    // ====================================================================
    println!("[4/6] Building IR...");
    let (ir_module, ir_errors) = IrBuilder::build_module(&module);
    if !ir_errors.is_empty() {
        for err in &ir_errors {
            eprintln!("IR error: {}", err);
        }
        process::exit(1);
    }

    // ====================================================================
    // Step 6: Codegen
    // ====================================================================
    println!("[5/6] Generating code via Cranelift...");
    let mut codegen = CraneliftCodegen::new().unwrap_or_else(|e| {
        eprintln!("Codegen init error: {}", e);
        process::exit(1);
    });

    codegen.compile_module(&ir_module).unwrap_or_else(|e| {
        eprintln!("Codegen error: {}", e);
        process::exit(1);
    });

    // ====================================================================
    // Step 7: Write object file
    // ====================================================================
    println!("[6/6] Writing object file...");
    codegen.finalize(output_file).unwrap_or_else(|e| {
        eprintln!("Object file error: {}", e);
        process::exit(1);
    });

    println!();
    println!("Compiled {} -> {}", input_file, output_file);
    println!("Link with: cc {} -o output", output_file);
}

/// Run the original proof-of-concept: emit a hardcoded "Hello from Gradient!"
/// program without going through the frontend pipeline.
fn run_poc() {
    println!("Gradient Compiler — Proof of Concept");
    println!("====================================");
    println!();

    println!("[1/3] Initializing Cranelift codegen for host target...");
    let mut codegen = match CraneliftCodegen::new() {
        Ok(cg) => cg,
        Err(e) => {
            eprintln!("Error: Failed to initialize codegen: {}", e);
            process::exit(1);
        }
    };

    println!("[2/3] Compiling hardcoded 'Hello from Gradient!' program...");
    if let Err(e) = codegen.emit_hello_world() {
        eprintln!("Error: Failed to compile: {}", e);
        process::exit(1);
    }

    let output_path = "hello.o";
    println!("[3/3] Writing object file...");
    if let Err(e) = codegen.finalize(output_path) {
        eprintln!("Error: Failed to write object file: {}", e);
        process::exit(1);
    }

    println!();
    println!("Success! To produce and run the final executable:");
    println!();
    println!("  cc hello.o -o hello");
    println!("  ./hello");
    println!();
    println!("Expected output: Hello from Gradient!");
}
