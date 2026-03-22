// gradient build — Compile the current project
//
// Finds the project root, resolves dependencies, invokes the Gradient
// compiler to produce an object file, then links with `cc` to produce
// the final executable binary.

use crate::lockfile::Lockfile;
use crate::project::Project;
use crate::resolver;
use std::fs;
use std::process::{self, Command};

/// Execute the `gradient build` subcommand.
/// Returns the path to the output binary on success, or exits the process on error.
pub fn execute(release: bool, verbose: bool) -> String {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    run_build(&project, release, verbose)
}

/// Perform the build for the given project. Extracted so `gradient run` can
/// reuse this without going through CLI arg parsing again.
///
/// Returns the path to the output binary on success, or exits on error.
pub fn run_build(project: &Project, release: bool, verbose: bool) -> String {
    let compiler = match Project::find_compiler() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    // Resolve dependencies
    let dep_source_files = if !project.manifest.dependencies.is_empty() {
        if verbose {
            println!("  Resolving dependencies...");
        }

        let graph =
            match resolver::resolve_from_manifest(&project.manifest, &project.root) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("Error resolving dependencies: {}", e);
                    process::exit(1);
                }
            };

        // Check if lockfile exists; if not, generate it
        let lock_path = project.root.join("gradient.lock");
        let should_write_lockfile = if lock_path.is_file() {
            // Validate existing lockfile checksums
            match Lockfile::load(&project.root) {
                Ok(existing) => {
                    match existing.validate_checksums(&project.root) {
                        Ok(mismatches) if mismatches.is_empty() => false,
                        Ok(mismatches) => {
                            if verbose {
                                for name in &mismatches {
                                    println!(
                                        "  Dependency '{}' has changed, updating lockfile",
                                        name
                                    );
                                }
                            }
                            true
                        }
                        Err(_) => true,
                    }
                }
                Err(_) => true,
            }
        } else {
            true
        };

        if should_write_lockfile {
            if let Err(e) = graph.lockfile.save(&project.root) {
                eprintln!("Warning: Failed to write gradient.lock: {}", e);
            } else if verbose {
                println!("  Generated gradient.lock");
            }
        }

        if verbose {
            for dep in &graph.dependencies {
                println!(
                    "  Dependency: {} v{} ({} source file{})",
                    dep.name,
                    dep.version,
                    dep.source_files.len(),
                    if dep.source_files.len() == 1 { "" } else { "s" }
                );
            }
        }

        // Collect all dependency source files
        graph
            .dependencies
            .iter()
            .flat_map(|d| d.source_files.iter().cloned())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
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

    let target_dir = project.target_dir(release);
    let object_file = project.output_object(release);
    let binary = project.output_binary(release);

    // Create target directory
    if let Err(e) = fs::create_dir_all(&target_dir) {
        eprintln!(
            "Error: Could not create target directory `{}`: {}",
            target_dir.display(),
            e
        );
        process::exit(1);
    }

    // Stage 1: Invoke the compiler
    if verbose {
        println!(
            "  Compiling: {} {} {}",
            compiler.display(),
            main_source.display(),
            object_file.display()
        );
    }

    let mut cmd = Command::new(&compiler);
    cmd.arg(main_source.to_str().unwrap_or("src/main.gr"))
        .arg(object_file.to_str().unwrap_or("output.o"));

    // Pass dependency source files to the compiler
    for dep_file in &dep_source_files {
        cmd.arg("--dep").arg(dep_file);
    }

    let compile_status = cmd.status();

    match compile_status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "Error: Compiler exited with status {}",
                status.code().unwrap_or(-1)
            );
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

    // Stage 2: Link with cc
    if verbose {
        println!(
            "  Linking: cc {} -o {}",
            object_file.display(),
            binary.display()
        );
    }

    let link_status = Command::new("cc")
        .arg(object_file.to_str().unwrap_or("output.o"))
        .arg("-o")
        .arg(binary.to_str().unwrap_or("output"))
        .status();

    match link_status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "Error: Linker exited with status {}",
                status.code().unwrap_or(-1)
            );
            process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: Failed to invoke linker `cc`: {}", e);
            eprintln!("Make sure a C compiler is installed (gcc, clang, etc.).");
            process::exit(1);
        }
    }

    let profile = if release { "release" } else { "debug" };
    println!(
        "Compiled {} -> target/{}/{}",
        project.name, profile, project.name
    );

    binary.to_string_lossy().to_string()
}
