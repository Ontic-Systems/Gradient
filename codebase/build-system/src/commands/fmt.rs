// gradient fmt — Format Gradient source files
//
// Future behavior:
// 1. Discover all `.gr` source files in the project
// 2. Parse each file into an AST
// 3. Pretty-print the AST according to the official Gradient style guide
// 4. In default mode: overwrite the source files with formatted output
// 5. In --check mode: report which files differ and exit non-zero
//    if any file would be changed (useful for CI)

/// Execute the `gradient fmt` subcommand.
pub fn execute(check: bool) {
    if check {
        println!("gradient fmt --check is not yet implemented");
    } else {
        println!("gradient fmt is not yet implemented");
    }
}
