// gradient add <path> — Add a path dependency to the current project
//
// Reads the target directory's `gradient.toml` to determine the dependency
// name, then adds it to the current project's `[dependencies]` table.

use crate::manifest;
use crate::project::Project;
use std::path::Path;
use std::process;

/// Execute the `gradient add` subcommand.
pub fn execute(dep_path: &str) {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    let dep_dir = Path::new(dep_path);

    // Check the dependency directory exists
    if !dep_dir.exists() {
        eprintln!(
            "Error: Path '{}' does not exist.",
            dep_dir.display()
        );
        process::exit(1);
    }

    // Check for gradient.toml in the dependency
    let dep_manifest_path = dep_dir.join("gradient.toml");
    if !dep_manifest_path.is_file() {
        eprintln!(
            "Error: No `gradient.toml` found at '{}'.\n\
             The path must point to a Gradient project directory.",
            dep_dir.display()
        );
        process::exit(1);
    }

    // Load the dependency manifest to get the package name
    let dep_manifest = match manifest::load(dep_dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "Error: Failed to parse `{}`: {}",
                dep_manifest_path.display(),
                e
            );
            process::exit(1);
        }
    };

    let dep_name = dep_manifest.package.name.clone();

    // Check if already a dependency
    if project.manifest.dependencies.contains_key(&dep_name) {
        eprintln!(
            "Warning: '{}' is already a dependency. Updating path.",
            dep_name
        );
    }

    // Add the dependency to our manifest
    let manifest_path = project.root.join("gradient.toml");
    if let Err(e) = manifest::add_dependency(&manifest_path, &dep_name, dep_path) {
        eprintln!("Error: Failed to update `gradient.toml`: {}", e);
        process::exit(1);
    }

    println!("Added dependency '{}' (path: {})", dep_name, dep_path);
}
