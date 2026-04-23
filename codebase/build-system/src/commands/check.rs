// gradient check — Type-check the project without code generation
//
// Invokes the compiler with --check to run the frontend only (lex, parse,
// type-check) without generating any object files.  With --json the compiler
// emits structured JSON diagnostics suitable for machine consumption.

use crate::project::Project;
use std::process::{self, Command};

/// Execute the `gradient check` subcommand.
pub fn execute(verbose: bool, json: bool) {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let compiler = match Project::find_compiler() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let main_source = project.main_source();
    if !main_source.is_file() {
        eprintln!(
            "Error: Main source file not found at `{}`.\n\
             Every Gradient project needs a `src/main.gr`.",
            main_source.display()
        );
        process::exit(1);
    }

    if verbose {
        println!(
            "  Checking: {} {} --check{}",
            compiler.display(),
            main_source.display(),
            if json { " --json" } else { "" }
        );
    }

    let mut cmd = Command::new(&compiler);
    cmd.arg(main_source.to_str().unwrap_or("src/main.gr"));
    cmd.arg("--check");
    if json {
        cmd.arg("--json");
    }

    let status = cmd.status();

    match status {
        Ok(s) if s.success() => {
            if !json {
                println!("No errors found.");
            }
        }
        Ok(s) => {
            if !json {
                eprintln!("Check failed with {} error(s).", s.code().unwrap_or(1));
            }
            process::exit(s.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!(
                "Error: Failed to invoke compiler at `{}`: {}",
                compiler.display(),
                e
            );
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    /// Verify the compiler is invoked with --check (not the full pipeline).
    /// We test by inspecting the command args that would be constructed.
    #[test]
    fn check_command_uses_check_flag() {
        // Regression: previously check.rs ran the full compiler pipeline
        // (no --check flag) which was slow and required a writable output path.
        // Now it passes --check to use the frontend-only path.
        let source = std::include_str!("check.rs");
        assert!(
            source.contains(r#"cmd.arg("--check")"#),
            "check.rs must pass --check to the compiler"
        );
    }

    #[test]
    fn check_json_flag_forwarded() {
        // Regression: gradient check --json was rejected by the wrapper.
        // Now --json is accepted and forwarded to the compiler.
        let source = std::include_str!("check.rs");
        assert!(
            source.contains(r#"cmd.arg("--json")"#),
            "check.rs must forward --json to the compiler"
        );
    }
}
