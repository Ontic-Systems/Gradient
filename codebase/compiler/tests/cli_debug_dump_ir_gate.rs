//! Integration test for issue #398: per-function DEBUG eprintln in
//! `CraneliftCodegen::compile_function` must be gated behind
//! `GRADIENT_DUMP_IR` (and only fire in debug builds).
//!
//! Pre-fix symptom: stderr was flooded with `DEBUG: Compiling function ...`
//! noise on every invocation, including `--asm`.

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

/// Default invocation must NOT emit the per-function DEBUG eprintln noise.
#[test]
fn default_compile_emits_no_debug_compiling_function_noise() {
    let path = write_tmp("gradient_dbg_gate_default.gr", SAMPLE);

    let output = gradient_bin()
        .arg(path.to_str().unwrap())
        .arg("--asm")
        .env_remove("GRADIENT_DUMP_IR")
        .output()
        .expect("Failed to run gradient --asm");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("DEBUG: Compiling function"),
        "stderr must NOT contain DEBUG: Compiling function noise when \
         GRADIENT_DUMP_IR is unset. Got stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("Block 0:"),
        "stderr must NOT contain per-block instruction dumps when \
         GRADIENT_DUMP_IR is unset. Got stderr:\n{}",
        stderr
    );

    let _ = std::fs::remove_file(&path);
}

/// When `GRADIENT_DUMP_IR=1` AND we are running a debug build (cargo test
/// uses dev profile by default), the gated DEBUG block still fires.
///
/// Skipped in release builds because the entire block is `#[cfg(debug_assertions)]`.
#[test]
#[cfg(debug_assertions)]
fn dump_ir_env_reactivates_debug_compiling_function() {
    let path = write_tmp("gradient_dbg_gate_enabled.gr", SAMPLE);

    let output = gradient_bin()
        .arg(path.to_str().unwrap())
        .arg("--asm")
        .env("GRADIENT_DUMP_IR", "1")
        .output()
        .expect("Failed to run gradient --asm with GRADIENT_DUMP_IR=1");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DEBUG: Compiling function"),
        "stderr SHOULD contain DEBUG: Compiling function when \
         GRADIENT_DUMP_IR=1 in a debug build. Got stderr:\n{}",
        stderr
    );

    let _ = std::fs::remove_file(&path);
}
