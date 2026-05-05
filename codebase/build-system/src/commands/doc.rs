// gradient doc — Generate API documentation from source
//
// Invokes the compiler with --doc to extract documentation comments
// from the project's main source file. With --json the compiler emits a
// structured JSON document; with --pretty the JSON is pretty-printed.
//
// Note: this is the MVP form of `gradient doc` (issue #424). The full
// HTML rendering / search box / effects+capabilities surface tracked by
// #372 is blocked by E2 (effects) and E3 (capabilities) and lives in a
// follow-up.

use crate::project::Project;
use std::process::{self, Command};

/// Execute the `gradient doc` subcommand.
pub fn execute(verbose: bool, json: bool, pretty: bool) {
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
            "  Documenting: {} {} --doc{}{}",
            compiler.display(),
            main_source.display(),
            if json { " --json" } else { "" },
            if pretty { " --pretty" } else { "" }
        );
    }

    let mut cmd = Command::new(&compiler);
    cmd.arg(main_source.to_str().unwrap_or("src/main.gr"));
    cmd.arg("--doc");
    if json {
        cmd.arg("--json");
    }
    if pretty {
        cmd.arg("--pretty");
    }

    let status = cmd.status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!(
                "Documentation generation failed with exit code {}.",
                s.code().unwrap_or(1)
            );
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
    /// Verify the compiler is invoked with --doc.
    #[test]
    fn doc_command_uses_doc_flag() {
        let source = std::include_str!("doc.rs");
        assert!(
            source.contains(r#"cmd.arg("--doc")"#),
            "doc.rs must pass --doc to the compiler"
        );
    }

    /// Verify --json is forwarded when requested.
    #[test]
    fn doc_json_flag_forwarded() {
        let source = std::include_str!("doc.rs");
        assert!(
            source.contains(r#"cmd.arg("--json")"#),
            "doc.rs must forward --json to the compiler when requested"
        );
    }

    /// Verify --pretty is forwarded when requested.
    #[test]
    fn doc_pretty_flag_forwarded() {
        let source = std::include_str!("doc.rs");
        assert!(
            source.contains(r#"cmd.arg("--pretty")"#),
            "doc.rs must forward --pretty to the compiler when requested"
        );
    }
}
