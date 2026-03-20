// gradient check — Type-check the project without code generation
//
// Future behavior:
// 1. Read the `gradient.toml` manifest
// 2. Resolve dependencies
// 3. Run the compiler pipeline up to (and including) type-checking:
//    - Lexing -> Parsing -> Type-checking with effect inference
// 4. Report all type errors and warnings
// 5. Skip IR lowering and code generation entirely
// 6. Exit with non-zero status if any errors were found

/// Execute the `gradient check` subcommand.
pub fn execute(verbose: bool) {
    println!(
        "gradient check (verbose={}) is not yet implemented",
        verbose
    );
}
