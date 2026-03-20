mod commands;
mod manifest;
mod project;

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
    /// Compile the current project
    Build {
        /// Build in release mode with optimizations
        #[arg(long)]
        release: bool,

        /// Enable verbose output
        #[arg(long, short)]
        verbose: bool,
    },

    /// Compile and run the current project
    Run {
        /// Build in release mode with optimizations
        #[arg(long)]
        release: bool,
    },

    /// [planned] Run tests for the current project
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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build { release, verbose } => {
            commands::build::execute(release, verbose);
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
    }
}
