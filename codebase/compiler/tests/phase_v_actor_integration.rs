//! Phase V Actor Runtime Integration Tests
//!
//! Tests for actor spawn, send, ask operations end-to-end.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;

/// Compile Gradient source and run, returning (stdout, exit_code).
fn compile_and_run(src: &str) -> (String, i32) {
    compile_and_run_with_stdin(src, None)
}

/// Compile Gradient source and run with optional stdin.
fn compile_and_run_with_stdin(src: &str, stdin_input: Option<&[u8]>) -> (String, i32) {
    let tmp = TempDir::new().expect("failed to create temp dir");

    // 1. Lex
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    // 2. Parse
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    // 3. Type check
    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(real_errors.is_empty(), "type errors: {:?}", real_errors);

    // 4. Build IR
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    assert!(ir_errors.is_empty(), "IR errors: {:?}", ir_errors);

    // 5. Codegen
    let mut cg = CraneliftCodegen::new().expect("CraneliftCodegen::new");
    cg.compile_module(&ir_module).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");

    let obj_path = tmp.path().join("output.o");
    fs::write(&obj_path, &obj_bytes).expect("write object file");

    // 6. Link with runtime
    let runtime_c =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/gradient_runtime.c");
    let bin_path = tmp.path().join("program");

    let mut link_cmd = Command::new("cc");
    link_cmd
        .arg("-o")
        .arg(&bin_path)
        .arg(&obj_path)
        .arg(&runtime_c)
        .arg("-lpthread")
        .arg("-lm")
        .arg("-lcurl");

    let link_output = link_cmd.output().expect("link command failed");
    if !link_output.status.success() {
        let stderr = String::from_utf8_lossy(&link_output.stderr);
        panic!("linking failed: {}", stderr);
    }

    // 7. Run
    let mut run_cmd = Command::new(&bin_path);
    if let Some(_input) = stdin_input {
        run_cmd.stdin(Stdio::piped());
    }

    let mut child = run_cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn program");

    if let Some(input) = stdin_input {
        let mut stdin = child.stdin.take().expect("failed to get stdin");
        stdin.write_all(input).expect("failed to write stdin");
    }

    let output = child
        .wait_with_output()
        .expect("failed to wait for program");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    (stdout, exit_code)
}

#[test]
fn actor_spawn_creates_actor() {
    let src = r#"
actor Counter:
    state count: Int = 0
    on Init:
        ret ()

fn main() -> !{Actor, IO} ():
    let c = spawn Counter
    print("spawned")
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("spawned"),
        "should have spawned actor, got: {}",
        out
    );
}

#[test]
fn actor_send_message() {
    let src = r#"
actor Counter:
    state count: Int = 0
    on Init:
        ret ()
    on Increment:
        ret ()

fn main() -> !{Actor, IO} ():
    let c = spawn Counter
    send c Increment
    print("sent")
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("sent"),
        "should have sent message, got: {}",
        out
    );
}

#[test]
fn actor_ask_returns_value() {
    let src = r#"
actor Counter:
    state count: Int = 0
    on Init:
        ret ()
    on GetCount -> Int:
        ret count

fn main() -> !{Actor, IO} ():
    let c = spawn Counter
    let n = ask c GetCount
    print_int(n)
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("0"),
        "should have gotten count 0, got: {}",
        out
    );
}

#[test]
#[ignore = "actor runtime state updates not yet implemented"]
fn actor_multiple_messages() {
    let src = r#"
actor Counter:
    state count: Int = 0
    on Init:
        ret ()
    on Increment:
        ret ()
    on GetCount -> Int:
        ret count

fn main() -> !{Actor, IO} ():
    let c = spawn Counter
    send c Increment
    send c Increment
    send c Increment
    let n = ask c GetCount
    print_int(n)
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("3"),
        "should have count 3 after 3 increments, got: {}",
        out
    );
}

#[test]
#[ignore = "actor runtime state updates not yet implemented"]
fn actor_multiple_actors() {
    let src = r#"
actor Counter:
    state count: Int = 0
    on Init:
        ret ()
    on Increment:
        ret ()
    on GetCount -> Int:
        ret count

fn main() -> !{Actor, IO} ():
    let c1 = spawn Counter
    let c2 = spawn Counter
    send c1 Increment
    send c2 Increment
    send c2 Increment
    let n1 = ask c1 GetCount
    let n2 = ask c2 GetCount
    print_int(n1)
    print_int(n2)
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("1"),
        "first actor should have count 1, got: {}",
        out
    );
    assert!(
        out.contains("2"),
        "second actor should have count 2, got: {}",
        out
    );
}

// Infix operator syntax tests

#[test]
fn actor_infix_send_operator() {
    let src = r#"
actor Counter:
    state count: Int = 0
    on Init:
        ret ()
    on Increment:
        ret ()

fn main() -> !{Actor, IO} ():
    let c = spawn Counter
    c ! Increment
    print("sent")
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("sent"),
        "should have sent message using infix ! operator, got: {}",
        out
    );
}

#[test]
fn actor_infix_ask_operator() {
    let src = r#"
actor Counter:
    state count: Int = 0
    on Init:
        ret ()
    on GetCount -> Int:
        ret count

fn main() -> !{Actor, IO} ():
    let c = spawn Counter
    let n = c ? GetCount
    print_int(n)
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("0"),
        "should have gotten count 0 using infix ? operator, got: {}",
        out
    );
}

#[test]
#[ignore = "actor runtime String type handling not yet implemented"]
fn actor_spawn_with_actor_type_annotation() {
    let src = r#"
actor Logger:
    state prefix: String = "[LOG]"
    on Log(msg: String):
        ret ()
    on GetPrefix -> String:
        ret prefix

fn main() -> !{Actor, IO} ():
    let logger: Actor[Logger] = spawn Logger
    logger ! Log("hello")
    let p = logger ? GetPrefix
    print("done")
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("done"),
        "should have spawned actor with Actor[T] type annotation, got: {}",
        out
    );
}

#[test]
#[ignore = "actor state mutation not yet implemented"]
fn actor_state_initialization() {
    let src = r#"
actor Counter:
    state count: Int = 42
    on GetCount -> Int:
        ret count

fn main() -> !{Actor, IO} ():
    let c = spawn Counter
    let n = c ? GetCount
    print_int(n)
    ret ()
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(
        code, 0,
        "exit code should be 0, got: {}, stdout: {}",
        code, out
    );
    assert!(
        out.contains("42"),
        "should have initial state of 42, got: {}",
        out
    );
}
