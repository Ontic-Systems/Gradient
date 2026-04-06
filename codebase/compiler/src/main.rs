//! Gradient compiler driver.
//!
//! This is the main entry point for the Gradient compiler. It supports multiple
//! modes of operation:
//!
//! 1. **Full pipeline** (with arguments):
//!    ```sh
//!    cargo run -- input.gr [output.o]
//!    ```
//!    Runs the complete compilation pipeline:
//!    Source (.gr) -> Lexer -> Parser -> Type Checker -> IR Builder -> Cranelift Codegen -> Object File
//!
//! 2. **REPL** (interactive type-check loop) [experimental]:
//!    ```sh
//!    cargo run -- --repl --experimental
//!    ```
//!    Starts an interactive REPL that type-checks expressions and reports
//!    their inferred types. Useful for exploration and agent scripting.
//!
//! 3. **Formatter** [experimental]:
//!    ```sh
//!    cargo run -- input.gr --fmt --experimental [--write]
//!    ```
//!    Formats Gradient source code.
//!
//! 4. **PoC fallback** (no arguments):
//!    ```sh
//!    cargo run
//!    ```
//!    Emits a hardcoded "Hello from Gradient!" program (backward compatible
//!    with the original proof-of-concept).
//!
//! Use --help for full usage information.

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

/// Prints help/usage information for the Gradient compiler.
fn print_help() {
    println!("Gradient Compiler");
    println!("=================");
    println!();
    println!("USAGE:");
    println!("    gradient [OPTIONS] <input.gr> [output.o]");
    println!();
    println!("COMMANDS (stable):");
    println!("    <input.gr>              Compile a Gradient source file");
    println!("    --check                  Run frontend only, output structured diagnostics");
    println!("    --doc                    Generate API documentation from source");
    println!("    --inspect                Output module contract as JSON");
    println!("    --effects                Output effect analysis as JSON");
    println!("    --complete <line> <col>  Output completion context as JSON");
    println!("    --context --budget <N> --function <name>");
    println!("                             Output context budget as JSON");
    println!("    --agent                  Start persistent JSON-RPC agent mode");
    println!("    --stdin                  Read source from stdin");
    println!();
    println!("COMMANDS [experimental] - requires --experimental flag:");
    println!("    --repl                   Start interactive REPL");
    println!("    --fmt                    Format source file (--write to modify in place)");
    println!("    --target wasm32|wasm64   Compile to WebAssembly target");
    println!();
    println!("OPTIONS:");
    println!("    --experimental           Enable experimental features");
    println!("    --release                Use LLVM backend for optimized release build");
    println!("    --backend <type>         Explicit backend: cranelift, llvm (if enabled), wasm");
    println!("    --json                   Output JSON format where applicable");
    println!("    --pretty                 Pretty-print JSON output");
    println!("    --verify                 Enable SMT contract verification (smt feature)");
    println!("    --index                  Show project structural index (with --inspect)");
    println!();
    println!("BOOTSTRAP TESTING FLAGS:");
    println!("    --parse-only             Stop after parsing");
    println!("    --typecheck-only         Stop after type checking");
    println!("    --emit-ir                Output IR and stop");
    println!();
    println!("EXAMPLES:");
    println!("    gradient hello.gr                    # Compile to hello.o");
    println!("    gradient hello.gr hello.wasm         # Compile to WebAssembly (experimental)");
    println!("    gradient hello.gr --check            # Type-check only");
    println!("    gradient --repl --experimental       # Start REPL (experimental)");
    println!("    gradient fmt hello.gr --experimental # Format file (experimental)");
    println!();
}

/// Check if a feature requires experimental flag and exit if not enabled.
fn require_experimental(experimental: bool, feature_name: &str, help_text: &str) {
    if !experimental {
        eprintln!(
            "Error: '{}' is experimental. Use --experimental to enable.",
            feature_name
        );
        eprintln!();
        eprintln!("Usage: {}", help_text);
        process::exit(1);
    }
}

/// Print warning when using experimental features.
fn warn_experimental(feature_name: &str) {
    eprintln!(
        "Warning: {} is experimental. API may change without notice.",
        feature_name
    );
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Handle help flag early
    if args.len() >= 2 && (args[1] == "--help" || args[1] == "-h") {
        print_help();
        process::exit(0);
    }

    if args.len() < 2 {
        run_poc();
        return;
    }

    let flag_args: Vec<&String> = args[1..].iter().filter(|a| a.starts_with("--")).collect();

    // Check for experimental flag
    let experimental = flag_args.iter().any(|a| a.as_str() == "--experimental");

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
    let agent_mode = flag_args.iter().any(|a| a.as_str() == "--agent");
    let pretty_output = flag_args.iter().any(|a| a.as_str() == "--pretty");

    // Bootstrap testing flags
    let parse_only = flag_args.iter().any(|a| a.as_str() == "--parse-only");
    let typecheck_only = flag_args.iter().any(|a| a.as_str() == "--typecheck-only");
    let emit_ir = flag_args.iter().any(|a| a.as_str() == "--emit-ir");
    let stdin_mode = flag_args.iter().any(|a| a.as_str() == "--stdin");

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

    // --repl: start the interactive REPL [experimental].
    if repl_mode {
        require_experimental(
            experimental,
            "gradient repl",
            "gradient --repl --experimental",
        );
        warn_experimental("gradient repl");
        use std::io::IsTerminal;
        let interactive = std::io::stdin().is_terminal();
        repl::run_repl(interactive);
        return;
    }

    // --agent: start persistent JSON-RPC agent mode on stdin/stdout.
    if agent_mode {
        gradient_compiler::agent::server::run(pretty_output);
        return;
    }

    // --stdin: read source from stdin, write to output file
    if stdin_mode {
        let output_file = positional_args
            .first()
            .map(|s| s.as_str())
            .unwrap_or("output.o");
        return compile_from_stdin(output_file, parse_only, typecheck_only, emit_ir);
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

    // --fmt: format the source file and print to stdout (or write back with --write) [experimental].
    if format_mode {
        require_experimental(
            experimental,
            "gradient fmt",
            "gradient <file.gr> --fmt --experimental [--write]",
        );
        warn_experimental("gradient fmt");
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

    // --parse-only: stop after parsing (for bootstrap testing)
    if parse_only {
        process::exit(0);
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

    // --typecheck-only: stop after type checking (for bootstrap testing)
    if typecheck_only {
        process::exit(0);
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
            let dep_name = use_decl.import_path_string();
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

    // --emit-ir: output IR and stop (for bootstrap testing)
    if emit_ir {
        let ir_text = format!("{:?}", ir_module);
        if output_file.ends_with(".ir") {
            fs::write(output_file, &ir_text).unwrap_or_else(|e| {
                eprintln!("Error writing IR to {}: {}", output_file, e);
                process::exit(1);
            });
            println!("IR output written to: {}", output_file);
        } else {
            println!("{}", ir_text);
        }
        process::exit(0);
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

    // Gate WASM target behind --experimental flag [experimental].
    // Native targets (cranelift, llvm) work without the flag.
    if is_wasm_target {
        require_experimental(
            experimental,
            "WASM target",
            "gradient <file.gr> --target wasm32 --experimental",
        );
        warn_experimental("WASM target");
    }

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

/// Compile source code from stdin instead of a file.
/// Used for bootstrap testing and piping source code.
fn compile_from_stdin(
    output_file: &str,
    parse_only: bool,
    typecheck_only: bool,
    emit_ir: bool,
) {
    use std::io::{self, Read};

    // Read source from stdin
    let mut source = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut source) {
        eprintln!("Error reading from stdin: {}", e);
        process::exit(1);
    }

    // Create a temporary file for the source
    let tmp_file = std::env::temp_dir().join("gradient_stdin_source.gr");
    if let Err(e) = fs::write(&tmp_file, &source) {
        eprintln!("Error writing temp file: {}", e);
        process::exit(1);
    }

    // Compile the temp file
    let input_path = &tmp_file;

    println!("[1/7] Resolving modules from stdin...");
    let resolver = ModuleResolver::new(input_path);
    let resolve_result = resolver.resolve_all(input_path);
    if !resolve_result.errors.is_empty() {
        for err in &resolve_result.errors {
            eprintln!("Resolution error: {}", err);
        }
        let _ = fs::remove_file(&tmp_file);
        process::exit(1);
    }

    // Check for parse errors
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
        let _ = fs::remove_file(&tmp_file);
        process::exit(1);
    }

    // --parse-only
    if parse_only {
        let _ = fs::remove_file(&tmp_file);
        process::exit(0);
    }

    let entry = resolve_result
        .modules
        .get(&resolve_result.entry_module)
        .unwrap();
    let entry_module = &entry.module;
    let imports = Session::build_import_map(entry_module, &resolve_result.modules);

    println!("[2/7] Lexing...");
    println!("[3/7] Parsing...");
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
        let _ = fs::remove_file(&tmp_file);
        process::exit(1);
    }

    // --typecheck-only
    if typecheck_only {
        let _ = fs::remove_file(&tmp_file);
        process::exit(0);
    }

    println!("[5/7] Building IR...");
    let imported_asts: Vec<_> = resolve_result
        .modules
        .iter()
        .filter(|(name, _)| *name != &resolve_result.entry_module)
        .map(|(_, resolved)| (resolved.name.as_str(), &resolved.module))
        .collect();
    let (ir_module, ir_errors) = IrBuilder::build_module_with_imports(entry_module, &imported_asts);
    if !ir_errors.is_empty() {
        for err in &ir_errors {
            eprintln!("IR error: {}", err);
        }
        let _ = fs::remove_file(&tmp_file);
        process::exit(1);
    }

    // --emit-ir
    if emit_ir {
        let ir_text = format!("{:?}", ir_module);
        if output_file.ends_with(".ir") {
            fs::write(output_file, &ir_text).unwrap_or_else(|e| {
                eprintln!("Error writing IR to {}: {}", output_file, e);
                process::exit(1);
            });
            println!("IR output written to: {}", output_file);
        } else {
            println!("{}", ir_text);
        }
        let _ = fs::remove_file(&tmp_file);
        process::exit(0);
    }

    // Generate code
    println!("[6/7] Generating code...");
    let mut backend: Box<dyn CodegenBackend> = Box::new(
        codegen::BackendWrapper::new(false).unwrap_or_else(|e| {
            eprintln!("Codegen init error: {}", e);
            process::exit(1);
        }),
    );

    backend.compile_module(&ir_module).unwrap_or_else(|e| {
        eprintln!("Codegen error: {}", e);
        process::exit(1);
    });

    println!("[7/7] Writing output file...");
    let object_bytes = backend.finish().unwrap_or_else(|e| {
        eprintln!("Output file error: {}", e);
        process::exit(1);
    });

    fs::write(output_file, object_bytes).unwrap_or_else(|e| {
        eprintln!("Error writing {}: {}", output_file, e);
        process::exit(1);
    });

    // Cleanup temp file
    let _ = fs::remove_file(&tmp_file);

    println!("Compiled to: {}", output_file);
}
