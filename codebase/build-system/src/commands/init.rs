// gradient init — Initialize a Gradient project in the current directory
//
// Verifies that no `gradient.toml` exists, infers the project name from
// the directory name, creates the manifest and a default `src/main.gr`.

use crate::manifest;
use std::env;
use std::fs;
use std::process;

/// The default `main.gr` content for a new project.
const HELLO_WORLD: &str = "\
mod main

fn main() -> !{IO} ():
    print(\"Hello, Gradient!\")
";

/// Execute the `gradient init` subcommand.
pub fn execute() {
    let cwd = match env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: Could not determine current directory: {}", e);
            process::exit(1);
        }
    };

    // Check for existing gradient.toml
    let manifest_path = cwd.join("gradient.toml");
    if manifest_path.exists() {
        eprintln!(
            "Error: `gradient.toml` already exists in `{}`.\n\
             This directory is already a Gradient project.",
            cwd.display()
        );
        process::exit(1);
    }

    // Infer project name from the current directory name
    let name = match cwd.file_name() {
        Some(n) => n.to_string_lossy().to_string(),
        None => {
            eprintln!("Error: Could not determine project name from directory path.");
            process::exit(1);
        }
    };

    // Create src/ directory if it doesn't exist
    let src_dir = cwd.join("src");
    if !src_dir.exists() {
        if let Err(e) = fs::create_dir(&src_dir) {
            eprintln!("Error: Could not create `src/` directory: {}", e);
            process::exit(1);
        }
    }

    // Write gradient.toml
    let manifest_content = manifest::create_default(&name);
    if let Err(e) = fs::write(&manifest_path, &manifest_content) {
        eprintln!("Error: Could not write `gradient.toml`: {}", e);
        process::exit(1);
    }

    // Write src/main.gr if it doesn't exist
    let main_path = src_dir.join("main.gr");
    if !main_path.exists() {
        if let Err(e) = fs::write(&main_path, HELLO_WORLD) {
            eprintln!("Error: Could not write `src/main.gr`: {}", e);
            process::exit(1);
        }
    }

    println!("Initialized Gradient project '{}' in `{}`", name, cwd.display());
}
