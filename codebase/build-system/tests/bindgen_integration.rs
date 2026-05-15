// Integration test for `gradient bindgen` (E3 #324 MVP).
//
// Exercises the full binary path: writes a tiny C header to a tempdir,
// runs the `gradient` binary's `bindgen` subcommand against it, and
// confirms the emitted Gradient source contains the acceptance markers
// (`@repr(C)` for structs, `!{FFI(C)}` for extern fns).
//
// A separate unit-test suite in `src/commands/bindgen.rs` covers the
// round-trip type-check guarantee via `Session::from_source`.

use std::path::PathBuf;
use std::process::Command;

fn gradient_bin() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_<name> for the same-crate binary.
    PathBuf::from(env!("CARGO_BIN_EXE_gradient"))
}

#[test]
fn bindgen_writes_to_stdout_with_repr_c_and_ffi_c() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let header_path = tempdir.path().join("tiny.h");
    let source = "\
typedef int my_pid_t;

struct Pair { int x; int y; };

int add(int a, int b);
void cleanup(void);
";
    std::fs::write(&header_path, source).expect("write header");

    let output = Command::new(gradient_bin())
        .arg("bindgen")
        .arg(&header_path)
        .output()
        .expect("run gradient bindgen");

    assert!(
        output.status.success(),
        "gradient bindgen exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Acceptance markers:
    //   * @repr(C) on emitted struct types.
    //   * !{FFI(C)} on emitted extern functions.
    //   * @extern annotation on emitted functions.
    assert!(stdout.contains("@repr(C)"), "missing @repr(C):\n{}", stdout);
    assert!(
        stdout.contains("type Pair:"),
        "missing struct decl:\n{}",
        stdout
    );
    assert!(stdout.contains("@extern"), "missing @extern:\n{}", stdout);
    assert!(
        stdout.contains("!{FFI(C)}"),
        "missing !{{FFI(C)}}:\n{}",
        stdout
    );
    assert!(
        stdout.contains("fn add(a: Int, b: Int)"),
        "missing fn add signature:\n{}",
        stdout
    );
    assert!(
        stdout.contains("type my_pid_t = Int"),
        "missing scalar typedef:\n{}",
        stdout
    );
}

#[test]
fn bindgen_writes_to_out_file_when_flag_given() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let header_path = tempdir.path().join("hdr.h");
    let out_path = tempdir.path().join("hdr.gr");
    std::fs::write(&header_path, "int foo(int x);\n").expect("write header");

    let output = Command::new(gradient_bin())
        .arg("bindgen")
        .arg(&header_path)
        .arg("--out")
        .arg(&out_path)
        .output()
        .expect("run gradient bindgen");

    assert!(
        output.status.success(),
        "non-zero exit: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Nothing should have gone to stdout in --out mode.
    assert!(
        output.stdout.is_empty(),
        "stdout should be empty in --out mode"
    );

    let written = std::fs::read_to_string(&out_path).expect("read produced file");
    assert!(written.contains("@extern"));
    assert!(written.contains("fn foo(x: Int)"));
    assert!(written.contains("!{FFI(C)}"));
}

#[test]
fn bindgen_missing_header_returns_error() {
    let output = Command::new(gradient_bin())
        .arg("bindgen")
        .arg("/nonexistent/path/to/header.h")
        .output()
        .expect("run gradient bindgen");

    assert!(!output.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to read header"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn bindgen_help_lists_subcommand() {
    let output = Command::new(gradient_bin())
        .arg("--help")
        .output()
        .expect("run gradient --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("bindgen"),
        "bindgen should be listed in top-level help:\n{}",
        stdout
    );
}
