//! End-to-end panic-strategy build integration tests (E5 #337).
//!
//! These tests invoke the real `gradient` binary against a tempdir Gradient
//! project that declares `@panic(abort|unwind|none)` and verify:
//!
//!   1. `gradient build --verbose` reports the correct strategy.
//!   2. The panic-strategy runtime object (`runtime_panic_<strategy>.o`) is
//!      compiled into the target dir.
//!   3. The resulting binary actually runs and exits successfully.
//!
//! These tests need the `gradient-compiler` binary on disk; they look for it
//! via `CARGO_BIN_EXE_gradient_compiler` (set automatically when Cargo runs
//! integration tests of the workspace), falling back to a relative
//! `target/debug/gradient-compiler` lookup. They are marked Unix-only because
//! they shell out to `cc`, mirroring the existing `tests/plugin_dispatch.rs`
//! convention.

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn gradient_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gradient"))
}

/// Locate the `gradient-compiler` sibling binary so the build subprocess can
/// invoke it via the `GRADIENT_COMPILER` env var. Cargo doesn't automatically
/// expose other-crate bins via `CARGO_BIN_EXE_*`, so we walk up from the
/// current binary's parent dir (= `target/debug/deps`) to `target/debug` and
/// look for `gradient-compiler` there.
fn compiler_bin() -> Option<PathBuf> {
    let me = PathBuf::from(env!("CARGO_BIN_EXE_gradient"));
    let target_debug = me.parent()?;
    let candidate = target_debug.join("gradient-compiler");
    if candidate.is_file() {
        return Some(candidate);
    }
    // Workspace root fallback: walk up to find `target/debug/gradient-compiler`.
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

/// Build a minimal Gradient project at `dir` with `gradient.toml`,
/// `src/main.gr`, and the supplied source body.
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

/// Run `gradient build --verbose` against `project_root` with
/// `GRADIENT_COMPILER` injected, and return the std::process::Output.
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
fn build_panic_abort_links_abort_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "panic_abort_demo",
        r#"@panic(abort)

fn main() -> !{IO} ():
    print_int(7)
"#,
    );

    let out = run_build(dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "build failed; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("Panic strategy: @panic(abort)"),
        "expected verbose strategy line for abort; stdout={stdout}"
    );
    let panic_obj = dir.path().join("target/debug/runtime_panic_abort.o");
    assert!(
        panic_obj.is_file(),
        "expected runtime_panic_abort.o at {}; stderr={stderr}",
        panic_obj.display()
    );

    // Run the binary and check exit code.
    let bin = dir.path().join("target/debug/panic_abort_demo");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    let run_status = Command::new(&bin).status().expect("run binary");
    assert!(
        run_status.success(),
        "@panic(abort) program should run cleanly when no panic-able op is hit"
    );
}

#[test]
fn build_panic_unwind_links_unwind_runtime_default() {
    // Omit the `@panic(...)` attribute entirely — should default to unwind.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "panic_unwind_default_demo",
        r#"fn main() -> !{IO} ():
    print_int(11)
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
        stdout.contains("Panic strategy: @panic(unwind)"),
        "default panic strategy should be unwind; stdout={stdout}"
    );
    let panic_obj = dir.path().join("target/debug/runtime_panic_unwind.o");
    assert!(
        panic_obj.is_file(),
        "expected runtime_panic_unwind.o at {}",
        panic_obj.display()
    );
}

#[test]
fn build_panic_none_links_none_runtime_and_runs() {
    // @panic(none) — checker rejects panic-able ops. Body avoids them.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "panic_none_demo",
        r#"@panic(none)

fn add(a: Int, b: Int) -> Int:
    ret a + b

fn main() -> !{IO} ():
    print_int(add(2, 3))
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
        stdout.contains("Panic strategy: @panic(none)"),
        "expected verbose strategy line for none; stdout={stdout}"
    );
    let panic_obj = dir.path().join("target/debug/runtime_panic_none.o");
    assert!(
        panic_obj.is_file(),
        "expected runtime_panic_none.o at {}",
        panic_obj.display()
    );

    let bin = dir.path().join("target/debug/panic_none_demo");
    assert!(bin.is_file());
    let run_status = Command::new(&bin).status().expect("run binary");
    assert!(
        run_status.success(),
        "@panic(none) program should run cleanly without invoking __gradient_panic"
    );
}

#[test]
fn build_panic_none_rejects_panic_able_division() {
    // `@panic(none)` + an integer division should be rejected by the checker.
    // The build should fail (compiler returns non-zero, build-system propagates).
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "panic_none_div_demo",
        r#"@panic(none)

fn divide(a: Int, b: Int) -> Int:
    ret a / b

fn main() -> !{IO} ():
    print_int(divide(10, 2))
"#,
    );

    let out = run_build(dir.path());
    assert!(
        !out.status.success(),
        "expected build to fail for @panic(none) + integer division; \
         stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("@panic(none)") || combined.contains("panic(none)"),
        "diagnostic should reference @panic(none); stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn panic_strategy_object_filename_locks_strategy_in_path() {
    // Defense-in-depth: the produced object file's name must literally
    // contain the strategy keyword, so debug builds of "what got linked"
    // can be eyeballed from `ls target/debug/*.o` alone.
    for (annotation, strategy) in [
        ("@panic(abort)", "abort"),
        ("@panic(unwind)", "unwind"),
        ("@panic(none)", "none"),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        write_project(
            dir.path(),
            "panic_filename_demo",
            &format!(
                "{}\n\nfn main() -> !{{IO}} ():\n    print_int(0)\n",
                annotation
            ),
        );
        let out = run_build(dir.path());
        assert!(
            out.status.success(),
            "build for {annotation} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let expected = dir
            .path()
            .join(format!("target/debug/runtime_panic_{}.o", strategy));
        assert!(
            expected.is_file(),
            "expected {} after building with {annotation}",
            expected.display()
        );
        // Make sure the OTHER two strategy objects are NOT present
        // (a fresh tempdir build with one strategy should never produce
        // multiple panic runtimes).
        for other in ["abort", "unwind", "none"] {
            if other == strategy {
                continue;
            }
            let unexpected = dir
                .path()
                .join(format!("target/debug/runtime_panic_{}.o", other));
            assert!(
                !unexpected.is_file(),
                "did NOT expect {} when building with {annotation}",
                unexpected.display()
            );
        }
    }
}
