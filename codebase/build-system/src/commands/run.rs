// gradient run — Compile and run the current project
//
// Future behavior:
// 1. Perform a full `gradient build` (debug or release)
// 2. Locate the compiled binary in `target/<profile>/<name>`
// 3. Execute the binary, forwarding stdin/stdout/stderr
// 4. Exit with the child process's exit code

/// Execute the `gradient run` subcommand.
pub fn execute(release: bool) {
    let mode = if release { "release" } else { "debug" };
    println!("gradient run ({} mode) is not yet implemented", mode);
}
