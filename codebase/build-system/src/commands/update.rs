// gradient update — Re-resolve dependencies and update `gradient.lock`
//
// Re-reads `gradient.toml`, resolves the full dependency graph, computes
// fresh checksums, and writes an updated `gradient.lock`.
//
// For registry packages, checks if newer versions are available and reports
// update information: "math: 1.2.0 → 1.3.0"

use crate::lockfile::{Lockfile, SourceType};
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

    // Check for registry updates before re-resolving
    check_registry_updates(&project);

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

/// Check for available updates to registry packages.
///
/// Reads the existing lockfile (if present) and checks each registry
/// package to see if a newer version is available.
fn check_registry_updates(project: &Project) {
    // Try to load existing lockfile
    let existing_lockfile = match Lockfile::load(&project.root) {
        Ok(lf) => lf,
        Err(_) => return, // No existing lockfile, nothing to check
    };

    let registry_packages = existing_lockfile.registry_packages();
    if registry_packages.is_empty() {
        return; // No registry packages to check
    }

    let mut updates_available = Vec::new();

    for pkg in registry_packages {
        // Parse the source to get registry info
        if let Ok(SourceType::Registry {
            registry,
            name,
            version: current_version,
        }) = pkg.source_type()
        {
            // Check if newer version is available
            match check_newer_version(&registry, &name, &current_version) {
                Ok(Some(newer_version)) => {
                    updates_available.push((pkg.name.clone(), current_version, newer_version));
                }
                Ok(None) => {
                    // No update available
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Could not check for updates to {}: {}",
                        pkg.name, e
                    );
                }
            }
        }
    }

    // Report available updates
    if !updates_available.is_empty() {
        println!("\nUpdates available:");
        for (name, current, newer) in &updates_available {
            println!("  {}: {} → {}", name, current, newer);
        }
        println!();
    }
}

/// Check if a newer version of a registry package is available.
///
/// TODO: Integrate with resolver from Workstream 2 for actual registry queries.
/// For now, this is a placeholder that will be replaced with actual registry client calls.
///
/// Returns:
/// - `Ok(Some(version))` if a newer version is available
/// - `Ok(None)` if no newer version is available
/// - `Err(msg)` if the check failed
fn check_newer_version(
    _registry: &str,
    _name: &str,
    _current_version: &str,
) -> Result<Option<String>, String> {
    // Placeholder: This will be replaced with actual registry client integration
    // from Workstream 2.
    //
    // The actual implementation will:
    // 1. Query the registry API (e.g., GitHub releases/tags)
    // 2. Parse the current version
    // 3. Compare with available versions
    // 4. Return the latest version if newer than current

    // For now, simulate no updates available
    // This prevents false positives during development
    Ok(None)
}
