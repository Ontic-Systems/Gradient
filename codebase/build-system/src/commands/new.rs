// gradient new <name> — Create a new Gradient project
//
// Creates a new directory with the given project name, generates a
// `gradient.toml` manifest, and scaffolds a `src/main.gr` entry point.

use crate::manifest;
use std::fs;
use std::path::Path;
use std::process;

/// The default `main.gr` content for a new project.
const HELLO_WORLD: &str = "\
mod main

fn main() -> !{IO} ():
    print(\"Hello, Gradient!\")
";

/// Execute the `gradient new` subcommand.
pub fn execute(name: &str) {
    let project_dir = Path::new(name);

    // Error if directory already exists
    if project_dir.exists() {
        eprintln!("Error: Destination `{}` already exists.", name);
        process::exit(1);
    }

    // Create project directory
    if let Err(e) = fs::create_dir(project_dir) {
        eprintln!("Error: Could not create directory `{}`: {}", name, e);
        process::exit(1);
    }

    // Create src/ directory
    let src_dir = project_dir.join("src");
    if let Err(e) = fs::create_dir(&src_dir) {
        eprintln!("Error: Could not create `{}/src/`: {}", name, e);
        process::exit(1);
    }

    // Write gradient.toml
    let manifest_content = manifest::create_default(name);
    let manifest_path = project_dir.join("gradient.toml");
    if let Err(e) = fs::write(&manifest_path, &manifest_content) {
        eprintln!("Error: Could not write `{}`: {}", manifest_path.display(), e);
        process::exit(1);
    }

    // Write src/main.gr
    let main_path = src_dir.join("main.gr");
    if let Err(e) = fs::write(&main_path, HELLO_WORLD) {
        eprintln!("Error: Could not write `{}`: {}", main_path.display(), e);
        process::exit(1);
    }

    // Success message
    println!("Created project '{}'", name);
    println!();
    println!("To get started:");
    println!("  cd {}", name);
    println!("  gradient build");
    println!("  gradient run");
}
