// gradient update — Re-resolve dependencies and update `gradient.lock`
//
// Re-reads `gradient.toml`, resolves the full dependency graph, computes
// fresh checksums, and writes an updated `gradient.lock`.

use crate::project::Project;
use crate::resolver;
use std::process;

/// Execute the `gradient update` subcommand.
pub fn execute() {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    println!("Resolving dependencies for '{}'...", project.name);

    let graph = match resolver::resolve_from_manifest(&project.manifest, &project.root) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    if graph.dependencies.is_empty() {
        println!("No dependencies to resolve.");
        // Remove stale lockfile if present
        let lock_path = project.root.join("gradient.lock");
        if lock_path.exists() {
            let _ = std::fs::remove_file(&lock_path);
        }
        return;
    }

    // Write the lockfile
    if let Err(e) = graph.lockfile.save(&project.root) {
        eprintln!("Error: Failed to write `gradient.lock`: {}", e);
        process::exit(1);
    }

    println!(
        "Updated gradient.lock ({} package{})",
        graph.dependencies.len(),
        if graph.dependencies.len() == 1 {
            ""
        } else {
            "s"
        }
    );

    for dep in &graph.dependencies {
        println!("  {} v{}", dep.name, dep.version);
    }
}
