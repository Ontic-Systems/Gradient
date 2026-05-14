//! End-to-end manifest effect/capability ceiling tests (#366).
//!
//! These invoke the real `gradient build` binary against a temporary project
//! and verify the package manifest's declared surface bounds the code's effect
//! surface before linking runtime helpers.

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn gradient_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gradient"))
}

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

fn write_project(dir: &Path, manifest_package: &str, source: &str) {
    fs::write(
        dir.join("gradient.toml"),
        format!("[package]\n{manifest_package}\n"),
    )
    .expect("write manifest");
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).expect("mkdir src");
    fs::write(src_dir.join("main.gr"), source).expect("write main.gr");
}

fn run_build(project_root: &Path) -> Output {
    let compiler = compiler_bin().expect(
        "gradient-compiler binary not found — build the workspace first \
         (cargo build -p gradient-compiler) or run via cargo test --workspace",
    );
    let env_path = std::env::var_os("PATH").unwrap_or_else(|| OsString::from(""));
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
fn build_fails_when_manifest_effects_omit_reachable_effect() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        r#"name = "manifest_ceiling_fail"
version = "0.1.0"
effects = []"#,
        r#"fn main() -> !{IO} ():
    print_int(42)
"#,
    );

    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "build unexpectedly succeeded; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stderr.contains("manifest effect/capability ceiling violation"),
        "expected manifest ceiling error; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stderr.contains("main") && stderr.contains("IO") && stderr.contains("gradient.toml"),
        "expected function/effect/manifest details; stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn build_reports_offending_non_main_function() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        r#"name = "manifest_every_fn_fail"
version = "0.1.0"
effects = ["IO"]"#,
        r#"fn helper() -> !{Heap} ():
    ()

fn main() -> !{IO} ():
    print_int(1)
"#,
    );

    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "build unexpectedly succeeded; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stderr.contains("function `helper` uses `Heap`"),
        "expected helper/Heap violation; stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn build_accepts_effects_declared_by_manifest_capabilities() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        dir.path(),
        r#"name = "manifest_capability_pass"
version = "0.1.0"
capabilities = ["IO"]"#,
        r#"fn main() -> !{IO} ():
    print_int(7)
"#,
    );

    let out = run_build(dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "build failed; stdout={stdout}\nstderr={stderr}"
    );
}
