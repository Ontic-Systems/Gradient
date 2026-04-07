//! Test for issue #52: match arms with bare variants cannot have multi-statement bodies

use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;

#[test]
fn match_arm_bare_variant_multi_statement_body_simple() {
    // Minimal reproduction based on the working test
    let src = r#"fn f(c: Color):
    match c:
        Red:
            ret 0
        Green:
            ret 1
"#;

    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (_ast, errors) = parser::parse(tokens, 0);

    // Should parse without errors
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
}

#[test]
fn match_arm_bare_variant_multi_statement_body_with_effect() {
    // Same but with effect annotation
    let src = r#"fn f(c: Color) -> !{IO} ():
    match c:
        Red:
            ret ()
        Green:
            ret ()
"#;

    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (_ast, errors) = parser::parse(tokens, 0);

    // Should parse without errors
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
}

#[test]
fn match_arm_bare_variant_multi_statement_body_with_println() {
    // With println call - this was the failing case
    let src = r#"fn f(c: Color) -> !{IO} ():
    match c:
        Red:
            println("hi")
            ret ()
        Green:
            ret ()
"#;

    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (_ast, errors) = parser::parse(tokens, 0);

    // Should parse without errors
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
}

#[test]
fn match_arm_bare_variant_multi_statement_body_original_issue() {
    // Original issue example
    let src = r#"type Cmd = Quit | Step

fn handle(c: Cmd) -> !{IO} ():
    match c:
        Quit:
            println("bye")
            ret ()
        Step:
            ret ()
"#;

    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (_ast, errors) = parser::parse(tokens, 0);

    // Should parse without errors
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
}
