// gradient build — Compile the current project
//
// Future behavior:
// 1. Read the `gradient.toml` manifest from the project root
// 2. Resolve all declared dependencies (local, git, registry)
// 3. Invoke the compiler pipeline:
//    - Lexing:       source text -> token stream
//    - Parsing:      token stream -> AST
//    - Type-checking: AST -> typed AST (with effect inference)
//    - IR lowering:  typed AST -> Gradient IR
//    - Code generation: IR -> target binary (via LLVM or cranelift)
// 4. Output the resulting binary to:
//    - `target/debug/<name>`   in debug mode (default)
//    - `target/release/<name>` in release mode (--release)

/// Execute the `gradient build` subcommand.
pub fn execute(release: bool, verbose: bool) {
    let mode = if release { "release" } else { "debug" };
    println!(
        "gradient build ({} mode, verbose={}) is not yet implemented",
        mode, verbose
    );
}
