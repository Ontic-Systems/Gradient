//! End-to-end plugin dispatch integration tests (E11 #377).
//!
//! These tests build the `gradient` binary via `CARGO_BIN_EXE_gradient`,
//! drop a synthetic `gradient-hello` plugin into a tempdir, prepend
//! that tempdir to `PATH`, and assert that `gradient hello` exec'd the
//! plugin and forwarded args + protocol-version env.

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn gradient_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gradient"))
}

/// Drop a `gradient-<name>` shell-script plugin into `dir`. The
/// script echoes args + selected env vars so tests can assert.
fn write_plugin(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(format!("gradient-{name}"));
    fs::write(&path, body).expect("write plugin");
    let mut perms = fs::metadata(&path).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
    path
}

/// Prepend `dir` to the inherited `PATH` for use as a child env.
fn path_with(dir: &std::path::Path) -> OsString {
    let mut out = OsString::from(dir);
    if let Some(existing) = std::env::var_os("PATH") {
        out.push(":");
        out.push(existing);
    }
    out
}

#[test]
fn dispatch_runs_plugin_on_path_and_forwards_args() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_plugin(
        dir.path(),
        "hello",
        "#!/bin/sh\necho \"plugin saw args: $*\"\nexit 0\n",
    );

    let output = Command::new(gradient_bin())
        .args(["hello", "alpha", "beta"])
        .env("PATH", path_with(dir.path()))
        .output()
        .expect("run gradient");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("plugin saw args: alpha beta"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn dispatch_propagates_plugin_exit_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_plugin(dir.path(), "hello", "#!/bin/sh\nexit 42\n");

    let status = Command::new(gradient_bin())
        .arg("hello")
        .env("PATH", path_with(dir.path()))
        .status()
        .expect("run gradient");
    assert_eq!(status.code(), Some(42));
}

#[test]
fn dispatch_sets_protocol_version_env() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_plugin(
        dir.path(),
        "hello",
        "#!/bin/sh\necho \"PROTO=$GRADIENT_PLUGIN_PROTOCOL_VERSION\"\necho \"VERSION=$GRADIENT_VERSION\"\nexit 0\n",
    );

    let output = Command::new(gradient_bin())
        .arg("hello")
        .env("PATH", path_with(dir.path()))
        .output()
        .expect("run gradient");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PROTO=1"), "stdout: {stdout}");
    assert!(stdout.contains("VERSION="), "stdout: {stdout}");
}

#[test]
fn dispatch_sets_gradient_bin_env() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_plugin(
        dir.path(),
        "hello",
        "#!/bin/sh\necho \"BIN=$GRADIENT_BIN\"\nexit 0\n",
    );

    let output = Command::new(gradient_bin())
        .arg("hello")
        .env("PATH", path_with(dir.path()))
        .output()
        .expect("run gradient");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("BIN="), "stdout: {stdout}");
    // Best-effort path; just assert it exists in the output and is non-empty.
    let line = stdout
        .lines()
        .find(|l| l.starts_with("BIN="))
        .expect("BIN= line");
    assert!(line.len() > "BIN=".len(), "GRADIENT_BIN should be set");
}

#[test]
fn dispatch_sets_project_root_when_in_project() {
    let dir = tempfile::tempdir().expect("plugin tempdir");
    write_plugin(
        dir.path(),
        "hello",
        "#!/bin/sh\necho \"ROOT=$GRADIENT_PROJECT_ROOT\"\nexit 0\n",
    );

    let project = tempfile::tempdir().expect("project tempdir");
    fs::write(
        project.path().join("gradient.toml"),
        "[package]\nname = \"plugin-test-project\"\nversion = \"0.0.1\"\n",
    )
    .expect("write gradient.toml");

    let output = Command::new(gradient_bin())
        .arg("hello")
        .current_dir(project.path())
        .env("PATH", path_with(dir.path()))
        .output()
        .expect("run gradient");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("ROOT="))
        .expect("ROOT= line");
    let project_canon =
        std::fs::canonicalize(project.path()).unwrap_or_else(|_| project.path().to_path_buf());
    let value = line.strip_prefix("ROOT=").expect("ROOT= prefix");
    let value_canon = std::fs::canonicalize(value).unwrap_or_else(|_| PathBuf::from(value));
    assert_eq!(
        value_canon, project_canon,
        "GRADIENT_PROJECT_ROOT mismatch (raw line: {line})"
    );
}

#[test]
fn builtin_subcommand_shadows_same_named_plugin() {
    // A `gradient-build` plugin on PATH must NOT pre-empt the
    // built-in `build` subcommand.
    let dir = tempfile::tempdir().expect("tempdir");
    write_plugin(
        dir.path(),
        "build",
        "#!/bin/sh\necho 'IF YOU SEE THIS THE PLUGIN STOLE A BUILTIN'\nexit 99\n",
    );

    // Run with `--help` so no real build is attempted; we just check
    // that the plugin's stdout does NOT appear (built-in handler won
    // dispatch).
    let output = Command::new(gradient_bin())
        .args(["build", "--help"])
        .env("PATH", path_with(dir.path()))
        .output()
        .expect("run gradient");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("PLUGIN STOLE A BUILTIN"),
        "plugin pre-empted built-in: {combined}"
    );
    // build --help is the clap-generated help and should mention the flag.
    assert!(
        combined.contains("--release") || combined.contains("Build"),
        "expected built-in build help, got: {combined}"
    );
}

#[test]
fn unknown_subcommand_falls_through_to_clap() {
    // No plugin on PATH for "doesnotexist" — clap should produce its
    // own error.
    let output = Command::new(gradient_bin())
        .arg("doesnotexist")
        // Use a clean PATH so no leftover plugin from prior tests can match.
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run gradient");
    assert!(!output.status.success(), "expected non-zero exit");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("doesnotexist")
            || combined.contains("unrecognized")
            || combined.contains("error"),
        "expected clap error, got: {combined}"
    );
}
