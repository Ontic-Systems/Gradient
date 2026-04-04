//! Gradient compiler driver.
//!
//! This is the main entry point for the Gradient compiler. It supports three
//! modes of operation:
//!
//! 1. **Full pipeline** (with arguments):
//!    ```sh
//!    cargo run -- input.gr [output.o]
//!    ```
//!    Runs the complete compilation pipeline:
//!    Source (.gr) -> Lexer -> Parser -> Type Checker -> IR Builder -> Cranelift Codegen -> Object File
//!
//! 2. **REPL** (interactive type-check loop):
//!    ```sh
//!    cargo run -- --repl
//!    ```
//!    Starts an interactive REPL that type-checks expressions and reports
//!    their inferred types. Useful for exploration and agent scripting.
//!
//! 3. **PoC fallback** (no arguments):
//!    ```sh
//!    cargo run
//!    ```
//!    Emits a hardcoded "Hello from Gradient!" program (backward compatible
//!    with the original proof-of-concept).

use gradient_compiler::codegen::{self, CodegenBackend, CraneliftCodegen};
use gradient_compiler::fmt;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::query::Session;
use gradient_compiler::repl;
use gradient_compiler::resolve::ModuleResolver;
use gradient_compiler::typechecker;

use std::env;
use std::fs;
use std::path::Path;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        run_poc();
        return;
    }

    let flag_args: Vec<&String> = args[1..].iter().filter(|a| a.starts_with("--")).collect();

    // Collect positional args, skipping values that follow --budget and --function flags.
    let positional_args: Vec<&String> = {
        let mut result = Vec::new();
        let mut skip_next = false;
        for arg in &args[1..] {
            if skip_next {
                skip_next = false;
                continue;
            }
            if arg == "--budget" || arg == "--function" {
                skip_next = true;
                continue;
            }
            if !arg.starts_with("--") {
                result.push(arg);
            }
        }
        result
    };

    let check_only = flag_args.iter().any(|a| a.as_str() == "--check");
    let json_output = flag_args.iter().any(|a| a.as_str() == "--json");
    let inspect = flag_args.iter().any(|a| a.as_str() == "--inspect");
    let effects = flag_args.iter().any(|a| a.as_str() == "--effects");
    let format_mode = flag_args.iter().any(|a| a.as_str() == "--fmt");
    let write_back = flag_args.iter().any(|a| a.as_str() == "--write");
    let repl_mode = flag_args.iter().any(|a| a.as_str() == "--repl");
    let complete_mode = flag_args.iter().any(|a| a.as_str() == "--complete");
    let context_mode = flag_args.iter().any(|a| a.as_str() == "--context");
    let index_mode = flag_args.iter().any(|a| a.as_str() == "--index");
    let release_mode = flag_args.iter().any(|a| a.as_str() == "--release");
    let doc_mode = flag_args.iter().any(|a| a.as_str() == "--doc");
    let verify_mode = flag_args.iter().any(|a| a.as_str() == "--verify");

    // Parse --backend <type> for explicit backend selection
    let backend_type: Option<&str> = {
        let mut val = None;
        for i in 1..args.len() - 1 {
            if args[i] == "--backend" {
                val = Some(args[i + 1].as_str());
            }
        }
        val
    };

    // Parse --target <triple> for target architecture selection
    // Supports: wasm32, wasm64, native (default)
    let target_triple: Option<&str> = {
        let mut val = None;
        for i in 1..args.len() - 1 {
            if args[i] == "--target" {
                val = Some(args[i + 1].as_str());
            }
        }
        val
    };

    // Parse --budget N and --function name from the args list.
    let budget_value: Option<usize> = {
        let mut val = None;
        for i in 1..args.len() - 1 {
            if args[i] == "--budget" {
                val = args[i + 1].parse().ok();
            }
        }
        val
    };
    let function_name: Option<&str> = {
        let mut val = None;
        for i in 1..args.len() - 1 {
            if args[i] == "--function" {
                val = Some(args[i + 1].as_str());
            }
        }
        val
    };

    // --repl: start the interactive REPL.
    if repl_mode {
        use std::io::IsTerminal;
        let interactive = std::io::stdin().is_terminal();
        repl::run_repl(interactive);
        return;
    }

    if positional_args.is_empty() {
        run_poc();
        return;
    }

    let input_file = positional_args[0].as_str();
    let output_file = positional_args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("output.o");
    let input_path = Path::new(input_file);

    // --complete <line> <col>: output completion context as JSON and exit.
    if complete_mode {
        let complete_line: u32 = positional_args
            .get(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                eprintln!("Usage: gradient <file> --complete <line> <col> [--json]");
                process::exit(1);
            });
        let complete_col: u32 = positional_args
            .get(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                eprintln!("Usage: gradient <file> --complete <line> <col> [--json]");
                process::exit(1);
            });

        let session = Session::from_file(input_path).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            process::exit(1);
        });
        let ctx = session.completion_context(complete_line, complete_col);
        if json_output {
            println!("{}", ctx.to_json_pretty());
        } else {
            println!("{}", ctx.to_json());
        }
        process::exit(0);
    }

    // --context --budget N --function name: output context budget as JSON and exit.
    if context_mode {
        let fn_name = function_name.unwrap_or_else(|| {
            eprintln!("Usage: gradient <file> --context --budget <N> --function <name> [--json]");
            process::exit(1);
        });
        let budget = budget_value.unwrap_or_else(|| {
            eprintln!("Usage: gradient <file> --context --budget <N> --function <name> [--json]");
            process::exit(1);
        });

        let session = Session::from_file(input_path).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            process::exit(1);
        });
        let result = session.context_budget(fn_name, budget);
        if json_output {
            println!("{}", result.to_json_pretty());
        } else {
            println!("{}", result.to_json());
        }
        process::exit(0);
    }

    // --inspect --index: output project structural index as JSON and exit.
    // --inspect: output the module contract as JSON and exit.
    if inspect {
        let session = Session::from_file(input_path).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            process::exit(1);
        });

        if index_mode {
            let index = session.project_index();
            if json_output {
                println!("{}", index.to_json_pretty());
            } else {
                println!("{}", index.to_json());
            }
            process::exit(0);
        }

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
        let session = Session::from_file(input_path).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            process::exit(1);
        });
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

    // --fmt: format the source file and print to stdout (or write back with --write).
    if format_mode {
        let source = fs::read_to_string(input_path).unwrap_or_else(|e| {
            eprintln!("Error reading {}: {}", input_file, e);
            process::exit(1);
        });
        match fmt::format_source(&source) {
            Ok(formatted) => {
                if write_back {
                    fs::write(input_path, &formatted).unwrap_or_else(|e| {
                        eprintln!("Error writing {}: {}", input_file, e);
                        process::exit(1);
                    });
                    eprintln!("Formatted {}", input_file);
                } else {
                    print!("{}", formatted);
                }
            }
            Err(errors) => {
                for err in &errors {
                    eprintln!("Parse error: {}", err);
                }
                process::exit(1);
            }
        }
        process::exit(0);
    }

    // --doc: generate API documentation from source.
    if doc_mode {
        let session = Session::from_file(input_path).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            process::exit(1);
        });
        if json_output {
            let doc = session.documentation();
            println!("{}", doc.to_json_pretty());
        } else {
            print!("{}", session.documentation_text());
        }
        process::exit(0);
    }

    // --check: run frontend only, output structured diagnostics.
    if check_only {
        let session = Session::from_file(input_path).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            process::exit(1);
        });
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

    // Full compilation pipeline with multi-file support.
    println!("[1/7] Resolving modules for {}...", input_file);
    let resolver = ModuleResolver::new(input_path);
    let resolve_result = resolver.resolve_all(input_path);
    if !resolve_result.errors.is_empty() {
        for err in &resolve_result.errors {
            eprintln!("Resolution error: {}", err);
        }
        process::exit(1);
    }

    // Check for parse errors in any module.
    let mut had_parse_errors = false;
    for (name, resolved) in &resolve_result.modules {
        if !resolved.parse_errors.is_empty() {
            for err in &resolved.parse_errors {
                eprintln!("Parse error in {}: {}", name, err);
            }
            had_parse_errors = true;
        }
    }
    if had_parse_errors {
        process::exit(1);
    }

    let entry = resolve_result
        .modules
        .get(&resolve_result.entry_module)
        .unwrap();
    let entry_module = &entry.module;

    // Build import map for multi-file type checking.
    let imports = Session::build_import_map(entry_module, &resolve_result.modules);

    println!("[2/7] Lexing {}...", input_file);
    // (Already lexed and parsed during resolution; this step is for display.)

    println!("[3/7] Parsing...");
    // (Already parsed during resolution.)

    println!("[4/7] Type checking...");
    let type_errors = if imports.is_empty() {
        typechecker::check_module(entry_module, entry.file_id)
    } else {
        let (errors, _summary) =
            typechecker::check_module_with_imports(entry_module, entry.file_id, &imports);
        errors
    };
    let has_type_errors = type_errors.iter().any(|e| !e.is_warning);
    for err in &type_errors {
        if err.is_warning {
            eprintln!("Warning: {}", err);
        } else {
            eprintln!("Type error: {}", err);
        }
    }
    if has_type_errors {
        process::exit(1);
    }

    // SMT Verification (only with --verify flag and smt feature)
    #[cfg(feature = "smt")]
    if verify_mode {
        use gradient_compiler::ast::item::ItemKind;
        use gradient_compiler::typechecker::smt::{verify_function_contracts, VerificationResult};

        println!("[4.5/7] Verifying contracts...");
        let mut verified_count = 0;
        let mut failed_count = 0;

        for item in &entry_module.items {
            if let ItemKind::FnDef(fn_def) = &item.node {
                if !fn_def.contracts.is_empty() {
                    let results = verify_function_contracts(fn_def);
                    for (idx, result) in results {
                        match result {
                            VerificationResult::Proved => {
                                println!("  ✓ {} contract #{}: Proved", fn_def.name, idx);
                                verified_count += 1;
                            }
                            VerificationResult::CounterExample { bindings } => {
                                eprintln!(
                                    "  ✗ {} contract #{}: Counterexample found",
                                    fn_def.name, idx
                                );
                                if !bindings.is_empty() {
                                    eprintln!("    Bindings: {:?}", bindings);
                                }
                                failed_count += 1;
                            }
                            VerificationResult::Unknown(msg) => {
                                eprintln!(
                                    "  ? {} contract #{}: Unknown ({})",
                                    fn_def.name, idx, msg
                                );
                            }
                            VerificationResult::Error(msg) => {
                                eprintln!("  ! {} contract #{}: Error ({})", fn_def.name, idx, msg);
                            }
                        }
                    }
                }
            }
        }

        if verified_count > 0 || failed_count > 0 {
            println!(
                "Contract verification: {} proved, {} failed",
                verified_count, failed_count
            );
            if failed_count > 0 {
                process::exit(1);
            }
        } else {
            println!("  (no contracts found)");
        }
    }

    #[cfg(not(feature = "smt"))]
    if verify_mode {
        eprintln!("Warning: --verify flag ignored (smt feature not enabled)");
    }

    println!("[5/7] Building IR...");
    // Build the list of imported module ASTs for the IR builder.
    let imported_asts: Vec<(&str, &gradient_compiler::ast::module::Module)> = entry_module
        .uses
        .iter()
        .filter_map(|use_decl| {
            let dep_name = use_decl.path.join(".");
            resolve_result
                .modules
                .get(&dep_name)
                .map(|m| (m.name.as_str(), &m.module))
        })
        .collect();
    let (ir_module, ir_errors) = IrBuilder::build_module_with_imports(entry_module, &imported_asts);
    if !ir_errors.is_empty() {
        for err in &ir_errors {
            eprintln!("IR error: {}", err);
        }
        process::exit(1);
    }

    // Determine effective backend type from:
    // 1. Explicit --backend flag (highest priority)
    // 2. --target flag (maps wasm32/wasm64 -> wasm)
    // 3. Output file extension (.wasm -> wasm)
    let effective_backend_type = backend_type
        .or_else(|| {
            // Check --target flag
            target_triple.and_then(|t| match t {
                "wasm32" | "wasm64" => Some("wasm"),
                _ => None,
            })
        })
        .or_else(|| {
            // Check output file extension
            if output_file.ends_with(".wasm") {
                Some("wasm")
            } else {
                None
            }
        });

    // Check if this is a WASM target (for output message customization)
    let is_wasm_target = effective_backend_type == Some("wasm");

    // Select the backend based on --backend flag, --target flag, file extension, or --release mode.
    // Use the BackendWrapper from codegen module which handles context lifetimes.
    let mut backend: Box<dyn CodegenBackend> = if let Some(bt) = effective_backend_type {
        // Explicit backend selection via --backend, --target, or file extension
        Box::new(
            codegen::BackendWrapper::new_with_backend(bt).unwrap_or_else(|e| {
                let available_backends = format!(
                    "cranelift{}{}",
                    if cfg!(feature = "llvm") { ", llvm" } else { "" },
                    if cfg!(feature = "wasm") { ", wasm" } else { "" }
                );
                eprintln!(
                    "Error: Failed to initialize '{}' backend: {}\nAvailable backends: {}",
                    bt, e, available_backends
                );
                process::exit(1);
            }),
        )
    } else {
        // Default selection based on --release flag
        Box::new(
            codegen::BackendWrapper::new(release_mode).unwrap_or_else(|e| {
                if release_mode && !codegen::llvm_available() {
                    eprintln!(
                        "Error: --release requires the LLVM backend, but this binary was compiled \
                 without it.\nRebuild with: cargo build --features llvm"
                    );
                } else {
                    eprintln!("Codegen init error: {}", e);
                }
                process::exit(1);
            }),
        )
    };

    let backend_name = backend.name().to_string();
    println!("[6/7] Generating code via {}...", backend_name);

    backend.compile_module(&ir_module).unwrap_or_else(|e| {
        eprintln!("Codegen error: {}", e);
        process::exit(1);
    });

    println!("[7/7] Writing output file...");
    let object_bytes = backend.finish().unwrap_or_else(|e| {
        eprintln!("Output file error: {}", e);
        process::exit(1);
    });

    std::fs::write(output_file, &object_bytes).unwrap_or_else(|e| {
        eprintln!("Failed to write output file '{}': {}", output_file, e);
        process::exit(1);
    });

    println!("Wrote: {}", output_file);
    println!();
    println!(
        "Compiled {} -> {} (backend: {})",
        input_file, output_file, backend_name
    );

    // Print target-specific instructions
    if is_wasm_target {
        println!();
        println!("Run with: wasmtime {}", output_file);
        println!("  (Install wasmtime: https://wasmtime.dev/)");
        println!("  (Or run in browser with a WebAssembly runtime)");
    } else {
        println!();
        println!(
            "Link with: cc {} runtime/gradient_runtime.c -o output",
            output_file
        );
        println!("  (gradient_runtime.c provides read_line, file I/O helpers; omit if unused)");
    }
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
