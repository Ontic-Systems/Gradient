//! End-to-end allocator-strategy build integration tests (E5 #336).
//!
//! Sibling of `tests/{panic,alloc,actor,async}_runtime.rs`. These tests
//! invoke the real `gradient` binary against a tempdir Gradient project
//! that declares `@allocator(...)` and verify:
//!
//!   1. `gradient build --verbose` reports the correct allocator
//!      strategy (`default` when unannotated or explicitly default,
//!      `pluggable` when explicitly annotated).
//!   2. The matching allocator-strategy runtime object
//!      (`runtime_allocator_<strategy>.o`) is compiled into the target
//!      dir.
//!   3. The OTHER strategy's object file is NOT present (single-strategy
//!      contract — exactly one allocator runtime per build).
//!   4. The resulting binary runs and exits successfully (the `default`
//!      variant ships a libc-backed body; the `pluggable` variant only
//!      defines the introspectable tag, so heap-free programs still
//!      link cleanly because the alloc/free symbol references DCE out).
//!   5. The introspectable tag symbol `__gradient_allocator_strategy`
//!      is linked into the binary (sanity-checked by reading the
//!      generated object's bytes).
//!   6. The orthogonality contract: a Heap-using program with no
//!      `@allocator(...)` attribute stays at `default` (the
//!      attribute-driven axis must NOT be flipped by the
//!      effect-driven `Heap` trigger that promotes alloc_strategy).
//!
//! Marked Unix-only because the build subprocess shells out to `cc`.

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn gradient_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gradient"))
}

/// Locate the `gradient-compiler` sibling binary so the build subprocess
/// can invoke it via the `GRADIENT_COMPILER` env var. Cargo doesn't
/// automatically expose other-crate bins via `CARGO_BIN_EXE_*`, so we
/// walk up from the current binary's parent dir (= `target/debug/deps`)
/// to `target/debug` and look for `gradient-compiler` there.
fn compiler_bin() -> Option<PathBuf> {
    let me = PathBuf::from(env!("CARGO_BIN_EXE_gradient"));
    let target_debug = me.parent()?;
    let candidate = target_debug.join("gradient-compiler");
    if candidate.is_file() {
        return Some(candidate);
    }
    let mut cur = target_debug.parent()?;
    for _ in 0..6 {
        let alt = cur.join("target").join("debug").join("gradient-compiler");
        if alt.is_file() {
            return Some(alt);
        }
        cur = cur.parent()?;
    }
    None
}

fn write_project(dir: &std::path::Path, name: &str, source: &str) {
    fs::write(
        dir.join("gradient.toml"),
        format!("[package]\nname = \"{}\"\nversion = \"0.1.0\"\n", name),
    )
    .expect("write manifest");
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).expect("mkdir src");
    fs::write(src_dir.join("main.gr"), source).expect("write main.gr");
}

fn run_build(project_root: &std::path::Path) -> std::process::Output {
    let compiler = compiler_bin().expect(
        "gradient-compiler binary not found — build the workspace first \
         (cargo build -p gradient-compiler) or run via cargo test --workspace",
    );
    let env_path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => OsString::from(""),
    };
    Command::new(gradient_bin())
        .arg("build")
        .arg("--verbose")
        .current_dir(project_root)
        .env("GRADIENT_COMPILER", &compiler)
        .env("PATH", env_path)
        .output()
        .expect("invoke gradient build")
}

#[test]
fn build_default_allocator_when_unannotated() {
    // No @allocator attribute -> allocator_strategy = default.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "alloc_default_demo",
        r#"fn main() -> !{IO} ():
    print_int(42)
"#,
    );

    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "build failed; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("Allocator strategy: @allocator(default)"),
        "expected verbose allocator strategy line `default`; stdout={stdout}"
    );
    let alloc_obj = dir.path().join("target/debug/runtime_allocator_default.o");
    assert!(
        alloc_obj.is_file(),
        "expected runtime_allocator_default.o at {}; stderr={stderr}",
        alloc_obj.display()
    );
    // The other strategy must NOT be linked into a fresh build.
    let unexpected = dir
        .path()
        .join("target/debug/runtime_allocator_pluggable.o");
    assert!(
        !unexpected.is_file(),
        "did NOT expect runtime_allocator_pluggable.o for an unannotated program at {}",
        unexpected.display()
    );

    let bin = dir.path().join("target/debug/alloc_default_demo");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    let run_status = Command::new(&bin).status().expect("run binary");
    assert!(
        run_status.success(),
        "default-allocator program should run cleanly"
    );
}

#[test]
fn build_pluggable_allocator_when_annotated() {
    // Explicit @allocator(pluggable) -> allocator_strategy = pluggable.
    // Heap-free body so the alloc/free symbol references DCE out and the
    // link succeeds without an embedder-supplied vtable.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "alloc_pluggable_demo",
        r#"@allocator(pluggable)

fn main() -> !{IO} ():
    print_int(99)
"#,
    );

    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "build failed; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("Allocator strategy: @allocator(pluggable)"),
        "expected verbose allocator strategy line `pluggable`; stdout={stdout}"
    );
    let alloc_obj = dir
        .path()
        .join("target/debug/runtime_allocator_pluggable.o");
    assert!(
        alloc_obj.is_file(),
        "expected runtime_allocator_pluggable.o at {}; stderr={stderr}",
        alloc_obj.display()
    );
    // Inverse contract: default must NOT be present.
    let unexpected = dir.path().join("target/debug/runtime_allocator_default.o");
    assert!(
        !unexpected.is_file(),
        "did NOT expect runtime_allocator_default.o for a pluggable-annotated program at {}",
        unexpected.display()
    );

    let bin = dir.path().join("target/debug/alloc_pluggable_demo");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    let run_status = Command::new(&bin).status().expect("run binary");
    assert!(
        run_status.success(),
        "pluggable-allocator heap-free program should run cleanly (alloc/free symbols DCE out)"
    );
}

#[test]
fn build_reports_binary_size_with_allocator_in_verbose() {
    // The binary-size delta is the long-term win of the allocator-strategy
    // split. Today the delta is ~50 bytes (the tag string difference);
    // future PRs that ship bumpalo/slab impls will widen it. The
    // verbose-mode reporting needs to be wired so future PRs that move
    // allocator machinery into the variant runtime can be measured
    // against today's baseline.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "alloc_size_demo",
        r#"fn main() -> !{IO} ():
    print_int(0)
"#,
    );
    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "build failed: {stdout}");
    assert!(
        stdout.contains("Binary size:")
            && stdout.contains("alloc=minimal")
            && stdout.contains("panic=unwind")
            && stdout.contains("actor=none")
            && stdout.contains("async=none")
            && stdout.contains("allocator=default"),
        "expected verbose binary-size report with alloc/panic/actor/async/allocator tags; stdout={stdout}"
    );
}

#[test]
fn build_links_allocator_strategy_tag_into_binary() {
    // Sanity: the generated runtime_allocator_<strategy>.o object must
    // contain the literal tag string `default` or `pluggable`. We just
    // scan the bytes — no `nm`/`strings` shell-out required.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "alloc_tag_demo",
        r#"fn main() -> !{IO} ():
    print_int(1)
"#,
    );
    let out = run_build(dir.path());
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let alloc_obj = dir.path().join("target/debug/runtime_allocator_default.o");
    let bytes = fs::read(&alloc_obj).expect("read allocator runtime object");
    // The tag string is a NUL-terminated C array initialiser, so the
    // byte sequence "default\0" must appear verbatim in the object.
    let needle = b"default\0";
    let found = bytes.windows(needle.len()).any(|w| w == needle);
    assert!(
        found,
        "tag string `default\\0` not found in runtime_allocator_default.o; \
         contents looked like {} bytes",
        bytes.len()
    );
}

#[test]
fn allocator_strategy_orthogonal_to_heap_effect() {
    // Defense-in-depth: the Heap effect flips alloc_strategy to "full"
    // (#333), but allocator_strategy is attribute-driven and MUST stay
    // at default when unannotated. Future refactors that conflate the
    // two axes get caught here.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "alloc_heap_demo",
        r#"fn make(s: String) -> !{Heap} String:
    ret s + s

fn main() -> !{IO, Heap} ():
    let s: String = make("hi")
    print_int(0)
"#,
    );
    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "build failed; stdout={stdout}\nstderr={stderr}"
    );
    // alloc_strategy DOES flip to full.
    assert!(
        stdout.contains("Alloc strategy: full"),
        "Heap-using program should flip alloc_strategy to full; stdout={stdout}"
    );
    // allocator_strategy stays at default.
    assert!(
        stdout.contains("Allocator strategy: @allocator(default)"),
        "allocator_strategy must remain default for unannotated Heap-using program; stdout={stdout}"
    );
    let alloc_obj = dir.path().join("target/debug/runtime_allocator_default.o");
    assert!(
        alloc_obj.is_file(),
        "expected runtime_allocator_default.o for unannotated module"
    );
    let unexpected = dir
        .path()
        .join("target/debug/runtime_allocator_pluggable.o");
    assert!(
        !unexpected.is_file(),
        "Heap effect must not flip allocator_strategy to pluggable"
    );
}

#[test]
fn allocator_strategy_object_filename_locks_strategy_in_path() {
    // Defense-in-depth mirror of the async-strategy filename test:
    // exactly one `runtime_allocator_<strategy>.o` per build, named
    // after the strategy.
    for (source, strategy) in [
        (
            r#"fn main() -> !{IO} ():
    print_int(0)
"#,
            "default",
        ),
        (
            r#"@allocator(pluggable)

fn main() -> !{IO} ():
    print_int(0)
"#,
            "pluggable",
        ),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        write_project(dir.path(), "alloc_filename_demo", source);
        let out = run_build(dir.path());
        assert!(
            out.status.success(),
            "build for `{strategy}` failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let expected = dir
            .path()
            .join(format!("target/debug/runtime_allocator_{}.o", strategy));
        assert!(
            expected.is_file(),
            "expected {} after building {} program",
            expected.display(),
            strategy
        );
        // The other strategy's object must NOT be present.
        let other = if strategy == "default" {
            "pluggable"
        } else {
            "default"
        };
        let unexpected = dir
            .path()
            .join(format!("target/debug/runtime_allocator_{}.o", other));
        assert!(
            !unexpected.is_file(),
            "did NOT expect runtime_allocator_{}.o for a `{}` build at {}",
            other,
            strategy,
            unexpected.display()
        );
    }
}
