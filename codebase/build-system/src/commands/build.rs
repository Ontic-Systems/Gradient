// gradient build — Compile the current project
//
// Finds the project root, resolves dependencies, invokes the Gradient
// compiler to produce an object file, then links with `cc` to produce
// the final executable binary.

use crate::lockfile::Lockfile;
use crate::project::Project;
use crate::resolver;
use gradient_compiler::ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use gradient_compiler::ast::item::{ContractKind, ItemKind};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// Execute the `gradient build` subcommand.
/// Returns the path to the output binary on success, or exits the process on error.
pub fn execute(release: bool, verbose: bool, backend: Option<&str>) -> String {
    let project = match Project::find() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    run_build(&project, release, verbose, backend)
}

/// Perform the build for the given project. Extracted so `gradient run` can
/// reuse this without going through CLI arg parsing again.
///
/// Returns the path to the output binary on success, or exits on error.
pub fn run_build(project: &Project, release: bool, verbose: bool, backend: Option<&str>) -> String {
    let compiler = match Project::find_compiler() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    // Resolve dependencies
    let dep_source_files = if !project.manifest.dependencies.is_empty() {
        if verbose {
            println!("  Resolving dependencies...");
        }

        let graph = match resolver::resolve_from_manifest(&project.manifest, &project.root) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("Error resolving dependencies: {}", e);
                process::exit(1);
            }
        };

        // Check if lockfile exists; if not, generate it
        let lock_path = project.root.join("gradient.lock");
        let should_write_lockfile = if lock_path.is_file() {
            // Validate existing lockfile checksums
            match Lockfile::load(&project.root) {
                Ok(existing) => match existing.validate_checksums(&project.root) {
                    Ok(mismatches) if mismatches.is_empty() => false,
                    Ok(mismatches) => {
                        if verbose {
                            for name in &mismatches {
                                println!("  Dependency '{}' has changed, updating lockfile", name);
                            }
                        }
                        true
                    }
                    Err(_) => true,
                },
                Err(_) => true,
            }
        } else {
            true
        };

        if should_write_lockfile {
            if let Err(e) = graph.lockfile.save(&project.root) {
                eprintln!("Warning: Failed to write gradient.lock: {}", e);
            } else if verbose {
                println!("  Generated gradient.lock");
            }
        }

        if verbose {
            for dep in &graph.dependencies {
                println!(
                    "  Dependency: {} v{} ({} source file{})",
                    dep.name,
                    dep.version,
                    dep.source_files.len(),
                    if dep.source_files.len() == 1 { "" } else { "s" }
                );
            }
        }

        // Collect all dependency source files
        graph
            .dependencies
            .iter()
            .flat_map(|d| d.source_files.iter().cloned())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let main_source = project.main_source();
    if !main_source.is_file() {
        eprintln!(
            "Error: Main source file not found at `{}`.\n\
             Every Gradient project needs a `src/main.gr`.",
            main_source.display()
        );
        process::exit(1);
    }

    let target_dir = project.target_dir(release);
    let object_file = project.output_object(release);
    let binary = project.output_binary(release);

    // Create target directory
    if let Err(e) = fs::create_dir_all(&target_dir) {
        eprintln!(
            "Error: Could not create target directory `{}`: {}",
            target_dir.display(),
            e
        );
        process::exit(1);
    }

    if release {
        write_runtime_only_audit(
            &target_dir,
            std::iter::once(main_source.clone())
                .chain(dep_source_files.iter().cloned())
                .collect(),
        );
    }

    // Stage 1: Invoke the compiler
    if verbose {
        println!(
            "  Compiling: {} {} {}",
            compiler.display(),
            main_source.display(),
            object_file.display()
        );
    }

    let mut cmd = Command::new(&compiler);
    cmd.arg(main_source.to_str().unwrap_or("src/main.gr"))
        .arg(object_file.to_str().unwrap_or("output.o"));

    // Pass dependency source files to the compiler
    for dep_file in &dep_source_files {
        cmd.arg("--dep").arg(dep_file);
    }

    if release {
        cmd.arg("--release");
    }

    // Forward --backend <type> to the compiler when explicitly requested.
    // The compiler itself defaults to cranelift in debug and llvm in --release;
    // this flag overrides that selection (e.g. --release --backend cranelift
    // forces cranelift codegen even in release mode).
    if let Some(b) = backend {
        cmd.arg("--backend").arg(b);
    }

    let compile_status = cmd.status();

    match compile_status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "Error: Compiler exited with status {}",
                status.code().unwrap_or(-1)
            );
            process::exit(1);
        }
        Err(e) => {
            eprintln!(
                "Error: Failed to invoke compiler at `{}`: {}",
                compiler.display(),
                e
            );
            process::exit(1);
        }
    }

    // Stage 2: Compile the C runtime helper if present, then link everything.
    //
    // The canonical runtime lives at `runtime/gradient_runtime.c` relative to
    // the compiler binary.  We look for it there first, then fall back to a
    // path relative to the current working directory (useful during
    // development when running `gradient build` from the repo root).
    let runtime_c: Option<std::path::PathBuf> = {
        // Search order:
        // 1. Source tree: <compiler_dir>/../../compiler/runtime/gradient_runtime.c
        // 2. Installed copy next to compiler: <compiler_dir>/runtime/gradient_runtime.c
        // 3. Development fallback: relative to cwd
        let candidates: Vec<std::path::PathBuf> = vec![
            // Source tree (preferred — always up to date)
            compiler
                .parent()
                .map(|d| d.join("../../compiler/runtime/gradient_runtime.c"))
                .unwrap_or_default(),
            // Installed copy next to compiler binary
            compiler
                .parent()
                .map(|d| d.join("runtime").join("gradient_runtime.c"))
                .unwrap_or_default(),
            // Development fallback: path relative to the build-system crate
            std::path::PathBuf::from("../compiler/runtime/gradient_runtime.c"),
        ];
        candidates.into_iter().find(|p| p.is_file())
    };

    // If we found the runtime source, compile it to a .o file in the target dir.
    let runtime_o: Option<std::path::PathBuf> = if let Some(ref rc) = runtime_c {
        let ro = target_dir.join("gradient_runtime.o");

        if verbose {
            println!(
                "  Compiling runtime: cc -c {} -o {}",
                rc.display(),
                ro.display()
            );
        }

        let status = Command::new("cc")
            .arg("-c")
            .arg(rc.to_str().unwrap())
            .arg("-o")
            .arg(ro.to_str().unwrap())
            .status();

        match status {
            Ok(s) if s.success() => Some(ro),
            Ok(s) => {
                // Non-fatal: warn but proceed; linking will fail if symbols are missing.
                eprintln!(
                    "Warning: Failed to compile runtime helper (exit {}). \
                     Linking without it.",
                    s.code().unwrap_or(-1)
                );
                None
            }
            Err(e) => {
                eprintln!("Warning: Could not invoke `cc` to compile runtime: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Stage 3: Link with cc
    if verbose {
        let extra = runtime_o
            .as_ref()
            .map(|p| format!(" {}", p.display()))
            .unwrap_or_default();
        println!(
            "  Linking: cc {}{} -o {}",
            object_file.display(),
            extra,
            binary.display()
        );
    }

    let mut link_cmd = Command::new("cc");
    link_cmd.arg(object_file.to_str().unwrap_or("output.o"));
    if let Some(ref ro) = runtime_o {
        link_cmd.arg(ro.to_str().unwrap());
    }
    link_cmd
        .arg("-o")
        .arg(binary.to_str().unwrap_or("output"))
        .arg("-lcurl");

    let link_status = link_cmd.status();

    match link_status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "Error: Linker exited with status {}",
                status.code().unwrap_or(-1)
            );
            process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: Failed to invoke linker `cc`: {}", e);
            eprintln!("Make sure a C compiler is installed (gcc, clang, etc.).");
            process::exit(1);
        }
    }

    let profile = if release { "release" } else { "debug" };
    println!(
        "Compiled {} -> target/{}/{}",
        project.name, profile, project.name
    );

    binary.to_string_lossy().to_string()
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeOnlyAudit {
    stripped_contracts: Vec<StrippedContractAuditItem>,
}

#[derive(Debug, Clone, Serialize)]
struct StrippedContractAuditItem {
    file: String,
    line: u32,
    function: String,
    kind: String,
    assertion: String,
}

fn write_runtime_only_audit(target_dir: &Path, source_files: Vec<PathBuf>) {
    let audit = RuntimeOnlyAudit {
        stripped_contracts: collect_runtime_only_audit_items(source_files),
    };
    let audit_path = target_dir.join("audit.json");
    let json = match serde_json::to_string_pretty(&audit) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Warning: Failed to serialize release contract audit: {}", e);
            return;
        }
    };
    if let Err(e) = fs::write(&audit_path, json) {
        eprintln!("Warning: Failed to write {}: {}", audit_path.display(), e);
        return;
    }

    if !audit.stripped_contracts.is_empty() {
        eprintln!(
            "Warning: stripped {} @runtime_only(off_in_release) contract(s); audit written to {}",
            audit.stripped_contracts.len(),
            audit_path.display()
        );
        for item in &audit.stripped_contracts {
            eprintln!(
                "  {}:{}: {} {}({})",
                item.file, item.line, item.function, item.kind, item.assertion
            );
        }
    }

    let forbidden: Vec<_> = audit
        .stripped_contracts
        .iter()
        .filter(|item| path_is_core_or_alloc(Path::new(&item.file)))
        .collect();
    if !forbidden.is_empty() {
        eprintln!(
            "Error: production release build stripped runtime-only contracts in core/alloc; see {}",
            audit_path.display()
        );
        process::exit(1);
    }
}

fn collect_runtime_only_audit_items(source_files: Vec<PathBuf>) -> Vec<StrippedContractAuditItem> {
    let mut items = Vec::new();
    for source_file in source_files {
        let source = match fs::read_to_string(&source_file) {
            Ok(source) => source,
            Err(e) => {
                eprintln!(
                    "Warning: Failed to read {} for contract audit: {}",
                    source_file.display(),
                    e
                );
                continue;
            }
        };
        let (module, parse_errors) = gradient_compiler::parse_source(&source, 0);
        if !parse_errors.is_empty() {
            continue;
        }
        for item in &module.items {
            if let ItemKind::FnDef(fn_def) = &item.node {
                for contract in &fn_def.contracts {
                    if contract.runtime_only_off_in_release {
                        items.push(StrippedContractAuditItem {
                            file: source_file.display().to_string(),
                            line: contract.span.start.line,
                            function: fn_def.name.clone(),
                            kind: match contract.kind {
                                ContractKind::Requires => "requires".to_string(),
                                ContractKind::Ensures => "ensures".to_string(),
                            },
                            assertion: format_audit_expr(&contract.condition),
                        });
                    }
                }
            }
        }
    }
    items
}

fn path_is_core_or_alloc(path: &Path) -> bool {
    path.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        part == "core" || part == "alloc"
    })
}

fn format_audit_expr(expr: &Expr) -> String {
    match &expr.node {
        ExprKind::IntLit(n) => n.to_string(),
        ExprKind::FloatLit(f) => f.to_string(),
        ExprKind::StringLit(s) => format!("\"{}\"", s),
        ExprKind::BoolLit(b) => b.to_string(),
        ExprKind::UnitLit => "()".to_string(),
        ExprKind::Ident(name) => name.clone(),
        ExprKind::BinaryOp { op, left, right } => {
            format!(
                "{} {} {}",
                format_audit_expr(left),
                format_binop(*op),
                format_audit_expr(right)
            )
        }
        ExprKind::UnaryOp { op, operand } => match op {
            UnaryOp::Neg => format!("-{}", format_audit_expr(operand)),
            UnaryOp::Not => format!("not {}", format_audit_expr(operand)),
        },
        ExprKind::Call { func, args } => {
            let args = args
                .iter()
                .map(format_audit_expr)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({})", format_audit_expr(func), args)
        }
        ExprKind::FieldAccess { object, field } => {
            format!("{}.{}", format_audit_expr(object), field)
        }
        ExprKind::Paren(inner) => format!("({})", format_audit_expr(inner)),
        _ => "<expr>".to_string(),
    }
}

fn format_binop(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Pipe => "|>",
    }
}

/// Execute the `gradient build --file <path>` subcommand.
/// Compiles a single file instead of the current project.
/// Used for bootstrap testing of the self-hosted compiler.
pub fn execute_single_file(
    file_path: &str,
    _release: bool,
    verbose: bool,
    parse_only: bool,
    typecheck_only: bool,
    emit_ir: bool,
    backend: Option<&str>,
) {
    let compiler = match super::super::project::Project::find_compiler() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    let source_file = std::path::PathBuf::from(file_path);
    if !source_file.is_file() {
        eprintln!("Error: Source file not found at `{}`", file_path);
        std::process::exit(1);
    }

    if verbose {
        println!("  Compiling single file: {}", file_path);
    }

    // Determine output file
    let output_ext = if emit_ir { "ir" } else { "o" };
    let output_file = source_file.with_extension(output_ext);

    // Build compiler command
    let mut cmd = std::process::Command::new(&compiler);
    cmd.arg(source_file.to_str().unwrap_or(file_path))
        .arg(output_file.to_str().unwrap_or("output.o"));

    // Add flags for bootstrap testing
    if parse_only {
        cmd.arg("--parse-only");
    }
    if typecheck_only {
        cmd.arg("--typecheck-only");
    }
    if emit_ir {
        cmd.arg("--emit-ir");
    }
    if let Some(b) = backend {
        cmd.arg("--backend").arg(b);
    }

    let compile_status = cmd.status();

    match compile_status {
        Ok(status) if status.success() => {
            if verbose {
                if emit_ir {
                    println!("  IR output written to: {}", output_file.display());
                } else {
                    println!("  Compiled to: {}", output_file.display());
                }
            }
        }
        Ok(status) => {
            eprintln!(
                "Error: Compilation failed with status {}",
                status.code().unwrap_or(-1)
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: Failed to invoke compiler: {}", e);
            std::process::exit(1);
        }
    }
}

/// Execute the `gradient build --stdin` subcommand.
/// Compiles source code read from stdin instead of a file.
/// Used for bootstrap testing and piping source code.
pub fn execute_stdin(
    _release: bool,
    verbose: bool,
    parse_only: bool,
    typecheck_only: bool,
    emit_ir: bool,
    backend: Option<&str>,
) {
    let compiler = match super::super::project::Project::find_compiler() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // L-4: use a uniquely-named temp file to avoid races on the fixed path.
    let output_suffix = if emit_ir { ".ir" } else { ".o" };
    let temp_file = match tempfile::Builder::new()
        .prefix("gradient_stdin_")
        .suffix(output_suffix)
        .tempfile()
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: Failed to create temp file: {}", e);
            std::process::exit(1);
        }
    };
    let output_path = temp_file.path().to_path_buf();

    if verbose {
        println!("  Compiling from stdin -> {}", output_path.display());
    }

    // Build compiler command with stdin flag
    let mut cmd = std::process::Command::new(&compiler);
    cmd.arg(output_path.to_str().expect("temp path is valid UTF-8"))
        .arg("--stdin");

    // Add flags for bootstrap testing
    if parse_only {
        cmd.arg("--parse-only");
    }
    if typecheck_only {
        cmd.arg("--typecheck-only");
    }
    if emit_ir {
        cmd.arg("--emit-ir");
    }
    if let Some(b) = backend {
        cmd.arg("--backend").arg(b);
    }

    // Pipe stdin through to compiler
    cmd.stdin(std::process::Stdio::inherit());

    let compile_status = cmd.status();

    // Keep the NamedTempFile alive until after the compiler runs so the path
    // remains valid; it is cleaned up when this binding drops.
    let _ = temp_file;

    match compile_status {
        Ok(status) if status.success() => {
            if verbose {
                if emit_ir {
                    println!("  IR output written to: {}", output_path.display());
                } else {
                    println!("  Compiled to: {}", output_path.display());
                }
            }
        }
        Ok(status) => {
            eprintln!(
                "Error: Compilation failed with status {}",
                status.code().unwrap_or(-1)
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: Failed to invoke compiler: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify build.rs accepts a `backend: Option<&str>` parameter on its
    /// public entry points so the CLI can forward `gradient build --backend X`
    /// down to the compiler. Closes #341.
    #[test]
    fn build_execute_accepts_backend() {
        let source = std::include_str!("build.rs");
        assert!(
            source.contains("pub fn execute(release: bool, verbose: bool, backend: Option<&str>)"),
            "build::execute must accept a backend: Option<&str> parameter"
        );
        assert!(
            source.contains(
                "pub fn run_build(project: &Project, release: bool, verbose: bool, backend: Option<&str>)"
            ),
            "build::run_build must accept a backend: Option<&str> parameter"
        );
    }

    /// Verify build.rs forwards `--backend <type>` to the compiler when a
    /// backend was explicitly requested. The compiler's existing `--backend`
    /// flag does the actual selection (and produces the
    /// "LLVM backend not available" diagnostic when llvm is requested
    /// without the cargo feature).
    #[test]
    fn build_forwards_backend_flag_to_compiler() {
        let source = std::include_str!("build.rs");
        assert!(
            source.contains(r#"cmd.arg("--backend").arg(b)"#),
            "build.rs must forward --backend <type> to the compiler when set"
        );
    }

    #[test]
    fn release_build_forwards_release_flag_to_compiler() {
        let source = std::include_str!("build.rs");
        assert!(
            source.contains(r#"cmd.arg("--release")"#),
            "release builds must forward --release so compiler strips runtime-only contracts"
        );
    }

    #[test]
    fn audit_collects_runtime_only_release_contracts() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@runtime_only(off_in_release)
@requires(x > 0)
fn f(x: Int) -> Int:
    ret x
",
        )
        .unwrap();

        let items = collect_runtime_only_audit_items(vec![source_path.clone()]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file, source_path.display().to_string());
        assert_eq!(items[0].function, "f");
        assert_eq!(items[0].kind, "requires");
        assert_eq!(items[0].assertion, "x > 0");
        assert!(items[0].line > 0);
    }

    #[test]
    fn audit_ci_rule_matches_core_or_alloc_path_components() {
        assert!(path_is_core_or_alloc(std::path::Path::new(
            "/tmp/project/core/src/main.gr"
        )));
        assert!(path_is_core_or_alloc(std::path::Path::new(
            "/tmp/project/alloc/src/main.gr"
        )));
        assert!(!path_is_core_or_alloc(std::path::Path::new(
            "/tmp/project/mycore/src/main.gr"
        )));
    }
}
