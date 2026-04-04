// gradient fmt — Format Gradient source files
//
// Discovers all `.gr` source files in the project, parses each file into an AST,
// and pretty-prints the AST according to the official Gradient style guide.
//
// In default mode: overwrites the source files with formatted output
// In --check mode: reports which files differ and exits non-zero
//    if any file would be changed (useful for CI)

use crate::project::Project;
use std::process::{self, Command};

/// Execute the `gradient fmt` subcommand.
pub fn execute(check: bool) {
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

    // Find all .gr source files in the project
    let src_dir = project.root.join("src");
    if !src_dir.is_dir() {
        eprintln!("Error: No src directory found at {}", src_dir.display());
        process::exit(1);
    }

    let mut files_to_format = vec![];
    fn collect_gr_files(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect_gr_files(&path, files);
                } else if path.extension().is_some_and(|e| e == "gr") {
                    files.push(path);
                }
            }
        }
    }
    collect_gr_files(&src_dir, &mut files_to_format);

    if files_to_format.is_empty() {
        println!("No .gr source files found to format.");
        return;
    }

    if check {
        println!(
            "Checking formatting for {} file(s)...",
            files_to_format.len()
        );
    } else {
        println!("Formatting {} file(s)...", files_to_format.len());
    }

    let mut any_changes = false;
    let mut any_errors = false;

    for file in &files_to_format {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {}: {}", file.display(), e);
                any_errors = true;
                continue;
            }
        };

        // Invoke compiler with --fmt to get formatted output
        // The compiler expects: gradient-compiler <input> [output] --fmt
        // In --fmt mode, it ignores the output argument and prints to stdout
        let output = Command::new(&compiler)
            .arg(file.to_str().unwrap_or(""))
            .arg("/dev/null") // Dummy output (ignored in --fmt mode)
            .arg("--fmt")
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let formatted = String::from_utf8_lossy(&output.stdout);

                if check {
                    if formatted != source {
                        println!("  [CHECK FAILED] {}", file.display());
                        any_changes = true;
                    } else {
                        println!("  [OK] {}", file.display());
                    }
                } else {
                    // Write back the formatted source
                    if formatted != source {
                        if let Err(e) = std::fs::write(file, formatted.as_bytes()) {
                            eprintln!("Error writing {}: {}", file.display(), e);
                            any_errors = true;
                        } else {
                            println!("  [FORMATTED] {}", file.display());
                            any_changes = true;
                        }
                    } else {
                        println!("  [UNCHANGED] {}", file.display());
                    }
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("Error formatting {}: {}", file.display(), stderr);
                any_errors = true;
            }
            Err(e) => {
                eprintln!("Error invoking compiler for {}: {}", file.display(), e);
                any_errors = true;
            }
        }
    }

    if any_errors {
        process::exit(1);
    }

    if check && any_changes {
        eprintln!("\nFormatting check failed. Run `gradient fmt` to fix.");
        process::exit(1);
    }

    if any_changes {
        println!("\nFormatting complete.");
    } else {
        println!("\nAll files already formatted.");
    }
}
