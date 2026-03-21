//! Comprehensive tests for the Gradient type checker.
//!
//! Each test parses a Gradient source snippet through the lexer and parser,
//! then runs the type checker and asserts on the resulting errors (or lack
//! thereof).

use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::ast::module::Module;

use super::checker::check_module;
use super::error::TypeError;
use super::types::Ty;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a source string into a Module AST. Panics if there are parse errors.
fn parse_ok(src: &str) -> Module {
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (module, errors) = Parser::parse(tokens, 0);
    assert!(
        errors.is_empty(),
        "expected no parse errors, got: {:?}",
        errors
    );
    module
}

/// Parse and type-check a source string. Returns the list of type errors.
fn check(src: &str) -> Vec<TypeError> {
    let module = parse_ok(src);
    check_module(&module, 0)
}

/// Assert that the source type-checks with no errors.
fn assert_no_errors(src: &str) {
    let errors = check(src);
    assert!(
        errors.is_empty(),
        "expected no type errors, got:\n{}",
        errors
            .iter()
            .map(|e| format!("  - {}", e))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Assert that the source produces at least one type error whose message
/// contains the given substring.
fn assert_error_contains(src: &str, substring: &str) {
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains(substring)),
        "expected a type error containing {:?}, but got:\n{}",
        substring,
        if errors.is_empty() {
            "  (no errors)".to_string()
        } else {
            errors
                .iter()
                .map(|e| format!("  - {}", e))
                .collect::<Vec<_>>()
                .join("\n")
        }
    );
}

/// Assert that the source produces exactly `n` type errors.
fn assert_error_count(src: &str, n: usize) {
    let errors = check(src);
    assert_eq!(
        errors.len(),
        n,
        "expected {} type error(s), got {}:\n{}",
        n,
        errors.len(),
        errors
            .iter()
            .map(|e| format!("  - {}", e))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ---------------------------------------------------------------------------
// Basic arithmetic and return types
// ---------------------------------------------------------------------------

#[test]
fn simple_int_arithmetic_function() {
    // A simple function that does integer arithmetic and returns Int.
    let src = "\
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
    assert_no_errors(src);
}

#[test]
fn int_arithmetic_all_ops() {
    let src = "\
fn math(a: Int, b: Int) -> Int:
    let sum: Int = a + b
    let diff: Int = a - b
    let prod: Int = a * b
    let quot: Int = a / b
    let rem: Int = a % b
    ret sum
";
    assert_no_errors(src);
}

#[test]
fn float_arithmetic() {
    let src = "\
fn fmath(a: Float, b: Float) -> Float:
    ret a + b
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// String concatenation
// ---------------------------------------------------------------------------

#[test]
fn string_concatenation_valid() {
    let src = "\
fn concat(a: String, b: String) -> String:
    ret a + b
";
    assert_no_errors(src);
}

#[test]
fn string_concatenation_literals() {
    let src = "\
fn greet() -> !{IO} ():
    let greeting: String = \"Hello\" + \", \" + \"Gradient!\"
    print(greeting)
";
    assert_no_errors(src);
}

#[test]
fn string_sub_error() {
    // Subtraction is NOT defined on strings.
    let src = "\
fn bad(a: String, b: String) -> String:
    ret a - b
";
    assert_error_contains(src, "requires numeric operands");
}

// ---------------------------------------------------------------------------
// Mismatched if/else branches
// ---------------------------------------------------------------------------

#[test]
fn mismatched_if_else_branches() {
    let src = "\
fn pick(flag: Bool) -> Int:
    if flag:
        1
    else:
        true
";
    assert_error_contains(src, "all branches of `if` expression must have the same type");
}

#[test]
fn matching_if_else_branches() {
    let src = "\
fn pick(flag: Bool) -> Int:
    if flag:
        1
    else:
        2
";
    assert_no_errors(src);
}

#[test]
fn if_without_else_is_unit() {
    // An if without else produces Unit, which is fine if the result is discarded.
    let src = "\
fn maybe(flag: Bool):
    if flag:
        42
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Function calls with wrong argument types
// ---------------------------------------------------------------------------

#[test]
fn call_with_wrong_arg_type() {
    let src = "\
fn double(x: Int) -> Int:
    ret x + x

fn main():
    double(true)
";
    assert_error_contains(src, "expected `Int`, found `Bool`");
}

#[test]
fn call_with_wrong_arg_count() {
    let src = "\
fn double(x: Int) -> Int:
    ret x + x

fn main():
    double(1, 2)
";
    assert_error_contains(src, "expects 1 argument(s), but 2 were provided");
}

#[test]
fn call_correct_args() {
    let src = "\
fn double(x: Int) -> Int:
    ret x + x

fn main() -> Int:
    let result: Int = double(5)
    ret result
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Let bindings: explicit type annotation match and mismatch
// ---------------------------------------------------------------------------

#[test]
fn let_with_matching_annotation() {
    let src = "\
fn f():
    let x: Int = 42
    let y: Bool = true
    let z: String = \"hello\"
    let w: Float = 3.14
";
    assert_no_errors(src);
}

#[test]
fn let_with_mismatching_annotation() {
    let src = "\
fn f():
    let x: Int = true
";
    assert_error_contains(src, "type mismatch in `let x`");
}

#[test]
fn let_inferred_type() {
    // Without annotation, the type is inferred from the value.
    let src = "\
fn f() -> Int:
    let x = 42
    ret x
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Ret with wrong type
// ---------------------------------------------------------------------------

#[test]
fn ret_wrong_type() {
    let src = "\
fn f() -> Int:
    ret true
";
    assert_error_contains(src, "`ret` type mismatch");
}

#[test]
fn ret_correct_type() {
    let src = "\
fn f() -> Bool:
    ret true
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Undefined variable
// ---------------------------------------------------------------------------

#[test]
fn undefined_variable() {
    let src = "\
fn f() -> Int:
    ret x
";
    assert_error_contains(src, "undefined variable `x`");
}

// ---------------------------------------------------------------------------
// Boolean operations
// ---------------------------------------------------------------------------

#[test]
fn boolean_and_or() {
    let src = "\
fn logic(a: Bool, b: Bool) -> Bool:
    let c: Bool = a and b
    let d: Bool = a or b
    ret c
";
    assert_no_errors(src);
}

#[test]
fn boolean_not() {
    let src = "\
fn negate(a: Bool) -> Bool:
    ret not a
";
    assert_no_errors(src);
}

#[test]
fn boolean_op_type_error() {
    let src = "\
fn bad():
    let x: Bool = 42 and true
";
    assert_error_contains(src, "requires Bool operands");
}

#[test]
fn not_on_int_error() {
    let src = "\
fn bad():
    let x: Bool = not 42
";
    assert_error_contains(src, "`not` requires a Bool operand");
}

// ---------------------------------------------------------------------------
// Comparison operators
// ---------------------------------------------------------------------------

#[test]
fn comparison_operators() {
    let src = "\
fn cmp(a: Int, b: Int) -> Bool:
    let r1: Bool = a < b
    let r2: Bool = a <= b
    let r3: Bool = a > b
    let r4: Bool = a >= b
    let r5: Bool = a == b
    let r6: Bool = a != b
    ret r1
";
    assert_no_errors(src);
}

#[test]
fn equality_on_bools() {
    let src = "\
fn eq(a: Bool, b: Bool) -> Bool:
    ret a == b
";
    assert_no_errors(src);
}

#[test]
fn ordering_on_strings_error() {
    let src = "\
fn bad(a: String, b: String) -> Bool:
    ret a < b
";
    assert_error_contains(src, "requires numeric operands");
}

// ---------------------------------------------------------------------------
// Nested scopes and shadowing
// ---------------------------------------------------------------------------

#[test]
fn nested_scope_shadowing() {
    let src = "\
fn f() -> Int:
    let x: Int = 1
    if true:
        let x: Bool = true
        x
    ret x
";
    // The inner `x` is Bool, the outer is Int. Both are fine in their scopes.
    // The `ret x` at the end refers to the outer Int x.
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Typed holes
// ---------------------------------------------------------------------------

#[test]
fn typed_hole_reports_error() {
    let src = "\
fn f():
    let x: Int = ?todo
";
    // The type checker should report the typed hole.
    assert_error_contains(src, "typed hole");
}

// ---------------------------------------------------------------------------
// Effect checking
// ---------------------------------------------------------------------------

#[test]
fn calling_io_function_outside_io_context() {
    let src = "\
fn main():
    print(\"hello\")
";
    assert_error_contains(src, "requires effect `IO`");
}

#[test]
fn calling_io_function_inside_io_context() {
    let src = "\
fn main() -> !{IO} ():
    print(\"hello\")
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Negation operator
// ---------------------------------------------------------------------------

#[test]
fn negation_of_int() {
    let src = "\
fn neg(x: Int) -> Int:
    ret -x
";
    assert_no_errors(src);
}

#[test]
fn negation_of_bool_error() {
    let src = "\
fn bad(x: Bool):
    let y: Int = -x
";
    assert_error_contains(src, "unary `-` requires a numeric operand");
}

// ---------------------------------------------------------------------------
// Mixed type arithmetic errors
// ---------------------------------------------------------------------------

#[test]
fn mixed_int_float_arithmetic() {
    let src = "\
fn bad(a: Int, b: Float) -> Float:
    ret a + b
";
    assert_error_contains(src, "must have the same type");
}

// ---------------------------------------------------------------------------
// Forward function references
// ---------------------------------------------------------------------------

#[test]
fn forward_function_reference() {
    // `main` calls `helper` which is defined after it.
    let src = "\
fn main() -> Int:
    ret helper(10)

fn helper(x: Int) -> Int:
    ret x + 1
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// For loops
// ---------------------------------------------------------------------------

#[test]
fn for_loop_basic() {
    let src = "\
fn sum_to(n: Int) -> Int:
    let total: Int = 0
    for i in range(n):
        total
    ret total
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Error recovery (Ty::Error does not cascade)
// ---------------------------------------------------------------------------

#[test]
fn error_does_not_cascade() {
    // `x` is undefined, but we should not get errors for `x + 1` or `let y = ...`.
    let src = "\
fn f():
    let y: Int = x + 1
";
    // Should only report one error: `x` is undefined.
    // The `+ 1` and `let y` should not produce additional errors because
    // Ty::Error propagates silently.
    assert_error_count(src, 1);
    assert_error_contains(src, "undefined variable");
}

// ---------------------------------------------------------------------------
// Complete hello.gr program
// ---------------------------------------------------------------------------

#[test]
fn hello_world_program() {
    let src = "\
fn main() -> !{IO} ():
    print(\"Hello, Gradient!\")
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Ty and TypeError Display / JSON
// ---------------------------------------------------------------------------

#[test]
fn ty_display() {
    assert_eq!(format!("{}", Ty::Int), "Int");
    assert_eq!(format!("{}", Ty::Float), "Float");
    assert_eq!(format!("{}", Ty::String), "String");
    assert_eq!(format!("{}", Ty::Bool), "Bool");
    assert_eq!(format!("{}", Ty::Unit), "()");
    assert_eq!(format!("{}", Ty::Error), "<error>");
    assert_eq!(
        format!(
            "{}",
            Ty::Fn {
                params: vec![Ty::Int, Ty::Int],
                ret: Box::new(Ty::Bool),
                effects: vec!["IO".to_string()],
            }
        ),
        "(Int, Int) !{IO} -> Bool"
    );
}

#[test]
fn ty_is_numeric() {
    assert!(Ty::Int.is_numeric());
    assert!(Ty::Float.is_numeric());
    assert!(!Ty::String.is_numeric());
    assert!(!Ty::Bool.is_numeric());
    assert!(!Ty::Unit.is_numeric());
    assert!(!Ty::Error.is_numeric());
}

#[test]
fn type_error_to_json() {
    use crate::ast::span::{Position, Span};

    let err = TypeError::mismatch(
        "type mismatch",
        Span::new(0, Position::new(1, 5, 4), Position::new(1, 10, 9)),
        Ty::Int,
        Ty::Bool,
    );
    let json = err.to_json();
    assert!(json.contains(r#""source_phase": "typechecker""#));
    assert!(json.contains(r#""severity": "error""#));
    assert!(json.contains(r#""message": "type mismatch""#));
    assert!(json.contains(r#""expected": "Int""#));
    assert!(json.contains(r#""found": "Bool""#));
    assert!(json.contains(r#""line": 1"#));
}

#[test]
fn type_error_display() {
    use crate::ast::span::{Position, Span};

    let err = TypeError::mismatch(
        "type mismatch",
        Span::new(0, Position::new(3, 5, 20), Position::new(3, 10, 25)),
        Ty::Int,
        Ty::Bool,
    )
    .with_note("consider using a cast");

    let display = format!("{}", err);
    assert!(display.contains("error[3:5]: type mismatch"));
    assert!(display.contains("expected `Int`, found `Bool`"));
    assert!(display.contains("note: consider using a cast"));
}

// ---------------------------------------------------------------------------
// Multiple functions interacting
// ---------------------------------------------------------------------------

#[test]
fn multiple_functions_with_calls() {
    let src = "\
fn square(x: Int) -> Int:
    ret x * x

fn sum_of_squares(a: Int, b: Int) -> Int:
    ret square(a) + square(b)
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// If condition must be Bool
// ---------------------------------------------------------------------------

#[test]
fn if_condition_must_be_bool() {
    let src = "\
fn f():
    if 42:
        true
";
    assert_error_contains(src, "`if` condition must be Bool");
}

// ---------------------------------------------------------------------------
// Parenthesized expressions
// ---------------------------------------------------------------------------

#[test]
fn parenthesized_expression() {
    let src = "\
fn f(a: Int, b: Int) -> Int:
    ret (a + b) * 2
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Unit return type (implicit)
// ---------------------------------------------------------------------------

#[test]
fn implicit_unit_return() {
    let src = "\
fn f():
    let x: Int = 42
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Empty function body
// ---------------------------------------------------------------------------

#[test]
fn function_with_only_let() {
    let src = "\
fn f() -> Int:
    let x: Int = 42
    ret x
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Builtin math functions
// ---------------------------------------------------------------------------

#[test]
fn builtin_abs() {
    let src = "\
fn f() -> Int:
    ret abs(-42)
";
    assert_no_errors(src);
}

#[test]
fn builtin_abs_wrong_type() {
    let src = "\
fn f() -> Int:
    ret abs(true)
";
    assert_error_contains(src, "expected `Int`, found `Bool`");
}

#[test]
fn builtin_min_max() {
    let src = "\
fn f() -> Int:
    let a: Int = min(10, 3)
    let b: Int = max(10, 3)
    ret a + b
";
    assert_no_errors(src);
}

#[test]
fn builtin_mod_int() {
    let src = "\
fn f() -> Int:
    ret mod_int(17, 5)
";
    assert_no_errors(src);
}

#[test]
fn builtin_print_float() {
    let src = "\
fn f() -> !{IO} ():
    print_float(3.14)
";
    assert_no_errors(src);
}

#[test]
fn builtin_print_bool() {
    let src = "\
fn f() -> !{IO} ():
    print_bool(true)
    print_bool(false)
";
    assert_no_errors(src);
}

#[test]
fn builtin_int_to_string() {
    let src = "\
fn f() -> String:
    ret int_to_string(42)
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Unknown type names
// ---------------------------------------------------------------------------

#[test]
fn unknown_type_in_param() {
    let src = "\
fn f(x: Foo):
    ret x
";
    assert_error_contains(src, "unknown type `Foo`");
}

#[test]
fn unknown_type_in_let_annotation() {
    let src = "\
fn f():
    let x: Bar = 42
";
    assert_error_contains(src, "unknown type `Bar`");
}

#[test]
fn unknown_type_in_return_type() {
    let src = "\
fn f(x: Int) -> Baz:
    ret x
";
    assert_error_contains(src, "unknown type `Baz`");
}

#[test]
fn known_types_still_resolve() {
    // Verify that all built-in types still work correctly.
    let src = "\
fn f(a: Int, b: Float, c: String, d: Bool) -> Int:
    ret a
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

#[test]
fn type_alias_basic() {
    let src = "\
type Count = Int
fn f() -> Count:
    let x: Count = 42
    ret x
";
    assert_no_errors(src);
}

#[test]
fn type_alias_used_in_param() {
    let src = "\
type Name = String
fn greet(name: Name) -> !{IO} ():
    print(name)
";
    assert_no_errors(src);
}

#[test]
fn type_alias_mismatch() {
    // A type alias resolves to its underlying type, so Count is Int.
    // Assigning a Bool to a Count should be an error.
    let src = "\
type Count = Int
fn f():
    let x: Count = true
";
    assert_error_contains(src, "type mismatch in `let x`");
}

// ---------------------------------------------------------------------------
// Mutable bindings and assignment
// ---------------------------------------------------------------------------

#[test]
fn mutable_binding_and_reassignment() {
    let src = "\
fn f() -> Int:
    let mut x: Int = 1
    x = 2
    ret x
";
    assert_no_errors(src);
}

#[test]
fn assign_to_immutable_fails() {
    let src = "\
fn f():
    let x: Int = 1
    x = 2
";
    assert_error_contains(src, "cannot assign to immutable variable `x`");
}

#[test]
fn assign_type_mismatch() {
    let src = "\
fn f():
    let mut x: Int = 1
    x = true
";
    assert_error_contains(src, "type mismatch in assignment to `x`");
}

#[test]
fn assign_to_undefined_variable() {
    let src = "\
fn f():
    y = 10
";
    assert_error_contains(src, "undefined variable `y`");
}

// ---------------------------------------------------------------------------
// While loops
// ---------------------------------------------------------------------------

#[test]
fn while_loop_basic() {
    let src = "\
fn f():
    let mut x: Int = 5
    while x > 0:
        x = x - 1
";
    assert_no_errors(src);
}

#[test]
fn while_condition_must_be_bool() {
    let src = "\
fn f():
    while 42:
        ()
";
    assert_error_contains(src, "`while` condition must be Bool");
}

#[test]
fn while_loop_with_effect() {
    let src = "\
fn countdown(n: Int) -> !{IO} ():
    let mut i: Int = n
    while i > 0:
        print_int(i)
        i = i - 1
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Match expressions
// ---------------------------------------------------------------------------

#[test]
fn match_int_basic() {
    let src = "\
fn describe(n: Int) -> String:
    match n:
        0:
            ret \"zero\"
        1:
            ret \"one\"
        _:
            ret \"other\"
";
    assert_no_errors(src);
}

#[test]
fn match_bool_basic() {
    let src = "\
fn to_word(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
        false:
            ret \"no\"
        _:
            ret \"unknown\"
";
    assert_no_errors(src);
}

#[test]
fn match_int_pattern_on_bool_scrutinee() {
    let src = "\
fn bad(b: Bool) -> Int:
    match b:
        0:
            ret 0
        _:
            ret 1
";
    assert_error_contains(src, "integer pattern cannot match scrutinee of type `Bool`");
}

#[test]
fn match_bool_pattern_on_int_scrutinee() {
    let src = "\
fn bad(n: Int) -> Int:
    match n:
        true:
            ret 0
        _:
            ret 1
";
    assert_error_contains(src, "boolean pattern cannot match scrutinee of type `Int`");
}

#[test]
fn match_non_exhaustive_warning() {
    let src = "\
fn partial(n: Int) -> String:
    match n:
        0:
            ret \"zero\"
        1:
            ret \"one\"
";
    assert_error_contains(src, "non-exhaustive match");
}

#[test]
fn match_bool_exhaustive_no_warning() {
    // Bool match with both true and false covered should NOT warn about
    // non-exhaustiveness because we still require `_` for exhaustiveness
    // in v0.1 (no enum-based exhaustiveness check yet).
    let src = "\
fn f(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
        false:
            ret \"no\"
";
    // This will produce a non-exhaustive warning since there's no wildcard.
    // That's expected for v0.1.
    assert_error_contains(src, "non-exhaustive match");
}

#[test]
fn match_with_wildcard_is_exhaustive() {
    let src = "\
fn f(n: Int) -> String:
    match n:
        0:
            ret \"zero\"
        _:
            ret \"other\"
";
    assert_no_errors(src);
}
