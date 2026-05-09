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

    // Stage 2: Compile the C runtime helpers, then link everything.
    //
    // The build links TWO C runtime objects into every binary:
    //   (a) the canonical `gradient_runtime.c` (libc helpers used by builtins).
    //   (b) ONE of the three `runtime_panic_<strategy>.c` objects, picked
    //       from the main module's `@panic(abort|unwind|none)` attribute
    //       (defaults to `unwind`). See codebase/compiler/runtime/panic/README.md.
    //
    // We resolve the panic strategy by parsing the main source with the
    // host compiler library; if parsing fails we fall back to the default
    // (`unwind`) so the build can still link.
    let panic_strategy: &'static str = detect_panic_strategy(&main_source);
    if verbose {
        println!("  Panic strategy: @panic({})", panic_strategy);
    }

    // Alloc strategy (#333): driven automatically by the effect summary
    // — `full` if the program reaches `Heap`, `minimal` otherwise. ADR
    // 0005 commits the runtime closure to effect-driven DCE.
    let alloc_strategy: &'static str = detect_alloc_strategy(&main_source);
    if verbose {
        println!(
            "  Alloc strategy: {} (auto-detected from effect surface)",
            alloc_strategy
        );
    }

    // Actor strategy (#334): driven automatically by the effect summary
    // — `full` if the program reaches `Actor`, `none` otherwise. Sibling
    // of the alloc-strategy split above; same ADR 0005 effect-driven DCE
    // commitment, different trigger effect.
    let actor_strategy: &'static str = detect_actor_strategy(&main_source);
    if verbose {
        println!(
            "  Actor strategy: {} (auto-detected from effect surface)",
            actor_strategy
        );
    }

    // Async strategy (#335): driven automatically by the effect summary
    // — `full` if the program reaches `Async`, `none` otherwise. Sibling
    // of the actor-strategy split above; same ADR 0005 effect-driven DCE
    // commitment, different trigger effect.
    let async_strategy: &'static str = detect_async_strategy(&main_source);
    if verbose {
        println!(
            "  Async strategy: {} (auto-detected from effect surface)",
            async_strategy
        );
    }

    // Allocator strategy (#336): attribute-driven selection of the
    // allocator runtime crate. `default` (system malloc) vs `pluggable`
    // (embedder supplies `__gradient_alloc`/`__gradient_free` at link
    // time). Sibling of the panic-strategy split (#318/#537) in that
    // respect — both are deployment decisions, NOT derivable from the
    // effect surface alone.
    let allocator_strategy: &'static str = detect_allocator_strategy(&main_source);
    if verbose {
        println!("  Allocator strategy: @allocator({})", allocator_strategy);
    }

    // Locate the canonical runtime C source.
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

    // Locate the panic-strategy runtime C source matching `panic_strategy`.
    // Mirrors the canonical-runtime search above but uses the
    // `runtime/panic/runtime_panic_<strategy>.c` filename convention.
    let panic_runtime_c: Option<std::path::PathBuf> =
        find_panic_runtime_source(&compiler, panic_strategy);

    // Locate the alloc-strategy runtime C source matching `alloc_strategy`.
    // Sibling of the panic-runtime locator; both share the same search
    // order and fall-through-with-warning semantics.
    let alloc_runtime_c: Option<std::path::PathBuf> =
        find_alloc_runtime_source(&compiler, alloc_strategy);

    // Locate the actor-strategy runtime C source matching `actor_strategy`.
    // Sibling of the alloc-runtime locator (#333); same search order and
    // fall-through-with-warning semantics.
    let actor_runtime_c: Option<std::path::PathBuf> =
        find_actor_runtime_source(&compiler, actor_strategy);

    // Locate the async-strategy runtime C source matching `async_strategy`.
    // Sibling of the actor-runtime locator (#334); same search order and
    // fall-through-with-warning semantics.
    let async_runtime_c: Option<std::path::PathBuf> =
        find_async_runtime_source(&compiler, async_strategy);

    // Locate the allocator-strategy runtime C source matching
    // `allocator_strategy`. Sibling of the async-runtime locator (#335);
    // same search order and fall-through-with-warning semantics.
    let allocator_runtime_c: Option<std::path::PathBuf> =
        find_allocator_runtime_source(&compiler, allocator_strategy);

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

    // Compile the panic-strategy runtime object (`runtime_panic_<strategy>.o`)
    // alongside the canonical runtime. Naming the object after the strategy
    // makes incremental rebuilds across `@panic(...)` flips behave correctly:
    // the linker sees a fresh object name, not a stale one with the previous
    // strategy's symbols.
    let panic_runtime_o: Option<std::path::PathBuf> = if let Some(ref pc) = panic_runtime_c {
        let po = target_dir.join(format!("runtime_panic_{}.o", panic_strategy));

        if verbose {
            println!(
                "  Compiling panic runtime: cc -c {} -o {}",
                pc.display(),
                po.display()
            );
        }

        let status = Command::new("cc")
            .arg("-c")
            .arg(pc.to_str().unwrap())
            .arg("-o")
            .arg(po.to_str().unwrap())
            .status();

        match status {
            Ok(s) if s.success() => Some(po),
            Ok(s) => {
                eprintln!(
                    "Warning: Failed to compile panic runtime ({}, exit {}). \
                     Linking without it; calls to __gradient_panic will be unresolved.",
                    panic_strategy,
                    s.code().unwrap_or(-1)
                );
                None
            }
            Err(e) => {
                eprintln!(
                    "Warning: Could not invoke `cc` to compile panic runtime ({}): {}",
                    panic_strategy, e
                );
                None
            }
        }
    } else {
        if verbose {
            eprintln!(
                "Warning: panic runtime source for strategy `{}` not found; \
                 calls to __gradient_panic will be unresolved at link time.",
                panic_strategy
            );
        }
        None
    };

    // Compile the alloc-strategy runtime object (`runtime_alloc_<strategy>.o`)
    // alongside the canonical + panic runtime objects. Sibling of the
    // panic-strategy compile block immediately above; same naming
    // convention (object named after the strategy so incremental rebuilds
    // across strategy flips don't reuse a stale object), same warn-only
    // failure mode (the alloc tag isn't strictly required for the link
    // to succeed today since the rc/COW machinery still lives in
    // `gradient_runtime.c`, but a missing tag produces a less-introspectable
    // binary so we surface a warning).
    let alloc_runtime_o: Option<std::path::PathBuf> = if let Some(ref ac) = alloc_runtime_c {
        let ao = target_dir.join(format!("runtime_alloc_{}.o", alloc_strategy));

        if verbose {
            println!(
                "  Compiling alloc runtime: cc -c {} -o {}",
                ac.display(),
                ao.display()
            );
        }

        let status = Command::new("cc")
            .arg("-c")
            .arg(ac.to_str().unwrap())
            .arg("-o")
            .arg(ao.to_str().unwrap())
            .status();

        match status {
            Ok(s) if s.success() => Some(ao),
            Ok(s) => {
                eprintln!(
                    "Warning: Failed to compile alloc runtime ({}, exit {}). \
                     Linking without it; binary will lack the alloc-strategy tag.",
                    alloc_strategy,
                    s.code().unwrap_or(-1)
                );
                None
            }
            Err(e) => {
                eprintln!(
                    "Warning: Could not invoke `cc` to compile alloc runtime ({}): {}",
                    alloc_strategy, e
                );
                None
            }
        }
    } else {
        if verbose {
            eprintln!(
                "Warning: alloc runtime source for strategy `{}` not found; \
                 binary will lack the alloc-strategy tag.",
                alloc_strategy
            );
        }
        None
    };

    // Compile the actor-strategy runtime object (`runtime_actor_<strategy>.o`)
    // alongside the canonical + panic + alloc runtime objects. Sibling of
    // the alloc-strategy compile block immediately above; same naming
    // convention, same warn-only failure mode.
    let actor_runtime_o: Option<std::path::PathBuf> = if let Some(ref ac) = actor_runtime_c {
        let ao = target_dir.join(format!("runtime_actor_{}.o", actor_strategy));

        if verbose {
            println!(
                "  Compiling actor runtime: cc -c {} -o {}",
                ac.display(),
                ao.display()
            );
        }

        let status = Command::new("cc")
            .arg("-c")
            .arg(ac.to_str().unwrap())
            .arg("-o")
            .arg(ao.to_str().unwrap())
            .status();

        match status {
            Ok(s) if s.success() => Some(ao),
            Ok(s) => {
                eprintln!(
                    "Warning: Failed to compile actor runtime ({}, exit {}). \
                     Linking without it; binary will lack the actor-strategy tag.",
                    actor_strategy,
                    s.code().unwrap_or(-1)
                );
                None
            }
            Err(e) => {
                eprintln!(
                    "Warning: Could not invoke `cc` to compile actor runtime ({}): {}",
                    actor_strategy, e
                );
                None
            }
        }
    } else {
        if verbose {
            eprintln!(
                "Warning: actor runtime source for strategy `{}` not found; \
                 binary will lack the actor-strategy tag.",
                actor_strategy
            );
        }
        None
    };

    // Compile the async-strategy runtime object (`runtime_async_<strategy>.o`)
    // alongside the canonical + panic + alloc + actor runtime objects. Sibling
    // of the actor-strategy compile block immediately above; same naming
    // convention, same warn-only failure mode.
    let async_runtime_o: Option<std::path::PathBuf> = if let Some(ref ac) = async_runtime_c {
        let ao = target_dir.join(format!("runtime_async_{}.o", async_strategy));

        if verbose {
            println!(
                "  Compiling async runtime: cc -c {} -o {}",
                ac.display(),
                ao.display()
            );
        }

        let status = Command::new("cc")
            .arg("-c")
            .arg(ac.to_str().unwrap())
            .arg("-o")
            .arg(ao.to_str().unwrap())
            .status();

        match status {
            Ok(s) if s.success() => Some(ao),
            Ok(s) => {
                eprintln!(
                    "Warning: Failed to compile async runtime ({}, exit {}). \
                     Linking without it; binary will lack the async-strategy tag.",
                    async_strategy,
                    s.code().unwrap_or(-1)
                );
                None
            }
            Err(e) => {
                eprintln!(
                    "Warning: Could not invoke `cc` to compile async runtime ({}): {}",
                    async_strategy, e
                );
                None
            }
        }
    } else {
        if verbose {
            eprintln!(
                "Warning: async runtime source for strategy `{}` not found; \
                 binary will lack the async-strategy tag.",
                async_strategy
            );
        }
        None
    };

    // Compile the allocator-strategy runtime object
    // (`runtime_allocator_<strategy>.o`) alongside the canonical + panic
    // + alloc + actor + async runtime objects. Sibling of the
    // async-strategy compile block immediately above; same naming
    // convention. Failure mode is stricter: the allocator runtime
    // exports `__gradient_alloc` / `__gradient_free` (default variant)
    // or declares them extern (pluggable variant), so a missing object
    // file produces an undefined-symbol link error rather than a
    // silently-wrong build. We still warn-and-continue so the user
    // gets a useful linker diagnostic.
    let allocator_runtime_o: Option<std::path::PathBuf> = if let Some(ref ac) = allocator_runtime_c
    {
        let ao = target_dir.join(format!("runtime_allocator_{}.o", allocator_strategy));

        if verbose {
            println!(
                "  Compiling allocator runtime: cc -c {} -o {}",
                ac.display(),
                ao.display()
            );
        }

        let status = Command::new("cc")
            .arg("-c")
            .arg(ac.to_str().unwrap())
            .arg("-o")
            .arg(ao.to_str().unwrap())
            .status();

        match status {
            Ok(s) if s.success() => Some(ao),
            Ok(s) => {
                eprintln!(
                    "Warning: Failed to compile allocator runtime ({}, exit {}). \
                         Linking without it; calls to __gradient_alloc / __gradient_free \
                         will be unresolved.",
                    allocator_strategy,
                    s.code().unwrap_or(-1)
                );
                None
            }
            Err(e) => {
                eprintln!(
                    "Warning: Could not invoke `cc` to compile allocator runtime ({}): {}",
                    allocator_strategy, e
                );
                None
            }
        }
    } else {
        if verbose {
            eprintln!(
                "Warning: allocator runtime source for strategy `{}` not found; \
                     calls to __gradient_alloc / __gradient_free will be unresolved.",
                allocator_strategy
            );
        }
        None
    };

    // Stage 3: Link with cc
    if verbose {
        let extra_runtime = runtime_o
            .as_ref()
            .map(|p| format!(" {}", p.display()))
            .unwrap_or_default();
        let extra_panic = panic_runtime_o
            .as_ref()
            .map(|p| format!(" {}", p.display()))
            .unwrap_or_default();
        let extra_alloc = alloc_runtime_o
            .as_ref()
            .map(|p| format!(" {}", p.display()))
            .unwrap_or_default();
        let extra_actor = actor_runtime_o
            .as_ref()
            .map(|p| format!(" {}", p.display()))
            .unwrap_or_default();
        let extra_async = async_runtime_o
            .as_ref()
            .map(|p| format!(" {}", p.display()))
            .unwrap_or_default();
        let extra_allocator = allocator_runtime_o
            .as_ref()
            .map(|p| format!(" {}", p.display()))
            .unwrap_or_default();
        println!(
            "  Linking: cc {}{}{}{}{}{}{} -o {}",
            object_file.display(),
            extra_runtime,
            extra_panic,
            extra_alloc,
            extra_actor,
            extra_async,
            extra_allocator,
            binary.display()
        );
    }

    let mut link_cmd = Command::new("cc");
    link_cmd.arg(object_file.to_str().unwrap_or("output.o"));
    if let Some(ref ro) = runtime_o {
        link_cmd.arg(ro.to_str().unwrap());
    }
    if let Some(ref po) = panic_runtime_o {
        link_cmd.arg(po.to_str().unwrap());
    }
    if let Some(ref ao) = alloc_runtime_o {
        link_cmd.arg(ao.to_str().unwrap());
    }
    if let Some(ref ao) = actor_runtime_o {
        link_cmd.arg(ao.to_str().unwrap());
    }
    if let Some(ref ao) = async_runtime_o {
        link_cmd.arg(ao.to_str().unwrap());
    }
    if let Some(ref ao) = allocator_runtime_o {
        link_cmd.arg(ao.to_str().unwrap());
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

    // Verbose-only binary-size report (#333). The alloc-strategy split's
    // long-term win is a binary-size delta when `minimal` is selected;
    // surface today's baseline so future PRs that actually move the
    // rc/COW machinery into `runtime_alloc_full.c` can be measured
    // against it.
    if verbose {
        if let Ok(meta) = fs::metadata(&binary) {
            println!(
                "  Binary size: {} bytes (alloc={}, panic={}, actor={}, async={}, allocator={})",
                meta.len(),
                alloc_strategy,
                panic_strategy,
                actor_strategy,
                async_strategy,
                allocator_strategy
            );
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

// =========================================================================
// Panic-strategy runtime selection (#337, E5)
//
// `gradient build` reads the main module's `@panic(abort|unwind|none)`
// attribute (parsed since #521) and links exactly ONE matching
// `runtime_panic_<strategy>.c` object. The three runtimes live under
// `codebase/compiler/runtime/panic/`. See that directory's README for the
// per-strategy contract.
// =========================================================================

/// Read the `@panic(...)` attribute from the main source file and return the
/// matching runtime strategy as a static string slice (`"abort"`,
/// `"unwind"`, or `"none"`).
///
/// Falls back to the AST default (`PanicStrategy::Unwind` -> `"unwind"`)
/// when the source cannot be read or fails to parse, so the build can
/// continue and the user sees the parse error (or the I/O error) from
/// the compiler invocation itself rather than a confusing build-system
/// abort here.
pub(crate) fn detect_panic_strategy(main_source: &Path) -> &'static str {
    let source = match fs::read_to_string(main_source) {
        Ok(s) => s,
        Err(_) => {
            return panic_strategy_to_str(gradient_compiler::ast::module::PanicStrategy::default())
        }
    };
    let (module, parse_errors) = gradient_compiler::parse_source(&source, 0);
    if !parse_errors.is_empty() {
        // Compiler invocation in stage 1 will report the parse errors and
        // exit before linking, so this branch is only reachable if the
        // parser is more lenient than `parse_source`. Fall back to default.
        return panic_strategy_to_str(gradient_compiler::ast::module::PanicStrategy::default());
    }
    panic_strategy_to_str(module.panic_strategy)
}

/// Map an AST `PanicStrategy` to the static lowercase string used in the
/// runtime filename and the Query API surface.
pub(crate) fn panic_strategy_to_str(
    strategy: gradient_compiler::ast::module::PanicStrategy,
) -> &'static str {
    use gradient_compiler::ast::module::PanicStrategy;
    match strategy {
        PanicStrategy::Abort => "abort",
        PanicStrategy::Unwind => "unwind",
        PanicStrategy::None => "none",
    }
}

/// Locate the `runtime_panic_<strategy>.c` source file matching the given
/// strategy. Mirrors the search order of the canonical runtime locator above.
///
/// Returns `None` when none of the candidate paths exist; callers should
/// fall through with a warning rather than failing the build (a missing
/// panic runtime only fails programs that actually call `__gradient_panic`,
/// which is the correct behavior for a defense-in-depth backstop).
pub(crate) fn find_panic_runtime_source(compiler: &Path, strategy: &str) -> Option<PathBuf> {
    let filename = format!("runtime_panic_{}.c", strategy);
    let candidates: Vec<PathBuf> = vec![
        // Source tree (preferred — always up to date)
        compiler
            .parent()
            .map(|d| d.join("../../compiler/runtime/panic").join(&filename))
            .unwrap_or_default(),
        // Installed copy next to compiler binary
        compiler
            .parent()
            .map(|d| d.join("runtime").join("panic").join(&filename))
            .unwrap_or_default(),
        // Development fallback: relative to the build-system crate
        PathBuf::from("../compiler/runtime/panic").join(&filename),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

// =========================================================================
// Alloc-strategy runtime selection (#333, E5)
//
// Sibling to the panic-strategy split, but driven automatically by the
// effect summary instead of by a user-facing module attribute. ADR 0005
// commits to effect-driven DCE for the runtime closure: the build picks
// `runtime_alloc_full.c` when the program's effect surface contains
// `Heap` (i.e. it uses heap-allocating builtins), and
// `runtime_alloc_minimal.c` otherwise.
//
// Today both files are tag-only: the rc/COW machinery still lives in
// `gradient_runtime.c` as static helpers. The follow-on PR will extract
// it into `runtime_alloc_full.c` so a heap-free program that selects
// `minimal` actually drops bytes from the binary. For now the dispatch
// is wired, the tag is introspectable via `nm`, and the binary-size
// delta is reported in verbose builds so future extractions can be
// measured against today's baseline.
// =========================================================================

/// Detect the alloc strategy ("full" or "minimal") for the main source file.
///
/// Returns `"full"` when the host typechecker reports `Heap` in the
/// module's `effects_used` set, and `"minimal"` otherwise.
///
/// Falls back to `"full"` (the safe default — links the heap helpers in)
/// when the source cannot be read or fails to parse, so the build can
/// continue and the user sees the parse error from the compiler
/// invocation in stage 1 rather than a confusing build-system abort here.
pub(crate) fn detect_alloc_strategy(main_source: &Path) -> &'static str {
    let source = match fs::read_to_string(main_source) {
        Ok(s) => s,
        Err(_) => return "full",
    };
    let session = gradient_compiler::query::Session::from_source(&source);
    let index = session.project_index();
    if index.modules.is_empty() {
        return "full";
    }
    match index.modules[0].alloc_strategy.as_str() {
        "minimal" => "minimal",
        // Default to `full` for any other value (including the empty
        // string and any future variants we don't yet know about) so we
        // err on the side of linking the heap helpers in.
        _ => "full",
    }
}

/// Locate the `runtime_alloc_<strategy>.c` source file matching the given
/// strategy. Mirrors the search order of the canonical-runtime locator
/// and the panic-runtime locator above.
///
/// Returns `None` when none of the candidate paths exist; callers should
/// fall through with a warning rather than failing the build.
pub(crate) fn find_alloc_runtime_source(compiler: &Path, strategy: &str) -> Option<PathBuf> {
    let filename = format!("runtime_alloc_{}.c", strategy);
    let candidates: Vec<PathBuf> = vec![
        // Source tree (preferred — always up to date)
        compiler
            .parent()
            .map(|d| d.join("../../compiler/runtime/alloc").join(&filename))
            .unwrap_or_default(),
        // Installed copy next to compiler binary
        compiler
            .parent()
            .map(|d| d.join("runtime").join("alloc").join(&filename))
            .unwrap_or_default(),
        // Development fallback: relative to the build-system crate
        PathBuf::from("../compiler/runtime/alloc").join(&filename),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

// =========================================================================
// Actor-strategy runtime selection (#334, E5)
//
// Sibling of the alloc-strategy split (#333): same effect-driven dispatch
// recipe, different trigger effect. ADR 0005 commits the runtime closure
// to effect-driven DCE — programs that never spawn an actor shouldn't pay
// for the scheduler. The build picks `runtime_actor_full.c` when the
// program's effect surface contains `Actor` (i.e. it uses `spawn`/`send`/
// `ask` or any actor-typed value), and `runtime_actor_none.c` otherwise.
//
// Today both files are tag-only: the actor scheduler still lives as
// `static` helpers inside `gradient_runtime.c` / the experimental actor
// module. The follow-on PR will extract it into `runtime_actor_full.c`
// so an actor-free program selecting `none` actually drops bytes from
// the binary.
// =========================================================================

/// Detect the actor strategy (`"full"` or `"none"`) for the main source file.
///
/// Returns `"full"` when the host typechecker reports `Actor` in the
/// module's `effects_used` set, and `"none"` otherwise.
///
/// Falls back to `"full"` (the safe default — links the actor helpers in)
/// when the source cannot be read or fails to parse, so the build can
/// continue and the user sees the parse error from the compiler
/// invocation in stage 1 rather than a confusing build-system abort here.
pub(crate) fn detect_actor_strategy(main_source: &Path) -> &'static str {
    let source = match fs::read_to_string(main_source) {
        Ok(s) => s,
        Err(_) => return "full",
    };
    let session = gradient_compiler::query::Session::from_source(&source);
    let index = session.project_index();
    if index.modules.is_empty() {
        return "full";
    }
    match index.modules[0].actor_strategy.as_str() {
        "none" => "none",
        // Default to `full` for any other value (including the empty
        // string and any future variants we don't yet know about) so we
        // err on the side of linking the actor helpers in.
        _ => "full",
    }
}

/// Locate the `runtime_actor_<strategy>.c` source file matching the given
/// strategy. Mirrors the search order of the alloc-runtime locator above.
///
/// Returns `None` when none of the candidate paths exist; callers should
/// fall through with a warning rather than failing the build.
pub(crate) fn find_actor_runtime_source(compiler: &Path, strategy: &str) -> Option<PathBuf> {
    let filename = format!("runtime_actor_{}.c", strategy);
    let candidates: Vec<PathBuf> = vec![
        // Source tree (preferred — always up to date)
        compiler
            .parent()
            .map(|d| d.join("../../compiler/runtime/actor").join(&filename))
            .unwrap_or_default(),
        // Installed copy next to compiler binary
        compiler
            .parent()
            .map(|d| d.join("runtime").join("actor").join(&filename))
            .unwrap_or_default(),
        // Development fallback: relative to the build-system crate
        PathBuf::from("../compiler/runtime/actor").join(&filename),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

// =========================================================================
// Async-strategy runtime selection (#335, E5)
//
// Sibling of the actor-strategy split (#334): same effect-driven dispatch
// recipe, different trigger effect. ADR 0005 commits the runtime closure
// to effect-driven DCE — programs that never await shouldn't pay for the
// async executor. The build picks `runtime_async_full.c` when the
// program's effect surface contains `Async` (i.e. it uses async/await,
// futures, or any async-typed value), and `runtime_async_none.c`
// otherwise.
//
// Today both files are tag-only: the async executor still lives as
// `static` helpers inside `gradient_runtime.c` / the experimental async
// module. The follow-on PR will extract it into `runtime_async_full.c`
// so an async-free program selecting `none` actually drops bytes from
// the binary.
// =========================================================================

/// Detect the async strategy (`"full"` or `"none"`) for the main source file.
///
/// Returns `"full"` when the host typechecker reports `Async` in the
/// module's `effects_used` set, and `"none"` otherwise.
///
/// Falls back to `"full"` (the safe default — links the async helpers in)
/// when the source cannot be read or fails to parse, so the build can
/// continue and the user sees the parse error from the compiler
/// invocation in stage 1 rather than a confusing build-system abort here.
pub(crate) fn detect_async_strategy(main_source: &Path) -> &'static str {
    let source = match fs::read_to_string(main_source) {
        Ok(s) => s,
        Err(_) => return "full",
    };
    let session = gradient_compiler::query::Session::from_source(&source);
    let index = session.project_index();
    if index.modules.is_empty() {
        return "full";
    }
    match index.modules[0].async_strategy.as_str() {
        "none" => "none",
        // Default to `full` for any other value (including the empty
        // string and any future variants we don't yet know about) so we
        // err on the side of linking the async helpers in.
        _ => "full",
    }
}

/// Locate the `runtime_async_<strategy>.c` source file matching the given
/// strategy. Mirrors the search order of the actor-runtime locator above.
///
/// Returns `None` when none of the candidate paths exist; callers should
/// fall through with a warning rather than failing the build.
pub(crate) fn find_async_runtime_source(compiler: &Path, strategy: &str) -> Option<PathBuf> {
    let filename = format!("runtime_async_{}.c", strategy);
    let candidates: Vec<PathBuf> = vec![
        // Source tree (preferred — always up to date)
        compiler
            .parent()
            .map(|d| d.join("../../compiler/runtime/async").join(&filename))
            .unwrap_or_default(),
        // Installed copy next to compiler binary
        compiler
            .parent()
            .map(|d| d.join("runtime").join("async").join(&filename))
            .unwrap_or_default(),
        // Development fallback: relative to the build-system crate
        PathBuf::from("../compiler/runtime/async").join(&filename),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Detect the allocator strategy declared by the main module (#336).
///
/// Reads the `@allocator(...)` module attribute via the host parser by
/// way of the Query API. `default` (the safe value — wraps libc's
/// `malloc`/`free`) when the source can't be read or parsed, when the
/// module declares nothing, or when the declared variant is unknown.
///
/// Attribute-driven (NOT effect-driven) — sibling of the panic-strategy
/// detector immediately above; distinct from the effect-driven
/// `detect_alloc_strategy` / `detect_actor_strategy` /
/// `detect_async_strategy` family which derive their selection from the
/// program's effect closure.
pub(crate) fn detect_allocator_strategy(main_source: &Path) -> &'static str {
    let source = match fs::read_to_string(main_source) {
        Ok(s) => s,
        Err(_) => return "default",
    };
    let session = gradient_compiler::query::Session::from_source(&source);
    let index = session.project_index();
    if index.modules.is_empty() {
        return "default";
    }
    match index.modules[0].allocator_strategy.as_str() {
        "pluggable" => "pluggable",
        "arena" => "arena",
        "slab" => "slab",
        "bumpalo" => "bumpalo",
        // Unknown values fall back to `default` (wraps system malloc) so
        // that a future variant added to the AST without updating this
        // matcher doesn't silently link the wrong runtime.
        _ => "default",
    }
}

/// Locate the `runtime_allocator_<strategy>.c` source file matching the
/// given strategy. Mirrors the search order of the async-runtime locator
/// above.
///
/// Returns `None` when none of the candidate paths exist; callers should
/// fall through with a warning rather than failing the build.
pub(crate) fn find_allocator_runtime_source(compiler: &Path, strategy: &str) -> Option<PathBuf> {
    let filename = format!("runtime_allocator_{}.c", strategy);
    let candidates: Vec<PathBuf> = vec![
        // Source tree (preferred — always up to date)
        compiler
            .parent()
            .map(|d| d.join("../../compiler/runtime/allocator").join(&filename))
            .unwrap_or_default(),
        // Installed copy next to compiler binary
        compiler
            .parent()
            .map(|d| d.join("runtime").join("allocator").join(&filename))
            .unwrap_or_default(),
        // Development fallback: relative to the build-system crate
        PathBuf::from("../compiler/runtime/allocator").join(&filename),
    ];
    candidates.into_iter().find(|p| p.is_file())
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

    // -----------------------------------------------------------------------
    // Panic-strategy runtime selection (#337)
    // -----------------------------------------------------------------------

    #[test]
    fn panic_strategy_to_str_maps_each_variant() {
        use gradient_compiler::ast::module::PanicStrategy;
        assert_eq!(panic_strategy_to_str(PanicStrategy::Abort), "abort");
        assert_eq!(panic_strategy_to_str(PanicStrategy::Unwind), "unwind");
        assert_eq!(panic_strategy_to_str(PanicStrategy::None), "none");
    }

    #[test]
    fn detect_panic_strategy_defaults_to_unwind_when_no_attribute() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(&source_path, "fn main() -> Int:\n    ret 0\n").unwrap();
        assert_eq!(detect_panic_strategy(&source_path), "unwind");
    }

    #[test]
    fn detect_panic_strategy_reads_abort() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@panic(abort)\n\nfn main() -> Int:\n    ret 0\n",
        )
        .unwrap();
        assert_eq!(detect_panic_strategy(&source_path), "abort");
    }

    #[test]
    fn detect_panic_strategy_reads_none() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@panic(none)\n\nfn main() -> Int:\n    ret 0\n",
        )
        .unwrap();
        assert_eq!(detect_panic_strategy(&source_path), "none");
    }

    #[test]
    fn detect_panic_strategy_reads_unwind_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@panic(unwind)\n\nfn main() -> Int:\n    ret 0\n",
        )
        .unwrap();
        assert_eq!(detect_panic_strategy(&source_path), "unwind");
    }

    #[test]
    fn detect_panic_strategy_falls_back_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist.gr");
        // Falling back instead of panicking lets the compiler invocation in
        // stage 1 produce the proper I/O error message.
        assert_eq!(detect_panic_strategy(&nonexistent), "unwind");
    }

    #[test]
    fn find_panic_runtime_source_returns_existing_path() {
        // The repo's source tree always carries the three runtimes under
        // codebase/compiler/runtime/panic/. Simulate the "compiler binary
        // lives at ../target/debug/gradient-compiler" layout so the
        // `<compiler_dir>/../../compiler/runtime/panic/...` candidate hits.
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        for strategy in ["abort", "unwind", "none"] {
            let found = find_panic_runtime_source(&fake_compiler, strategy);
            assert!(
                found.is_some(),
                "runtime source for `{}` strategy should resolve from compiler-relative path; \
                 missing under codebase/compiler/runtime/panic/runtime_panic_{}.c",
                strategy,
                strategy
            );
            let path = found.unwrap();
            assert!(path.is_file());
            assert!(path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s == format!("runtime_panic_{}.c", strategy))
                .unwrap_or(false));
        }
    }

    #[test]
    fn find_panic_runtime_source_returns_none_for_unknown_strategy() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        // Unknown strategy = no matching file = None (caller falls through).
        assert!(find_panic_runtime_source(&fake_compiler, "garbage").is_none());
    }

    #[test]
    fn panic_runtime_filenames_follow_strategy_convention() {
        // Lock the convention `runtime_panic_<strategy>.c`. If anyone
        // renames the runtime files this test fails first, before the
        // build wiring or external consumers (issue trackers, plugin
        // authors building inspectable runtimes).
        let runtime_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("compiler/runtime/panic");
        for strategy in ["abort", "unwind", "none"] {
            let expected = runtime_dir.join(format!("runtime_panic_{}.c", strategy));
            assert!(
                expected.is_file(),
                "panic runtime file missing for strategy `{}`: expected at {}",
                strategy,
                expected.display()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Alloc-strategy runtime selection (#333)
    // -----------------------------------------------------------------------

    #[test]
    fn detect_alloc_strategy_minimal_for_pure_arithmetic() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(&source_path, "fn main() -> Int:\n    ret 0\n").unwrap();
        assert_eq!(detect_alloc_strategy(&source_path), "minimal");
    }

    #[test]
    fn detect_alloc_strategy_full_when_heap_declared() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn make(n: Int) -> !{Heap} String:\n    int_to_string(n)\n",
        )
        .unwrap();
        assert_eq!(detect_alloc_strategy(&source_path), "full");
    }

    #[test]
    fn detect_alloc_strategy_minimal_when_only_io_declared() {
        // IO is heap-free; alloc strategy stays minimal.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn shout(n: Int) -> !{IO} ():\n    print_int(n)\n",
        )
        .unwrap();
        assert_eq!(detect_alloc_strategy(&source_path), "minimal");
    }

    #[test]
    fn detect_alloc_strategy_falls_back_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist.gr");
        // Falling back to `full` is the safe default — links the heap
        // helpers in even though stage 1 will surface the I/O error.
        assert_eq!(detect_alloc_strategy(&nonexistent), "full");
    }

    #[test]
    fn find_alloc_runtime_source_returns_existing_path() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        for strategy in ["full", "minimal"] {
            let found = find_alloc_runtime_source(&fake_compiler, strategy);
            assert!(
                found.is_some(),
                "runtime source for `{}` strategy should resolve from compiler-relative path; \
                 missing under codebase/compiler/runtime/alloc/runtime_alloc_{}.c",
                strategy,
                strategy
            );
            let path = found.unwrap();
            assert!(path.is_file());
            assert!(path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s == format!("runtime_alloc_{}.c", strategy))
                .unwrap_or(false));
        }
    }

    #[test]
    fn find_alloc_runtime_source_returns_none_for_unknown_strategy() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        assert!(find_alloc_runtime_source(&fake_compiler, "garbage").is_none());
    }

    #[test]
    fn alloc_runtime_filenames_follow_strategy_convention() {
        // Lock the convention `runtime_alloc_<strategy>.c`. Mirrors the
        // panic-strategy convention lock above.
        let runtime_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("compiler/runtime/alloc");
        for strategy in ["full", "minimal"] {
            let expected = runtime_dir.join(format!("runtime_alloc_{}.c", strategy));
            assert!(
                expected.is_file(),
                "alloc runtime file missing for strategy `{}`: expected at {}",
                strategy,
                expected.display()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Actor-strategy runtime selection (#334)
    // -----------------------------------------------------------------------

    #[test]
    fn detect_actor_strategy_none_for_pure_arithmetic() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(&source_path, "fn main() -> Int:\n    ret 0\n").unwrap();
        assert_eq!(detect_actor_strategy(&source_path), "none");
    }

    #[test]
    fn detect_actor_strategy_full_when_actor_declared() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn launch(n: Int) -> !{Actor} ():\n    ret ()\n",
        )
        .unwrap();
        assert_eq!(detect_actor_strategy(&source_path), "full");
    }

    #[test]
    fn detect_actor_strategy_none_when_only_io_declared() {
        // IO is actor-free; actor strategy stays none.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn shout(n: Int) -> !{IO} ():\n    print_int(n)\n",
        )
        .unwrap();
        assert_eq!(detect_actor_strategy(&source_path), "none");
    }

    #[test]
    fn detect_actor_strategy_none_when_only_heap_declared() {
        // Heap is orthogonal to Actor — heap-using programs may still
        // be actor-free. Pin against accidental promotion.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn make(n: Int) -> !{Heap} String:\n    int_to_string(n)\n",
        )
        .unwrap();
        assert_eq!(detect_actor_strategy(&source_path), "none");
    }

    #[test]
    fn detect_actor_strategy_falls_back_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist.gr");
        // Falling back to `full` is the safe default — links the actor
        // helpers in even though stage 1 will surface the I/O error.
        assert_eq!(detect_actor_strategy(&nonexistent), "full");
    }

    #[test]
    fn find_actor_runtime_source_returns_existing_path() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        for strategy in ["full", "none"] {
            let found = find_actor_runtime_source(&fake_compiler, strategy);
            assert!(
                found.is_some(),
                "runtime source for `{}` strategy should resolve from compiler-relative path; \
                 missing under codebase/compiler/runtime/actor/runtime_actor_{}.c",
                strategy,
                strategy
            );
            let path = found.unwrap();
            assert!(path.is_file());
            assert!(path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s == format!("runtime_actor_{}.c", strategy))
                .unwrap_or(false));
        }
    }

    #[test]
    fn find_actor_runtime_source_returns_none_for_unknown_strategy() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        assert!(find_actor_runtime_source(&fake_compiler, "garbage").is_none());
    }

    #[test]
    fn actor_runtime_filenames_follow_strategy_convention() {
        // Lock the convention `runtime_actor_<strategy>.c`. Mirrors the
        // alloc-strategy convention lock above.
        let runtime_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("compiler/runtime/actor");
        for strategy in ["full", "none"] {
            let expected = runtime_dir.join(format!("runtime_actor_{}.c", strategy));
            assert!(
                expected.is_file(),
                "actor runtime file missing for strategy `{}`: expected at {}",
                strategy,
                expected.display()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Async-strategy runtime selection (#335)
    // -----------------------------------------------------------------------

    #[test]
    fn detect_async_strategy_none_for_pure_arithmetic() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(&source_path, "fn main() -> Int:\n    ret 0\n").unwrap();
        assert_eq!(detect_async_strategy(&source_path), "none");
    }

    #[test]
    fn detect_async_strategy_full_when_async_declared() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn await_thing(n: Int) -> !{Async} Int:\n    ret n\n",
        )
        .unwrap();
        assert_eq!(detect_async_strategy(&source_path), "full");
    }

    #[test]
    fn detect_async_strategy_none_when_only_io_declared() {
        // IO is async-free; async strategy stays none.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn shout(n: Int) -> !{IO} ():\n    print_int(n)\n",
        )
        .unwrap();
        assert_eq!(detect_async_strategy(&source_path), "none");
    }

    #[test]
    fn detect_async_strategy_none_when_only_actor_declared() {
        // Actor is orthogonal to Async — actor-using programs may still
        // be synchronous. Pin against accidental cross-promotion.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn launch(n: Int) -> !{Actor} ():\n    ret ()\n",
        )
        .unwrap();
        assert_eq!(detect_async_strategy(&source_path), "none");
    }

    #[test]
    fn detect_async_strategy_falls_back_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist.gr");
        // Falling back to `full` is the safe default — links the async
        // helpers in even though stage 1 will surface the I/O error.
        assert_eq!(detect_async_strategy(&nonexistent), "full");
    }

    #[test]
    fn find_async_runtime_source_returns_existing_path() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        for strategy in ["full", "none"] {
            let found = find_async_runtime_source(&fake_compiler, strategy);
            assert!(
                found.is_some(),
                "runtime source for `{}` strategy should resolve from compiler-relative path; \
                 missing under codebase/compiler/runtime/async/runtime_async_{}.c",
                strategy,
                strategy
            );
            let path = found.unwrap();
            assert!(path.is_file());
            assert!(path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s == format!("runtime_async_{}.c", strategy))
                .unwrap_or(false));
        }
    }

    #[test]
    fn find_async_runtime_source_returns_none_for_unknown_strategy() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        assert!(find_async_runtime_source(&fake_compiler, "garbage").is_none());
    }

    #[test]
    fn async_runtime_filenames_follow_strategy_convention() {
        // Lock the convention `runtime_async_<strategy>.c`. Mirrors the
        // actor-strategy convention lock above.
        let runtime_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("compiler/runtime/async");
        for strategy in ["full", "none"] {
            let expected = runtime_dir.join(format!("runtime_async_{}.c", strategy));
            assert!(
                expected.is_file(),
                "async runtime file missing for strategy `{}`: expected at {}",
                strategy,
                expected.display()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Allocator-strategy runtime selection (#336)
    //
    // Attribute-driven (NOT effect-driven). Sibling of the panic-strategy
    // detector above; distinct from the alloc/actor/async effect-driven
    // detectors immediately above. The orthogonality test below pins
    // that the Heap effect (which DOES flip alloc_strategy to "full")
    // does NOT flip allocator_strategy.
    // -----------------------------------------------------------------------

    #[test]
    fn detect_allocator_strategy_default_when_unannotated() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(&source_path, "fn main() -> Int:\n    ret 0\n").unwrap();
        assert_eq!(detect_allocator_strategy(&source_path), "default");
    }

    #[test]
    fn detect_allocator_strategy_default_when_explicitly_default() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@allocator(default)\n\nfn main() -> Int:\n    ret 0\n",
        )
        .unwrap();
        assert_eq!(detect_allocator_strategy(&source_path), "default");
    }

    #[test]
    fn detect_allocator_strategy_pluggable_when_annotated() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@allocator(pluggable)\n\nfn main() -> Int:\n    ret 0\n",
        )
        .unwrap();
        assert_eq!(detect_allocator_strategy(&source_path), "pluggable");
    }

    #[test]
    fn detect_allocator_strategy_arena_when_annotated() {
        // #320 / #336 follow-on: a third allocator variant `arena`
        // backed by a process-global bump-pointer arena. Annotated
        // modules surface as `"arena"` through the same Query API
        // field used to pick the runtime crate.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@allocator(arena)\n\nfn main() -> Int:\n    ret 0\n",
        )
        .unwrap();
        assert_eq!(detect_allocator_strategy(&source_path), "arena");
    }

    #[test]
    fn detect_allocator_strategy_slab_when_annotated() {
        // #545: a fourth allocator variant `slab` backed by a
        // fixed-size-class slab allocator. Annotated modules surface
        // as `"slab"` through the same Query API field used to pick
        // the runtime crate. Sibling pin to
        // `detect_allocator_strategy_arena_when_annotated`.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@allocator(slab)\n\nfn main() -> Int:\n    ret 0\n",
        )
        .unwrap();
        assert_eq!(detect_allocator_strategy(&source_path), "slab");
    }

    #[test]
    fn detect_allocator_strategy_bumpalo_when_annotated() {
        // #547: a fifth allocator variant `bumpalo` backed by a
        // multi-chunk bump-arena allocator. Annotated modules surface
        // as `"bumpalo"` through the same Query API field used to pick
        // the runtime crate. Sibling pin to
        // `detect_allocator_strategy_slab_when_annotated`.
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "@allocator(bumpalo)\n\nfn main() -> Int:\n    ret 0\n",
        )
        .unwrap();
        assert_eq!(detect_allocator_strategy(&source_path), "bumpalo");
    }

    #[test]
    fn detect_allocator_strategy_orthogonal_to_heap_effect() {
        // Orthogonality pin: the Heap effect flips alloc_strategy to
        // "full" (#333), but allocator_strategy is attribute-driven and
        // MUST stay at its declared value (or default if unannotated).
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("main.gr");
        std::fs::write(
            &source_path,
            "fn make(s: String) -> !{Heap} String:\n    ret s + s\n",
        )
        .unwrap();
        assert_eq!(detect_allocator_strategy(&source_path), "default");
    }

    #[test]
    fn detect_allocator_strategy_falls_back_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does_not_exist.gr");
        // Falling back to `default` is the safe default — wraps system
        // malloc which always works on host targets.
        assert_eq!(detect_allocator_strategy(&nonexistent), "default");
    }

    #[test]
    fn find_allocator_runtime_source_returns_existing_path() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        for strategy in ["default", "pluggable", "arena", "slab", "bumpalo"] {
            let found = find_allocator_runtime_source(&fake_compiler, strategy);
            assert!(
                found.is_some(),
                "runtime source for `{}` strategy should resolve from compiler-relative path; \
                 missing under codebase/compiler/runtime/allocator/runtime_allocator_{}.c",
                strategy,
                strategy
            );
            let path = found.unwrap();
            assert!(path.is_file());
            assert!(path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s == format!("runtime_allocator_{}.c", strategy))
                .unwrap_or(false));
        }
    }

    #[test]
    fn find_allocator_runtime_source_returns_none_for_unknown_strategy() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fake_compiler = manifest_dir
            .parent()
            .unwrap()
            .join("target/debug/gradient-compiler");
        assert!(find_allocator_runtime_source(&fake_compiler, "garbage").is_none());
    }

    #[test]
    fn allocator_runtime_filenames_follow_strategy_convention() {
        // Lock the convention `runtime_allocator_<strategy>.c`. Mirrors
        // the async-strategy convention lock above. Arena variant
        // (#320 / #336 follow-on) included to pin the third file.
        let runtime_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("compiler/runtime/allocator");
        for strategy in ["default", "pluggable", "arena", "slab", "bumpalo"] {
            let expected = runtime_dir.join(format!("runtime_allocator_{}.c", strategy));
            assert!(
                expected.is_file(),
                "allocator runtime file missing for strategy `{}`: expected at {}",
                strategy,
                expected.display()
            );
        }
    }
}
