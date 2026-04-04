mod commands;
#[allow(dead_code)]
mod lockfile;
#[allow(dead_code)]
mod manifest;
mod project;
#[allow(dead_code)]
mod registry;
#[allow(dead_code)]
mod resolver;

use clap::{Parser, Subcommand};

/// The Gradient programming language CLI
#[derive(Parser)]
#[command(name = "gradient")]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Compile the current project or a single file
    Build {
        /// Build in release mode with optimizations
        #[arg(long)]
        release: bool,

        /// Enable verbose output
        #[arg(long, short)]
        verbose: bool,

        /// Compile a single file instead of the current project
        #[arg(long, short)]
        file: Option<String>,

        /// Stop after parsing (for bootstrap testing)
        #[arg(long)]
        parse_only: bool,

        /// Stop after type checking (for bootstrap testing)
        #[arg(long)]
        typecheck_only: bool,

        /// Emit IR instead of machine code (for bootstrap testing)
        #[arg(long)]
        emit_ir: bool,

        /// Read source from stdin (for bootstrap testing)
        #[arg(long)]
        stdin: bool,
    },

    /// Compile and run the current project
    Run {
        /// Build in release mode with optimizations
        #[arg(long)]
        release: bool,
    },

    /// Run tests for the current project
    Test {
        /// Filter tests by name pattern
        #[arg(long)]
        filter: Option<String>,
    },

    /// Type-check the project without code generation
    Check {
        /// Enable verbose diagnostic output
        #[arg(long, short)]
        verbose: bool,
    },

    /// [planned] Format Gradient source files
    Fmt {
        /// Check formatting without modifying files
        #[arg(long)]
        check: bool,
    },

    /// Create a new Gradient project
    New {
        /// Name of the project to create
        name: String,
    },

    /// Initialize a Gradient project in the current directory
    Init,

    /// [planned] Start the interactive Gradient REPL
    Repl,

    /// Add a dependency to the current project
    Add {
        /// Package to add (path, git URL, or registry name with optional version)
        /// Examples: ../math, https://github.com/user/repo.git, math@1.2.0
        arg: String,
    },

    /// Download registry dependencies to cache
    Fetch {
        /// Package name to fetch (optional, fetches all if omitted)
        name: Option<String>,
    },

    /// Re-resolve dependencies and update gradient.lock
    Update,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build { release, verbose, file, parse_only, typecheck_only, emit_ir, stdin } => {
            if stdin {
                commands::build::execute_stdin(release, verbose, parse_only, typecheck_only, emit_ir);
            } else if let Some(file_path) = file {
                commands::build::execute_single_file(&file_path, release, verbose, parse_only, typecheck_only, emit_ir);
            } else {
                commands::build::execute(release, verbose);
            }
        }
        Commands::Run { release } => {
            commands::run::execute(release);
        }
        Commands::Test { filter } => {
            commands::test::execute(filter);
        }
        Commands::Check { verbose } => {
            commands::check::execute(verbose);
        }
        Commands::Fmt { check } => {
            commands::fmt::execute(check);
        }
        Commands::New { name } => {
            commands::new::execute(&name);
        }
        Commands::Init => {
            commands::init::execute();
        }
        Commands::Repl => {
            commands::repl::execute();
        }
        Commands::Add { arg } => {
            commands::add::execute(&arg);
        }
        Commands::Fetch { name } => {
            commands::fetch::execute(name.as_deref());
        }
        Commands::Update => {
            commands::update::execute();
        }
    }
}
