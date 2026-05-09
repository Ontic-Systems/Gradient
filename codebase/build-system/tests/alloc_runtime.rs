//! End-to-end alloc-strategy build integration tests (E5 #333).
//!
//! Sibling of `tests/panic_runtime.rs`. These tests invoke the real
//! `gradient` binary against a tempdir Gradient project and verify:
//!
//!   1. `gradient build --verbose` reports the correct strategy
//!      (`full` when `Heap` is reachable, `minimal` otherwise).
//!   2. The matching alloc-strategy runtime object
//!      (`runtime_alloc_<strategy>.o`) is compiled into the target dir.
//!   3. The OTHER strategy's object file is NOT present (single-strategy
//!      contract — exactly one alloc runtime per build).
//!   4. The resulting binary runs and exits successfully.
//!   5. The introspectable tag symbol `__gradient_alloc_strategy` is
//!      linked into the binary (sanity-checked by reading the generated
//!      object's bytes — we don't shell out to `nm` since this needs to
//!      run on stripped CI runners too).
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
fn build_minimal_alloc_for_pure_arithmetic() {
    // No `Heap` effect anywhere -> alloc_strategy = minimal.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "alloc_minimal_demo",
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
        stdout.contains("Alloc strategy: minimal"),
        "expected verbose alloc strategy line `minimal`; stdout={stdout}"
    );
    let alloc_obj = dir.path().join("target/debug/runtime_alloc_minimal.o");
    assert!(
        alloc_obj.is_file(),
        "expected runtime_alloc_minimal.o at {}; stderr={stderr}",
        alloc_obj.display()
    );
    // The other strategy must NOT be linked into a fresh build.
    let unexpected = dir.path().join("target/debug/runtime_alloc_full.o");
    assert!(
        !unexpected.is_file(),
        "did NOT expect runtime_alloc_full.o for a heap-free program at {}",
        unexpected.display()
    );

    let bin = dir.path().join("target/debug/alloc_minimal_demo");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    let run_status = Command::new(&bin).status().expect("run binary");
    assert!(
        run_status.success(),
        "heap-free program should run cleanly with minimal alloc strategy"
    );
}

#[test]
fn build_full_alloc_when_heap_declared() {
    // Declare `!{Heap}` so the alloc strategy flips to `full`.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "alloc_full_demo",
        r#"fn make(n: Int) -> !{Heap} String:
    int_to_string(n)

fn main() -> !{IO, Heap} ():
    let s: String = make(42)
    print(s)
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
        stdout.contains("Alloc strategy: full"),
        "expected verbose alloc strategy line `full`; stdout={stdout}"
    );
    let alloc_obj = dir.path().join("target/debug/runtime_alloc_full.o");
    assert!(
        alloc_obj.is_file(),
        "expected runtime_alloc_full.o at {}; stderr={stderr}",
        alloc_obj.display()
    );
    // Inverse contract: minimal must NOT be present.
    let unexpected = dir.path().join("target/debug/runtime_alloc_minimal.o");
    assert!(
        !unexpected.is_file(),
        "did NOT expect runtime_alloc_minimal.o for a heap-using program at {}",
        unexpected.display()
    );

    let bin = dir.path().join("target/debug/alloc_full_demo");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    let run_status = Command::new(&bin).status().expect("run binary");
    assert!(
        run_status.success(),
        "heap-using program should run cleanly with full alloc strategy"
    );
}

#[test]
fn build_reports_binary_size_in_verbose() {
    // The binary-size delta is the long-term win of the alloc-strategy
    // split. Today the delta is small (just one tag symbol), but the
    // verbose-mode reporting should be wired so future PRs that move
    // rc/COW machinery into `runtime_alloc_full.c` can be measured.
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
            && stdout.contains("panic=unwind"),
        "expected verbose binary-size report with alloc/panic strategy tags; stdout={stdout}"
    );
}

#[test]
fn build_links_alloc_strategy_tag_into_binary() {
    // Sanity: the generated runtime_alloc_<strategy>.o object must
    // contain the literal tag string `full` or `minimal`. We just scan
    // the bytes — no `nm`/`strings` shell-out required.
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
    let alloc_obj = dir.path().join("target/debug/runtime_alloc_minimal.o");
    let bytes = fs::read(&alloc_obj).expect("read alloc runtime object");
    // The tag string is a NUL-terminated C array initialiser, so the
    // byte sequence "minimal\0" must appear verbatim in the object.
    let needle = b"minimal\0";
    let found = bytes.windows(needle.len()).any(|w| w == needle);
    assert!(
        found,
        "tag string `minimal\\0` not found in runtime_alloc_minimal.o; \
         contents looked like {} bytes",
        bytes.len()
    );
}

#[test]
fn alloc_strategy_object_filename_locks_strategy_in_path() {
    // Defense-in-depth mirror of the panic-strategy filename test:
    // exactly one `runtime_alloc_<strategy>.o` per build, named after
    // the strategy.
    for (source, strategy) in [
        (
            r#"fn main() -> !{IO} ():
    print_int(0)
"#,
            "minimal",
        ),
        (
            r#"fn make(n: Int) -> !{Heap} String:
    int_to_string(n)

fn main() -> !{IO, Heap} ():
    let s: String = make(0)
    print(s)
"#,
            "full",
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
            .join(format!("target/debug/runtime_alloc_{}.o", strategy));
        assert!(
            expected.is_file(),
            "expected {} after building heap-{} program",
            expected.display(),
            if strategy == "minimal" {
                "free"
            } else {
                "using"
            }
        );
        // The other strategy's object must NOT be present.
        let other = if strategy == "minimal" {
            "full"
        } else {
            "minimal"
        };
        let unexpected = dir
            .path()
            .join(format!("target/debug/runtime_alloc_{}.o", other));
        assert!(
            !unexpected.is_file(),
            "did NOT expect runtime_alloc_{}.o for a `{}` build at {}",
            other,
            strategy,
            unexpected.display()
        );
    }
}
