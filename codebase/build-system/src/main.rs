mod commands;
#[allow(dead_code)]
mod lockfile;
#[allow(dead_code)]
mod manifest;
#[allow(dead_code)]
mod name_validation;
mod project;
#[allow(dead_code)]
mod registry;
#[allow(dead_code)]
mod resolver;
#[allow(dead_code)]
mod zip_safe;

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

        /// Backend to use for code generation (cranelift, llvm, wasm).
        /// Defaults to cranelift in debug mode and llvm in --release mode.
        /// LLVM requires the compiler to be built with the `llvm` cargo feature.
        #[arg(long, value_name = "BACKEND")]
        backend: Option<String>,

        /// Target triple for cross-compilation (e.g. riscv32-unknown-none-elf,
        /// armv7-unknown-none-eabi, x86_64-unknown-linux-gnu). Forwarded to the
        /// underlying compiler as `--target <TRIPLE>`. Currently honored by the
        /// LLVM backend only; Cranelift always targets the host. (E6 #342)
        #[arg(long, value_name = "TRIPLE")]
        target: Option<String>,
    },

    /// Compile and run the current project
    Run {
        /// Build in release mode with optimizations
        #[arg(long)]
        release: bool,

        /// Backend to use for code generation (cranelift, llvm, wasm).
        /// Defaults to cranelift in debug mode and llvm in --release mode.
        #[arg(long, value_name = "BACKEND")]
        backend: Option<String>,

        /// Target triple for cross-compilation. Note: cross-compiled
        /// binaries are unlikely to execute on the host; `gradient run` is
        /// primarily useful for the host triple. (E6 #342)
        #[arg(long, value_name = "TRIPLE")]
        target: Option<String>,
    },

    /// Run tests for the current project
    Test {
        /// Filter tests by name pattern
        #[arg(long)]
        filter: Option<String>,
    },

    /// Run benchmarks for the current project (E11 #371)
    Bench {
        /// Filter benches by name pattern
        #[arg(long)]
        filter: Option<String>,

        /// Compare results against a baseline JSON file produced by an
        /// earlier `gradient bench --json` run. Exits non-zero if any
        /// bench regresses by more than 10%.
        #[arg(long)]
        baseline: Option<String>,

        /// Emit results as JSON instead of human-readable text. The schema
        /// is stable (schema_version = 1) and intended for CI baselines.
        #[arg(long)]
        json: bool,
    },

    /// Type-check the project without code generation
    Check {
        /// Enable verbose diagnostic output
        #[arg(long, short)]
        verbose: bool,

        /// Output structured JSON diagnostics
        #[arg(long)]
        json: bool,
    },

    /// Generate API documentation from the project's main source
    Doc {
        /// Enable verbose output (prints the underlying compiler invocation)
        #[arg(long, short)]
        verbose: bool,

        /// Output structured JSON documentation
        #[arg(long)]
        json: bool,

        /// Pretty-print JSON output (only meaningful with --json)
        #[arg(long)]
        pretty: bool,

        /// Render a self-contained static HTML site instead of printing
        /// to stdout. Output goes to `target/doc/` by default; override
        /// with `--out-dir <path>`. Includes per-function effect badges,
        /// capability ceiling display, contracts/budget rendering, and a
        /// client-side search box. (E11 #372)
        #[arg(long)]
        html: bool,

        /// Output directory for `--html` mode (default: `target/doc`).
        /// Ignored when `--html` is not set.
        #[arg(long, value_name = "PATH")]
        out_dir: Option<String>,
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

    /// Package, sigstore-sign, and upload the current project
    Publish {
        /// Registry upload target. Launch tier supports file:// paths.
        #[arg(long, value_name = "URL")]
        registry: Option<String>,

        /// Directory for the package artifact, sigstore bundle, and metadata.
        #[arg(long, value_name = "PATH")]
        out_dir: Option<String>,

        /// Build the artifact and metadata without invoking sigstore or uploading.
        #[arg(long)]
        dry_run: bool,

        /// Permit publishing from a dirty working tree.
        #[arg(long)]
        allow_dirty: bool,

        /// Sigstore signer binary. Defaults to `cosign`.
        #[arg(long, value_name = "PATH")]
        cosign: Option<String>,
    },

    /// Re-resolve dependencies and update gradient.lock
    Update,
}

fn main() {
    // Plugin dispatch (E11 #377): if argv[1] is not a built-in
    // subcommand and matches a `gradient-<name>` binary on PATH, exec
    // the plugin with the remaining args. Built-ins always win;
    // unknown invalid tokens (e.g. `--unknown`) fall through to clap
    // for normal error handling.
    let raw_args: Vec<String> = std::env::args().collect();
    if let Some(sub) = raw_args.get(1) {
        if commands::plugin::is_valid_plugin_name(sub) && !commands::plugin::is_builtin(sub) {
            match commands::plugin::dispatch_plugin(sub, &raw_args[2..]) {
                commands::plugin::DispatchOutcome::Ran { exit_code } => {
                    std::process::exit(exit_code);
                }
                commands::plugin::DispatchOutcome::NotFound => {
                    // Fall through — clap will produce its own
                    // unrecognized-subcommand error. We previously
                    // tried to print a tailored message here but that
                    // doubled up with clap's own output for typos.
                }
            }
        }
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Build {
            release,
            verbose,
            file,
            parse_only,
            typecheck_only,
            emit_ir,
            stdin,
            backend,
            target,
        } => {
            if stdin {
                commands::build::execute_stdin(
                    release,
                    verbose,
                    parse_only,
                    typecheck_only,
                    emit_ir,
                    backend.as_deref(),
                    target.as_deref(),
                );
            } else if let Some(file_path) = file {
                commands::build::execute_single_file(
                    &file_path,
                    release,
                    verbose,
                    parse_only,
                    typecheck_only,
                    emit_ir,
                    backend.as_deref(),
                    target.as_deref(),
                );
            } else {
                commands::build::execute(release, verbose, backend.as_deref(), target.as_deref());
            }
        }
        Commands::Run {
            release,
            backend,
            target,
        } => {
            commands::run::execute(release, backend.as_deref(), target.as_deref());
        }
        Commands::Test { filter } => {
            commands::test::execute(filter);
        }
        Commands::Bench {
            filter,
            baseline,
            json,
        } => {
            commands::bench::execute(filter, baseline, json);
        }
        Commands::Check { verbose, json } => {
            commands::check::execute(verbose, json);
        }
        Commands::Doc {
            verbose,
            json,
            pretty,
            html,
            out_dir,
        } => {
            commands::doc::execute(verbose, json, pretty, html, out_dir);
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
        Commands::Publish {
            registry,
            out_dir,
            dry_run,
            allow_dirty,
            cosign,
        } => {
            commands::publish::execute(commands::publish::PublishOptions {
                registry: registry.as_deref(),
                out_dir: out_dir.as_deref(),
                dry_run,
                allow_dirty,
                cosign_bin: cosign.as_deref(),
            });
        }
        Commands::Update => {
            commands::update::execute();
        }
    }
}
