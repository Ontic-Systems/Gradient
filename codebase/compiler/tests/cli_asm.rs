//! Integration tests for the `--asm` CLI subcommand (E11 #373).
//!
//! Verifies that `gradient <file.gr> --asm` dumps human-readable Cranelift IR
//! (CLIF) for each compiled function, with optional `--function <name>`
//! filtering. Object-file emission is skipped.

use std::io::Write;
use std::process::Command;

fn gradient_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gradient-compiler"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd
}

fn write_tmp(name: &str, src: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(name);
    let mut f = std::fs::File::create(&path).expect("create tmp .gr");
    f.write_all(src.as_bytes()).expect("write tmp .gr");
    path
}

const SAMPLE: &str = "fn add(a: Int, b: Int) -> Int:\n    a + b\n\nfn main() -> Int:\n    add(2, 3)\n";

#[test]
fn asm_dumps_clif_for_all_functions() {
    let path = write_tmp("gradient_asm_test_all.gr", SAMPLE);

    let output = gradient_bin()
        .arg(path.to_str().unwrap())
        .arg("--asm")
        .output()
        .expect("Failed to run gradient --asm");

    assert!(
        output.status.success(),
        "--asm should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("=== Cranelift IR for 'add' ==="),
        "stdout should label the 'add' function. Got: {}",
        stdout
    );
    assert!(
        stdout.contains("=== Cranelift IR for 'main' ==="),
        "stdout should label the 'main' function. Got: {}",
        stdout
    );
    // CLIF for `add` should contain an iadd instruction.
    assert!(
        stdout.contains("iadd"),
        "CLIF for 'add' should contain iadd. Got: {}",
        stdout
    );
    // No pipeline progress prints should leak into stdout (would interleave
    // with the structured CLIF output).
    assert!(
        !stdout.contains("[1/7]") && !stdout.contains("[5/7]"),
        "Progress prints must be suppressed in --asm mode. Got: {}",
        stdout
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn asm_function_filter_selects_single_function() {
    let path = write_tmp("gradient_asm_test_filter.gr", SAMPLE);

    let output = gradient_bin()
        .arg(path.to_str().unwrap())
        .arg("--asm")
        .arg("--function")
        .arg("add")
        .output()
        .expect("Failed to run gradient --asm --function add");

    assert!(
        output.status.success(),
        "--asm --function add should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("=== Cranelift IR for 'add' ==="),
        "stdout should label 'add'. Got: {}",
        stdout
    );
    assert!(
        !stdout.contains("=== Cranelift IR for 'main' ==="),
        "stdout must NOT include 'main' when --function add filters. Got: {}",
        stdout
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn asm_function_filter_unknown_name_exits_nonzero() {
    let path = write_tmp("gradient_asm_test_unknown.gr", SAMPLE);

    let output = gradient_bin()
        .arg(path.to_str().unwrap())
        .arg("--asm")
        .arg("--function")
        .arg("does_not_exist")
        .output()
        .expect("Failed to run gradient --asm --function does_not_exist");

    assert!(
        !output.status.success(),
        "--asm --function <unknown> should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no function named 'does_not_exist'"),
        "stderr should explain missing function. Got: {}",
        stderr
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn help_mentions_asm_flag() {
    let output = gradient_bin()
        .arg("--help")
        .output()
        .expect("Failed to run gradient --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--asm"),
        "Help should mention --asm flag. Got: {}",
        stdout
    );
}
