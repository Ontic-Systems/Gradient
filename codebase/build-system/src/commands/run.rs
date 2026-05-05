// gradient run — Compile and run the current project
//
// Performs a full build, then executes the resulting binary. The child
// process's exit code is forwarded as the CLI's own exit code.

use crate::commands::build;
use crate::project::Project;
use std::process::{self, Command};

/// Execute the `gradient run` subcommand.
pub fn execute(release: bool, backend: Option<&str>) {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    // Build first (non-verbose for `run` — the user wants to see the program output)
    let binary_path = build::run_build(&project, release, false, backend);

    // Execute the binary
    let status = Command::new(&binary_path).status();

    match status {
        Ok(s) => {
            process::exit(s.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!("Error: Failed to execute `{}`: {}", binary_path, e);
            process::exit(1);
        }
    }
}
