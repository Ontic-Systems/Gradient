//! Import sandbox tests (issue #181).
//!
//! Verifies that the module resolver enforces a source-root sandbox:
//!
//! 1. `../escape.gr` — a relative import that climbs out of the source root
//!    must be rejected even though the target file exists on disk.
//! 2. Absolute path imports — `import "/etc/passwd"` style — must be denied
//!    by default (no stdlib allowlist configured).
//! 3. Symlink-out-of-root — a file that *appears* to be inside the source
//!    root but is actually a symlink whose target is outside must be
//!    rejected (canonicalize follows the link).
//!
//! The sandbox is the security boundary; the resolver's "search relative
//! to base_dir" behaviour is preserved for normal in-root imports.

use std::fs;
use std::path::PathBuf;

use gradient_compiler::resolve::ModuleResolver;

/// Build a temp project layout:
///
///   <tmp>/
///     outside/
///       escape.gr           (target of `../escape.gr`)
///     project/
///       main.gr             (entry; written by caller)
///       (other files written by caller)
///
/// Returns `(tmp, project_dir)`.
fn make_project() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    let project = tmp.path().join("project");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(&project).unwrap();
    fs::write(
        outside.join("escape.gr"),
        "mod escape\n\nfn pwned() -> Int:\n    1\n",
    )
    .unwrap();
    (tmp, project)
}

#[test]
fn rejects_parent_dir_escape_via_use() {
    // `use "../escape.gr"` should be rejected: escape.gr exists but lives
    // outside the source root (= project/).
    let (_tmp, project) = make_project();
    let entry = project.join("main.gr");
    fs::write(
        &entry,
        "mod main\n\nuse \"../escape.gr\"\n\nfn main():\n    ()\n",
    )
    .unwrap();

    let resolver = ModuleResolver::new(&entry);
    let result = resolver.resolve_all(&entry);

    assert!(
        !result.errors.is_empty(),
        "expected resolution error for ../ escape, got none"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("cannot resolve import")),
        "expected `cannot resolve import` error, got: {:?}",
        result.errors
    );
    // The escape module must NOT have been loaded.
    assert!(
        !result.modules.contains_key("escape"),
        "escape module should not have been loaded; modules: {:?}",
        result.modules.keys().collect::<Vec<_>>()
    );
}

#[test]
fn rejects_parent_dir_escape_via_import_statement() {
    // Same check, but for the `import "..."` top-level statement form.
    let (_tmp, project) = make_project();
    let entry = project.join("main.gr");
    fs::write(
        &entry,
        "mod main\n\nimport \"../escape.gr\"\n\nfn main():\n    ()\n",
    )
    .unwrap();

    let resolver = ModuleResolver::new(&entry);
    let result = resolver.resolve_all(&entry);

    assert!(
        !result.errors.is_empty(),
        "expected resolution error for `import \"../escape.gr\"`"
    );
    assert!(
        !result.modules.contains_key("escape"),
        "escape module should not have been loaded"
    );
}

#[test]
fn rejects_absolute_path_import_outside_root() {
    // An absolute path that points outside the source root must be rejected
    // even though the file exists. The default policy denies absolute
    // imports unless they resolve under an allowlisted stdlib root.
    let (_tmp, project) = make_project();
    // The "outside" file lives at <tmp>/outside/escape.gr. Its absolute
    // path is by construction not under the source root <tmp>/project.
    let outside_abs = project
        .parent()
        .unwrap()
        .join("outside")
        .join("escape.gr")
        .canonicalize()
        .unwrap();

    let entry = project.join("main.gr");
    fs::write(
        &entry,
        format!(
            "mod main\n\nimport \"{}\"\n\nfn main():\n    ()\n",
            outside_abs.display()
        ),
    )
    .unwrap();

    let resolver = ModuleResolver::new(&entry);
    let result = resolver.resolve_all(&entry);

    assert!(
        !result.errors.is_empty(),
        "expected resolution error for absolute import outside root, got none"
    );
    assert!(
        !result.modules.contains_key("escape"),
        "escape module must not be loaded via absolute path; modules: {:?}",
        result.modules.keys().collect::<Vec<_>>()
    );
}

#[test]
fn rejects_symlink_pointing_outside_root() {
    // Layout:
    //   <tmp>/outside/secret.gr   (real file outside the project)
    //   <tmp>/project/main.gr
    //   <tmp>/project/secret.gr   -> symlink -> ../outside/secret.gr
    //
    // The path `./secret.gr` looks innocuous but canonicalize() follows the
    // symlink, yielding <tmp>/outside/secret.gr, which is outside the
    // source root. The resolver must reject it.
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    let project = tmp.path().join("project");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(&project).unwrap();

    let real = outside.join("secret.gr");
    fs::write(&real, "mod secret\n\nfn leak() -> Int:\n    42\n").unwrap();

    let link = project.join("secret.gr");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real, &link).unwrap();
    }
    #[cfg(not(unix))]
    {
        // On platforms without unix-style symlinks, skip the test rather
        // than fail. The sandbox still protects against `..` and absolute
        // escapes (covered by the other tests).
        eprintln!("skipping symlink test on non-unix platform");
        return;
    }

    let entry = project.join("main.gr");
    fs::write(
        &entry,
        "mod main\n\nimport \"./secret.gr\"\n\nfn main():\n    ()\n",
    )
    .unwrap();

    let resolver = ModuleResolver::new(&entry);
    let result = resolver.resolve_all(&entry);

    assert!(
        !result.errors.is_empty(),
        "expected resolution error for symlink-out-of-root, got none"
    );
    assert!(
        !result.modules.contains_key("secret"),
        "secret module must not be loaded via symlink escape; modules: {:?}",
        result.modules.keys().collect::<Vec<_>>()
    );
}

#[test]
fn allows_in_root_relative_imports() {
    // Sanity check: the sandbox does not break legitimate same-directory
    // imports. `use sibling` must still work.
    let (_tmp, project) = make_project();
    let entry = project.join("main.gr");
    fs::write(
        &entry,
        "mod main\n\nuse sibling\n\nfn main():\n    ()\n",
    )
    .unwrap();
    fs::write(
        project.join("sibling.gr"),
        "mod sibling\n\nfn hello() -> Int:\n    1\n",
    )
    .unwrap();

    let resolver = ModuleResolver::new(&entry);
    let result = resolver.resolve_all(&entry);

    assert!(
        result.errors.is_empty(),
        "in-root import must succeed, got errors: {:?}",
        result.errors
    );
    assert!(result.modules.contains_key("main"));
    assert!(result.modules.contains_key("sibling"));
}

#[test]
fn allows_absolute_path_when_under_stdlib_root() {
    // If a stdlib root is explicitly allowlisted, absolute imports under
    // that root are permitted. This exercises the configurable allowlist
    // half of the spec.
    let tmp = tempfile::tempdir().unwrap();
    let stdlib = tmp.path().join("stdlib");
    let project = tmp.path().join("project");
    fs::create_dir_all(&stdlib).unwrap();
    fs::create_dir_all(&project).unwrap();

    fs::write(
        stdlib.join("io.gr"),
        "mod io\n\nfn print_line() -> Int:\n    0\n",
    )
    .unwrap();
    let stdlib_io_abs = stdlib.join("io.gr").canonicalize().unwrap();

    let entry = project.join("main.gr");
    fs::write(
        &entry,
        format!(
            "mod main\n\nimport \"{}\"\n\nfn main():\n    ()\n",
            stdlib_io_abs.display()
        ),
    )
    .unwrap();

    let resolver = ModuleResolver::new(&entry).allow_stdlib_root(&stdlib);
    let result = resolver.resolve_all(&entry);

    assert!(
        result.errors.is_empty(),
        "import under allowlisted stdlib root must succeed, got: {:?}",
        result.errors
    );
    assert!(
        result.modules.contains_key("io"),
        "io module should be loaded via stdlib allowlist"
    );
}

#[test]
fn with_source_root_uses_explicit_root() {
    // `with_source_root` lets callers pin a project root that is not the
    // entry file's parent directory. Imports must still be sandboxed to
    // the explicit root.
    let (_tmp, project) = make_project();
    let sub = project.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let entry = sub.join("main.gr");
    fs::write(
        &entry,
        "mod main\n\nuse \"../escape.gr\"\n\nfn main():\n    ()\n",
    )
    .unwrap();
    // Even though `../escape.gr` from sub/ resolves to project/escape.gr
    // (which doesn't exist), we want to confirm that pinning the source
    // root to `project/` does not let you climb above it. Build a file
    // at <tmp>/escape.gr (one level above project/) and confirm it stays
    // unreachable.
    let above = project.parent().unwrap().join("escape.gr");
    fs::write(&above, "mod escape\nfn x() -> Int:\n    1\n").unwrap();

    // Entry is sub/main.gr; explicit source_root is project/.
    // `../escape.gr` from sub/main.gr resolves to project/escape.gr, which
    // does not exist; the climb to <tmp>/escape.gr would require `../../`.
    // What matters is that even an explicit `../../escape.gr` cannot
    // escape the pinned root.
    fs::write(
        &entry,
        "mod main\n\nuse \"../../escape.gr\"\n\nfn main():\n    ()\n",
    )
    .unwrap();

    let resolver = ModuleResolver::with_source_root(&entry, &project);
    let result = resolver.resolve_all(&entry);

    assert!(
        !result.errors.is_empty(),
        "expected resolution error climbing above explicit source root"
    );
    assert!(
        !result.modules.contains_key("escape"),
        "escape module must not load via `../../` against pinned root"
    );
}
