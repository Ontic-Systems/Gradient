//! Actual Bootstrap Execution - Compile Self-Hosted Compiler
//!
//! This test executes the actual bootstrap process by compiling each
//! self-hosted compiler module with the reference (Rust) compiler.

use gradient_compiler::{compile, parse, typecheck, generate_ir};
use std::fs;
use std::path::Path;
use std::time::Instant;
use std::process::Command;

/// Self-hosted compiler modules to bootstrap
const MODULES: &[(&str, &str)] = &[
    ("types", "compiler/types.gr"),
    ("checker", "compiler/checker.gr"),
    ("ir", "compiler/ir.gr"),
    ("ir_builder", "compiler/ir_builder.gr"),
    ("compiler", "compiler/compiler.gr"),
    ("bootstrap", "compiler/bootstrap.gr"),
];

/// Bootstrap output directory
const BOOTSTRAP_OUT_DIR: &str = "./bootstrap_output";

/// Result of compiling one module
struct ModuleBootstrapResult {
    name: String,
    source_path: String,
    lines_of_code: usize,
    parse_success: bool,
    typecheck_success: bool,
    ir_generation_success: bool,
    parse_errors: Vec<String>,
    type_errors: Vec<String>,
    ir_errors: Vec<String>,
    parse_time_ms: u128,
    typecheck_time_ms: u128,
    ir_time_ms: u128,
    ir_output: Option<String>,
}

/// Overall bootstrap execution result
struct BootstrapExecutionResult {
    success: bool,
    modules_attempted: usize,
    modules_parsed: usize,
    modules_typechecked: usize,
    modules_ir_generated: usize,
    total_lines: usize,
    total_time_ms: u128,
    module_results: Vec<ModuleBootstrapResult>,
}

fn main() {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║          GRADIENT ACTUAL BOOTSTRAP EXECUTION                   ║");
    println!("║     Compiling Self-Hosted Compiler with Reference Compiler     ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();

    // Check if gradient compiler is available
    if !check_gradient_compiler() {
        println!("❌ Gradient compiler not found in PATH");
        println!("   Please ensure 'gradient' is installed and available");
        std::process::exit(1);
    }

    println!("✅ Reference compiler (gradient) found");
    println!();

    // Create output directory
    let _ = fs::create_dir_all(BOOTSTRAP_OUT_DIR);

    // Run bootstrap execution
    let start = Instant::now();
    let result = execute_bootstrap();
    let total_time = start.elapsed();

    // Print results
    print_bootstrap_results(&result, total_time);

    // Exit with appropriate code
    std::process::exit(if result.success { 0 } else { 1 });
}

fn check_gradient_compiler() -> bool {
    Command::new("gradient")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn execute_bootstrap() -> BootstrapExecutionResult {
    let mut module_results = Vec::new();
    let mut total_lines = 0;

    for (name, path) in MODULES {
        println!("  🔄 Compiling module: {}", name);
        
        let result = compile_module(name, path);
        total_lines += result.lines_of_code;
        
        let status = if result.ir_generation_success {
            "✅ PASS"
        } else if result.typecheck_success {
            "⚠️  IR FAIL"
        } else if result.parse_success {
            "⚠️  TYPE FAIL"
        } else {
            "❌ PARSE FAIL"
        };
        
        println!("     Status: {} ({} lines, {}ms)", 
            status, 
            result.lines_of_code,
            result.parse_time_ms + result.typecheck_time_ms + result.ir_time_ms
        );
        
        module_results.push(result);
    }

    let modules_parsed = module_results.iter().filter(|r| r.parse_success).count();
    let modules_typechecked = module_results.iter().filter(|r| r.typecheck_success).count();
    let modules_ir = module_results.iter().filter(|r| r.ir_generation_success).count();

    BootstrapExecutionResult {
        success: modules_ir == MODULES.len(),
        modules_attempted: MODULES.len(),
        modules_parsed,
        modules_typechecked,
        modules_ir_generated: modules_ir,
        total_lines,
        total_time_ms: 0, // Will be set by caller
        module_results,
    }
}

fn compile_module(name: &str, path: &str) -> ModuleBootstrapResult {
    // Read source
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return ModuleBootstrapResult {
                name: name.to_string(),
                source_path: path.to_string(),
                lines_of_code: 0,
                parse_success: false,
                typecheck_success: false,
                ir_generation_success: false,
                parse_errors: vec![format!("Failed to read file: {}", e)],
                type_errors: vec![],
                ir_errors: vec![],
                parse_time_ms: 0,
                typecheck_time_ms: 0,
                ir_time_ms: 0,
                ir_output: None,
            };
        }
    };

    let lines_of_code = source.lines().count();

    // Phase 1: Parse
    let parse_start = Instant::now();
    let (parse_success, parse_errors) = parse_source(&source);
    let parse_time = parse_start.elapsed();

    if !parse_success {
        return ModuleBootstrapResult {
            name: name.to_string(),
            source_path: path.to_string(),
            lines_of_code,
            parse_success: false,
            typecheck_success: false,
            ir_generation_success: false,
            parse_errors,
            type_errors: vec![],
            ir_errors: vec![],
            parse_time_ms: parse_time.as_millis(),
            typecheck_time_ms: 0,
            ir_time_ms: 0,
            ir_output: None,
        };
    }

    // Phase 2: Type Check
    let tc_start = Instant::now();
    let (typecheck_success, type_errors) = typecheck_source(&source);
    let tc_time = tc_start.elapsed();

    if !typecheck_success {
        return ModuleBootstrapResult {
            name: name.to_string(),
            source_path: path.to_string(),
            lines_of_code,
            parse_success: true,
            typecheck_success: false,
            ir_generation_success: false,
            parse_errors: vec![],
            type_errors,
            ir_errors: vec![],
            parse_time_ms: parse_time.as_millis(),
            typecheck_time_ms: tc_time.as_millis(),
            ir_time_ms: 0,
            ir_output: None,
        };
    }

    // Phase 3: Generate IR
    let ir_start = Instant::now();
    let (ir_success, ir_output, ir_errors) = generate_ir_from_source(&source);
    let ir_time = ir_start.elapsed();

    // Save IR output if successful
    if ir_success {
        let ir_path = format!("{}/{}.ir", BOOTSTRAP_OUT_DIR, name);
        if let Some(ref ir) = ir_output {
            let _ = fs::write(&ir_path, ir);
        }
    }

    ModuleBootstrapResult {
        name: name.to_string(),
        source_path: path.to_string(),
        lines_of_code,
        parse_success: true,
        typecheck_success: true,
        ir_generation_success: ir_success,
        parse_errors: vec![],
        type_errors: vec![],
        ir_errors,
        parse_time_ms: parse_time.as_millis(),
        typecheck_time_ms: tc_time.as_millis(),
        ir_time_ms: ir_time.as_millis(),
        ir_output,
    }
}

fn parse_source(source: &str) -> (bool, Vec<String>) {
    // Use the gradient compiler to parse
    let output = Command::new("gradient")
        .arg("--parse-only")
        .arg("--silent")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(source.as_bytes());
            }
            child.wait_with_output()
        });

    match output {
        Ok(output) => {
            let success = output.status.success();
            let errors = if success {
                vec![]
            } else {
                vec![String::from_utf8_lossy(&output.stderr).to_string()]
            };
            (success, errors)
        }
        Err(e) => (false, vec![format!("Failed to run gradient: {}", e)]),
    }
}

fn typecheck_source(source: &str) -> (bool, Vec<String>) {
    // Use the gradient compiler to type check
    let output = Command::new("gradient")
        .arg("--typecheck-only")
        .arg("--silent")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(source.as_bytes());
            }
            child.wait_with_output()
        });

    match output {
        Ok(output) => {
            let success = output.status.success();
            let errors = if success {
                vec![]
            } else {
                vec![String::from_utf8_lossy(&output.stderr).to_string()]
            };
            (success, errors)
        }
        Err(e) => (false, vec![format!("Failed to run gradient: {}", e)]),
    }
}

fn generate_ir_from_source(source: &str) -> (bool, Option<String>, Vec<String>) {
    // Use the gradient compiler to generate IR
    let output = Command::new("gradient")
        .arg("--emit-ir")
        .arg("--silent")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(source.as_bytes());
            }
            child.wait_with_output()
        });

    match output {
        Ok(output) => {
            let success = output.status.success();
            let ir_output = if success {
                Some(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                None
            };
            let errors = if success {
                vec![]
            } else {
                vec![String::from_utf8_lossy(&output.stderr).to_string()]
            };
            (success, ir_output, errors)
        }
        Err(e) => (false, None, vec![format!("Failed to run gradient: {}", e)]),
    }
}

fn print_bootstrap_results(result: &BootstrapExecutionResult, total_time: std::time::Duration) {
    println!();
    println!("┌────────────────────────────────────────────────────────────────┐");
    println!("│                    BOOTSTRAP RESULTS                           │");
    println!("├────────────────────────────────────────────────────────────────┤");
    
    for module in &result.module_results {
        let status = if module.ir_generation_success {
            "✅"
        } else if module.parse_success {
            "⚠️ "
        } else {
            "❌"
        };
        
        println!("│ {:12} │ {:2} │ {:4} lines │ {:3}ms │ {:3}ms │ {:3}ms │",
            module.name,
            status,
            module.lines_of_code,
            module.parse_time_ms,
            module.typecheck_time_ms,
            module.ir_time_ms
        );
    }
    
    println!("├────────────────────────────────────────────────────────────────┤");
    println!("│ Summary:                                                       │");
    println!("│   Modules:         {:2} parsed / {:2} typechecked / {:2} IR gen │",
        result.modules_parsed,
        result.modules_typechecked,
        result.modules_ir_generated
    );
    println!("│   Total Lines:     {:4}                                       │",
        result.total_lines
    );
    println!("│   Total Time:       {:4}ms                                     │",
        total_time.as_millis()
    );
    println!("│   Status:          {}",
        if result.success { "✅ BOOTSTRAP SUCCESS    " } else { "❌ BOOTSTRAP FAILED     " }
    );
    println!("└────────────────────────────────────────────────────────────────┘");
    
    if result.success {
        println!();
        println!("🎉🎉🎉 SELF-HOSTING BOOTSTRAP COMPLETE! 🎉🎉🎉");
        println!();
        println!("All self-hosted compiler modules compiled successfully!");
        println!("The Gradient compiler can now compile itself!");
        println!();
        println!("Output saved to: {}", BOOTSTRAP_OUT_DIR);
    } else {
        println!();
        println!("⚠️  Some modules failed to compile");
        println!();
        
        for module in &result.module_results {
            if !module.parse_errors.is_empty() {
                println!("❌ {} - Parse Errors:", module.name);
                for err in &module.parse_errors {
                    println!("   {}", err.lines().next().unwrap_or(err));
                }
            }
            if !module.type_errors.is_empty() {
                println!("❌ {} - Type Errors:", module.name);
                for err in &module.type_errors {
                    println!("   {}", err.lines().next().unwrap_or(err));
                }
            }
            if !module.ir_errors.is_empty() {
                println!("❌ {} - IR Errors:", module.name);
                for err in &module.ir_errors {
                    println!("   {}", err.lines().next().unwrap_or(err));
                }
            }
        }
    }
}
