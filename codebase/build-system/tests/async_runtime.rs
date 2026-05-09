//! End-to-end async-strategy build integration tests (E5 #335).
//!
//! Sibling of `tests/actor_runtime.rs`, `tests/alloc_runtime.rs`, and
//! `tests/panic_runtime.rs`. These tests invoke the real `gradient`
//! binary against a tempdir Gradient project and verify:
//!
//!   1. `gradient build --verbose` reports the correct strategy
//!      (`full` when `Async` is reachable, `none` otherwise).
//!   2. The matching async-strategy runtime object
//!      (`runtime_async_<strategy>.o`) is compiled into the target dir.
//!   3. The OTHER strategy's object file is NOT present (single-strategy
//!      contract — exactly one async runtime per build).
//!   4. The resulting binary runs and exits successfully.
//!   5. The introspectable tag symbol `__gradient_async_strategy` is
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
fn build_none_async_for_pure_arithmetic() {
    // No `Async` effect anywhere -> async_strategy = none.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "async_none_demo",
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
        stdout.contains("Async strategy: none"),
        "expected verbose async strategy line `none`; stdout={stdout}"
    );
    let async_obj = dir.path().join("target/debug/runtime_async_none.o");
    assert!(
        async_obj.is_file(),
        "expected runtime_async_none.o at {}; stderr={stderr}",
        async_obj.display()
    );
    // The other strategy must NOT be linked into a fresh build.
    let unexpected = dir.path().join("target/debug/runtime_async_full.o");
    assert!(
        !unexpected.is_file(),
        "did NOT expect runtime_async_full.o for an async-free program at {}",
        unexpected.display()
    );

    let bin = dir.path().join("target/debug/async_none_demo");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    let run_status = Command::new(&bin).status().expect("run binary");
    assert!(
        run_status.success(),
        "async-free program should run cleanly with none async strategy"
    );
}

#[test]
fn build_full_async_when_async_declared() {
    // Declare `!{Async}` so the async strategy flips to `full`.
    // The fixture function exists but is never called from main —
    // declaring the effect in any reachable symbol is sufficient to
    // promote the program to async_strategy = full because the Query
    // API scans every symbol's effect row.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "async_full_demo",
        r#"fn await_thing(n: Int) -> !{Async} Int:
    ret n

fn main() -> !{IO} ():
    print_int(await_proxy(7))

fn await_proxy(n: Int) -> Int:
    ret n + n
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
        stdout.contains("Async strategy: full"),
        "expected verbose async strategy line `full`; stdout={stdout}"
    );
    let async_obj = dir.path().join("target/debug/runtime_async_full.o");
    assert!(
        async_obj.is_file(),
        "expected runtime_async_full.o at {}; stderr={stderr}",
        async_obj.display()
    );
    // Inverse contract: none must NOT be present.
    let unexpected = dir.path().join("target/debug/runtime_async_none.o");
    assert!(
        !unexpected.is_file(),
        "did NOT expect runtime_async_none.o for an async-using program at {}",
        unexpected.display()
    );

    let bin = dir.path().join("target/debug/async_full_demo");
    assert!(bin.is_file(), "expected output binary at {}", bin.display());
    let run_status = Command::new(&bin).status().expect("run binary");
    assert!(
        run_status.success(),
        "async-using program should run cleanly with full async strategy"
    );
}

#[test]
fn build_reports_binary_size_with_async_in_verbose() {
    // The binary-size delta is the long-term win of the async-strategy
    // split. Today the delta is small (just one tag symbol), but the
    // verbose-mode reporting should be wired so future PRs that move
    // async-executor machinery into `runtime_async_full.c` can be
    // measured against today's baseline.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "async_size_demo",
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
            && stdout.contains("async=none"),
        "expected verbose binary-size report with alloc/panic/actor/async tags; stdout={stdout}"
    );
}

#[test]
fn build_links_async_strategy_tag_into_binary() {
    // Sanity: the generated runtime_async_<strategy>.o object must
    // contain the literal tag string `full` or `none`. We just scan
    // the bytes — no `nm`/`strings` shell-out required.
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        "async_tag_demo",
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
    let async_obj = dir.path().join("target/debug/runtime_async_none.o");
    let bytes = fs::read(&async_obj).expect("read async runtime object");
    // The tag string is a NUL-terminated C array initialiser, so the
    // byte sequence "none\0" must appear verbatim in the object.
    let needle = b"none\0";
    let found = bytes.windows(needle.len()).any(|w| w == needle);
    assert!(
        found,
        "tag string `none\\0` not found in runtime_async_none.o; \
         contents looked like {} bytes",
        bytes.len()
    );
}

#[test]
fn async_strategy_object_filename_locks_strategy_in_path() {
    // Defense-in-depth mirror of the actor-strategy filename test:
    // exactly one `runtime_async_<strategy>.o` per build, named after
    // the strategy.
    for (source, strategy) in [
        (
            r#"fn main() -> !{IO} ():
    print_int(0)
"#,
            "none",
        ),
        (
            r#"fn await_thing(n: Int) -> !{Async} Int:
    ret n

fn main() -> !{IO} ():
    print_int(0)
"#,
            "full",
        ),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        write_project(dir.path(), "async_filename_demo", source);
        let out = run_build(dir.path());
        assert!(
            out.status.success(),
            "build for `{strategy}` failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let expected = dir
            .path()
            .join(format!("target/debug/runtime_async_{}.o", strategy));
        assert!(
            expected.is_file(),
            "expected {} after building async-{} program",
            expected.display(),
            if strategy == "none" { "free" } else { "using" }
        );
        // The other strategy's object must NOT be present.
        let other = if strategy == "none" { "full" } else { "none" };
        let unexpected = dir
            .path()
            .join(format!("target/debug/runtime_async_{}.o", other));
        assert!(
            !unexpected.is_file(),
            "did NOT expect runtime_async_{}.o for a `{}` build at {}",
            other,
            strategy,
            unexpected.display()
        );
    }
}
