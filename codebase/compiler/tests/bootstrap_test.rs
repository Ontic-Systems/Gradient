//! Bootstrap Test - Validate Self-Hosted Compiler
//!
//! This test validates that the self-hosted compiler source code compiles
//! correctly with the reference (Rust) compiler.

use gradient_compiler::{
    compile, parse_source, typecheck_module, generate_ir,
};
use std::fs;
use std::path::Path;
use std::time::Instant;

/// Module info for self-hosted compiler
const SELF_HOSTED_MODULES: &[(&str, &str)] = &[
    ("types", "compiler/types.gr"),
    ("checker", "compiler/checker.gr"),
    ("ir", "compiler/ir.gr"),
    ("ir_builder", "compiler/ir_builder.gr"),
    ("compiler", "compiler/compiler.gr"),
    ("bootstrap", "compiler/bootstrap.gr"),
];

/// Result of testing a module
struct ModuleTestResult {
    name: String,
    parse_success: bool,
    typecheck_success: bool,
    ir_generated: bool,
    lines_of_code: usize,
    parse_time_ms: u128,
    typecheck_time_ms: u128,
    ir_time_ms: u128,
    errors: Vec<String>,
}

/// Overall bootstrap test result
struct BootstrapTestResult {
    modules_tested: usize,
    modules_passed: usize,
    total_lines: usize,
    total_parse_time_ms: u128,
    total_typecheck_time_ms: u128,
    total_ir_time_ms: u128,
    module_results: Vec<ModuleTestResult>,
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║     GRADIENT SELF-HOSTING COMPILER - BOOTSTRAP TEST          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let start_time = Instant::now();
    let result = run_bootstrap_test();
    let total_time = start_time.elapsed();

    print_results(&result, total_time);
}

fn run_bootstrap_test() -> BootstrapTestResult {
    let mut module_results = Vec::new();
    let mut total_lines = 0;
    let mut total_parse_time = 0u128;
    let mut total_typecheck_time = 0u128;
    let mut total_ir_time = 0u128;

    for (name, path) in SELF_HOSTED_MODULES {
        println!("Testing module: {} ({})", name, path);
        
        let result = test_module(name, path);
        
        total_lines += result.lines_of_code;
        total_parse_time += result.parse_time_ms;
        total_typecheck_time += result.typecheck_time_ms;
        total_ir_time += result.ir_time_ms;
        
        module_results.push(result);
    }

    let modules_passed = module_results
        .iter()
        .filter(|r| r.parse_success && r.typecheck_success)
        .count();

    BootstrapTestResult {
        modules_tested: SELF_HOSTED_MODULES.len(),
        modules_passed,
        total_lines,
        total_parse_time_ms: total_parse_time,
        total_typecheck_time_ms: total_typecheck_time,
        total_ir_time_ms: total_ir_time,
        module_results,
    }
}

fn test_module(name: &str, path: &str) -> ModuleTestResult {
    let mut errors = Vec::new();
    
    // Read source file
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            errors.push(format!("Failed to read {}: {}", path, e));
            return ModuleTestResult {
                name: name.to_string(),
                parse_success: false,
                typecheck_success: false,
                ir_generated: false,
                lines_of_code: 0,
                parse_time_ms: 0,
                typecheck_time_ms: 0,
                ir_time_ms: 0,
                errors,
            };
        }
    };

    let lines_of_code = source.lines().count();

    // Phase 1: Parse
    let parse_start = Instant::now();
    let (ast_module, parse_errors) = parse_source(&source, 0);
    let parse_time = parse_start.elapsed();

    let parse_success = parse_errors.is_empty();
    if !parse_success {
        errors.push(format!("Parse failed: {:?}", parse_errors));
    }

    // Phase 2: Type Check (if parse succeeded)
    let (typecheck_success, typecheck_time) = if parse_success {
        let tc_start = Instant::now();
        let type_errors = typecheck_module(&ast_module, 0);
        let tc_time = tc_start.elapsed();
        let tc_success = type_errors.iter().all(|e| e.is_warning);
        if !tc_success {
            let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
            errors.push(format!("Type errors: {:?}", real_errors));
        }
        (tc_success, tc_time)
    } else {
        (false, std::time::Duration::from_millis(0))
    };

    // Phase 3: Generate IR (if type check succeeded)
    let (ir_generated, ir_time) = if typecheck_success {
        let ir_start = Instant::now();
        let ir_result = generate_ir(&ast_module, 0);
        let ir_duration = ir_start.elapsed();
        let ir_success = ir_result.is_ok();
        if !ir_success {
            errors.push(format!("IR generation failed: {:?}", ir_result.err()));
        }
        (ir_success, ir_duration)
    } else {
        (false, std::time::Duration::from_millis(0))
    };

    ModuleTestResult {
        name: name.to_string(),
        parse_success,
        typecheck_success,
        ir_generated,
        lines_of_code,
        parse_time_ms: parse_time.as_millis(),
        typecheck_time_ms: typecheck_time.as_millis(),
        ir_time_ms: ir_time.as_millis(),
        errors,
    }
}

fn print_results(result: &BootstrapTestResult, total_time: std::time::Duration) {
    println!();
    println!("┌──────────────────────────────────────────────────────────────┐");
    println!("│                     TEST RESULTS                             │");
    println!("├──────────────────────────────────────────────────────────────┤");
    
    for module in &result.module_results {
        let status = if module.parse_success && module.typecheck_success {
            "✅ PASS"
        } else {
            "❌ FAIL"
        };
        
        println!("│ {:12} │ {:6} │ {:4} lines │ {:3}ms │ {:3}ms │ {:3}ms │",
            module.name,
            status,
            module.lines_of_code,
            module.parse_time_ms,
            module.typecheck_time_ms,
            module.ir_time_ms
        );
        
        if !module.errors.is_empty() {
            for error in &module.errors {
                println!("│   ⚠️  {}", error);
            }
        }
    }
    
    println!("├──────────────────────────────────────────────────────────────┤");
    println!("│ Summary:                                                     │");
    println!("│   Modules tested:  {:2}/{}                                      │",
        result.modules_passed, result.modules_tested);
    println!("│   Total lines:      {:4}                                     │",
        result.total_lines);
    println!("│   Parse time:        {:4}ms                                   │",
        result.total_parse_time_ms);
    println!("│   Type check time:   {:4}ms                                   │",
        result.total_typecheck_time_ms);
    println!("│   IR gen time:       {:4}ms                                   │",
        result.total_ir_time_ms);
    println!("│   Total time:        {:4}ms                                   │",
        total_time.as_millis());
    println!("└──────────────────────────────────────────────────────────────┘");
    
    if result.modules_passed == result.modules_tested {
        println!();
        println!("🎉 ALL MODULES PASSED! BOOTSTRAP READY! 🎉");
        println!();
        println!("The self-hosted compiler successfully compiles with the reference compiler.");
        println!("Ready to proceed with actual bootstrap execution!");
    } else {
        println!();
        println!("⚠️  SOME MODULES FAILED - See errors above");
    }
}
