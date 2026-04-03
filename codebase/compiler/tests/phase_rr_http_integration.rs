//! Integration tests for Phase RR: HTTP Client Builtins.
//!
//! These tests verify:
//! - Type checker correctly requires the Net effect for HTTP builtins
//! - Type checker correctly reports return type as Result[String, String]
//! - Full pipeline compiles HTTP calls to object code
//! - Effect enforcement: calling http_get without !{Net} is a type error

use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::codegen::CraneliftCodegen;

/// Compile a Gradient source through the full pipeline (lex, parse, typecheck,
/// IR build, Cranelift codegen) and return the object bytes.
/// Returns Err if any phase fails.
fn compile_to_object(src: &str) -> Result<Vec<u8>, String> {
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    if !parse_errors.is_empty() {
        return Err(format!("parse errors: {:?}", parse_errors));
    }

    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    if !real_errors.is_empty() {
        return Err(format!(
            "type errors: {}",
            real_errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ")
        ));
    }

    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    if !ir_errors.is_empty() {
        return Err(format!("IR errors: {:?}", ir_errors));
    }

    let mut cg = CraneliftCodegen::new().map_err(|e| format!("codegen init: {}", e))?;
    cg.compile_module(&ir_module).map_err(|e| format!("compile: {}", e))?;
    cg.emit_bytes().map_err(|e| format!("emit: {}", e))
}

// ---------------------------------------------------------------------------
// Type checker: effect enforcement
// ---------------------------------------------------------------------------

#[test]
fn http_get_requires_net_effect() {
    let src = "\
mod test
fn main() -> !{IO} ():
    let resp: Result[String, String] = http_get(\"https://example.com\")
    print(\"done\")
";
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, _) = parser::parse(tokens, 0);
    let errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        !real_errors.is_empty(),
        "expected type error for calling http_get without Net effect"
    );
    let has_net_error = real_errors.iter().any(|e| {
        let msg = e.to_string();
        msg.contains("Net") || msg.contains("effect")
    });
    assert!(
        has_net_error,
        "expected error about missing Net effect, got: {:?}",
        real_errors
    );
}

#[test]
fn http_post_requires_net_effect() {
    let src = "\
mod test
fn main() -> !{IO} ():
    let resp: Result[String, String] = http_post(\"https://example.com\", \"body\")
    print(\"done\")
";
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, _) = parser::parse(tokens, 0);
    let errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        !real_errors.is_empty(),
        "expected type error for calling http_post without Net effect"
    );
}

#[test]
fn http_post_json_requires_net_effect() {
    let src = "\
mod test
fn main() -> !{IO} ():
    let resp: Result[String, String] = http_post_json(\"https://example.com\", \"{}\")
    print(\"done\")
";
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, _) = parser::parse(tokens, 0);
    let errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        !real_errors.is_empty(),
        "expected type error for calling http_post_json without Net effect"
    );
}

// ---------------------------------------------------------------------------
// Type checker: correct effect declaration passes
// ---------------------------------------------------------------------------

#[test]
fn http_get_with_net_effect_typechecks() {
    let src = "\
mod test
fn main() -> !{IO, Net} ():
    let resp: Result[String, String] = http_get(\"https://example.com\")
    print(\"done\")
";
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, _) = parser::parse(tokens, 0);
    let errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        real_errors.is_empty(),
        "http_get with Net effect should typecheck, got: {:?}",
        real_errors
    );
}

#[test]
fn http_post_with_net_effect_typechecks() {
    let src = "\
mod test
fn main() -> !{IO, Net} ():
    let resp: Result[String, String] = http_post(\"https://example.com\", \"data\")
    print(\"done\")
";
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, _) = parser::parse(tokens, 0);
    let errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        real_errors.is_empty(),
        "http_post with Net effect should typecheck, got: {:?}",
        real_errors
    );
}

#[test]
fn http_post_json_with_net_effect_typechecks() {
    let src = "\
mod test
fn main() -> !{IO, Net} ():
    let resp: Result[String, String] = http_post_json(\"https://example.com\", \"{\\\"key\\\": \\\"value\\\"}\")
    print(\"done\")
";
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, _) = parser::parse(tokens, 0);
    let errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        real_errors.is_empty(),
        "http_post_json with Net effect should typecheck, got: {:?}",
        real_errors
    );
}

// ---------------------------------------------------------------------------
// Full pipeline: compiles through to object code
// ---------------------------------------------------------------------------

#[test]
fn http_get_compiles_to_object() {
    let src = "\
mod test
fn main() -> !{IO, Net} ():
    let resp: Result[String, String] = http_get(\"https://example.com\")
    match resp:
        Ok(body):
            print(body)
        Err(msg):
            print(msg)
";
    let result = compile_to_object(src);
    assert!(
        result.is_ok(),
        "http_get full pipeline failed: {:?}",
        result.err()
    );
}

#[test]
fn http_post_compiles_to_object() {
    let src = "\
mod test
fn main() -> !{IO, Net} ():
    let resp: Result[String, String] = http_post(\"https://httpbin.org/post\", \"hello\")
    match resp:
        Ok(body):
            print(body)
        Err(msg):
            print(msg)
";
    let result = compile_to_object(src);
    assert!(
        result.is_ok(),
        "http_post full pipeline failed: {:?}",
        result.err()
    );
}

#[test]
fn http_post_json_compiles_to_object() {
    let src = "\
mod test
fn main() -> !{IO, Net} ():
    let resp: Result[String, String] = http_post_json(\"https://httpbin.org/post\", \"{}\")
    match resp:
        Ok(body):
            print(body)
        Err(msg):
            print(msg)
";
    let result = compile_to_object(src);
    assert!(
        result.is_ok(),
        "http_post_json full pipeline failed: {:?}",
        result.err()
    );
}

// ---------------------------------------------------------------------------
// Effect propagation in helper functions
// ---------------------------------------------------------------------------

#[test]
fn http_effect_propagates_through_helper_function() {
    let src = "\
mod test
fn fetch(url: String) -> !{Net} Result[String, String]:
    http_get(url)

fn main() -> !{IO, Net} ():
    let resp: Result[String, String] = fetch(\"https://example.com\")
    match resp:
        Ok(body):
            print(body)
        Err(msg):
            print(msg)
";
    let result = compile_to_object(src);
    assert!(
        result.is_ok(),
        "http effect propagation failed: {:?}",
        result.err()
    );
}

#[test]
fn http_in_pure_helper_is_type_error() {
    let src = "\
mod test
fn fetch(url: String) -> String:
    http_get(url)

fn main() -> !{IO} ():
    print(\"done\")
";
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, _) = parser::parse(tokens, 0);
    let errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        !real_errors.is_empty(),
        "calling http_get in pure function should be a type error"
    );
}
