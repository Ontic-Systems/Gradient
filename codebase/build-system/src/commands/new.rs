// gradient new <name> — Create a new Gradient project
//
// Future behavior:
// 1. Create a new directory with the given project name
// 2. Generate a `gradient.toml` manifest from the built-in template
// 3. Create `src/main.gr` with a "Hello, Gradient!" entry point
// 4. Optionally initialize a git repository
// 5. Print a success message with next-step instructions

/// Execute the `gradient new` subcommand.
pub fn execute(name: &str) {
    println!("gradient new \"{}\" is not yet implemented", name);
}
