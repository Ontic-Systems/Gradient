// gradient repl — Start the interactive Gradient REPL
//
// Initializes the compiler pipeline in interactive mode with a welcome banner.
// Reads Gradient source line by line, type-checks and evaluates expressions,
// and maintains session state (bindings, imports) across iterations.
//
// Supports REPL-specific commands (:quit, :type, :help, etc.)

use crate::project::Project;
use std::process::{self, Command};

/// Execute the `gradient repl` subcommand.
pub fn execute() {
    // First, try to find the project context (optional for REPL)
    let _project = Project::find().ok();

    let compiler = match Project::find_compiler() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    // Invoke the compiler with --repl flag.
    // --experimental is required for --repl (the compiler gates it).
    let mut cmd = Command::new(&compiler);
    cmd.arg("--repl");
    cmd.arg("--experimental");

    // If we're in a project context, set the working directory
    // so the REPL can access project modules
    if let Some(ref project) = _project {
        cmd.current_dir(&project.root);
    }

    // The compiler's REPL will detect if stdin is a TTY and adjust behavior:
    // - Interactive mode: show banner, prompts, etc.
    // - Piped mode: read commands without prompts (for scripting)
    let status = cmd.status();

    match status {
        Ok(s) if s.success() => {
            // REPL exited normally
        }
        Ok(s) => {
            // REPL exited with error code
            process::exit(s.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!("Error: Failed to start REPL: {}", e);
            process::exit(1);
        }
    }
}
