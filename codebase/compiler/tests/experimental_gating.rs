//! Integration tests for experimental feature gating.
//!
//! These tests verify that experimental features are properly gated behind
//! the --experimental flag.

use std::process::Command;

fn gradient_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gradient-compiler"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd
}

#[test]
fn test_help_shows_experimental_commands() {
    let output = gradient_bin()
        .arg("--help")
        .output()
        .expect("Failed to run gradient --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Help should succeed
    assert!(
        output.status.success(),
        "Help should exit 0. stderr: {}",
        stderr
    );

    // Help should mention experimental commands
    assert!(
        stdout.contains("[experimental]"),
        "Help should show [experimental] tag"
    );
    assert!(stdout.contains("--repl"), "Help should mention --repl");
    assert!(stdout.contains("--fmt"), "Help should mention --fmt");
    assert!(
        stdout.contains("--target wasm32"),
        "Help should mention wasm target"
    );
    assert!(
        stdout.contains("--experimental"),
        "Help should mention --experimental flag"
    );
}

#[test]
fn test_repl_requires_experimental() {
    let output = gradient_bin()
        .arg("--repl")
        .output()
        .expect("Failed to run gradient --repl");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should fail without --experimental
    assert!(
        !output.status.success(),
        "REPL should fail without --experimental"
    );
    assert!(
        stderr.contains("experimental"),
        "Error should mention 'experimental'"
    );
    assert!(
        stderr.contains("--experimental"),
        "Error should suggest using --experimental"
    );
}

#[test]
fn test_repl_works_with_experimental() {
    // Create a simple test to verify the warning is shown
    // We can't actually run the REPL interactively, but we can check it gets past the gate
    let mut child = gradient_bin()
        .arg("--repl")
        .arg("--experimental")
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn REPL");

    // Send exit command immediately
    if let Some(ref mut stdin) = child.stdin {
        use std::io::Write;
        let _ = stdin.write_all(b":quit\n");
    }

    // Give it a moment to process
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Kill the process if it's still running
    let _ = child.kill();

    let output = child.wait_with_output().expect("Failed to get REPL output");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should have shown the warning
    assert!(
        stderr.contains("experimental"),
        "Should show experimental warning"
    );
}

#[test]
fn test_fmt_requires_experimental() {
    // Create a temporary file
    let temp_file = "test_fmt_temp.gr";
    std::fs::write(temp_file, "fn main() = 42").unwrap();

    let output = gradient_bin()
        .arg(temp_file)
        .arg("--fmt")
        .output()
        .expect("Failed to run gradient --fmt");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Cleanup
    let _ = std::fs::remove_file(temp_file);

    // Should fail without --experimental
    assert!(
        !output.status.success(),
        "fmt should fail without --experimental"
    );
    assert!(
        stderr.contains("experimental"),
        "Error should mention 'experimental'"
    );
}

#[test]
fn test_wasm_target_requires_experimental() {
    let output = gradient_bin()
        .arg("tests/hello_e2e.gr")
        .arg("/tmp/test_out.wasm")
        .output()
        .expect("Failed to run gradient with wasm output");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should fail without --experimental
    assert!(
        !output.status.success(),
        "WASM target should fail without --experimental"
    );
    assert!(
        stderr.contains("experimental"),
        "Error should mention 'experimental': {}",
        stderr
    );
}

#[test]
fn test_native_build_works_without_experimental() {
    let output = gradient_bin()
        .arg("tests/hello_e2e.gr")
        .arg("/tmp/test_native.o")
        .output()
        .expect("Failed to run gradient native build");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should succeed without --experimental
    assert!(
        output.status.success(),
        "Native build should work without --experimental: {}",
        stderr
    );
}

#[test]
fn test_check_works_without_experimental() {
    let output = gradient_bin()
        .arg("tests/hello_e2e.gr")
        .arg("--check")
        .output()
        .expect("Failed to run gradient --check");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should succeed without --experimental
    assert!(
        output.status.success(),
        "Check should work without --experimental: {}",
        stderr
    );
}
