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
use gradient_compiler::query::Session;
use gradient_compiler::typechecker;

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        run_poc();
        return;
    }

    let flag_args: Vec<&String> = args[1..].iter().filter(|a| a.starts_with("--")).collect();
    let positional_args: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with("--")).collect();

    let check_only = flag_args.iter().any(|a| a.as_str() == "--check");
    let json_output = flag_args.iter().any(|a| a.as_str() == "--json");
    let inspect = flag_args.iter().any(|a| a.as_str() == "--inspect");
    let effects = flag_args.iter().any(|a| a.as_str() == "--effects");

    if positional_args.is_empty() {
        run_poc();
        return;
    }

    let input_file = positional_args[0].as_str();
    let output_file = positional_args.get(1).map(|s| s.as_str()).unwrap_or("output.o");

    let source = fs::read_to_string(input_file).unwrap_or_else(|e| {
        eprintln!("Error reading '{}': {}", input_file, e);
        process::exit(1);
    });

    // --inspect: output the module contract as JSON and exit.
    if inspect {
        let session = Session::from_source(&source);
        let contract = session.module_contract();
        if json_output {
            println!("{}", contract.to_json_pretty());
        } else {
            println!("{}", contract.to_json());
        }
        process::exit(if contract.has_errors { 1 } else { 0 });
    }

    // --effects: output effect analysis as JSON and exit.
    if effects {
        let session = Session::from_source(&source);
        if let Some(summary) = session.effect_summary() {
            let json = if json_output {
                serde_json::to_string_pretty(summary).unwrap()
            } else {
                serde_json::to_string(summary).unwrap()
            };
            println!("{}", json);
        } else {
            eprintln!("Effect analysis unavailable (parse errors).");
            process::exit(1);
        }
        process::exit(0);
    }

    // --check: run frontend only, output structured diagnostics.
    if check_only {
        let session = Session::from_source(&source);
        let result = session.check();
        if json_output {
            println!("{}", result.to_json_pretty());
        } else if result.is_ok() {
            println!("No errors found.");
        } else {
            for diag in &result.diagnostics {
                eprintln!(
                    "{}[{}:{}]: {}",
                    match diag.phase {
                        gradient_compiler::query::Phase::Parser => "parse error",
                        gradient_compiler::query::Phase::Typechecker => "type error",
                        _ => "error",
                    },
                    diag.span.start.line,
                    diag.span.start.col,
                    diag.message
                );
            }
        }
        process::exit(if result.is_ok() { 0 } else { 1 });
    }

    // Full compilation pipeline.
    println!("[1/6] Lexing {}...", input_file);
    let mut lexer = Lexer::new(&source, 0);
    let tokens = lexer.tokenize();

    println!("[2/6] Parsing...");
    let (module, parse_errors) = Parser::parse(tokens, 0);
    if !parse_errors.is_empty() {
        for err in &parse_errors {
            eprintln!("Parse error: {}", err);
        }
        process::exit(1);
    }

    println!("[3/6] Type checking...");
    let type_errors = typechecker::check_module(&module, 0);
    if !type_errors.is_empty() {
        for err in &type_errors {
            eprintln!("Type error: {}", err);
        }
        process::exit(1);
    }

    println!("[4/6] Building IR...");
    let (ir_module, ir_errors) = IrBuilder::build_module(&module);
    if !ir_errors.is_empty() {
        for err in &ir_errors {
            eprintln!("IR error: {}", err);
        }
        process::exit(1);
    }

    println!("[5/6] Generating code via Cranelift...");
    let mut codegen = CraneliftCodegen::new().unwrap_or_else(|e| {
        eprintln!("Codegen init error: {}", e);
        process::exit(1);
    });

    codegen.compile_module(&ir_module).unwrap_or_else(|e| {
        eprintln!("Codegen error: {}", e);
        process::exit(1);
    });

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
