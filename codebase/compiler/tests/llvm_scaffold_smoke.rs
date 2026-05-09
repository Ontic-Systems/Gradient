//! LLVM backend scaffold smoke tests.
//!
//! These are the minimum tests required to demonstrate that the LLVM
//! backend (`#339`) compiles, accepts our IR, emits a valid object file,
//! and round-trips through `llc` / `clang` to a runnable binary.
//!
//! The richer companion suite lives in `llvm_backend_integration.rs`. As
//! of #339 launch tier, that suite is partially bit-rotted from the IR's
//! growth (new `Instruction` variants, builtin call lowerings the LLVM
//! backend never grew, list/string/enum machinery). This file pins the
//! subset we have actually shipped so future regressions are visible.
//!
//! # Feature gate
//!
//! These tests are only compiled when the `llvm` feature is enabled:
//!
//! ```bash
//! cargo test -p gradient-compiler --features llvm --test llvm_scaffold_smoke
//! ```

#![cfg(feature = "llvm")]

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use gradient_compiler::codegen::llvm::LlvmCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::{parser, typechecker};
use inkwell::context::Context;

/// Lex + parse + check + IR-build the source. Returns the IR module on
/// success; panics with a clear message on any earlier-stage failure so a
/// scaffold smoke can fail loudly when its fixture drifts.
fn lower_to_ir(src: &str) -> gradient_compiler::ir::Module {
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);
    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        real_errors.is_empty(),
        "type errors: {}",
        real_errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    assert!(ir_errors.is_empty(), "IR errors: {:?}", ir_errors);
    ir_module
}

/// Smallest possible test: empty `main`. Pins (a) inkwell builds against
/// LLVM 18 and (b) the backend can declare and compile a function with
/// the `main` C-ABI shape (i32 argc, i8** argv -> i32).
#[test]
fn empty_main_compiles_and_runs() {
    let src = "fn main() -> !{IO} ():\n    ()\n";
    let tmp = TempDir::new().unwrap();
    let ir = lower_to_ir(src);

    let context = Context::create();
    let mut cg = LlvmCodegen::new(&context).expect("LlvmCodegen::new");
    cg.compile_module(&ir).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");
    assert!(!obj_bytes.is_empty(), "object file should not be empty");

    let obj_path = tmp.path().join("out.o");
    let bin_path = tmp.path().join("out");
    fs::write(&obj_path, &obj_bytes).unwrap();

    let runtime_src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("gradient_runtime.c");
    let runtime_obj = tmp.path().join("gradient_runtime.o");
    let cc = Command::new("cc")
        .args(["-c"])
        .arg(&runtime_src)
        .arg("-o")
        .arg(&runtime_obj)
        .status()
        .expect("cc compile runtime");
    assert!(cc.success(), "runtime compile failed: {:?}", cc);

    let link = Command::new("cc")
        .arg(&obj_path)
        .arg(&runtime_obj)
        .arg("-o")
        .arg(&bin_path)
        .arg("-lcurl")
        .status()
        .expect("cc link");
    assert!(link.success(), "link failed: {:?}", link);

    let run = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");
    assert!(run.status.success(), "binary exited non-zero: {:?}", run);
}

/// Verify the LLVM module text produced for a recursive `factorial`
/// function contains the expected structure: the function definition,
/// at least one call to itself, and a multiplication.
///
/// This is the "factorial.gr produces correct LLVM IR" half of
/// acceptance bullet 2 on issue #339. A user-facing builtin (`print_int`)
/// is intentionally NOT used here because the LLVM backend's builtin
/// lowering is incomplete (tracked separately).
#[test]
fn factorial_emits_recursive_llvm_ir() {
    let src = "\
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

fn main() -> !{IO} ():
    ()
";
    let ir = lower_to_ir(src);
    let context = Context::create();
    let mut cg = LlvmCodegen::new(&context).expect("LlvmCodegen::new");
    cg.compile_module(&ir).expect("compile_module");

    let llvm_text = cg.print_to_string_for_test();
    // Function definition for factorial.
    assert!(
        llvm_text.contains("define ") && llvm_text.contains("@factorial"),
        "expected `define ... @factorial` in emitted IR:\n{}",
        llvm_text
    );
    // Self-recursive call.
    assert!(
        llvm_text.contains("call ") && llvm_text.contains("@factorial("),
        "expected `call ... @factorial(...)` in emitted IR:\n{}",
        llvm_text
    );
    // The multiply that combines `n` with the recursive result.
    assert!(
        llvm_text.contains(" mul "),
        "expected an integer multiplication instruction in emitted IR:\n{}",
        llvm_text
    );
}

/// `llc` round-trip: emit a valid LLVM IR text file for a small program
/// and assert that `llc -filetype=obj` accepts it without errors. This
/// is acceptance bullet 3 on issue #339 ("LLVM IR validated via `llc`
/// round-trip").
///
/// We deliberately use an arithmetic-only fixture so no Gradient
/// builtins are referenced — the goal is to validate the emitted IR
/// structurally, not to link a runnable binary.
#[test]
fn llc_round_trip_accepts_emitted_ir() {
    // Locate llc. CI installs the LLVM 18 toolchain at /usr/lib/llvm-18.
    // Locally the developer typically has `llc-18` on PATH.
    let llc = which_llc().expect(
        "neither `llc` nor `llc-18` found on PATH; install LLVM 18 toolchain to run this test",
    );

    let src = "\
fn add(x: Int, y: Int) -> Int:
    ret x + y

fn main() -> !{IO} ():
    ()
";
    let ir = lower_to_ir(src);
    let context = Context::create();
    let mut cg = LlvmCodegen::new(&context).expect("LlvmCodegen::new");
    cg.compile_module(&ir).expect("compile_module");

    let llvm_text = cg.print_to_string_for_test();
    let tmp = TempDir::new().unwrap();
    let ll_path = tmp.path().join("scaffold.ll");
    let obj_path = tmp.path().join("scaffold.o");
    fs::write(&ll_path, &llvm_text).expect("write .ll file");

    let status = Command::new(&llc)
        .arg("-filetype=obj")
        .arg(&ll_path)
        .arg("-o")
        .arg(&obj_path)
        .status()
        .expect("spawn llc");
    assert!(
        status.success(),
        "llc rejected emitted LLVM IR (path={}, status={:?}). \
         IR was:\n{}",
        ll_path.display(),
        status,
        llvm_text
    );

    let meta = fs::metadata(&obj_path).expect("llc produced an object file");
    assert!(meta.len() > 0, "llc produced an empty object file");
}

/// Locate an `llc` binary on PATH. Tries `llc-18` first (Ubuntu/Debian
/// LLVM 18 toolchain), falls back to bare `llc`. Returns `None` if
/// neither exists.
fn which_llc() -> Option<std::path::PathBuf> {
    for name in ["llc-18", "llc"] {
        let probe = Command::new(name).arg("--version").output();
        if let Ok(out) = probe {
            if out.status.success() {
                return Some(std::path::PathBuf::from(name));
            }
        }
    }
    None
}
