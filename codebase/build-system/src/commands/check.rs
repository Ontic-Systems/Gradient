// gradient check — Type-check the project without code generation
//
// Invokes the compiler on the project's source files. If the compiler
// succeeds, reports no errors. If it fails, the compiler's error output
// (printed to stderr) is shown to the user.
//
// In a future version this will use a dedicated `--check` flag to skip
// code generation, but for v0.1 it runs the full compiler pipeline.

use crate::project::Project;
use std::process::{self, Command};

/// Execute the `gradient check` subcommand.
pub fn execute(verbose: bool) {
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
            "  Checking: {} {}",
            compiler.display(),
            main_source.display()
        );
    }

    // For v0.1, we invoke the full compiler. The object file goes to a
    // temporary location so we don't pollute the target directory.
    let tmp_output = std::env::temp_dir().join(format!("gradient_check_{}.o", project.name));

    let status = Command::new(&compiler)
        .arg(main_source.to_str().unwrap_or("src/main.gr"))
        .arg(tmp_output.to_str().unwrap_or("/tmp/gradient_check.o"))
        .status();

    // Clean up the temp object file regardless of outcome
    let _ = std::fs::remove_file(&tmp_output);

    match status {
        Ok(s) if s.success() => {
            println!("No errors found.");
        }
        Ok(s) => {
            // Compiler already printed errors to stderr
            eprintln!("Check failed with {} error(s).", s.code().unwrap_or(1));
            process::exit(1);
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
