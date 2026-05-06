//! Comprehensive tests for the Gradient type checker.
//!
//! Each test parses a Gradient source snippet through the lexer and parser,
//! then runs the type checker and asserts on the resulting errors (or lack
//! thereof).

use crate::ast::module::Module;
use crate::lexer::Lexer;
use crate::parser::Parser;

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

/// Assert that the source type-checks with no errors (warnings are ignored).
fn assert_no_errors(src: &str) {
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
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
/// contains the given substring (ignores warnings).
fn assert_error_contains(src: &str, substring: &str) {
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
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

/// Assert that the source produces at least one warning whose message
/// contains the given substring.
fn assert_warning_contains(src: &str, substring: &str) {
    let all = check(src);
    let warnings: Vec<_> = all.iter().filter(|e| e.is_warning).collect();
    assert!(
        warnings.iter().any(|e| e.message.contains(substring)),
        "expected a warning containing {:?}, but got:\n{}",
        substring,
        if warnings.is_empty() {
            "  (no warnings)".to_string()
        } else {
            warnings
                .iter()
                .map(|e| format!("  - {}", e))
                .collect::<Vec<_>>()
                .join("\n")
        }
    );
}

/// Assert that the source produces exactly `n` type errors (ignores warnings).
fn assert_error_count(src: &str, n: usize) {
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
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
    assert_error_contains(
        src,
        "all branches of `if` expression must have the same type",
    );
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
// Phase MM: Standard I/O builtins
// ---------------------------------------------------------------------------

#[test]
fn builtin_read_line_requires_io_effect() {
    // read_line() has IO effect; calling from a pure function is a type error.
    let src = "\
fn f() -> String:
    ret read_line()
";
    assert_error_contains(src, "requires effect `IO`");
}

#[test]
fn builtin_read_line_ok_in_io_context() {
    let src = "\
fn f() -> !{IO} String:
    ret read_line()
";
    assert_no_errors(src);
}

#[test]
fn builtin_exit_requires_io_effect() {
    // exit() has IO effect; calling from a pure function is a type error.
    let src = "\
fn f():
    exit(0)
";
    assert_error_contains(src, "requires effect `IO`");
}

#[test]
fn builtin_exit_ok_in_io_context() {
    let src = "\
fn f() -> !{IO} ():
    exit(0)
";
    assert_no_errors(src);
}

#[test]
fn builtin_parse_int_returns_int() {
    // parse_int(String) -> Int — no IO effect needed.
    let src = "\
fn f() -> Int:
    ret parse_int(\"42\")
";
    assert_no_errors(src);
}

#[test]
fn builtin_parse_float_returns_float() {
    // parse_float(String) -> Float — no IO effect needed.
    let src = "\
fn f() -> Float:
    ret parse_float(\"3.14\")
";
    assert_no_errors(src);
}

#[test]
fn builtin_parse_int_type_error_wrong_arg() {
    // parse_int expects a String; passing an Int should be a type error.
    let src = "\
fn f() -> Int:
    ret parse_int(42)
";
    assert_error_contains(src, "expected `String`, found `Int`");
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
    // Bool match with both true and false covered is exhaustive — no error.
    let src = "\
fn f(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
        false:
            ret \"no\"
";
    assert_no_errors(src);
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

// ---------------------------------------------------------------------------
// Enum declarations
// ---------------------------------------------------------------------------

#[test]
fn enum_unit_variants_typecheck() {
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        Green:
            ret \"green\"
        Blue:
            ret \"blue\"
";
    assert_no_errors(src);
}

#[test]
fn enum_variant_used_as_value() {
    // Unit variants should be usable as values of the enum type.
    let src = "\
type Color = Red | Green | Blue

fn get_red() -> Color:
    ret Red
";
    assert_no_errors(src);
}

#[test]
fn enum_variant_passed_to_function() {
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        Green:
            ret \"green\"
        Blue:
            ret \"blue\"

fn main() -> !{IO} ():
    print(describe(Red))
";
    assert_no_errors(src);
}

#[test]
fn enum_exhaustiveness_error() {
    // Missing a variant should produce an error.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        Green:
            ret \"green\"
";
    let errors = check(src);
    assert!(!errors.is_empty(), "should report non-exhaustive match");
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("non-exhaustive") || e.message.contains("missing")),
        "should mention missing variants, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn enum_exhaustiveness_with_wildcard() {
    // A wildcard arm should make the match exhaustive.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        _:
            ret \"other\"
";
    assert_no_errors(src);
}

#[test]
fn enum_wrong_variant_in_match() {
    // Using a variant from a different enum (or nonexistent) should error.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        Yellow:
            ret \"yellow\"
        _:
            ret \"other\"
";
    let errors = check(src);
    assert!(
        errors.iter().any(|e| e.message.contains("not a member")),
        "should report Yellow is not a variant of Color, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn enum_tuple_variant_typecheck() {
    let src = "\
type Option = Some(Int) | None

fn unwrap(o: Option) -> Int:
    match o:
        Some(x):
            ret x
        None:
            ret 0
";
    assert_no_errors(src);
}

#[test]
fn enum_type_in_function_param() {
    let src = "\
type Direction = North | South | East | West

fn is_north(d: Direction) -> Bool:
    match d:
        North:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn typed_expr_valid_annotation_typechecks() {
    let src = "\
fn main() -> Int:
    Int:
        42
";
    assert_no_errors(src);
}

#[test]
fn typed_expr_invalid_annotation_reports_mismatch() {
    let src = "\
fn main() -> Bool:
    Bool:
        1
";
    assert_error_contains(src, "type annotation mismatch");
}

#[test]
fn enum_constructor_named_fields_validates_payload_types() {
    let src = "\
type PairT = MkPair(Int, Int)

fn main() -> PairT:
    ret MkPair(a: 1, b: 2)
";
    assert_no_errors(src);
}

#[test]
fn enum_constructor_named_fields_reports_wrong_arity() {
    let src = "\
type PairT = MkPair(Int, Int)

fn main() -> PairT:
    ret MkPair(a: 1)
";
    assert_error_contains(src, "expects 2 field");
}

#[test]
fn enum_constructor_named_fields_reports_wrong_field_type() {
    let src = "\
type PairT = MkPair(Int, Int)

fn main() -> PairT:
    ret MkPair(a: 1, b: \"x\")
";
    assert_error_contains(src, "expected `Int`");
}

// ---------------------------------------------------------------------------
// Multi-file module resolution and qualified calls
// ---------------------------------------------------------------------------

use super::checker::{check_module_with_imports, ImportedModuleInfo, ImportedModules};
use super::env::FnSig;

/// Build an ImportedModules map with a single module containing the given functions.
fn make_imports(module_name: &str, fns: Vec<(&str, FnSig)>) -> ImportedModules {
    let mut info = ImportedModuleInfo::default();
    for (name, sig) in fns {
        info.functions.insert(name.to_string(), sig);
    }
    let mut imports = ImportedModules::new();
    imports.insert(module_name.to_string(), info);
    imports
}

/// Parse and type-check with imported modules. Returns the list of type errors.
fn check_with_imports(src: &str, imports: &ImportedModules) -> Vec<TypeError> {
    let module = parse_ok(src);
    let (errors, _summary) = check_module_with_imports(&module, 0, imports);
    errors
}

#[test]
fn qualified_call_basic() {
    // math.add(3, 4) should resolve when math module is imported.
    let imports = make_imports(
        "math",
        vec![(
            "add",
            FnSig {
                type_params: vec![],
                params: vec![
                    ("a".to_string(), Ty::Int, false),
                    ("b".to_string(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        )],
    );

    let src = "\
use math

fn main() -> Int:
    ret math.add(3, 4)
";
    let errors = check_with_imports(src, &imports);
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

#[test]
fn qualified_call_wrong_arg_type() {
    let imports = make_imports(
        "math",
        vec![(
            "add",
            FnSig {
                type_params: vec![],
                params: vec![
                    ("a".to_string(), Ty::Int, false),
                    ("b".to_string(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        )],
    );

    let src = "\
use math

fn main() -> Int:
    ret math.add(3, true)
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("expected `Int`, found `Bool`")),
        "expected type mismatch error, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn qualified_call_wrong_arg_count() {
    let imports = make_imports(
        "math",
        vec![(
            "add",
            FnSig {
                type_params: vec![],
                params: vec![
                    ("a".to_string(), Ty::Int, false),
                    ("b".to_string(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        )],
    );

    let src = "\
use math

fn main() -> Int:
    ret math.add(3)
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.iter().any(|e| e
            .message
            .contains("expects 2 argument(s), but 1 were provided")),
        "expected arg count error, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn qualified_call_nonexistent_function() {
    let imports = make_imports(
        "math",
        vec![(
            "add",
            FnSig {
                type_params: vec![],
                params: vec![
                    ("a".to_string(), Ty::Int, false),
                    ("b".to_string(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        )],
    );

    let src = "\
use math

fn main() -> Int:
    ret math.subtract(3, 4)
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.iter().any(|e| e
            .message
            .contains("module `math` has no function `subtract`")),
        "expected 'no function' error, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn qualified_call_nonexistent_module() {
    let imports = ImportedModules::new();

    let src = "\
use utils

fn main() -> Int:
    ret utils.helper(3)
";
    let errors = check_with_imports(src, &imports);
    // Since 'utils' is not imported, it should be treated as an undefined variable.
    assert!(!errors.is_empty(), "expected errors for unresolved module");
}

#[test]
fn qualified_call_with_effects() {
    // Imported function with IO effect should require IO in caller.
    let imports = make_imports(
        "io_mod",
        vec![(
            "write_line",
            FnSig {
                type_params: vec![],
                params: vec![("msg".to_string(), Ty::String, false)],
                ret: Ty::Unit,
                effects: vec!["IO".to_string()],
            },
        )],
    );

    let src = "\
use io_mod

fn main() -> !{IO} ():
    io_mod.write_line(\"hello\")
";
    let errors = check_with_imports(src, &imports);
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

#[test]
fn qualified_call_missing_effect() {
    // Calling an imported IO function without declaring IO should error.
    let imports = make_imports(
        "io_mod",
        vec![(
            "write_line",
            FnSig {
                type_params: vec![],
                params: vec![("msg".to_string(), Ty::String, false)],
                ret: Ty::Unit,
                effects: vec!["IO".to_string()],
            },
        )],
    );

    let src = "\
use io_mod

fn main():
    io_mod.write_line(\"hello\")
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("requires effect `IO`")),
        "expected effect error, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn qualified_call_return_type_used() {
    // The return type of a qualified call should be properly tracked.
    let imports = make_imports(
        "math",
        vec![(
            "double",
            FnSig {
                type_params: vec![],
                params: vec![("x".to_string(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        )],
    );

    let src = "\
use math

fn main() -> Int:
    let result: Int = math.double(21)
    ret result
";
    let errors = check_with_imports(src, &imports);
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

#[test]
fn qualified_call_return_type_mismatch() {
    // Assigning a qualified call result to wrong type should error.
    let imports = make_imports(
        "math",
        vec![(
            "double",
            FnSig {
                type_params: vec![],
                params: vec![("x".to_string(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        )],
    );

    let src = "\
use math

fn main():
    let result: String = math.double(21)
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.iter().any(|e| e.message.contains("type mismatch")),
        "expected type mismatch error, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn multiple_modules_imported() {
    // Multiple modules should be resolvable simultaneously.
    let mut imports = ImportedModules::new();

    let mut math_info = ImportedModuleInfo::default();
    math_info.functions.insert(
        "add".to_string(),
        FnSig {
            type_params: vec![],
            params: vec![
                ("a".to_string(), Ty::Int, false),
                ("b".to_string(), Ty::Int, false),
            ],
            ret: Ty::Int,
            effects: vec![],
        },
    );
    imports.insert("math".to_string(), math_info);

    let mut str_info = ImportedModuleInfo::default();
    str_info.functions.insert(
        "concat".to_string(),
        FnSig {
            type_params: vec![],
            params: vec![
                ("a".to_string(), Ty::String, false),
                ("b".to_string(), Ty::String, false),
            ],
            ret: Ty::String,
            effects: vec![],
        },
    );
    imports.insert("str_utils".to_string(), str_info);

    let src = "\
use math
use str_utils

fn main() -> !{IO} ():
    let sum: Int = math.add(1, 2)
    let msg: String = str_utils.concat(\"hello \", \"world\")
    print_int(sum)
    print(msg)
";
    let errors = check_with_imports(src, &imports);
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

#[test]
fn local_and_imported_coexist() {
    // Local functions and imported functions should coexist.
    let imports = make_imports(
        "helper",
        vec![(
            "inc",
            FnSig {
                type_params: vec![],
                params: vec![("x".to_string(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        )],
    );

    let src = "\
use helper

fn local_double(x: Int) -> Int:
    x * 2

fn main() -> Int:
    let a: Int = local_double(5)
    let b: Int = helper.inc(a)
    ret b
";
    let errors = check_with_imports(src, &imports);
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

// ---------------------------------------------------------------------------
// Design-by-contract: @requires and @ensures
// ---------------------------------------------------------------------------

#[test]
fn requires_valid_bool_condition() {
    let src = "\
@requires(x > 0)
fn positive(x: Int) -> Int:
    ret x
";
    assert_no_errors(src);
}

#[test]
fn ensures_valid_bool_condition() {
    let src = "\
@ensures(result >= 0)
fn abs_val(x: Int) -> Int:
    if x >= 0:
        x
    else:
        0 - x
";
    assert_no_errors(src);
}

#[test]
fn requires_non_bool_condition_is_error() {
    let src = "\
@requires(x + 1)
fn f(x: Int) -> Int:
    ret x
";
    assert_error_contains(src, "@requires condition must be Bool");
}

#[test]
fn ensures_non_bool_condition_is_error() {
    let src = "\
@ensures(result + 1)
fn f(x: Int) -> Int:
    ret x
";
    assert_error_contains(src, "@ensures condition must be Bool");
}

#[test]
fn ensures_result_has_correct_type() {
    let src = "\
@ensures(result > 0)
fn f(x: Int) -> Int:
    ret x + 1
";
    assert_no_errors(src);
}

#[test]
fn ensures_result_type_mismatch() {
    let src = r#"
@ensures(result == "hello")
fn f(x: Int) -> Int:
    ret x
"#;
    assert_error_contains(src, "must have the same type");
}

#[test]
fn requires_references_parameter() {
    let src = "\
@requires(a > b)
fn max_val(a: Int, b: Int) -> Int:
    ret a
";
    assert_no_errors(src);
}

#[test]
fn requires_undefined_variable_is_error() {
    let src = "\
@requires(z > 0)
fn f(x: Int) -> Int:
    ret x
";
    assert_error_contains(src, "undefined variable `z`");
}

#[test]
fn multiple_contracts_valid() {
    let src = "\
@requires(x > 0)
@requires(y > 0)
@ensures(result > 0)
fn multiply(x: Int, y: Int) -> Int:
    ret x * y
";
    assert_no_errors(src);
}

#[test]
fn result_as_regular_variable_still_works() {
    let src = "\
fn f() -> Int:
    let result: Int = 42
    ret result
";
    assert_no_errors(src);
}

#[test]
fn ensures_with_logical_operators() {
    let src = "\
@ensures(result >= 0 and result <= 100)
fn clamp(x: Int) -> Int:
    if x < 0:
        0
    else:
        if x > 100:
            100
        else:
            x
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Generics and bidirectional type inference
// ---------------------------------------------------------------------------

#[test]
fn generic_identity_function_int() {
    // A generic identity function called with an Int argument.
    let src = "\
fn identity[T](x: T) -> T:
    ret x

fn main() -> !{IO} ():
    let n: Int = identity(42)
    print_int(n)
";
    assert_no_errors(src);
}

#[test]
fn generic_identity_function_string() {
    // A generic identity function called with a String argument.
    let src = "\
fn identity[T](x: T) -> T:
    ret x

fn main() -> !{IO} ():
    let s: String = identity(\"hello\")
    print(s)
";
    assert_no_errors(src);
}

#[test]
fn generic_identity_function_bool() {
    // A generic identity function called with a Bool argument.
    let src = "\
fn identity[T](x: T) -> T:
    ret x

fn main() -> !{IO} ():
    let b: Bool = identity(true)
    print_bool(b)
";
    assert_no_errors(src);
}

#[test]
fn generic_function_two_type_params() {
    // A generic function with two type parameters.
    let src = "\
fn first[T, U](x: T, y: U) -> T:
    ret x

fn main() -> !{IO} ():
    let n: Int = first(42, \"hello\")
    print_int(n)
";
    assert_no_errors(src);
}

#[test]
fn generic_function_inferred_return_used_in_arithmetic() {
    // The return type of a generic call is inferred and used in arithmetic.
    let src = "\
fn identity[T](x: T) -> T:
    ret x

fn main() -> Int:
    let a: Int = identity(10)
    let b: Int = identity(20)
    ret a + b
";
    assert_no_errors(src);
}

#[test]
fn generic_function_type_mismatch_at_call() {
    // Calling a generic function with conflicting type variable bindings.
    // Both params share T, but one is Int and the other is String.
    let src = "\
fn same[T](x: T, y: T) -> T:
    ret x

fn main() -> Int:
    ret same(42, \"hello\")
";
    assert_error_contains(src, "expected `Int`, found `String`");
}

#[test]
fn generic_function_wrong_arg_count() {
    // Calling a generic function with wrong number of arguments.
    let src = "\
fn identity[T](x: T) -> T:
    ret x

fn main() -> Int:
    ret identity(1, 2)
";
    assert_error_contains(src, "expects 1 argument(s), but 2 were provided");
}

#[test]
fn generic_enum_declaration_parses() {
    // A generic enum declaration should parse and type-check without errors.
    let src = "\
type Option[T] = Some(Int) | None
fn main() -> !{IO} ():
    let x: Option = Some(42)
    print_int(0)
";
    assert_no_errors(src);
}

#[test]
fn generic_function_return_type_inferred_from_arg() {
    // The return type annotation on the let binding matches the inferred
    // return type from the generic call.
    let src = "\
fn wrap[T](x: T) -> T:
    ret x

fn main() -> Float:
    ret wrap(3.14)
";
    assert_no_errors(src);
}

#[test]
fn generic_function_return_type_mismatch_let() {
    // The let binding declares Int but the generic call returns String.
    let src = "\
fn identity[T](x: T) -> T:
    ret x

fn main() -> Int:
    let n: Int = identity(\"hello\")
    ret n
";
    assert_error_contains(src, "type mismatch in `let n`");
}

#[test]
fn generic_function_multiple_calls_different_types() {
    // The same generic function can be called with different types.
    let src = "\
fn identity[T](x: T) -> T:
    ret x

fn main() -> !{IO} ():
    let n: Int = identity(42)
    let s: String = identity(\"hello\")
    let b: Bool = identity(true)
    print_int(n)
    print(s)
    print_bool(b)
";
    assert_no_errors(src);
}

#[test]
fn generic_function_nested_call() {
    // A generic function used in a nested position.
    let src = "\
fn identity[T](x: T) -> T:
    ret x

fn add(a: Int, b: Int) -> Int:
    ret a + b

fn main() -> Int:
    ret add(identity(1), identity(2))
";
    assert_no_errors(src);
}

#[test]
fn generic_type_param_in_type_annotation() {
    // Using a generic type in a type annotation: List[Int]
    // (parsed correctly, type-checks against enum)
    let src = "\
type List[T] = Cons(Int) | Nil
fn main() -> List:
    ret Nil
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Effect polymorphism
// ---------------------------------------------------------------------------

#[test]
fn effect_poly_definition_with_effect_var() {
    let src = "\
fn apply(f: (Int) -> !{e} Int, x: Int) -> !{e} Int:
    ret f(x)
";
    assert_no_errors(src);
}

#[test]
fn effect_poly_call_with_pure_function() {
    let src = "\
fn apply(f: (Int) -> !{e} Int, x: Int) -> !{e} Int:
    ret f(x)

fn pure_double(x: Int) -> Int:
    ret x * 2

fn main() -> Int:
    ret apply(pure_double, 5)
";
    assert_no_errors(src);
}

#[test]
fn effect_poly_call_with_effectful_function() {
    let src = "\
fn apply(f: (Int) -> !{e} Int, x: Int) -> !{e} Int:
    ret f(x)

fn io_print(x: Int) -> !{IO} Int:
    print_int(x)
    ret x

fn main() -> !{IO} Int:
    ret apply(io_print, 5)
";
    assert_no_errors(src);
}

#[test]
fn effect_poly_call_missing_effect_in_caller() {
    let src = "\
fn apply(f: (Int) -> !{e} Int, x: Int) -> !{e} Int:
    ret f(x)

fn io_print(x: Int) -> !{IO} Int:
    print_int(x)
    ret x

fn main() -> Int:
    ret apply(io_print, 5)
";
    assert_error_contains(src, "requires effect `IO`");
}

#[test]
fn effect_poly_multiple_effect_variables() {
    let src = "\
fn compose(f: (Int) -> !{e1} Int, g: (Int) -> !{e2} Int, x: Int) -> !{e1, e2} Int:
    ret f(g(x))

fn pure_inc(x: Int) -> Int:
    ret x + 1

fn main() -> Int:
    ret compose(pure_inc, pure_inc, 5)
";
    assert_no_errors(src);
}

#[test]
fn effect_poly_mixed_concrete_and_variable() {
    let src = "\
fn apply_and_print(f: (Int) -> !{e} Int, x: Int) -> !{IO, e} Int:
    let result: Int = f(x)
    print_int(result)
    ret result

fn pure_double(x: Int) -> Int:
    ret x * 2

fn main() -> !{IO} Int:
    ret apply_and_print(pure_double, 5)
";
    assert_no_errors(src);
}

#[test]
fn effect_poly_effect_var_not_flagged_as_unknown() {
    let src = "\
fn identity(f: (Int) -> !{e} Int, x: Int) -> !{e} Int:
    ret f(x)
";
    let errors = check(src);
    assert!(
        !errors.iter().any(|e| e.message.contains("unknown effect")),
        "effect variable `e` should not be flagged as unknown, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn effect_poly_wrong_param_types_still_error() {
    let src = "\
fn apply(f: (Int) -> !{e} Int, x: Int) -> !{e} Int:
    ret f(x)

fn string_fn(s: String) -> String:
    ret s

fn main() -> Int:
    ret apply(string_fn, 5)
";
    assert_error_contains(src, "expected");
}

#[test]
fn effect_poly_full_example() {
    let src = "\
fn apply(f: (Int) -> !{e} Int, x: Int) -> !{e} Int:
    ret f(x)

fn pure_double(x: Int) -> Int:
    ret x * 2

fn io_print(x: Int) -> !{IO} Int:
    print_int(x)
    ret x

fn main() -> !{IO} ():
    let a: Int = apply(pure_double, 5)
    let b: Int = apply(io_print, 5)
    print_int(a)
    print_int(b)
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Runtime capability budgets: @budget
// ---------------------------------------------------------------------------

#[test]
fn budget_valid_cpu_and_mem() {
    let src = "\
@budget(cpu: 5s, mem: 100mb)
fn process(x: Int) -> Int:
    ret x * 2
";
    assert_no_errors(src);
}

#[test]
fn budget_valid_cpu_only() {
    let src = "\
@budget(cpu: 10s)
fn compute(x: Int) -> Int:
    ret x + 1
";
    assert_no_errors(src);
}

#[test]
fn budget_valid_mem_only() {
    let src = "\
@budget(mem: 512mb)
fn allocate(x: Int) -> Int:
    ret x
";
    assert_no_errors(src);
}

#[test]
fn budget_invalid_cpu_unit() {
    let src = "\
@budget(cpu: 5x)
fn bad(x: Int) -> Int:
    ret x
";
    assert_error_contains(src, "invalid cpu budget");
}

#[test]
fn budget_invalid_mem_unit() {
    let src = "\
@budget(mem: 100xx)
fn bad(x: Int) -> Int:
    ret x
";
    assert_error_contains(src, "invalid mem budget");
}

#[test]
fn budget_containment_ok() {
    // Inner budget fits within outer budget.
    let src = "\
@budget(cpu: 2s, mem: 50mb)
fn inner(x: Int) -> Int:
    ret x

@budget(cpu: 10s, mem: 100mb)
fn outer(x: Int) -> Int:
    ret inner(x)
";
    assert_no_errors(src);
}

#[test]
fn budget_containment_violation_cpu() {
    // Inner cpu exceeds outer cpu.
    let src = "\
@budget(cpu: 20s, mem: 50mb)
fn inner(x: Int) -> Int:
    ret x

@budget(cpu: 5s, mem: 100mb)
fn outer(x: Int) -> Int:
    ret inner(x)
";
    assert_error_contains(src, "cpu budget");
}

#[test]
fn budget_containment_violation_mem() {
    // Inner mem exceeds outer mem.
    let src = "\
@budget(cpu: 1s, mem: 200mb)
fn inner(x: Int) -> Int:
    ret x

@budget(cpu: 5s, mem: 100mb)
fn outer(x: Int) -> Int:
    ret inner(x)
";
    assert_error_contains(src, "mem budget");
}

// ---------------------------------------------------------------------------
// FFI: @extern and @export type validation
// ---------------------------------------------------------------------------

#[test]
fn extern_fn_with_valid_ffi_types() {
    // All FFI-compatible types: Int, Float, Bool, String, ().
    let src = "\
@extern
fn puts(s: String) -> Int
";
    assert_warning_contains(src, "defaults to the conservative set");
}

#[test]
fn extern_fn_with_float_params() {
    let src = "\
@extern
fn sin(x: Float) -> Float
";
    assert_warning_contains(src, "defaults to the conservative set");
}

#[test]
fn extern_fn_with_bool_param() {
    let src = "\
@extern
fn check(b: Bool) -> Int
";
    assert_warning_contains(src, "defaults to the conservative set");
}

#[test]
fn extern_fn_without_effects_requires_safe_default_effects() {
    let src = "\
@extern
fn system(cmd: String) -> Int

fn run(cmd: String) -> Int:
    ret system(cmd)
";
    assert_error_contains(src, "requires effect `IO`");
    assert_warning_contains(src, "defaults to the conservative set");
}

#[test]
fn extern_fn_without_effects_exceeds_module_capability_ceiling() {
    let src = "\
@cap(IO)

@extern
fn system(cmd: String) -> Int
";
    assert_error_contains(src, "exceeds the module capability ceiling");
}

#[test]
fn extern_fn_with_explicit_effects_stays_precise() {
    let src = "\
@extern
fn puts(s: String) -> !{IO} Int

fn run() -> !{IO} Int:
    ret puts(\"hi\")
";
    assert_no_errors(src);
}

#[test]
fn extern_fn_with_invalid_param_type() {
    // Enum types are not FFI-compatible.
    let src = "\
type Color = Red | Green | Blue

@extern
fn draw(c: Color) -> Int
";
    assert_error_contains(src, "not FFI-compatible");
}

#[test]
fn extern_fn_with_invalid_return_type() {
    // Function types are not FFI-compatible for return.
    let src = "\
type Color = Red | Green | Blue

@extern
fn get_color() -> Color
";
    assert_error_contains(src, "not FFI-compatible");
}

#[test]
fn export_fn_with_valid_ffi_types() {
    let src = "\
@export
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
    assert_no_errors(src);
}

#[test]
fn export_fn_with_invalid_param_type() {
    let src = "\
type Color = Red | Green | Blue

@export
fn process(c: Color) -> Int:
    ret 0
";
    assert_error_contains(src, "not FFI-compatible");
}

// ---------------------------------------------------------------------------
// Actor declarations and operations
// ---------------------------------------------------------------------------

#[test]
fn actor_decl_valid() {
    let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1
    on GetCount -> Int:
        ret count
";
    assert_no_errors(src);
}

#[test]
fn actor_decl_state_type_mismatch() {
    let src = "\
actor Bad:
    state count: Int = \"not an int\"
    on Ping:
        count = count + 1
";
    assert_error_contains(src, "default value has type");
}

#[test]
fn actor_spawn_valid() {
    let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1

fn main() -> !{Actor} ():
    let c: Actor[Counter] = spawn Counter
    send c Increment
";
    assert_no_errors(src);
}

#[test]
fn actor_spawn_unknown_actor() {
    let src = "\
fn main() -> !{Actor} ():
    let c = spawn NonExistent
";
    assert_error_contains(src, "unknown actor type");
}

#[test]
fn actor_spawn_requires_actor_effect() {
    let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1

fn main():
    let c = spawn Counter
";
    assert_error_contains(src, "requires effect `Actor`");
}

#[test]
fn actor_send_valid_message() {
    let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1

fn main() -> !{Actor} ():
    let c = spawn Counter
    send c Increment
";
    assert_no_errors(src);
}

#[test]
fn actor_send_unknown_message() {
    let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1

fn main() -> !{Actor} ():
    let c = spawn Counter
    send c Decrement
";
    assert_error_contains(src, "does not handle message `Decrement`");
}

#[test]
fn actor_ask_returns_correct_type() {
    let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1
    on GetCount -> Int:
        ret count

fn main() -> !{Actor} ():
    let c = spawn Counter
    send c Increment
    let n: Int = ask c GetCount
";
    assert_no_errors(src);
}

#[test]
fn actor_ask_unknown_message() {
    let src = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1

fn main() -> !{Actor} ():
    let c = spawn Counter
    let n: Int = ask c GetCount
";
    assert_error_contains(src, "does not handle message `GetCount`");
}

#[test]
fn actor_send_to_non_actor() {
    let src = "\
fn main() -> !{Actor} ():
    let x: Int = 42
    send x Increment
";
    assert_error_contains(src, "must be an actor handle");
}

#[test]
fn actor_ask_to_non_actor() {
    let src = "\
fn main() -> !{Actor} ():
    let x: Int = 42
    let n = ask x GetCount
";
    assert_error_contains(src, "must be an actor handle");
}

#[test]
fn actor_effect_is_known() {
    // Validate that "Actor" is a recognized effect.
    let src = "\
fn do_stuff() -> !{Actor} ():
    ret ()
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// String builtin functions
// ---------------------------------------------------------------------------

#[test]
fn builtin_string_length() {
    let src = "\
fn f(s: String) -> Int:
    ret string_length(s)
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_length_wrong_type() {
    let src = "\
fn f() -> Int:
    ret string_length(42)
";
    assert_error_contains(src, "expected `String`, found `Int`");
}

#[test]
fn builtin_string_contains() {
    let src = "\
fn f(s: String) -> Bool:
    ret string_contains(s, \"hello\")
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_contains_wrong_type() {
    let src = "\
fn f() -> Bool:
    ret string_contains(\"hello\", 42)
";
    assert_error_contains(src, "expected `String`, found `Int`");
}

#[test]
fn builtin_string_starts_with() {
    let src = "\
fn f(s: String) -> Bool:
    ret string_starts_with(s, \"pre\")
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_ends_with() {
    let src = "\
fn f(s: String) -> Bool:
    ret string_ends_with(s, \"suf\")
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_substring() {
    let src = "\
fn f(s: String) -> String:
    ret string_substring(s, 0, 3)
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_substring_wrong_types() {
    let src = "\
fn f(s: String) -> String:
    ret string_substring(s, \"a\", 3)
";
    assert_error_contains(src, "expected `Int`, found `String`");
}

#[test]
fn builtin_string_trim() {
    let src = "\
fn f(s: String) -> String:
    ret string_trim(s)
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_to_upper() {
    let src = "\
fn f(s: String) -> String:
    ret string_to_upper(s)
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_to_lower() {
    let src = "\
fn f(s: String) -> String:
    ret string_to_lower(s)
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_replace() {
    let src = "\
fn f(s: String) -> String:
    ret string_replace(s, \"old\", \"new\")
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_index_of() {
    let src = "\
fn f(s: String) -> Int:
    ret string_index_of(s, \"needle\")
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_char_at() {
    let src = "\
fn f(s: String) -> String:
    ret string_char_at(s, 0)
";
    assert_no_errors(src);
}

#[test]
fn builtin_string_split() {
    let src = "\
fn f(s: String) -> List[String]:
    ret string_split(s, \",\")
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Numeric builtin functions
// ---------------------------------------------------------------------------

#[test]
fn builtin_float_to_int() {
    let src = "\
fn f(x: Float) -> Int:
    ret float_to_int(x)
";
    assert_no_errors(src);
}

#[test]
fn builtin_float_to_int_wrong_type() {
    let src = "\
fn f() -> Int:
    ret float_to_int(42)
";
    assert_error_contains(src, "expected `Float`, found `Int`");
}

#[test]
fn builtin_int_to_float() {
    let src = "\
fn f(n: Int) -> Float:
    ret int_to_float(n)
";
    assert_no_errors(src);
}

#[test]
fn builtin_pow() {
    let src = "\
fn f() -> Int:
    ret pow(2, 10)
";
    assert_no_errors(src);
}

#[test]
fn builtin_float_abs() {
    let src = "\
fn f(x: Float) -> Float:
    ret float_abs(x)
";
    assert_no_errors(src);
}

#[test]
fn builtin_float_sqrt() {
    let src = "\
fn f(x: Float) -> Float:
    ret float_sqrt(x)
";
    assert_no_errors(src);
}

#[test]
fn builtin_float_to_string() {
    let src = "\
fn f(x: Float) -> String:
    ret float_to_string(x)
";
    assert_no_errors(src);
}

#[test]
fn builtin_float_to_string_wrong_type() {
    let src = "\
fn f() -> String:
    ret float_to_string(42)
";
    assert_error_contains(src, "expected `Float`, found `Int`");
}

#[test]
fn builtin_pow_wrong_type() {
    let src = "\
fn f() -> Int:
    ret pow(2.0, 3)
";
    assert_error_contains(src, "expected `Int`, found `Float`");
}

#[test]
fn builtin_int_to_float_wrong_type() {
    let src = "\
fn f() -> Float:
    ret int_to_float(true)
";
    assert_error_contains(src, "expected `Int`, found `Bool`");
}

// ---------------------------------------------------------------------------
// Closure / lambda expressions
// ---------------------------------------------------------------------------

#[test]
fn closure_simple_typed() {
    let src = "\
fn main():
    let f = |x: Int| x + 1
";
    assert_no_errors(src);
}

#[test]
fn closure_multi_param_typed() {
    let src = "\
fn main():
    let f = |x: Int, y: Int| x + y
";
    assert_no_errors(src);
}

#[test]
fn closure_with_return_type_annotation() {
    let src = "\
fn main():
    let f = |x: Int| -> Int: x + 1
";
    assert_no_errors(src);
}

#[test]
fn closure_return_type_mismatch() {
    let src = "\
fn main():
    let f = |x: Int| -> Bool: x + 1
";
    assert_error_contains(src, "does not match declared return type");
}

#[test]
fn closure_zero_params() {
    let src = "\
fn main():
    let f = || 42
";
    assert_no_errors(src);
}

#[test]
fn closure_as_function_argument() {
    // Pass a closure to a higher-order function.
    let src = "\
fn apply(f: (Int) -> Int, x: Int) -> Int:
    ret f(x)

fn main():
    let result = apply(|x: Int| x + 1, 10)
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// @test annotation validation
// ---------------------------------------------------------------------------

#[test]
fn test_annotation_valid_bool_return() {
    let src = "\
@test
fn test_add() -> Bool:
    1 + 1 == 2
";
    assert_no_errors(src);
}

#[test]
fn test_annotation_valid_unit_return() {
    let src = "\
@test
fn test_unit():
    let x: Int = 1
";
    assert_no_errors(src);
}

#[test]
fn test_annotation_rejects_params() {
    let src = "\
@test
fn test_bad(x: Int) -> Bool:
    x == 1
";
    assert_error_contains(src, "@test function 'test_bad' must take no parameters");
}

#[test]
fn test_annotation_rejects_non_bool_non_unit_return() {
    let src = "\
@test
fn test_bad() -> Int:
    42
";
    assert_error_contains(src, "@test function 'test_bad' must return () or Bool");
}

#[test]
fn test_annotation_rejects_string_return() {
    let src = "\
@test
fn test_bad() -> String:
    \"hello\"
";
    assert_error_contains(src, "@test function 'test_bad' must return () or Bool");
}

// ---------------------------------------------------------------------------
// Tuple types
// ---------------------------------------------------------------------------

#[test]
fn tuple_literal_basic() {
    let src = "\
fn f() -> (Int, Int):
    ret (1, 2)
";
    assert_no_errors(src);
}

#[test]
fn tuple_type_annotation() {
    let src = "\
fn f():
    let pair: (Int, String) = (42, \"hello\")
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn tuple_field_access_first() {
    let src = "\
fn f() -> Int:
    let pair = (10, 20)
    ret pair.0
";
    assert_no_errors(src);
}

#[test]
fn tuple_field_access_second() {
    let src = "\
fn f() -> Int:
    let pair = (10, 20)
    ret pair.1
";
    assert_no_errors(src);
}

#[test]
fn tuple_field_access_out_of_bounds() {
    let src = "\
fn f() -> Int:
    let pair = (10, 20)
    ret pair.5
";
    assert_error_contains(src, "tuple index `5` out of bounds");
}

#[test]
fn tuple_field_access_on_non_tuple() {
    let src = "\
fn f() -> Int:
    let x = 42
    ret x.0
";
    assert_error_contains(src, "tuple field access `.0` on non-tuple type");
}

#[test]
fn tuple_destructuring_basic() {
    let src = "\
fn f() -> Int:
    let (a, b) = (1, 2)
    ret a
";
    assert_no_errors(src);
}

#[test]
fn tuple_destructuring_wrong_count() {
    let src = "\
fn f():
    let (a, b, c) = (1, 2)
    ret ()
";
    assert_error_contains(
        src,
        "tuple destructuring has 3 names but the tuple has 2 elements",
    );
}

#[test]
fn tuple_destructuring_non_tuple() {
    let src = "\
fn f():
    let (a, b) = 42
    ret ()
";
    assert_error_contains(src, "cannot destructure non-tuple type");
}

#[test]
fn tuple_three_elements() {
    let src = "\
fn f() -> Bool:
    let triple = (1, \"hello\", true)
    ret triple.2
";
    assert_no_errors(src);
}

#[test]
fn tuple_type_mismatch_in_annotation() {
    let src = "\
fn f():
    let pair: (Int, Int) = (1, \"hello\")
    ret ()
";
    assert_error_contains(src, "type mismatch");
}

// ---------------------------------------------------------------------------
// Trait declarations and impl blocks
// ---------------------------------------------------------------------------

#[test]
fn trait_decl_no_errors() {
    let src = "\
trait Display:
    fn display(self) -> String
";
    assert_no_errors(src);
}

#[test]
fn trait_decl_multiple_methods_no_errors() {
    let src = "\
trait Eq:
    fn equals(self, other: Int) -> Bool
    fn not_equals(self, other: Int) -> Bool
";
    assert_no_errors(src);
}

#[test]
fn impl_block_satisfies_trait() {
    let src = "\
trait Display:
    fn display(self) -> String

impl Display for Int:
    fn display(self) -> String:
        ret int_to_string(self)
";
    assert_no_errors(src);
}

#[test]
fn impl_block_missing_method() {
    let src = "\
trait Eq:
    fn equals(self, other: Int) -> Bool
    fn not_equals(self, other: Int) -> Bool

impl Eq for Int:
    fn equals(self, other: Int) -> Bool:
        ret self == other
";
    assert_error_contains(src, "missing method `not_equals`");
}

#[test]
fn impl_block_wrong_return_type() {
    let src = "\
trait Display:
    fn display(self) -> String

impl Display for Int:
    fn display(self) -> Int:
        ret 42
";
    assert_error_contains(src, "returns `Int`, expected `String`");
}

#[test]
fn impl_block_wrong_param_type() {
    let src = "\
trait Eq:
    fn equals(self, other: Int) -> Bool

impl Eq for Int:
    fn equals(self, other: String) -> Bool:
        ret true
";
    assert_error_contains(src, "parameter `other`");
}

#[test]
fn impl_block_extra_method() {
    let src = "\
trait Display:
    fn display(self) -> String

impl Display for Int:
    fn display(self) -> String:
        ret int_to_string(self)
    fn extra(self) -> Int:
        ret 42
";
    assert_error_contains(src, "not defined in trait");
}

#[test]
fn impl_block_unknown_trait() {
    let src = "\
impl UnknownTrait for Int:
    fn display(self) -> String:
        ret int_to_string(self)
";
    assert_error_contains(src, "unknown trait `UnknownTrait`");
}

#[test]
fn trait_bound_on_generic_function() {
    let src = "\
trait Display:
    fn display(self) -> String

fn print_value[T: Display](x: T) -> String:
    ret \"hello\"
";
    assert_no_errors(src);
}

#[test]
fn impl_block_with_self_type_in_trait() {
    // Self in trait method signature resolves to the target type in impl.
    let src = "\
trait Eq:
    fn equals(self, other: Self) -> Bool

impl Eq for Int:
    fn equals(self, other: Int) -> Bool:
        ret self == other
";
    assert_no_errors(src);
}
// ---------------------------------------------------------------------------
// Built-in Result type and ? operator
// ---------------------------------------------------------------------------

#[test]
fn result_ok_constructor() {
    // Ok(42) should type-check as a call to a built-in constructor.
    let src = "\
fn f() -> Result:
    ret Ok(42)
";
    assert_no_errors(src);
}

#[test]
fn result_err_constructor() {
    // Err("oops") should type-check as a call to a built-in constructor.
    let src = "\
fn f() -> Result:
    ret Err(\"oops\")
";
    assert_no_errors(src);
}

#[test]
fn result_type_annotation() {
    // Result[Int, String] should resolve as a known enum type.
    let src = "\
fn f(r: Result[Int, String]) -> Bool:
    ret is_ok(r)
";
    assert_no_errors(src);
}

#[test]
fn try_operator_on_result() {
    // ? on a Result value should type-check when the function returns Result.
    let src = "\
fn inner() -> Result:
    ret Ok(1)

fn outer() -> Result:
    let x = inner()?
    ret Ok(x)
";
    assert_no_errors(src);
}

#[test]
fn try_operator_on_non_result_is_error() {
    // ? applied to a non-Result type should be a type error.
    let src = "\
fn f() -> Result:
    let x: Int = 42
    ret Ok(x?)
";
    assert_error_contains(src, "can only be applied to `Result`");
}

#[test]
fn try_operator_in_non_result_function_is_error() {
    // ? used in a function that doesn't return Result should be a type error.
    let src = "\
fn inner() -> Result:
    ret Ok(1)

fn outer() -> Int:
    let x = inner()?
    ret x
";
    assert_error_contains(src, "enclosing function to return `Result`");
}

#[test]
fn is_ok_builtin() {
    // is_ok should accept a Result and return Bool.
    let src = "\
fn f() -> Bool:
    let r = Ok(42)
    ret is_ok(r)
";
    assert_no_errors(src);
}

#[test]
fn is_err_builtin() {
    // is_err should accept a Result and return Bool.
    let src = "\
fn f() -> Bool:
    let r = Err(\"oops\")
    ret is_err(r)
";
    assert_no_errors(src);
}

#[test]
fn builtin_option_some_constructor() {
    // Some(42) should type-check against the built-in Option type.
    let src = "\
fn f() -> Option:
    ret Some(42)
";
    assert_no_errors(src);
}

#[test]
fn builtin_option_none_value() {
    // None should type-check as a built-in Option value.
    let src = "\
fn f() -> Option:
    ret None
";
    assert_no_errors(src);
}

#[test]
fn match_on_result_type() {
    // Pattern matching on Ok/Err variants of the built-in Result type.
    let src = "\
fn handle(r: Result) -> Int:
    match r:
        Ok(v):
            ret v
        Err(e):
            ret 0
";
    assert_no_errors(src);
}

#[test]
fn try_operator_extracts_ok_type() {
    // The result of expr? should be the Ok inner type (TypeVar in v0.1).
    // The value can be passed through to another Ok constructor.
    let src = "\
fn get() -> Result:
    ret Ok(10)

fn use_it() -> Result:
    let n = get()?
    ret Ok(n)
";
    assert_no_errors(src);
}
// ---------------------------------------------------------------------------
// List type
// ---------------------------------------------------------------------------

#[test]
fn list_literal_type_inference() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_literal_with_annotation() {
    let src = "\
fn f():
    let nums: List[Int] = [1, 2, 3]
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_empty_with_annotation() {
    let src = "\
fn f():
    let empty: List[Int] = []
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_element_type_mismatch() {
    let src = "\
fn f():
    let bad = [1, \"hello\", 3]
    ret ()
";
    assert_error_contains(src, "list element type mismatch");
}

#[test]
fn list_annotation_mismatch() {
    let src = "\
fn f():
    let nums: List[String] = [1, 2, 3]
    ret ()
";
    assert_error_contains(src, "type mismatch");
}

#[test]
fn list_length_type_check() {
    let src = "\
fn f() -> Int:
    let nums = [1, 2, 3]
    ret list_length(nums)
";
    assert_no_errors(src);
}

#[test]
fn list_length_wrong_arg_type() {
    let src = "\
fn f() -> Int:
    ret list_length(42)
";
    assert_error_contains(src, "expected a List type");
}

#[test]
fn list_get_type_check() {
    let src = "\
fn f() -> Int:
    let nums = [10, 20, 30]
    ret list_get(nums, 0)
";
    assert_no_errors(src);
}

#[test]
fn list_get_returns_element_type() {
    let src = "\
fn f() -> Int:
    let nums = [10, 20, 30]
    let first = list_get(nums, 0)
    ret first + 1
";
    assert_no_errors(src);
}

#[test]
fn list_push_type_check() {
    let src = "\
fn f():
    let nums = [1, 2]
    let nums2 = list_push(nums, 3)
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_push_wrong_element_type() {
    let src = "\
fn f():
    let nums = [1, 2]
    let nums2 = list_push(nums, \"hello\")
    ret ()
";
    assert_error_contains(src, "expected `Int`, found `String`");
}

#[test]
fn list_concat_type_check() {
    let src = "\
fn f():
    let a = [1, 2]
    let b = [3, 4]
    let c = list_concat(a, b)
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_is_empty_type_check() {
    let src = "\
fn f() -> Bool:
    let nums = [1]
    ret list_is_empty(nums)
";
    assert_no_errors(src);
}

#[test]
fn list_head_type_check() {
    let src = "\
fn f() -> Int:
    let nums = [10, 20]
    ret list_head(nums)
";
    assert_no_errors(src);
}

#[test]
fn list_tail_type_check() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let rest = list_tail(nums)
    let len = list_length(rest)
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_contains_type_check() {
    let src = "\
fn f() -> Bool:
    let nums = [1, 2, 3]
    ret list_contains(nums, 2)
";
    assert_no_errors(src);
}

#[test]
fn list_contains_wrong_element_type() {
    let src = "\
fn f() -> Bool:
    let nums = [1, 2, 3]
    ret list_contains(nums, \"hello\")
";
    assert_error_contains(src, "expected `Int`, found `String`");
}

#[test]
fn list_type_display() {
    // Verify that the Ty::List Display impl works correctly
    let ty = Ty::List(Box::new(Ty::Int));
    assert_eq!(format!("{}", ty), "List[Int]");
    let nested = Ty::List(Box::new(Ty::List(Box::new(Ty::String))));
    assert_eq!(format!("{}", nested), "List[List[String]]");
}
// ---------------------------------------------------------------------------
// String interpolation
// ---------------------------------------------------------------------------

#[test]
fn interp_string_with_string_var() {
    let src = "\
fn greet(name: String) -> String:
    ret f\"hello {name}\"
";
    assert_no_errors(src);
}

#[test]
fn interp_string_with_int_expr() {
    let src = "\
fn show(n: Int) -> String:
    ret f\"count = {n}\"
";
    assert_no_errors(src);
}

#[test]
fn interp_string_with_float_expr() {
    let src = "\
fn show(x: Float) -> String:
    ret f\"value = {x}\"
";
    assert_no_errors(src);
}

#[test]
fn interp_string_with_bool_expr() {
    let src = "\
fn show(flag: Bool) -> String:
    ret f\"flag is {flag}\"
";
    assert_no_errors(src);
}

#[test]
fn interp_string_result_is_string_type() {
    // Assigning an f-string to a String variable should work.
    let src = "\
fn f() -> String:
    let x = 42
    let s: String = f\"answer is {x}\"
    ret s
";
    assert_no_errors(src);
}

#[test]
fn interp_string_invalid_type_error() {
    // Unit type cannot be interpolated.
    let src = "\
fn f():
    let u = ()
    let s = f\"value is {u}\"
    ret ()
";
    assert_error_contains(src, "cannot be interpolated");
}

// ---------------------------------------------------------------------------
// Higher-order list functions
// ---------------------------------------------------------------------------

#[test]
fn list_map_type_inference() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let doubled = list_map(nums, |x: Int| x * 2)
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_map_returns_transformed_type() {
    let src = "\
fn f() -> Bool:
    let nums = [1, 2, 3]
    let bools = list_map(nums, |x: Int| x > 0)
    ret list_get(bools, 0)
";
    assert_no_errors(src);
}

#[test]
fn list_map_wrong_closure_param_type() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let bad = list_map(nums, |x: String| x)
    ret ()
";
    assert_error_contains(src, "closure parameter type");
}

#[test]
fn list_map_non_list_arg() {
    let src = "\
fn f():
    let bad = list_map(42, |x: Int| x)
    ret ()
";
    assert_error_contains(src, "expected a List type");
}

#[test]
fn list_map_non_function_arg() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let bad = list_map(nums, 42)
    ret ()
";
    assert_error_contains(src, "expected a function type");
}

#[test]
fn list_filter_type_check() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let evens = list_filter(nums, |x: Int| x > 1)
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_filter_preserves_list_type() {
    let src = "\
fn f() -> Int:
    let nums = [1, 2, 3]
    let evens = list_filter(nums, |x: Int| x > 1)
    ret list_get(evens, 0)
";
    assert_no_errors(src);
}

#[test]
fn list_filter_wrong_return_type() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let bad = list_filter(nums, |x: Int| x + 1)
    ret ()
";
    assert_error_contains(src, "closure must return Bool");
}

#[test]
fn list_foreach_type_check() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    list_foreach(nums, |x: Int| x + 1)
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_fold_type_check() {
    let src = "\
fn f() -> Int:
    let nums = [1, 2, 3]
    ret list_fold(nums, 0, |acc: Int, x: Int| acc + x)
";
    assert_no_errors(src);
}

#[test]
fn list_fold_wrong_accumulator_type() {
    let src = "\
fn f() -> Int:
    let nums = [1, 2, 3]
    ret list_fold(nums, 0, |acc: String, x: Int| acc)
";
    assert_error_contains(src, "does not match accumulator type");
}

#[test]
fn list_fold_wrong_element_param() {
    let src = "\
fn f() -> Int:
    let nums = [1, 2, 3]
    ret list_fold(nums, 0, |acc: Int, x: String| acc)
";
    assert_error_contains(src, "does not match list element type");
}

#[test]
fn list_any_type_check() {
    let src = "\
fn f() -> Bool:
    let nums = [1, 2, 3]
    ret list_any(nums, |x: Int| x > 2)
";
    assert_no_errors(src);
}

#[test]
fn list_all_type_check() {
    let src = "\
fn f() -> Bool:
    let nums = [1, 2, 3]
    ret list_all(nums, |x: Int| x > 0)
";
    assert_no_errors(src);
}

#[test]
fn list_any_wrong_return_type() {
    let src = "\
fn f() -> Bool:
    let nums = [1, 2, 3]
    ret list_any(nums, |x: Int| x + 1)
";
    assert_error_contains(src, "closure must return Bool");
}

#[test]
fn list_find_type_check() {
    let src = "\
fn f() -> Int:
    let nums = [1, 2, 3]
    ret list_find(nums, |x: Int| x > 1)
";
    assert_no_errors(src);
}

#[test]
fn list_find_returns_element_type() {
    let src = "\
fn f() -> Int:
    let nums = [10, 20, 30]
    let found = list_find(nums, |x: Int| x > 15)
    ret found + 1
";
    assert_no_errors(src);
}

#[test]
fn list_sort_type_check() {
    let src = "\
fn f():
    let nums = [3, 1, 2]
    let sorted = list_sort(nums)
    ret ()
";
    assert_no_errors(src);
}

#[test]
fn list_sort_rejects_non_int_list() {
    let src = "\
fn f():
    let bools = [true, false]
    let bad = list_sort(bools)
    ret ()
";
    assert_error_contains(src, "expected List[Int]");
}

#[test]
fn list_reverse_type_check() {
    let src = "\
fn f() -> Int:
    let nums = [1, 2, 3]
    let rev = list_reverse(nums)
    ret list_get(rev, 0)
";
    assert_no_errors(src);
}

#[test]
fn list_map_wrong_arg_count() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let bad = list_map(nums)
    ret ()
";
    assert_error_contains(src, "expects 2 argument(s)");
}

#[test]
fn list_fold_wrong_arg_count() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let bad = list_fold(nums, 0)
    ret ()
";
    assert_error_contains(src, "expects 3 argument(s)");
}
// =========================================================================
// Method call syntax tests
// =========================================================================

#[test]
fn method_string_length() {
    let src = "\
fn f() -> Int:
    ret \"hello\".length()
";
    assert_no_errors(src);
}

#[test]
fn method_string_contains() {
    let src = "\
fn f() -> Bool:
    ret \"hello world\".contains(\"world\")
";
    assert_no_errors(src);
}

#[test]
fn method_string_starts_with() {
    let src = "\
fn f() -> Bool:
    ret \"hello\".starts_with(\"he\")
";
    assert_no_errors(src);
}

#[test]
fn method_string_trim() {
    let src = "\
fn f() -> String:
    ret \"  hello  \".trim()
";
    assert_no_errors(src);
}

#[test]
fn method_string_to_upper() {
    let src = "\
fn f() -> String:
    ret \"hello\".to_upper()
";
    assert_no_errors(src);
}

#[test]
fn method_list_length() {
    let src = "\
fn f() -> Int:
    let xs = [1, 2, 3]
    ret xs.length()
";
    assert_no_errors(src);
}

#[test]
fn method_list_push() {
    let src = "\
fn f() -> List[Int]:
    let xs = [1, 2, 3]
    ret xs.push(4)
";
    assert_no_errors(src);
}

#[test]
fn method_list_is_empty() {
    let src = "\
fn f() -> Bool:
    let xs = [1, 2, 3]
    ret xs.is_empty()
";
    assert_no_errors(src);
}

#[test]
fn method_list_get() {
    let src = "\
fn f() -> Int:
    let xs = [1, 2, 3]
    ret xs.get(0)
";
    assert_no_errors(src);
}

#[test]
fn method_chained_string_trim_length() {
    let src = "\
fn f() -> Int:
    ret \"  hello  \".trim().length()
";
    assert_no_errors(src);
}

#[test]
fn method_chained_string_to_upper_contains() {
    let src = "\
fn f() -> Bool:
    ret \"hello\".to_upper().contains(\"HELLO\")
";
    assert_no_errors(src);
}

#[test]
fn method_unknown_method_error() {
    let src = "\
fn f() -> Int:
    ret \"hello\".nonexistent()
";
    assert_error_contains(src, "has no method `nonexistent`");
}

#[test]
fn method_unknown_method_on_int() {
    let src = "\
fn f() -> Int:
    let x = 42
    ret x.foo()
";
    assert_error_contains(src, "has no method `foo`");
}

#[test]
fn method_trait_impl_dispatch() {
    let src = "\
trait Display:
    fn display(self) -> String

impl Display for Int:
    fn display(self) -> String:
        ret int_to_string(self)

fn f() -> String:
    let x = 42
    ret x.display()
";
    assert_no_errors(src);
}

#[test]
fn method_trait_impl_missing_method_error() {
    // Bool does not implement Display in this program.
    let src = "\
trait Display:
    fn display(self) -> String

impl Display for Int:
    fn display(self) -> String:
        ret int_to_string(self)

fn f() -> String:
    let b = true
    ret b.display()
";
    assert_error_contains(src, "has no method `display`");
}

#[test]
fn method_string_replace() {
    let src = "\
fn f() -> String:
    ret \"hello world\".replace(\"world\", \"there\")
";
    assert_no_errors(src);
}

#[test]
fn method_string_substring() {
    let src = "\
fn f() -> String:
    ret \"hello\".substring(0, 3)
";
    assert_no_errors(src);
}

#[test]
fn method_string_index_of() {
    let src = "\
fn f() -> Int:
    ret \"hello\".index_of(\"ll\")
";
    assert_no_errors(src);
}

#[test]
fn method_string_ends_with() {
    let src = "\
fn f() -> Bool:
    ret \"hello\".ends_with(\"lo\")
";
    assert_no_errors(src);
}

#[test]
fn method_list_head() {
    let src = "\
fn f() -> Int:
    let xs = [1, 2, 3]
    ret xs.head()
";
    assert_no_errors(src);
}

#[test]
fn method_list_tail() {
    let src = "\
fn f() -> List[Int]:
    let xs = [1, 2, 3]
    ret xs.tail()
";
    assert_no_errors(src);
}

#[test]
fn method_on_variable_string() {
    let src = "\
fn greet(name: String) -> Int:
    ret name.length()
";
    assert_no_errors(src);
}

#[test]
fn method_wrong_arg_type() {
    let src = "\
fn f() -> Bool:
    ret \"hello\".contains(42)
";
    assert_error_contains(src, "expected `String`, found `Int`");
}
// ---------------------------------------------------------------------------
// Pipe operator (|>)
// ---------------------------------------------------------------------------

#[test]
fn pipe_simple_function_call() {
    // x |> f desugars to f(x), where f: Int -> Int.
    let src = "\
fn double(x: Int) -> Int:
    ret x + x

fn main() -> Int:
    ret 5 |> double
";
    assert_no_errors(src);
}

#[test]
fn pipe_chained() {
    // x |> f |> g desugars to g(f(x)).
    let src = "\
fn double(x: Int) -> Int:
    ret x + x

fn negate(x: Int) -> Int:
    ret 0 - x

fn main() -> Int:
    ret 5 |> double |> negate
";
    assert_no_errors(src);
}

#[test]
fn pipe_type_mismatch() {
    // Pipe feeds Int to a function expecting String.
    let src = "\
fn greet(name: String) -> String:
    ret name

fn main() -> String:
    ret 42 |> greet
";
    assert_error_contains(src, "expected `String`, found `Int`");
}

#[test]
fn pipe_with_closure() {
    // x |> (|y: Int| y + 1) should also work.
    let src = "\
fn main() -> Int:
    ret 10 |> |y: Int| -> Int y + 1
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Range expressions
// ---------------------------------------------------------------------------

#[test]
fn range_expression_valid() {
    let src = "\
fn f() -> ():
    let r = 0..10
";
    assert_no_errors(src);
}

#[test]
fn range_requires_int_start() {
    let src = "\
fn f() -> ():
    let r = 3.14..10
";
    assert_error_contains(src, "range start must be `Int`");
}

#[test]
fn range_requires_int_end() {
    let src = "\
fn f() -> ():
    let r = 0..true
";
    assert_error_contains(src, "range end must be `Int`");
}

// ---------------------------------------------------------------------------
// For-in over lists
// ---------------------------------------------------------------------------

#[test]
fn for_in_list_infers_element_type() {
    // The loop variable should get Int type from iterating over List[Int].
    let src = "\
fn f() -> Int:
    let nums: List[Int] = [1, 2, 3]
    let total: Int = 0
    for x in nums:
        let y: Int = x
    ret total
";
    assert_no_errors(src);
}

#[test]
fn for_in_list_literal() {
    let src = "\
fn f() -> ():
    for x in [10, 20, 30]:
        let y: Int = x
";
    assert_no_errors(src);
}

#[test]
fn for_in_over_non_iterable_errors() {
    let src = "\
fn f() -> ():
    for x in 42:
        println(x)
";
    assert_error_contains(src, "cannot iterate over type `Int`");
}

#[test]
fn for_in_over_string_errors() {
    let src = "\
fn f() -> ():
    for x in \"hello\":
        println(x)
";
    assert_error_contains(src, "cannot iterate over type `String`");
}

#[test]
fn for_in_over_bool_errors() {
    let src = "\
fn f() -> ():
    for x in true:
        println(x)
";
    assert_error_contains(src, "cannot iterate over type `Bool`");
}

// ---------------------------------------------------------------------------
// For-in over ranges
// ---------------------------------------------------------------------------

#[test]
fn for_in_range_gives_int() {
    let src = "\
fn f() -> ():
    for i in 0..10:
        let x: Int = i
";
    assert_no_errors(src);
}

#[test]
fn for_in_range_backward_compat() {
    // Legacy range() still works.
    let src = "\
fn f() -> ():
    for i in range(10):
        let x: Int = i
";
    assert_no_errors(src);
}
// ---------------------------------------------------------------------------
// Match guards
// ---------------------------------------------------------------------------

#[test]
fn match_guard_basic() {
    let src = "\
fn classify(n: Int) -> String:
    match n:
        x if x > 0:
            ret \"positive\"
        x if x == 0:
            ret \"zero\"
        _:
            ret \"negative\"
";
    assert_no_errors(src);
}

#[test]
fn match_guard_must_be_bool() {
    let src = "\
fn bad(n: Int) -> String:
    match n:
        x if x + 1:
            ret \"oops\"
        _:
            ret \"ok\"
";
    assert_error_contains(src, "match guard must be a `Bool` expression");
}

#[test]
fn match_guard_variable_bound_to_scrutinee_type() {
    // The variable `x` should be bound to Int (the scrutinee type),
    // so `x > 0` is valid.
    let src = "\
fn f(n: Int) -> Int:
    match n:
        x if x > 0:
            ret x
        _:
            ret 0
";
    assert_no_errors(src);
}

#[test]
fn match_guard_with_variant_pattern() {
    let src = "\
type Option = Some(Int) | None

fn describe(opt: Option) -> String:
    match opt:
        Some(x) if x > 10:
            ret \"big\"
        Some(x):
            ret \"small\"
        None:
            ret \"nothing\"
";
    assert_no_errors(src);
}

#[test]
fn match_guard_on_wildcard() {
    let src = "\
fn f(n: Int) -> String:
    match n:
        0:
            ret \"zero\"
        _ if true:
            ret \"non-zero\"
        _:
            ret \"fallback\"
";
    assert_no_errors(src);
}

#[test]
fn match_guard_multiple_guarded_arms() {
    let src = "\
fn f(n: Int) -> String:
    match n:
        x if x > 100:
            ret \"big\"
        x if x > 10:
            ret \"medium\"
        x if x > 0:
            ret \"small\"
        _:
            ret \"non-positive\"
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// String patterns
// ---------------------------------------------------------------------------

#[test]
fn match_string_pattern_basic() {
    let src = "\
fn greet(name: String) -> String:
    match name:
        \"Alice\":
            ret \"Hi Alice\"
        \"Bob\":
            ret \"Hi Bob\"
        _:
            ret \"Hello stranger\"
";
    assert_no_errors(src);
}

#[test]
fn match_string_pattern_on_int_scrutinee() {
    let src = "\
fn bad(n: Int) -> String:
    match n:
        \"hello\":
            ret \"oops\"
        _:
            ret \"ok\"
";
    assert_error_contains(src, "string pattern cannot match scrutinee of type `Int`");
}

// ---------------------------------------------------------------------------
// Variable binding patterns
// ---------------------------------------------------------------------------

#[test]
fn match_variable_binding_no_guard() {
    // A variable pattern without a guard is like a wildcard that also binds.
    let src = "\
fn f(n: Int) -> Int:
    match n:
        x:
            ret x
";
    assert_no_errors(src);
}

#[test]
fn match_variable_binding_exhaustive() {
    // A variable pattern without a guard should be considered exhaustive.
    let src = "\
fn f(n: Int) -> Int:
    match n:
        0:
            ret 0
        x:
            ret x
";
    assert_no_errors(src);
}
// ---------------------------------------------------------------------------
// Match exhaustiveness checking
// ---------------------------------------------------------------------------

#[test]
fn exhaustive_enum_missing_one_variant() {
    // Missing a single variant should produce an error naming it.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        Green:
            ret \"green\"
";
    assert_error_contains(src, "non-exhaustive match on `Color`");
    assert_error_contains(src, "Blue");
}

#[test]
fn exhaustive_enum_missing_multiple_variants() {
    // Missing multiple variants should list them all.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
";
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
    let msg = errors
        .iter()
        .find(|e| e.message.contains("non-exhaustive"))
        .unwrap();
    assert!(
        msg.message.contains("Green"),
        "should mention Green: {}",
        msg.message
    );
    assert!(
        msg.message.contains("Blue"),
        "should mention Blue: {}",
        msg.message
    );
}

#[test]
fn exhaustive_enum_complete_coverage_no_error() {
    // All enum variants covered — no error.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        Green:
            ret \"green\"
        Blue:
            ret \"blue\"
";
    assert_no_errors(src);
}

#[test]
fn exhaustive_enum_wildcard_covers_all() {
    // Wildcard should make the match exhaustive even if some variants are missing.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        _:
            ret \"other\"
";
    assert_no_errors(src);
}

#[test]
fn exhaustive_bool_missing_true() {
    // Bool match missing `true` should error.
    let src = "\
fn f(b: Bool) -> String:
    match b:
        false:
            ret \"no\"
";
    assert_error_contains(src, "non-exhaustive match on `Bool`");
    assert_error_contains(src, "true");
}

#[test]
fn exhaustive_bool_missing_false() {
    // Bool match missing `false` should error.
    let src = "\
fn f(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
";
    assert_error_contains(src, "non-exhaustive match on `Bool`");
    assert_error_contains(src, "false");
}

#[test]
fn exhaustive_bool_both_covered() {
    // Bool match with both true and false — no error.
    let src = "\
fn f(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
        false:
            ret \"no\"
";
    assert_no_errors(src);
}

#[test]
fn exhaustive_bool_with_wildcard() {
    // Bool match with wildcard — no error.
    let src = "\
fn f(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
        _:
            ret \"no\"
";
    assert_no_errors(src);
}

#[test]
fn exhaustive_bool_missing_both() {
    // Bool match with neither true nor false (only int patterns, which are type errors too).
    // This is a contrived case — use an empty-ish match.
    let src = "\
fn f(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
";
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
    assert!(errors
        .iter()
        .any(|e| e.message.contains("non-exhaustive") && e.message.contains("false")));
}

#[test]
fn exhaustive_option_complete() {
    // Built-in Option: Some + None is exhaustive.
    let src = "\
fn unwrap_or(o: Option, default: Int) -> Int:
    match o:
        Some(x):
            ret x
        None:
            ret default
";
    assert_no_errors(src);
}

#[test]
fn exhaustive_option_missing_none() {
    // Built-in Option: missing None should error.
    let src = "\
fn unwrap(o: Option) -> Int:
    match o:
        Some(x):
            ret x
";
    assert_error_contains(src, "non-exhaustive match on `Option`");
    assert_error_contains(src, "None");
}

#[test]
fn exhaustive_result_missing_err() {
    // Built-in Result: missing Err should error.
    let src = "\
fn get_value(r: Result) -> Int:
    match r:
        Ok(v):
            ret v
";
    assert_error_contains(src, "non-exhaustive match on `Result`");
    assert_error_contains(src, "Err");
}

#[test]
fn exhaustive_result_complete() {
    // Built-in Result: Ok + Err is exhaustive.
    let src = "\
fn handle(r: Result) -> Int:
    match r:
        Ok(v):
            ret v
        Err(e):
            ret 0
";
    assert_no_errors(src);
}

#[test]
fn unreachable_wildcard_before_last() {
    // Wildcard before the last arm should produce a warning about unreachable patterns.
    let src = "\
fn f(n: Int) -> String:
    match n:
        0:
            ret \"zero\"
        _:
            ret \"other\"
        1:
            ret \"one\"
";
    assert_warning_contains(src, "unreachable pattern");
    assert_warning_contains(src, "wildcard");
}

#[test]
fn unreachable_duplicate_variant() {
    // Matching the same variant twice should produce a warning.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        Red:
            ret \"RED\"
        Green:
            ret \"green\"
        Blue:
            ret \"blue\"
";
    assert_warning_contains(src, "unreachable pattern");
    assert_warning_contains(src, "Red");
}

#[test]
fn unreachable_duplicate_bool_true() {
    // Matching `true` twice should produce a warning.
    let src = "\
fn f(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
        true:
            ret \"also yes\"
        false:
            ret \"no\"
";
    assert_warning_contains(src, "unreachable pattern");
    assert_warning_contains(src, "true");
}

#[test]
fn unreachable_multiple_arms_after_wildcard() {
    // Multiple arms after a wildcard should all be warned.
    let src = "\
fn f(n: Int) -> String:
    match n:
        _:
            ret \"anything\"
        0:
            ret \"zero\"
        1:
            ret \"one\"
";
    let all = check(src);
    let warnings: Vec<_> = all.iter().filter(|e| e.is_warning).collect();
    assert!(
        warnings.len() >= 2,
        "expected at least 2 warnings, got {}",
        warnings.len()
    );
}

#[test]
fn exhaustive_error_has_note() {
    // The exhaustiveness error should include a helpful note.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
";
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
    let err = errors
        .iter()
        .find(|e| e.message.contains("non-exhaustive"))
        .unwrap();
    assert!(
        !err.notes.is_empty(),
        "exhaustiveness error should have a note"
    );
    assert!(
        err.notes[0].contains("wildcard") || err.notes[0].contains("missing"),
        "note should mention wildcard or missing: {:?}",
        err.notes
    );
}

#[test]
fn exhaustive_error_json_structured() {
    // The exhaustiveness error should produce structured JSON.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
";
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
    let err = errors
        .iter()
        .find(|e| e.message.contains("non-exhaustive"))
        .unwrap();
    let json = err.to_json();
    assert!(
        json.contains("\"severity\": \"error\""),
        "JSON should have error severity"
    );
    assert!(
        json.contains("non-exhaustive"),
        "JSON should contain error message"
    );
}

#[test]
fn warning_json_has_warning_severity() {
    // Warnings should produce JSON with "warning" severity.
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> String:
    match c:
        Red:
            ret \"red\"
        Red:
            ret \"RED\"
        Green:
            ret \"green\"
        Blue:
            ret \"blue\"
";
    let all = check(src);
    let warnings: Vec<_> = all.iter().filter(|e| e.is_warning).collect();
    assert!(!warnings.is_empty(), "should have at least one warning");
    let json = warnings[0].to_json();
    assert!(
        json.contains("\"severity\": \"warning\""),
        "JSON should have warning severity"
    );
}

// ---------------------------------------------------------------------------
// Phase NN: File I/O builtins (FS effect)
// ---------------------------------------------------------------------------

#[test]
fn file_read_requires_fs_effect() {
    // Calling file_read from a pure function must produce a type error.
    let src = "\
fn read_without_fs() -> String:
    ret file_read(\"/tmp/test.txt\")
";
    assert_error_contains(src, "requires effect `FS`");
}

#[test]
fn file_read_in_fs_context_ok() {
    // Calling file_read from a !{FS} function must succeed.
    let src = "\
fn read_with_fs(path: String) -> !{FS} String:
    ret file_read(path)
";
    assert_no_errors(src);
}

#[test]
fn file_read_in_io_context_fails() {
    // FS and IO are distinct effects; an !{IO} function cannot use file_read.
    let src = "\
fn bad_io() -> !{IO} String:
    ret file_read(\"/tmp/test.txt\")
";
    assert_error_contains(src, "requires effect `FS`");
}

#[test]
fn file_read_in_io_fs_context_ok() {
    // A function declaring both IO and FS may call file_read.
    let src = "\
fn dual_effect() -> !{IO, FS} String:
    ret file_read(\"/tmp/test.txt\")
";
    assert_no_errors(src);
}

#[test]
fn file_write_requires_fs_effect() {
    // file_write from a pure context is a type error.
    let src = "\
fn write_pure() -> Bool:
    ret file_write(\"/tmp/out.txt\", \"data\")
";
    assert_error_contains(src, "requires effect `FS`");
}

#[test]
fn file_write_in_fs_context_ok() {
    let src = "\
fn write_with_fs() -> !{FS} Bool:
    ret file_write(\"/tmp/out.txt\", \"data\")
";
    assert_no_errors(src);
}

#[test]
fn file_write_returns_bool() {
    // The result type must be Bool; assigning to Int must fail.
    let src = "\
fn bad_type() -> !{FS} Int:
    ret file_write(\"/tmp/out.txt\", \"data\")
";
    assert_error_contains(src, "mismatch");
}

#[test]
fn file_exists_requires_fs_effect() {
    let src = "\
fn exists_pure() -> Bool:
    ret file_exists(\"/tmp/test.txt\")
";
    assert_error_contains(src, "requires effect `FS`");
}

#[test]
fn file_exists_in_fs_context_ok() {
    let src = "\
fn exists_fs() -> !{FS} Bool:
    ret file_exists(\"/tmp/test.txt\")
";
    assert_no_errors(src);
}

#[test]
fn file_exists_returns_bool() {
    // file_exists result must type-check as Bool.
    let src = "\
fn check(path: String) -> !{FS} Bool:
    let ok: Bool = file_exists(path)
    ret ok
";
    assert_no_errors(src);
}

#[test]
fn file_append_requires_fs_effect() {
    let src = "\
fn append_pure() -> Bool:
    ret file_append(\"/tmp/log.txt\", \"line\")
";
    assert_error_contains(src, "requires effect `FS`");
}

#[test]
fn file_append_in_fs_context_ok() {
    let src = "\
fn append_fs() -> !{FS} Bool:
    ret file_append(\"/tmp/log.txt\", \"line\")
";
    assert_no_errors(src);
}

#[test]
fn cap_io_cannot_use_file_read() {
    // A module with @cap(IO) must not be allowed to use FS functions.
    let src = "\
@cap(IO)

fn bad() -> !{FS} String:
    ret file_read(\"/tmp/x\")
";
    assert_error_contains(src, "exceeds the module capability ceiling");
}

#[test]
fn cap_io_fs_can_use_file_read() {
    // A module with @cap(IO, FS) may declare FS functions.
    let src = "\
@cap(IO, FS)

fn ok(path: String) -> !{FS} String:
    ret file_read(path)
";
    assert_no_errors(src);
}

#[test]
fn file_io_combined_program() {
    // A realistic program using all four file builtins in a single function.
    let src = "\
fn run(path: String) -> !{IO, FS} ():
    let ok: Bool = file_write(path, \"hello\")
    let exists: Bool = file_exists(path)
    let content: String = file_read(path)
    let ok2: Bool = file_append(path, \" world\")
    print(content)
";
    assert_no_errors(src);
}

// ---------------------------------------------------------------------------
// Phase OO: Map[K, V] type tests
// ---------------------------------------------------------------------------

#[test]
fn map_string_string_is_valid_type() {
    // Map[String, String] is a valid type annotation.
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, String] = map_new()
";
    assert_no_errors(src);
}

#[test]
fn map_string_int_is_valid_type() {
    // Map[String, Int] is a valid type annotation.
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, Int] = map_new()
";
    assert_no_errors(src);
}

#[test]
fn map_new_returns_map() {
    // map_new() must type-check without errors.
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, String] = map_new()
";
    assert_no_errors(src);
}

#[test]
fn map_set_returns_map() {
    // map_set should type-check and return the same map type.
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, String] = map_new()
    let m2: Map[String, String] = map_set(m, \"key\", \"value\")
";
    assert_no_errors(src);
}

#[test]
fn map_set_wrong_first_arg_is_error() {
    // Passing a non-map to map_set must be an error.
    let src = "\
mod test
fn main() -> !{IO} ():
    let n: Int = 42
    let m2 = map_set(n, \"key\", \"value\")
";
    assert_error_contains(src, "map_set");
}

#[test]
fn map_size_returns_int() {
    // map_size should return Int.
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, String] = map_new()
    let sz: Int = map_size(m)
";
    assert_no_errors(src);
}

#[test]
fn map_contains_returns_bool() {
    // map_contains should return Bool.
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, String] = map_new()
    let has: Bool = map_contains(m, \"hello\")
";
    assert_no_errors(src);
}

#[test]
fn map_get_returns_option() {
    // map_get should return Option[String] for a Map[String, String].
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, String] = map_new()
    let m2: Map[String, String] = map_set(m, \"hello\", \"world\")
    let result: Option[String] = map_get(m2, \"hello\")
";
    assert_no_errors(src);
}

#[test]
fn map_remove_returns_map() {
    // map_remove should return the same map type.
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, String] = map_new()
    let m2: Map[String, String] = map_set(m, \"key\", \"val\")
    let m3: Map[String, String] = map_remove(m2, \"key\")
";
    assert_no_errors(src);
}

#[test]
fn map_keys_returns_list_string() {
    // map_keys should return List[String].
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: Map[String, String] = map_new()
    let ks: List[String] = map_keys(m)
";
    assert_no_errors(src);
}

#[test]
fn map_size_wrong_arg_is_error() {
    // Passing a non-map to map_size must be an error.
    let src = "\
mod test
fn main() -> !{IO} ():
    let n: Int = 42
    let sz: Int = map_size(n)
";
    assert_error_contains(src, "map_size");
}

#[test]
fn map_type_display() {
    // Ty::Map should display as "Map[String, Int]".
    let ty = crate::typechecker::types::Ty::Map(
        Box::new(crate::typechecker::types::Ty::String),
        Box::new(crate::typechecker::types::Ty::Int),
    );
    assert_eq!(format!("{}", ty), "Map[String, Int]");
}

// ============================================================================
// Self-Hosting Phase 1.1: HashMap Tests
// ============================================================================
//
// NOTE: Full HashMap type inference requires bidirectional type checking
// to flow expected types from annotations back to generic function calls.
// Currently, explicit type instantiation syntax (e.g., hashmap_new[String, Int]())
// is not supported by the parser. This is a known limitation.
//
// The HashMap type is implemented and usable via:
// 1. Type annotations: let m: HashMap[String, Int] = ...
// 2. Runtime support via C hashmap implementation
// 3. Type checker support for HashMap[K, V] type expressions
//
// Full generic instantiation will be addressed in a future update.

#[test]
fn hashmap_type_display() {
    // Ty::HashMap should display correctly
    let ty = crate::typechecker::types::Ty::HashMap(
        Box::new(crate::typechecker::types::Ty::String),
        Box::new(crate::typechecker::types::Ty::Int),
    );
    assert_eq!(format!("{}", ty), "HashMap[String, Int]");

    let ty2 = crate::typechecker::types::Ty::HashMap(
        Box::new(crate::typechecker::types::Ty::Int),
        Box::new(crate::typechecker::types::Ty::String),
    );
    assert_eq!(format!("{}", ty2), "HashMap[Int, String]");
}

#[test]
fn hashmap_function_signatures_exist() {
    // Verify that HashMap builtin functions are registered in the type environment
    // by checking that function references can be resolved
    let src = "\
mod test
fn use_hashmap_funcs():
    // These should resolve to their polymorphic function types
    let _ = hashmap_new
    let _ = hashmap_insert
    let _ = hashmap_get
    let _ = hashmap_remove
    let _ = hashmap_contains
    let _ = hashmap_len
    let _ = hashmap_clear
";
    assert_no_errors(src);
}

#[test]
fn hashmap_bidirectional_inference() {
    // Bidirectional type inference: annotation flows to infer type parameters
    let src = "\
mod test
fn main() -> !{IO} ():
    let m: HashMap[String, Int] = hashmap_new()
    let n = hashmap_len(m)
";
    assert_no_errors(src);
}

#[test]
fn list_iter_bidirectional_inference() {
    // Iterator type inference from context
    let src = "\
mod test
fn main() -> !{IO} ():
    let list = [1, 2, 3]
    let iter: Iterator[Int] = list_iter(list)
    let count = iter_count(iter)
";
    assert_no_errors(src);
}

// ============================================================================
// Self-Hosting Phase 1.2: Iterator Protocol Tests
// ============================================================================

#[test]
fn iterator_type_display() {
    // Ty::Iterator should display correctly
    let ty = crate::typechecker::types::Ty::Iterator(Box::new(crate::typechecker::types::Ty::Int));
    assert_eq!(format!("{}", ty), "Iterator[Int]");

    let ty2 =
        crate::typechecker::types::Ty::Iterator(Box::new(crate::typechecker::types::Ty::String));
    assert_eq!(format!("{}", ty2), "Iterator[String]");
}

#[test]
fn iterator_function_signatures_exist() {
    // Verify that Iterator builtin functions are registered in the type environment
    let src = "\
mod test
fn use_iter_funcs():
    // Core iterator functions
    let _ = list_iter
    let _ = range_iter
    let _ = iter_next
    let _ = iter_has_next
    let _ = iter_count
";
    assert_no_errors(src);
}

#[test]
fn list_iter_returns_iterator() {
    // list_iter should return an iterator
    // Note: full type inference for generic return types needs work
    let src = "\
mod test
fn use_list_iter(list: List[Int]):
    let iter = list_iter(list)
    // iter has type Iterator[T], we can use it
    let _ = iter
";
    assert_no_errors(src);
}

#[test]
fn range_iter_returns_iterator_int() {
    // range_iter should return Iterator[Int]
    let src = "\
mod test
fn use_range_iter():
    let iter = range_iter(0, 10)
    let _ = iter
";
    assert_no_errors(src);
}

#[test]
fn iter_functions_accept_iterator() {
    // iter_next, iter_has_next, iter_count should accept Iterator types
    // Note: Full type inference from generic arguments needs work
    let src = "\
mod test
fn use_iter_functions():
    // Functions exist and can be referenced
    let _ = iter_next
    let _ = iter_has_next
    let _ = iter_count
";
    assert_no_errors(src);
}

// ============================================================================
// Self-Hosting Phase 1.3: StringBuilder Tests
// ============================================================================

#[test]
fn stringbuilder_type_display() {
    // Ty::StringBuilder should display correctly
    let ty = crate::typechecker::types::Ty::StringBuilder;
    assert_eq!(format!("{}", ty), "StringBuilder");
}

#[test]
fn stringbuilder_function_signatures_exist() {
    // Verify that StringBuilder builtin functions are registered
    let src = "\
mod test
fn use_stringbuilder_funcs():
    let _ = stringbuilder_new
    let _ = stringbuilder_with_capacity
    let _ = stringbuilder_append
    let _ = stringbuilder_append_char
    let _ = stringbuilder_append_int
    let _ = stringbuilder_length
    let _ = stringbuilder_capacity
    let _ = stringbuilder_to_string
    let _ = stringbuilder_clear
";
    assert_no_errors(src);
}

#[test]
fn stringbuilder_new_returns_stringbuilder() {
    // stringbuilder_new should return StringBuilder
    let src = "\
mod test
fn use_stringbuilder():
    let sb = stringbuilder_new()
    let _ = sb
";
    assert_no_errors(src);
}

#[test]
fn stringbuilder_append_returns_stringbuilder() {
    // stringbuilder_append should return StringBuilder
    let src = "\
mod test
fn use_stringbuilder():
    let sb = stringbuilder_new()
    let sb2 = stringbuilder_append(sb, \"hello\")
    let _ = sb2
";
    assert_no_errors(src);
}

#[test]
fn stringbuilder_length_returns_int() {
    // stringbuilder_length should return Int
    let src = "\
mod test
fn use_stringbuilder():
    let sb = stringbuilder_new()
    let len = stringbuilder_length(sb)
    let _ = len
";
    assert_no_errors(src);
}

#[test]
fn stringbuilder_to_string_returns_string() {
    // stringbuilder_to_string should return String
    let src = "\
mod test
fn use_stringbuilder():
    let sb = stringbuilder_new()
    let s = stringbuilder_to_string(sb)
    let _ = s
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 1.4: Directory Listing Tests
// ============================================================================

#[test]
fn file_list_directory_basic() {
    // file_list_directory should return List[String]
    let src = "\
mod test
fn list_dir() -> !{FS} List[String]:
    let entries = file_list_directory(\"/tmp\")
    let _ = entries
";
    assert_no_errors(src);
}

#[test]
fn file_is_directory_basic() {
    // file_is_directory should return Bool
    let src = "\
mod test
fn check_dir() -> !{FS} Bool:
    let is_dir = file_is_directory(\"/tmp\")
    let _ = is_dir
";
    assert_no_errors(src);
}

#[test]
fn file_exists_basic() {
    // file_exists should return Bool
    let src = "\
mod test
fn check_exists() -> !{FS} Bool:
    let exists = file_exists(\"/tmp\")
    let _ = exists
";
    assert_no_errors(src);
}

#[test]
fn file_size_returns_option() {
    // file_size should return Option[Int]
    let src = "\
mod test
fn get_size() -> !{FS} Option[Int]:
    let size = file_size(\"/etc/passwd\")
    let _ = size
";
    assert_no_errors(src);
}

#[test]
fn file_list_directory_with_filter() {
    // Using file_list_directory in a realistic scenario with filter
    let src = "\
mod test
fn find_gradient_files(dir: String) -> !{FS} List[String]:
    let entries = file_list_directory(dir)
    let filtered = list_filter(entries, |entry: String| string_ends_with(entry, \".gradient\"))
    ret filtered
";
    assert_no_errors(src);
}

#[test]
fn file_operations_combined() {
    // Combining file operations for module discovery
    let src = "\
mod test
fn discover_modules(dir: String) -> !{FS} List[String]:
    if file_exists(dir):
        if file_is_directory(dir):
            let entries = file_list_directory(dir)
            let modules = list_filter(entries, |f: String| string_ends_with(f, \".gradient\"))
            ret modules
        else:
            let empty: List[String] = []
            ret empty
    else:
        let empty: List[String] = []
        ret empty
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 2: Self-Hosting Compiler Components
// ============================================================================

#[test]
fn token_module_token_kind_enum() {
    // TokenKind enum with variants using Gradient syntax
    let src = "\
type TokenKind = IntLit(Int) | FloatLit(Float) | StringLit(String) | BoolLit(Bool) | Fn | Let | Ident(String) | Eof

fn is_literal(kind: TokenKind) -> Bool:
    match kind:
        IntLit(x):
            ret true
        FloatLit(x):
            ret true
        StringLit(x):
            ret true
        BoolLit(x):
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn token_module_simple_token_kind() {
    // Simplified TokenKind for core tokens
    let src = "\
type TokenKind = IntLit(Int) | Ident(String) | Eof

fn get_kind_name(kind: TokenKind) -> String:
    match kind:
        IntLit(x):
            ret \"integer\"
        Ident(x):
            ret \"identifier\"
        Eof:
            ret \"eof\"
";
    assert_no_errors(src);
}

#[test]
fn token_module_eof_and_error() {
    // EOF and Error token kinds
    let src = "\
type TokenKind = Eof | Error(String)

fn is_eof(kind: TokenKind) -> Bool:
    match kind:
        Eof:
            ret true
        _:
            ret false

fn make_error(msg: String) -> TokenKind:
    ret Error(msg)
";
    assert_no_errors(src);
}

#[test]
fn token_module_keyword_lookup() {
    // Keyword lookup function using enums
    let src = "\
type TokenKind = Fn | Let | If | Else | Ident(String) | NoneKind

fn lookup_keyword(name: String) -> TokenKind:
    if name == \"fn\":
        ret Fn
    if name == \"let\":
        ret Let
    if name == \"if\":
        ret If
    if name == \"else\":
        ret Else
    ret Ident(name)
";
    assert_no_errors(src);
}

#[test]
fn token_module_predicates() {
    // Token predicates (is_keyword, is_literal, etc.)
    let src = "\
type TokenKind = Fn | Let | IntLit(Int) | StringLit(String) | Ident(String) | Plus

fn is_keyword(kind: TokenKind) -> Bool:
    match kind:
        Fn:
            ret true
        Let:
            ret true
        _:
            ret false

fn is_literal(kind: TokenKind) -> Bool:
    match kind:
        IntLit(x):
            ret true
        StringLit(x):
            ret true
        _:
            ret false

fn is_identifier(kind: TokenKind) -> Bool:
    match kind:
        Ident(x):
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 2: Self-Hosting Compiler - Lexer Module
// ============================================================================

#[test]
fn lexer_identifier_recognition() {
    // is_ident_start and is_ident_continue functions
    let src = "\
fn is_ident_start(ch: Int) -> Bool:
    if ch == 95:
        ret true
    if ch >= 65 and ch <= 90:
        ret true
    if ch >= 97 and ch <= 122:
        ret true
    ret false

fn is_ident_continue(ch: Int) -> Bool:
    if is_ident_start(ch):
        ret true
    if ch >= 48 and ch <= 57:
        ret true
    ret false
";
    assert_no_errors(src);
}

#[test]
fn lexer_keyword_lookup() {
    // Keyword lookup from identifier string
    let src = "\
type TokenKind = Fn | Let | If | Else | Ident(String)

fn lookup_keyword(name: String) -> TokenKind:
    if name == \"fn\":
        ret Fn
    if name == \"let\":
        ret Let
    if name == \"if\":
        ret If
    if name == \"else\":
        ret Else
    ret Ident(name)
";
    assert_no_errors(src);
}

#[test]
fn lexer_is_digit_function() {
    // is_digit for number recognition
    let src = "\
fn is_digit(ch: Int) -> Bool:
    ret ch >= 48 and ch <= 57

fn is_eof(pos: Int, len: Int) -> Bool:
    ret pos >= len
";
    assert_no_errors(src);
}

#[test]
fn lexer_single_char_tokens() {
    // Match single-character operators using enum
    let src = "\
type TokenKind = Plus | Minus | Star | Slash | LParen | RParen | LBrace | RBrace | Eof

fn match_single_char(ch: Int) -> TokenKind:
    if ch == 43:
        ret Plus
    if ch == 45:
        ret Minus
    if ch == 42:
        ret Star
    if ch == 47:
        ret Slash
    if ch == 40:
        ret LParen
    if ch == 41:
        ret RParen
    if ch == 123:
        ret LBrace
    if ch == 125:
        ret RBrace
    ret Eof
";
    assert_no_errors(src);
}

#[test]
fn lexer_double_char_tokens() {
    // Match two-character operators
    let src = "\
type TokenKind = Eq | Ne | Le | Ge | Arrow | PlusAssign | Eof

fn match_double_char(ch1: Int, ch2: Int) -> TokenKind:
    if ch1 == 61 and ch2 == 61:
        ret Eq
    if ch1 == 33 and ch2 == 61:
        ret Ne
    if ch1 == 60 and ch2 == 61:
        ret Le
    if ch1 == 62 and ch2 == 61:
        ret Ge
    if ch1 == 45 and ch2 == 62:
        ret Arrow
    if ch1 == 43 and ch2 == 61:
        ret PlusAssign
    ret Eof
";
    assert_no_errors(src);
}

#[test]
fn lexer_whitespace_check() {
    // Check for whitespace characters
    let src = "\
fn is_whitespace(ch: Int) -> Bool:
    if ch == 32:
        ret true
    if ch == 9:
        ret true
    if ch == 13:
        ret true
    if ch == 10:
        ret true
    ret false
";
    assert_no_errors(src);
}

#[test]
fn lexer_advance_simulation() {
    // Simulating lexer position advancement
    let src = "\
fn advance(pos: Int, line: Int, col: Int, ch: Int) -> (Int, Int, Int):
    let new_pos = pos + 1
    let mut new_line = line
    let mut new_col = col + 1
    if ch == 10:
        new_line = line + 1
        new_col = 1
    ret (new_pos, new_line, new_col)
";
    assert_no_errors(src);
}

#[test]
fn lexer_string_stub_functions() {
    // Stub string functions that would be builtins
    let src = "\
fn string_length(s: String) -> Int:
    ret 0

fn string_char_at(s: String, pos: Int) -> Int:
    ret 0

fn string_to_int(s: String) -> Int:
    ret 0

fn string_to_float(s: String) -> Float:
    ret 0.0
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 2: Self-Hosting Compiler - Parser Module
// ============================================================================

#[test]
fn parser_binop_enum() {
    // Binary operator enum for expression parsing
    let src = "\
type BinOp = Add | Sub | Mul | Div | Mod | Eq | Ne | Lt | Le | Gt | Ge | And | Or
";
    assert_no_errors(src);
}

#[test]
fn parser_unop_enum() {
    // Unary operator enum
    let src = "\
type UnOp = Neg | Not | Ref | Deref
";
    assert_no_errors(src);
}

#[test]
fn parser_precedence_enum() {
    // Precedence levels for Pratt parser
    let src = "\
type Precedence = Lowest | Assignment | Or | And | Equality | Comparison | Term | Factor | Unary | Call | Primary
";
    assert_no_errors(src);
}

#[test]
fn parser_expression_enum() {
    // Expression AST node enum - using simpler record types
    let src = "\
type BinOp = Add | Sub | Mul | Div

type UnOp = Neg | Not

type ExprRef = Int

type Expr = IntLit(Int) | FloatLit(Float) | StringLit(String) | BoolLit(Bool) | Ident(String) | Binary(BinOp, Int, Int) | Unary(UnOp, Int)
";
    assert_no_errors(src);
}

#[test]
fn parser_precedence_helpers() {
    // Helper functions for precedence
    let src = "\
type TokenKind = Plus | Minus | Star | Slash | Eq | Ne | Lt | Gt | Assign | Eof

type Precedence = Lowest | Term | Factor | Comparison | Equality | Assignment
";
    assert_no_errors(src);
}

#[test]
fn parser_list_helpers() {
    // List helper function stubs - using linked list style
    let src = "\
type IntList = Empty | Cons(Int, Int)

fn list_new() -> IntList:
    ret Empty

fn list_length(list: IntList) -> Int:
    ret 0
";
    assert_no_errors(src);
}

#[test]
fn parser_parser_type() {
    // Parser type definition - simplified without recursive types
    let src = "\
type TokenKind = Ident(String) | IntLit(Int) | Plus | Eof

type Token = Tok(TokenKind)

type TokenList = Empty | Item(Token)

type Parser = P(TokenList, Int, Int)

fn new_parser(tokens: TokenList, file_id: Int) -> Parser:
    ret P(tokens, 0, file_id)
";
    assert_no_errors(src);
}

#[test]
fn parser_token_access_helpers() {
    // Token access helper functions
    let src = "\
type TokenKind = Plus | Minus | Eof

type Token = Tok(TokenKind)

type TokenList = Empty | Item(Token)

type Parser = P(TokenList, Int)

type IntOption = Some(Int) | None

fn list_get(list: TokenList, idx: Int) -> IntOption:
    ret None

fn current_token(p: Parser) -> Token:
    ret Tok(Eof)
";
    assert_no_errors(src);
}

#[test]
fn parser_simple_expression_parsing() {
    // Simple expression parsing functions
    let src = "\
type TokenKind = IntLit(Int) | Ident(String) | Plus | Eof

type Token = Tok(TokenKind)

type TokenList = Empty | Item(Token)

type Parser = P(TokenList, Int)

type ExprRef = Int

fn make_int_expr(value: Int) -> ExprRef:
    ret value

fn make_ident_expr(name: String) -> ExprRef:
    ret 0

fn parse_prefix(p: Parser) -> (Parser, ExprRef):
    ret (p, 0)
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 2: Self-Hosting Compiler - Token Module
// ============================================================================

#[test]
fn token_token_kind_enum_literals() {
    // TokenKind enum - literal variants
    let src = "\
type TokenKind = IntLit(Int) | FloatLit(Float) | StringLit(String) | BoolLit(Bool)

fn is_literal(kind: TokenKind) -> Bool:
    match kind:
        IntLit(_):
            ret true
        FloatLit(_):
            ret true
        StringLit(_):
            ret true
        BoolLit(_):
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn token_token_kind_enum_keywords() {
    // TokenKind enum - keyword variants
    let src = "\
type TokenKind = Fn | Let | Mut | If | Else | For | While | Match | Ret | Type | Actor | Spawn | Send | Ask | Use | Mod | Extern | Export | Comptime

fn is_keyword(kind: TokenKind) -> Bool:
    match kind:
        Fn:
            ret true
        Let:
            ret true
        Mut:
            ret true
        If:
            ret true
        Else:
            ret true
        For:
            ret true
        While:
            ret true
        Match:
            ret true
        Ret:
            ret true
        Type:
            ret true
        Actor:
            ret true
        Spawn:
            ret true
        Send:
            ret true
        Ask:
            ret true
        Use:
            ret true
        Mod:
            ret true
        Extern:
            ret true
        Export:
            ret true
        Comptime:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn token_token_kind_enum_operators() {
    // TokenKind enum - operator variants
    let src = "\
type TokenKind = Plus | Minus | Star | Slash | Percent | Eq | Ne | Lt | Le | Gt | Ge | And | Or | Not | Assign | Arrow | Pipe | Dot | DotDot

fn is_operator(kind: TokenKind) -> Bool:
    match kind:
        Plus:
            ret true
        Minus:
            ret true
        Star:
            ret true
        Slash:
            ret true
        Percent:
            ret true
        Eq:
            ret true
        Ne:
            ret true
        Lt:
            ret true
        Le:
            ret true
        Gt:
            ret true
        Ge:
            ret true
        And:
            ret true
        Or:
            ret true
        Not:
            ret true
        Assign:
            ret true
        Arrow:
            ret true
        Pipe:
            ret true
        Dot:
            ret true
        DotDot:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn token_token_kind_enum_delimiters() {
    // TokenKind enum - delimiter variants
    let src = "\
type TokenKind = LParen | RParen | LBracket | RBracket | LBrace | RBrace | Colon | Comma

fn is_delimiter(kind: TokenKind) -> Bool:
    match kind:
        LParen:
            ret true
        RParen:
            ret true
        LBracket:
            ret true
        RBracket:
            ret true
        LBrace:
            ret true
        RBrace:
            ret true
        Colon:
            ret true
        Comma:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn token_token_kind_enum_special() {
    // TokenKind enum - special token variants
    let src = "\
type TokenKind = Ident(String) | Indent | Dedent | Newline | Eof | Error(String)

fn is_special(kind: TokenKind) -> Bool:
    match kind:
        Ident(_):
            ret true
        Indent:
            ret true
        Dedent:
            ret true
        Newline:
            ret true
        Eof:
            ret true
        Error(_):
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn token_complete_token_kind_enum() {
    // Complete TokenKind enum combining all variants
    let src = "\
type TokenKind = IntLit(Int) | StringLit(String) | Ident(String) | Plus | Fn | If | LParen | Eof | Error(String)

fn classify_token(kind: TokenKind) -> Int:
    match kind:
        IntLit(_):
            ret 1
        StringLit(_):
            ret 2
        Ident(_):
            ret 3
        Plus:
            ret 4
        Fn:
            ret 5
        If:
            ret 6
        LParen:
            ret 7
        Eof:
            ret 8
        Error(_):
            ret 9
        _:
            ret 0
";
    assert_no_errors(src);
}

#[test]
fn token_helper_constructors() {
    // Helper functions for creating specific token kinds
    let src = "\
type TokenKind = IntLit(Int) | StringLit(String) | Ident(String) | Eof | Error(String)

fn make_int_token(value: Int) -> TokenKind:
    ret IntLit(value)

fn make_string_token(value: String) -> TokenKind:
    ret StringLit(value)

fn make_ident_token(name: String) -> TokenKind:
    ret Ident(name)

fn make_eof_token() -> TokenKind:
    ret Eof

fn make_error_token(msg: String) -> TokenKind:
    ret Error(msg)
";
    assert_no_errors(src);
}

#[test]
fn token_classification_functions() {
    // Classification functions for different token categories
    let src = "\
type TokenKind = IntLit(Int) | Plus | Fn | LParen | Ident(String) | Eof

fn is_literal(kind: TokenKind) -> Bool:
    match kind:
        IntLit(_):
            ret true
        _:
            ret false

fn is_operator(kind: TokenKind) -> Bool:
    match kind:
        Plus:
            ret true
        _:
            ret false

fn is_keyword(kind: TokenKind) -> Bool:
    match kind:
        Fn:
            ret true
        _:
            ret false

fn is_delimiter(kind: TokenKind) -> Bool:
    match kind:
        LParen:
            ret true
        _:
            ret false

fn is_identifier(kind: TokenKind) -> Bool:
    match kind:
        Ident(_):
            ret true
        _:
            ret false

fn is_eof(kind: TokenKind) -> Bool:
    match kind:
        Eof:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 2: Self-Hosting Compiler - Type System Module
// ============================================================================

#[test]
fn types_primitive_types_enum() {
    // Ty enum with all primitive type variants
    let src = "\
type Ty = Unknown | Never | Unit | I8 | I16 | I32 | I64 | U8 | U16 | U32 | U64 | F32 | F64 | Bool | String | Char

fn is_primitive(ty: Ty) -> Bool:
    match ty:
        I8:
            ret true
        I16:
            ret true
        I32:
            ret true
        I64:
            ret true
        U8:
            ret true
        U16:
            ret true
        U32:
            ret true
        U64:
            ret true
        F32:
            ret true
        F64:
            ret true
        Bool:
            ret true
        Char:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn types_capability_enum() {
    // Capability enum for reference types
    let src = "\
type Capability = Iso | Val | Ref | Box | Trn | Tag

fn can_read(cap: Capability) -> Bool:
    match cap:
        Iso:
            ret true
        Val:
            ret true
        Ref:
            ret true
        Box:
            ret true
        Trn:
            ret true
        Tag:
            ret false

fn can_write(cap: Capability) -> Bool:
    match cap:
        Ref:
            ret true
        Trn:
            ret true
        Iso:
            ret false
        Val:
            ret false
        Box:
            ret false
        Tag:
            ret false

fn is_sendable(cap: Capability) -> Bool:
    match cap:
        Iso:
            ret true
        Val:
            ret true
        Ref:
            ret false
        Box:
            ret false
        Trn:
            ret false
        Tag:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn types_reference_types() {
    // Reference types with capabilities
    let src = "\
type Capability = Iso | Val | Ref | Box | Trn | Tag
type Ty = Unit | RefCap | ListElem | MapKeyValue

fn classify_ref_cap(cap: Capability) -> Ty:
    match cap:
        Iso:
            ret RefCap
        Val:
            ret RefCap
        Ref:
            ret RefCap
        Box:
            ret RefCap
        Trn:
            ret RefCap
        Tag:
            ret Unit
";
    assert_no_errors(src);
}

#[test]
fn types_function_type() {
    // Function types with effects - using simple enums
    let src = "\
type EffectKind = Pure | Impure | IO | FS | Network
type Ty = UnitTy | I32Ty | FnTy | EffectTy

fn classify_effect(effect: EffectKind) -> Ty:
    match effect:
        Pure:
            ret EffectTy
        Impure:
            ret EffectTy
        IO:
            ret EffectTy
        FS:
            ret EffectTy
        Network:
            ret EffectTy
";
    assert_no_errors(src);
}

#[test]
fn types_user_defined_types() {
    // User-defined types (Enum, Actor, Struct) - using enum variants
    let src = "\
type Ty = UnitTy | EnumType | ActorType | StructType | TypeVar | Constructor

fn classify_type(ty: Ty) -> String:
    match ty:
        UnitTy:
            ret \"unit\"
        EnumType:
            ret \"enum\"
        ActorType:
            ret \"actor\"
        StructType:
            ret \"struct\"
        TypeVar:
            ret \"type_var\"
        Constructor:
            ret \"constructor\"
";
    assert_no_errors(src);
}

#[test]
fn types_type_classification() {
    // Type classification helper functions
    let src = "\
type Ty = Unknown | Never | Unit | I8 | I16 | I32 | I64 | U8 | U16 | U32 | U64 | F32 | F64 | Bool | String | Char

fn is_integer(ty: Ty) -> Bool:
    match ty:
        I8:
            ret true
        I16:
            ret true
        I32:
            ret true
        I64:
            ret true
        U8:
            ret true
        U16:
            ret true
        U32:
            ret true
        U64:
            ret true
        _:
            ret false

fn is_signed(ty: Ty) -> Bool:
    match ty:
        I8:
            ret true
        I16:
            ret true
        I32:
            ret true
        I64:
            ret true
        _:
            ret false

fn is_unsigned(ty: Ty) -> Bool:
    match ty:
        U8:
            ret true
        U16:
            ret true
        U32:
            ret true
        U64:
            ret true
        _:
            ret false

fn is_float(ty: Ty) -> Bool:
    match ty:
        F32:
            ret true
        F64:
            ret true
        _:
            ret false

fn is_primitive(ty: Ty) -> Bool:
    match ty:
        I8:
            ret true
        I16:
            ret true
        I32:
            ret true
        I64:
            ret true
        U8:
            ret true
        U16:
            ret true
        U32:
            ret true
        U64:
            ret true
        F32:
            ret true
        F64:
            ret true
        Bool:
            ret true
        Char:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn types_subtyping_primitive() {
    // Subtyping relations for primitive types (integer widening)
    let src = "\
type Ty = Never | Unit | I8 | I16 | I32 | I64 | U8 | U16 | U32 | U64 | F32 | F64

fn is_subtype(t1: Ty, t2: Ty) -> Bool:
    if t1 == t2:
        ret true

    match t1:
        Never:
            // Never is subtype of everything
            ret true
        I8:
            // Integer widening (signed)
            if t2 == I16 or t2 == I32 or t2 == I64:
                ret true
            ret false
        I16:
            if t2 == I32 or t2 == I64:
                ret true
            ret false
        I32:
            if t2 == I64:
                ret true
            ret false
        U8:
            // Integer widening (unsigned)
            if t2 == U16 or t2 == U32 or t2 == U64:
                ret true
            ret false
        U16:
            if t2 == U32 or t2 == U64:
                ret true
            ret false
        U32:
            if t2 == U64:
                ret true
            ret false
        F32:
            // Float widening
            if t2 == F64:
                ret true
            ret false
        _:
            ret false

fn test_subtyping() -> Bool:
    // I8 <: I16
    ret is_subtype(I8, I16)
";
    assert_no_errors(src);
}

#[test]
fn types_capability_subtyping() {
    // Subtyping relations for reference capabilities
    let src = "\
type Capability = Iso | Val | Ref | Box | Trn | Tag

fn is_subtype_cap(c1: Capability, c2: Capability) -> Bool:
    // Capability subtyping rules
    // Iso <: Val, Iso <: Ref
    // Val <: Box
    // Ref <: Box
    // Trn <: Val, Trn <: Ref
    if c1 == Iso and (c2 == Val or c2 == Ref):
        ret true
    if c1 == Val and c2 == Box:
        ret true
    if c1 == Ref and c2 == Box:
        ret true
    if c1 == Trn and (c2 == Val or c2 == Ref):
        ret true
    ret false

fn can_read(cap: Capability) -> Bool:
    match cap:
        Iso:
            ret true
        Val:
            ret true
        Ref:
            ret true
        Box:
            ret true
        Trn:
            ret true
        Tag:
            ret false

fn can_write(cap: Capability) -> Bool:
    match cap:
        Ref:
            ret true
        Trn:
            ret true
        Iso:
            ret false
        Val:
            ret false
        Box:
            ret false
        Tag:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn types_effect_set_operations() {
    // EffectSet operations - simplified to use enums
    let src = "\
type EffectKind = Pure | IO | FS | Network
type EffectSet = EmptySet | SingleEffect | ManyEffects

fn is_pure_effect(e: EffectKind) -> Bool:
    match e:
        Pure:
            ret true
        IO:
            ret false
        FS:
            ret false
        Network:
            ret false

fn classify_set(s: EffectSet) -> String:
    match s:
        EmptySet:
            ret \"empty\"
        SingleEffect:
            ret \"single\"
        ManyEffects:
            ret \"many\"
";
    assert_no_errors(src);
}

#[test]
fn types_function_signature() {
    // Function signature type - simplified using enums
    let src = "\
type FnKind = PureFn | EffectfulFn | ComptimeFn | GenericFn

fn classify_fn(kind: FnKind) -> String:
    match kind:
        PureFn:
            ret \"pure\"
        EffectfulFn:
            ret \"effectful\"
        ComptimeFn:
            ret \"comptime\"
        GenericFn:
            ret \"generic\"
";
    assert_no_errors(src);
}

#[test]
fn types_display_string_conversion() {
    // Type to string conversion for error messages - simplified
    let src = "\
type Ty = Unknown | Never | Unit | I8 | I16 | I32 | I64 | U8 | U16 | U32 | U64 | F32 | F64 | Bool | String | Char

fn type_to_string(ty: Ty) -> String:
    match ty:
        Unknown:
            ret \"_\"
        Never:
            ret \"Never\"
        Unit:
            ret \"Unit\"
        I8:
            ret \"I8\"
        I16:
            ret \"I16\"
        I32:
            ret \"I32\"
        I64:
            ret \"I64\"
        U8:
            ret \"U8\"
        U16:
            ret \"U16\"
        U32:
            ret \"U32\"
        U64:
            ret \"U64\"
        F32:
            ret \"F32\"
        F64:
            ret \"F64\"
        Bool:
            ret \"Bool\"
        String:
            ret \"String\"
        Char:
            ret \"Char\"
        _:
            ret \"unknown\"
";
    assert_no_errors(src);
}

#[test]
fn types_complex_type_construction() {
    // Complex type construction (tuples, nested types) - simplified
    let src = "\
type Capability = Iso | Val | Ref | Box | Trn | Tag
type TyKind = UnitTy | I32Ty | StringTy | RefTy | ListTy | TupleTy | FnTy

fn classify_cap(cap: Capability) -> TyKind:
    match cap:
        Iso:
            ret RefTy
        Val:
            ret RefTy
        Ref:
            ret RefTy
        Box:
            ret RefTy
        Trn:
            ret RefTy
        Tag:
            ret UnitTy

fn classify_kind(kind: TyKind) -> String:
    match kind:
        UnitTy:
            ret \"unit\"
        I32Ty:
            ret \"i32\"
        StringTy:
            ret \"string\"
        RefTy:
            ret \"ref\"
        ListTy:
            ret \"list\"
        TupleTy:
            ret \"tuple\"
        FnTy:
            ret \"fn\"
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 2: Self-Hosting Compiler - Type Checker Module
// ============================================================================

#[test]
fn checker_type_error_enum() {
    // TypeError enum with common error variants
    let src = "\
type TypeError = TypeMismatch | UndefinedVar | EffectError | UnificationError

type Severity = ErrorSeverity | WarningSeverity | NoteSeverity

fn is_error(err: TypeError) -> Bool:
    match err:
        TypeMismatch:
            ret true
        UndefinedVar:
            ret true
        EffectError:
            ret true
        UnificationError:
            ret true
";
    assert_no_errors(src);
}

#[test]
fn checker_binding_type() {
    // Binding type for variable tracking
    let src = "\
type Binding = MutableBinding | ImmutableBinding | ComptimeBinding

type BindingKind = VarBinding | FnBinding | TypeBinding

fn is_mutable_binding(b: Binding) -> Bool:
    match b:
        MutableBinding:
            ret true
        _:
            ret false

fn is_comptime_binding(b: Binding) -> Bool:
    match b:
        ComptimeBinding:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_scope_management() {
    // Scope management types
    let src = "\
type Scope = GlobalScope | LocalScope | BlockScope

type ScopeLevel = Level0 | Level1 | Level2 | Level3

fn is_global_scope(s: Scope) -> Bool:
    match s:
        GlobalScope:
            ret true
        LocalScope:
            ret false
        BlockScope:
            ret false

fn scope_level_num(level: ScopeLevel) -> Int:
    match level:
        Level0:
            ret 0
        Level1:
            ret 1
        Level2:
            ret 2
        Level3:
            ret 3
";
    assert_no_errors(src);
}

#[test]
fn checker_type_environment() {
    // Type environment tracking
    let src = "\
type TypeEnv = EmptyEnv | ScopedEnv | FunctionEnv | ModuleEnv

type Scope = GlobalScope | LocalScope

type Binding = VarBinding | FnBinding

type TypeDef = SimpleType | GenericType | ComptimeType

type FnDef = PureFn | EffectfulFn | ComptimeFn

fn env_to_string(env: TypeEnv) -> String:
    match env:
        EmptyEnv:
            ret \"empty\"
        ScopedEnv:
            ret \"scoped\"
        FunctionEnv:
            ret \"function\"
        ModuleEnv:
            ret \"module\"
";
    assert_no_errors(src);
}

#[test]
fn checker_type_checker_state() {
    // Type checker state structure
    let src = "\
type TypeChecker = NormalChecker | ComptimeChecker | GenericChecker

type TypeEnv = EmptyEnv | PopulatedEnv

type FnDef = PureFn | EffectfulFn

type TypeError = TypeMismatch | UndefinedVar | UnificationError

type Option[T] = Some(T) | None

fn is_comptime_context(tc: TypeChecker) -> Bool:
    match tc:
        ComptimeChecker:
            ret true
        _:
            ret false

fn has_return_type(opt: Option[Int]) -> Bool:
    match opt:
        Some(_):
            ret true
        None:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_check_result() {
    // CheckResult for expression type checking
    let src = "\
type CheckResult = SuccessResult | ErrorResult | PartialResult

type TypeError = TypeMismatch | UndefinedVar

fn is_success(result: CheckResult) -> Bool:
    match result:
        SuccessResult:
            ret true
        _:
            ret false

fn has_errors(result: CheckResult) -> Bool:
    match result:
        ErrorResult:
            ret true
        PartialResult:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_param_info() {
    // ParamInfo for function parameters
    let src = "\
type ParamInfo = NormalParam | ComptimeParam | GenericParam

type ParamKind = ValParam | RefParam | IsoParam

fn is_comptime_param(p: ParamInfo) -> Bool:
    match p:
        ComptimeParam:
            ret true
        _:
            ret false

fn param_kind_to_string(k: ParamKind) -> String:
    match k:
        ValParam:
            ret \"val\"
        RefParam:
            ret \"ref\"
        IsoParam:
            ret \"iso\"
";
    assert_no_errors(src);
}

#[test]
fn checker_type_inference_fresh_var() {
    // Fresh type variable generation
    let src = "\
type TypeVarState = FreshVar | UnifiedVar | ResolvedVar

type TypeVarGen = Gen0 | Gen1 | Gen2 | Gen3

fn fresh_var_state(gen: TypeVarGen) -> TypeVarState:
    match gen:
        Gen0:
            ret FreshVar
        Gen1:
            ret FreshVar
        Gen2:
            ret FreshVar
        Gen3:
            ret FreshVar
";
    assert_no_errors(src);
}

#[test]
fn checker_substitution_tracking() {
    // Substitution tracking for unification
    let src = "\
type Subst = EmptySubst | SingleSubst | MultiSubst

type OptionInt = SomeInt(Int) | NoneInt

fn add_substitution(s: Subst, new_sub: Subst) -> Subst:
    // Simplified - would add to substitutions
    ret MultiSubst

fn lookup_substitution(s: Subst) -> OptionInt:
    // Simplified - would lookup in substitutions
    ret NoneInt
";
    assert_no_errors(src);
}

#[test]
fn checker_occurs_check() {
    // Occurs check for type variable unification
    let src = "\
type OccursResult = NoOccurrence | OccursDirect | OccursNested

type TypeError = OccursCheckFailed | CannotUnify

fn occurs_in(result: OccursResult) -> Bool:
    match result:
        NoOccurrence:
            ret false
        OccursDirect:
            ret true
        OccursNested:
            ret true

fn is_cyclic(t1: Int, t2: Int) -> Bool:
    // Simplified - would check for cycles
    ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_scope_lookup() {
    // Scope lookup functions
    let src = "\
type LookupResult = FoundBinding | NotFound | Shadowed

type Scope = GlobalScope | LocalScope | BlockScope

type Binding = VarBinding | FnBinding

fn find_binding(result: LookupResult) -> Bool:
    match result:
        FoundBinding:
            ret true
        NotFound:
            ret false
        Shadowed:
            ret true

fn is_local_scope(s: Scope) -> Bool:
    match s:
        LocalScope:
            ret true
        BlockScope:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_effect_tracking() {
    // Effect tracking in type checker
    let src = "\
type EffectSet = PureSet | IOSet | FSSet | NetworkSet | MixedSet

type EffectEnv = PureEnv | ImpureEnv

fn current_effects(env: EffectEnv) -> EffectSet:
    match env:
        PureEnv:
            ret PureSet
        ImpureEnv:
            ret MixedSet

fn is_pure_effect_set(s: EffectSet) -> Bool:
    match s:
        PureSet:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_error_collection() {
    // Error collection in type checker
    let src = "\
type ErrorCollection = NoErrors | HasErrors | HasWarnings

type TypeError = TypeMismatch | UndefinedVar

fn has_errors(coll: ErrorCollection) -> Bool:
    match coll:
        HasErrors:
            ret true
        _:
            ret false

fn has_warnings(coll: ErrorCollection) -> Bool:
    match coll:
        HasWarnings:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_function_type_info() {
    // Function type information
    let src = "\
type FnKind = PureFn | EffectfulFn | GenericFn | ComptimeFn

type ParamMode = ValMode | RefMode | BoxMode

fn is_function_pure(kind: FnKind) -> Bool:
    match kind:
        PureFn:
            ret true
        ComptimeFn:
            ret true
        _:
            ret false

fn param_count_estimate(mode: ParamMode) -> Int:
    match mode:
        ValMode:
            ret 1
        RefMode:
            ret 1
        BoxMode:
            ret 1
";
    assert_no_errors(src);
}

#[test]
fn checker_unification_helpers() {
    // Unification helper functions
    let src = "\
type UnifyResult = Success | FailedCannotUnify | FailedOccursCheck

type TypeError = CannotUnify | OccursCheckFailed

fn try_unify_result(result: UnifyResult) -> Bool:
    match result:
        Success:
            ret true
        _:
            ret false

fn unify_result_to_string(result: UnifyResult) -> String:
    match result:
        Success:
            ret \"success\"
        FailedCannotUnify:
            ret \"cannot_unify\"
        FailedOccursCheck:
            ret \"occurs_check\"
";
    assert_no_errors(src);
}

#[test]
fn checker_span_tracking() {
    // Source location tracking for errors
    let src = "\
type Span = EmptySpan | SingleLineSpan | MultiLineSpan

type Position = StartPos | EndPos | MidPos

fn is_empty_span(s: Span) -> Bool:
    match s:
        EmptySpan:
            ret true
        _:
            ret false

fn is_multiline(s: Span) -> Bool:
    match s:
        MultiLineSpan:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_type_comparisons() {
    // Type comparison utilities
    let src = "\
type Ty = I32 | I64 | F32 | F64 | Bool | String | Unknown

fn types_equal(t1: Ty, t2: Ty) -> Bool:
    ret t1 == t2

fn is_numeric(t: Ty) -> Bool:
    match t:
        I32:
            ret true
        I64:
            ret true
        F32:
            ret true
        F64:
            ret true
        _:
            ret false

fn is_integer(t: Ty) -> Bool:
    match t:
        I32:
            ret true
        I64:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn checker_option_type() {
    // Option type for type checker
    let src = "\
type Option[T] = Some(T) | None

fn is_some(opt: Option[Int]) -> Bool:
    match opt:
        Some(_):
            ret true
        None:
            ret false

fn is_none(opt: Option[Int]) -> Bool:
    match opt:
        Some(_):
            ret false
        None:
            ret true

fn unwrap_or(opt: Option[Int], default: Int) -> Int:
    match opt:
        Some(v):
            ret v
        None:
            ret default
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 2: Self-Hosting Compiler - IR Module
// ============================================================================

#[test]
fn ir_value_enum() {
    // IR Value types for SSA form
    let src = "\
type Value = ConstInt | ConstBool | Register | Param | Global

type ValueId = V0 | V1 | V2 | V3 | V4

fn is_constant(v: Value) -> Bool:
    match v:
        ConstInt:
            ret true
        ConstBool:
            ret true
        _:
            ret false

fn is_register(v: Value) -> Bool:
    match v:
        Register:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_type_enum() {
    // IR low-level types
    let src = "\
type IrType = I8 | I16 | I32 | I64 | U8 | U16 | U32 | U64 | F32 | F64 | Bool | Ptr | Void

fn type_size(ty: IrType) -> Int:
    match ty:
        I8:
            ret 1
        I16:
            ret 2
        I32:
            ret 4
        I64:
            ret 8
        U8:
            ret 1
        U16:
            ret 2
        U32:
            ret 4
        U64:
            ret 8
        F32:
            ret 4
        F64:
            ret 8
        Bool:
            ret 1
        Ptr:
            ret 8
        Void:
            ret 0

fn is_integer(ty: IrType) -> Bool:
    match ty:
        I8:
            ret true
        I16:
            ret true
        I32:
            ret true
        I64:
            ret true
        U8:
            ret true
        U16:
            ret true
        U32:
            ret true
        U64:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_capability_enum() {
    // Capability for reference types in IR
    let src = "\
type Capability = IsoCap | ValCap | RefCap | BoxCap | TrnCap | TagCap

fn is_readonly_cap(cap: Capability) -> Bool:
    match cap:
        ValCap:
            ret true
        TagCap:
            ret true
        _:
            ret false

fn is_sendable_cap(cap: Capability) -> Bool:
    match cap:
        IsoCap:
            ret true
        ValCap:
            ret true
        TagCap:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_arithmetic_instructions() {
    // Integer arithmetic instructions
    let src = "\
type Inst = IAdd | ISub | IMul | IDiv | IRem | INeg

type ValueId = ResultId(Int)

fn is_binary_arith(inst: Inst) -> Bool:
    match inst:
        IAdd:
            ret true
        ISub:
            ret true
        IMul:
            ret true
        IDiv:
            ret true
        IRem:
            ret true
        _:
            ret false

fn is_unary_arith(inst: Inst) -> Bool:
    match inst:
        INeg:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_float_instructions() {
    // Floating point arithmetic
    let src = "\
type FInst = FAdd | FSub | FMul | FDiv | FNeg

fn is_float_binary(inst: FInst) -> Bool:
    match inst:
        FAdd:
            ret true
        FSub:
            ret true
        FMul:
            ret true
        FDiv:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_bitwise_instructions() {
    // Bitwise operations
    let src = "\
type BitInst = And | Or | Xor | Not | Shl | AShr | LShr

fn is_bitwise_binary(inst: BitInst) -> Bool:
    match inst:
        And:
            ret true
        Or:
            ret true
        Xor:
            ret true
        Shl:
            ret true
        AShr:
            ret true
        LShr:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_comparison_instructions() {
    // Integer comparison instructions
    let src = "\
type ICmpInst = IEq | INe | ISlt | ISle | ISgt | ISge | IUlt | IUle | IUgt | IUge

fn is_equality_cmp(inst: ICmpInst) -> Bool:
    match inst:
        IEq:
            ret true
        INe:
            ret true
        _:
            ret false

fn is_signed_cmp(inst: ICmpInst) -> Bool:
    match inst:
        ISlt:
            ret true
        ISle:
            ret true
        ISgt:
            ret true
        ISge:
            ret true
        _:
            ret false

fn is_unsigned_cmp(inst: ICmpInst) -> Bool:
    match inst:
        IUlt:
            ret true
        IUle:
            ret true
        IUgt:
            ret true
        IUge:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_float_comparison() {
    // Float comparison instructions
    let src = "\
type FCmpInst = FEq | FNe | FLt | FLe | FGt | FGe

fn is_float_cmp(inst: FCmpInst) -> Bool:
    ret true
";
    assert_no_errors(src);
}

#[test]
fn ir_conversion_instructions() {
    // Type conversion instructions
    let src = "\
type ConvInst = Trunc | SExt | ZExt | FTrunc | FExt | SIToFP | UIToFP | FPToSI | FPToUI | PtrToInt | IntToPtr | Bitcast

fn is_int_conversion(inst: ConvInst) -> Bool:
    match inst:
        Trunc:
            ret true
        SExt:
            ret true
        ZExt:
            ret true
        _:
            ret false

fn is_float_conversion(inst: ConvInst) -> Bool:
    match inst:
        FTrunc:
            ret true
        FExt:
            ret true
        SIToFP:
            ret true
        UIToFP:
            ret true
        FPToSI:
            ret true
        FPToUI:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_memory_instructions() {
    // Memory operations
    let src = "\
type MemInst = Alloca | Load | Store | GEP

type AtomicOrdering = Unordered | Monotonic | Acquire | Release | AcqRel | SeqCst

fn is_memory_read(inst: MemInst) -> Bool:
    match inst:
        Load:
            ret true
        _:
            ret false

fn is_memory_write(inst: MemInst) -> Bool:
    match inst:
        Store:
            ret true
        _:
            ret false

fn is_stack_alloc(inst: MemInst) -> Bool:
    match inst:
        Alloca:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_heap_instructions() {
    // Heap memory management
    let src = "\
type HeapInst = Malloc | Free | RefCountIncr | RefCountDecr | COWTrigger

fn is_allocation(inst: HeapInst) -> Bool:
    match inst:
        Malloc:
            ret true
        _:
            ret false

fn is_deallocation(inst: HeapInst) -> Bool:
    match inst:
        Free:
            ret true
        _:
            ret false

fn is_ref_count(inst: HeapInst) -> Bool:
    match inst:
        RefCountIncr:
            ret true
        RefCountDecr:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_control_flow() {
    // Control flow instructions
    let src = "\
type CFInst = Ret | RetVoid | Br | CondBr | Switch | Unreachable

type BlockId = Block0 | Block1 | Block2 | Block3

fn is_terminator(inst: CFInst) -> Bool:
    match inst:
        Ret:
            ret true
        RetVoid:
            ret true
        Br:
            ret true
        CondBr:
            ret true
        Switch:
            ret true
        Unreachable:
            ret true
";
    assert_no_errors(src);
}

#[test]
fn ir_function_calls() {
    // Function call instructions
    let src = "\
type CallInst = Call | TailCall | CallIndirect

type Option[T] = Some(T) | None

fn is_direct_call(inst: CallInst) -> Bool:
    match inst:
        Call:
            ret true
        TailCall:
            ret true
        _:
            ret false

fn is_indirect_call(inst: CallInst) -> Bool:
    match inst:
        CallIndirect:
            ret true
        _:
            ret false

fn is_tail_call(inst: CallInst) -> Bool:
    match inst:
        TailCall:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_phi_node() {
    // PHI node for SSA merges
    let src = "\
type PhiNode = PhiMerge | PhiSelect

type Block = BlockA | BlockB | BlockC

type Value = Reg(Int) | Const(Int)

fn is_phi_merge(node: PhiNode) -> Bool:
    match node:
        PhiMerge:
            ret true
        PhiSelect:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_basic_block() {
    // Basic block structure
    let src = "\
type BlockId = B0 | B1 | B2 | B3 | B4

type BasicBlock = EntryBlock | BodyBlock | ExitBlock

type BlockInfo = BlockInfoId(BlockId) | BlockInfoName(String)

fn is_entry_block(b: BasicBlock) -> Bool:
    match b:
        EntryBlock:
            ret true
        _:
            ret false

fn is_exit_block(b: BasicBlock) -> Bool:
    match b:
        ExitBlock:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_function() {
    // IR Function structure
    let src = "\
type IrFunction = PureFn | EffectfulFn | ExternFn | ComptimeFn

type FnAttr = NoEffects | ExternAttr | InlineAttr

fn is_pure_fn(f: IrFunction) -> Bool:
    match f:
        PureFn:
            ret true
        _:
            ret false

fn is_extern_fn(f: IrFunction) -> Bool:
    match f:
        ExternFn:
            ret true
        _:
            ret false

fn is_comptime_fn(f: IrFunction) -> Bool:
    match f:
        ComptimeFn:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_module() {
    // IR Module structure
    let src = "\
type IrModule = EmptyMod | PopulatedMod

type ModuleInfo = ModuleInfoBasic | ModuleInfoFull

fn empty_module() -> IrModule:
    ret EmptyMod

fn is_empty(m: IrModule) -> Bool:
    match m:
        EmptyMod:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_global_var() {
    // Global variable definitions
    let src = "\
type GlobalVar = ConstGlobal | MutableGlobal

type GlobalInit = Initialized | Uninitialized

fn is_const_global(g: GlobalVar) -> Bool:
    match g:
        ConstGlobal:
            ret true
        _:
            ret false

fn is_mutable_global(g: GlobalVar) -> Bool:
    match g:
        MutableGlobal:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_atomic_ordering() {
    // Atomic memory ordering
    let src = "\
type AtomicOrdering = Unordered | Monotonic | Acquire | Release | AcqRel | SeqCst

fn is_weak_ordering(o: AtomicOrdering) -> Bool:
    match o:
        Unordered:
            ret true
        Monotonic:
            ret true
        _:
            ret false

fn is_strong_ordering(o: AtomicOrdering) -> Bool:
    match o:
        SeqCst:
            ret true
        _:
            ret false

fn is_acquire(o: AtomicOrdering) -> Bool:
    match o:
        Acquire:
            ret true
        AcqRel:
            ret true
        SeqCst:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_verify_error() {
    // IR verification errors
    let src = "\
type VerifyError = InvalidType | UndefinedValue | TypeMismatch | MissingTerminator | InvalidTerminator

fn is_critical_error(err: VerifyError) -> Bool:
    match err:
        InvalidType:
            ret true
        UndefinedValue:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_debug_info() {
    // Debug information
    let src = "\
type DbgInst = DbgLoc | DbgVar

type DebugScope = GlobalScope | LocalScope | InlineScope

fn has_location(inst: DbgInst) -> Bool:
    match inst:
        DbgLoc:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 2: Self-Hosting Compiler - IR Builder Module
// ============================================================================

#[test]
fn ir_builder_state() {
    // IR Builder state structure
    let src = "type IrBuilder = EmptyBuilder | ActiveBuilder | FunctionBuilder

type BuilderConfig = DebugConfig | ReleaseConfig | OptimizedConfig

fn is_active_builder(b: IrBuilder) -> Bool:
    match b:
        ActiveBuilder:
            ret true
        FunctionBuilder:
            ret true
        _:
            ret false

fn is_empty_builder(b: IrBuilder) -> Bool:
    match b:
        EmptyBuilder:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_function_ops() {
    // Function-level operations in builder
    let src = "type FuncState = NoFunction | BuildingFunction | FinishedFunction

type FunctionOp = StartFn | FinishFn | AddParam

fn is_building(func_state: FuncState) -> Bool:
    match func_state:
        BuildingFunction:
            ret true
        _:
            ret false

fn is_finished(func_state: FuncState) -> Bool:
    match func_state:
        FinishedFunction:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_block_ops() {
    // Block creation and positioning
    let src = "type BlockOp = CreateBlock | PositionAt | SealBlock

type BlockState = EntryBlock | BodyBlock | ExitBlock

fn is_entry_block(block_state: BlockState) -> Bool:
    match block_state:
        EntryBlock:
            ret true
        _:
            ret false

fn is_exit_block(block_state: BlockState) -> Bool:
    match block_state:
        ExitBlock:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_arithmetic_emit() {
    // Arithmetic instruction emission
    let src = "type ArithOp = EmitIAdd | EmitISub | EmitIMul | EmitIDiv | EmitINeg

type ValueType = IntValue | FloatValue | BoolValue

fn is_binary_op(op: ArithOp) -> Bool:
    match op:
        EmitIAdd:
            ret true
        EmitISub:
            ret true
        EmitIMul:
            ret true
        EmitIDiv:
            ret true
        _:
            ret false

fn is_unary_op(op: ArithOp) -> Bool:
    match op:
        EmitINeg:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_comparison_emit() {
    // Comparison instruction emission
    let src = "type CmpOp = EmitIEq | EmitINe | EmitISlt | EmitISgt | EmitISle | EmitISge

type CmpResult = Equal | Less | Greater | Unordered

fn is_equality_cmp(op: CmpOp) -> Bool:
    match op:
        EmitIEq:
            ret true
        EmitINe:
            ret true
        _:
            ret false

fn is_ordered_cmp(op: CmpOp) -> Bool:
    match op:
        EmitISlt:
            ret true
        EmitISgt:
            ret true
        EmitISle:
            ret true
        EmitISge:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_memory_emit() {
    // Memory operation emission
    let src = "type MemOp = EmitAlloca | EmitLoad | EmitStore

type MemOpKind = StackOp | HeapOp | GlobalOp

fn is_allocation(op: MemOp) -> Bool:
    match op:
        EmitAlloca:
            ret true
        _:
            ret false

fn is_access_op(op: MemOp) -> Bool:
    match op:
        EmitLoad:
            ret true
        EmitStore:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_control_flow_emit() {
    // Control flow emission
    let src = "type CFOp = EmitBr | EmitCondBr | EmitRet | EmitRetVoid | EmitUnreachable

type BranchKind = Unconditional | Conditional | Return

fn is_branch_op(op: CFOp) -> Bool:
    match op:
        EmitBr:
            ret true
        EmitCondBr:
            ret true
        _:
            ret false

fn is_return_op(op: CFOp) -> Bool:
    match op:
        EmitRet:
            ret true
        EmitRetVoid:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_call_emit() {
    // Function call emission
    let src = "type CallOp = EmitCall | EmitTailCall | EmitCallIndirect

type CallKind = DirectCall | IndirectCall

fn is_direct_call(op: CallOp) -> Bool:
    match op:
        EmitCall:
            ret true
        EmitTailCall:
            ret true
        _:
            ret false

fn is_indirect_call(op: CallOp) -> Bool:
    match op:
        EmitCallIndirect:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_locals() {
    // Local variable management
    let src = "type LocalOp = CreateLocal | GetLocal | SetLocal

type LocalState = Uninitialized | Initialized | Modified

fn is_local_create(op: LocalOp) -> Bool:
    match op:
        CreateLocal:
            ret true
        _:
            ret false

fn is_local_access(op: LocalOp) -> Bool:
    match op:
        GetLocal:
            ret true
        SetLocal:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_expression_lowering() {
    // Expression lowering to IR
    let src = "type LowerOp = LowerIntLit | LowerBoolLit | LowerVarRef | LowerBinaryOp

type LowerResult = LowerSuccess | LowerFailure

fn is_literal_lowering(op: LowerOp) -> Bool:
    match op:
        LowerIntLit:
            ret true
        LowerBoolLit:
            ret true
        _:
            ret false

fn is_complex_lowering(op: LowerOp) -> Bool:
    match op:
        LowerVarRef:
            ret true
        LowerBinaryOp:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_statement_lowering() {
    // Statement lowering to IR
    let src = "type LowerStmtOp = LowerLet | LowerAssign

type LowerStmtResult = StmtSuccess | StmtFailure

fn is_binding_stmt(op: LowerStmtOp) -> Bool:
    match op:
        LowerLet:
            ret true
        _:
            ret false

fn is_mutation_stmt(op: LowerStmtOp) -> Bool:
    match op:
        LowerAssign:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_control_flow_gen() {
    // Control flow generation
    let src = "type CFGen = GenIfThenElse | GenWhileLoop | GenMatch

type CFGenResult = GenSuccess | GenFailure

fn is_conditional_gen(gen: CFGen) -> Bool:
    match gen:
        GenIfThenElse:
            ret true
        GenMatch:
            ret true
        _:
            ret false

fn is_loop_gen(gen: CFGen) -> Bool:
    match gen:
        GenWhileLoop:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_value_allocation() {
    // Value ID allocation
    let src = "type AllocResult = AllocSuccess | AllocFailure

type ValueId = V0 | V1 | V2 | V3 | V4 | V5

fn is_valid_value_id(id: ValueId) -> Bool:
    match id:
        V0:
            ret true
        V1:
            ret true
        V2:
            ret true
        V3:
            ret true
        V4:
            ret true
        V5:
            ret true
";
    assert_no_errors(src);
}

#[test]
fn ir_builder_config() {
    // Builder configuration
    let src = "type OptLevel = OptNone | OptBasic | OptAggressive

type TargetArch = TargetX86 | TargetARM | TargetWASM

fn is_debug_build(level: OptLevel) -> Bool:
    match level:
        OptNone:
            ret true
        _:
            ret false

fn is_optimized_build(level: OptLevel) -> Bool:
    match level:
        OptAggressive:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 3: Self-Hosting Compiler - Main Compiler Driver
// ============================================================================

#[test]
fn compiler_pipeline_state() {
    // Compilation pipeline state
    let src = "type CompilePhase = IdlePhase | LexingPhase | ParsingPhase | TypeCheckPhase | IRPhase | CodeGenPhase | FinishedPhase

type PipelineState = Running | Paused | Failed | Completed

fn is_active_phase(phase: CompilePhase) -> Bool:
    match phase:
        LexingPhase:
            ret true
        ParsingPhase:
            ret true
        TypeCheckPhase:
            ret true
        IRPhase:
            ret true
        CodeGenPhase:
            ret true
        _:
            ret false

fn is_terminal_phase(phase: CompilePhase) -> Bool:
    match phase:
        FinishedPhase:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_config() {
    // Compiler configuration options
    let src = "type CompilerConfig = DebugConfig | ReleaseConfig | CustomConfig

type OptLevel = Opt0 | Opt1 | Opt2 | Opt3

type TargetArch = NativeTarget | WasmTarget | X86Target | ARMTarget

fn is_debug_config(cfg: CompilerConfig) -> Bool:
    match cfg:
        DebugConfig:
            ret true
        _:
            ret false

fn is_release_config(cfg: CompilerConfig) -> Bool:
    match cfg:
        ReleaseConfig:
            ret true
        _:
            ret false

fn is_optimized_opt_level(level: OptLevel) -> Bool:
    match level:
        Opt2:
            ret true
        Opt3:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_entry_points() {
    // Main compiler entry points
    let src = "type CompileEntry = CompileFile | CompileString | CompileProject

type EntryResult = EntrySuccess | EntryFailure

fn is_file_entry(entry: CompileEntry) -> Bool:
    match entry:
        CompileFile:
            ret true
        _:
            ret false

fn is_string_entry(entry: CompileEntry) -> Bool:
    match entry:
        CompileString:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_results() {
    // Compilation results
    let src = "type CompileResult = SuccessResult | ErrorResult | PartialResult

type CompileStatus = CompilationOK | CompilationFailed | CompilationPartial

fn is_success_result(result: CompileResult) -> Bool:
    match result:
        SuccessResult:
            ret true
        _:
            ret false

fn is_error_result(result: CompileResult) -> Bool:
    match result:
        ErrorResult:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_errors() {
    // Compilation errors
    let src = "type CompileError = LexError | ParseError | TypeError | CodeGenError | LinkError

type ErrorSeverity = Error | Warning | Note

fn is_fatal_error(err: CompileError) -> Bool:
    match err:
        LexError:
            ret true
        ParseError:
            ret true
        TypeError:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_phases() {
    // Compilation phases
    let src = "type PhaseName = LexPhase | ParsePhase | TypeCheckPhase | IRBuildPhase | GenPhase

type PhaseResult = PhaseSuccess | PhaseSkipped | PhaseFailed

fn is_analysis_phase(phase: PhaseName) -> Bool:
    match phase:
        LexPhase:
            ret true
        ParsePhase:
            ret true
        TypeCheckPhase:
            ret true
        _:
            ret false

fn is_codegen_phase(phase: PhaseName) -> Bool:
    match phase:
        IRBuildPhase:
            ret true
        GenPhase:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_verification() {
    // Compiler verification options
    let src = "type VerifyMode = NoVerify | BasicVerify | FullVerify

type VerifyResult = Verified | VerifyFailed

fn is_verification_enabled(mode: VerifyMode) -> Bool:
    match mode:
        NoVerify:
            ret false
        _:
            ret true

fn is_full_verify(mode: VerifyMode) -> Bool:
    match mode:
        FullVerify:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_debug_info() {
    // Debug information options
    let src = "type DebugInfo = NoDebugInfo | MinimalDebug | FullDebug

type DebugLevel = DebugNone | DebugBasic | DebugFull

fn has_debug_info(level: DebugLevel) -> Bool:
    match level:
        DebugNone:
            ret false
        _:
            ret true

fn is_full_debug(level: DebugLevel) -> Bool:
    match level:
        DebugFull:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_output() {
    // Compiler output options
    let src = "type OutputFormat = ExecutableOutput | LibraryOutput | ObjectOutput | IROutput

type OutputTarget = NativeBinary | WasmModule | LLVMIR

fn is_binary_output(fmt: OutputFormat) -> Bool:
    match fmt:
        ExecutableOutput:
            ret true
        ObjectOutput:
            ret true
        _:
            ret false

fn is_text_output(fmt: OutputFormat) -> Bool:
    match fmt:
        IROutput:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn compiler_quick_compile() {
    // Quick compile helper
    let src = "type QuickCompileResult = QuickSuccess | QuickFailure

type CompileMode = FastMode | BalancedMode | SlowMode

fn is_fast_mode(mode: CompileMode) -> Bool:
    match mode:
        FastMode:
            ret true
        _:
            ret false

fn is_slow_mode(mode: CompileMode) -> Bool:
    match mode:
        SlowMode:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

// ============================================================================
// Phase 3: Bootstrap - Self-Hosting Compiler Validation
// ============================================================================

#[test]
fn bootstrap_config() {
    // Bootstrap configuration
    let src = "type BootstrapConfig = DebugBootstrap | ReleaseBootstrap | TestBootstrap

type BootstrapMode = FullBootstrap | PartialBootstrap | VerifyOnly

fn is_full_bootstrap(mode: BootstrapMode) -> Bool:
    match mode:
        FullBootstrap:
            ret true
        _:
            ret false

fn is_verify_only(mode: BootstrapMode) -> Bool:
    match mode:
        VerifyOnly:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn bootstrap_phases() {
    // Bootstrap process phases
    let src = "type BootstrapPhase = NotStarted | CompilingPhase | ValidatingPhase | SelfCompilingPhase | BootstrapComplete | BootstrapFailed

type PhaseResult = PhaseSuccess | PhaseFailure | PhaseSkipped

fn is_compilation_phase(phase: BootstrapPhase) -> Bool:
    match phase:
        CompilingPhase:
            ret true
        _:
            ret false

fn is_validation_phase(phase: BootstrapPhase) -> Bool:
    match phase:
        ValidatingPhase:
            ret true
        _:
            ret false

fn is_terminal_phase(phase: BootstrapPhase) -> Bool:
    match phase:
        BootstrapComplete:
            ret true
        BootstrapFailed:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn bootstrap_module_results() {
    // Module compilation results
    let src = "type ModuleResult = CompileSuccess | CompileFailure | CompileSkipped

type ModuleStatus = ModuleOK | ModuleError | ModuleWarning

fn is_successful_compile(result: ModuleResult) -> Bool:
    match result:
        CompileSuccess:
            ret true
        _:
            ret false

fn has_errors(status: ModuleStatus) -> Bool:
    match status:
        ModuleError:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn bootstrap_validation() {
    // Validation against reference compiler
    let src = "type ValidationMode = StrictValidation | LenientValidation | IgnoreValidation

type ValidationResult = ValidationMatch | ValidationMismatch | ValidationSkipped

fn is_strict_validation(mode: ValidationMode) -> Bool:
    match mode:
        StrictValidation:
            ret true
        _:
            ret false

fn should_validate(mode: ValidationMode) -> Bool:
    match mode:
        IgnoreValidation:
            ret false
        _:
            ret true
";
    assert_no_errors(src);
}

#[test]
fn bootstrap_comparison() {
    // Output comparison
    let src = "type OutputDiff = MissingOutput | ExtraOutput | DifferentOutput

type ComparisonResult = OutputsEqual | OutputsDifferent

fn is_different_diff(diff: OutputDiff) -> Bool:
    match diff:
        DifferentOutput:
            ret true
        _:
            ret false

fn is_missing_output(diff: OutputDiff) -> Bool:
    match diff:
        MissingOutput:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn bootstrap_errors() {
    // Bootstrap errors
    let src =
        "type BootstrapError = CompileError | ValidationError | SelfCompileError | BootstrapIOError

type ErrorSeverity = FatalError | RecoverableError | Warning

fn is_fatal_error(err: BootstrapError) -> Bool:
    match err:
        CompileError:
            ret true
        SelfCompileError:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn bootstrap_self_compilation() {
    // Self-compilation process
    let src = "type SelfCompileMode = FullSelfCompile | IncrementalSelfCompile | NoSelfCompile

type SelfCompileResult = SelfCompileSuccess | SelfCompileFailure

fn should_self_compile(mode: SelfCompileMode) -> Bool:
    match mode:
        FullSelfCompile:
            ret true
        IncrementalSelfCompile:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn bootstrap_final_result() {
    // Bootstrap final result
    let src =
        "type FinalBootstrapResult = BootstrapSuccess | BootstrapPartialSuccess | BootstrapFailure

type BootstrapStats = StatsSuccess | StatsPartial | StatsFailure

fn is_complete_success(result: FinalBootstrapResult) -> Bool:
    match result:
        BootstrapSuccess:
            ret true
        _:
            ret false

fn is_complete_failure(result: FinalBootstrapResult) -> Bool:
    match result:
        BootstrapFailure:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

#[test]
fn bootstrap_test_mode() {
    // Bootstrap test/validation modes
    let src = "type TestMode = FullTest | SmokeTest | RegressionTest

type TestResult = TestPass | TestFail | TestSkip

fn is_full_test(mode: TestMode) -> Bool:
    match mode:
        FullTest:
            ret true
        _:
            ret false

fn is_regression_test(mode: TestMode) -> Bool:
    match mode:
        RegressionTest:
            ret true
        _:
            ret false
";
    assert_no_errors(src);
}

// ============================================================================
// Record types: construction + field reads
// ============================================================================

#[test]
fn record_field_read_returns_field_type() {
    let src = r#"
type Position:
    line: Int
    col: Int

fn get_line(p: Position) -> Int:
    ret p.line
"#;
    assert_no_errors(src);
}

#[test]
fn record_literal_validates_field_types() {
    let src = r#"
type Position:
    line: Int
    col: Int

fn make() -> Position:
    ret Position { line = "oops", col = 0 }
"#;
    assert_error_contains(src, "field `line`");
}

#[test]
fn record_literal_rejects_unknown_field() {
    let src = r#"
type Position:
    line: Int

fn make() -> Position:
    ret Position { line = 1, bogus = 2 }
"#;
    assert_error_contains(src, "no field `bogus`");
}

#[test]
fn record_literal_rejects_missing_field() {
    let src = r#"
type Position:
    line: Int
    col: Int

fn make() -> Position:
    ret Position { line = 1 }
"#;
    assert_error_contains(src, "missing field `col`");
}

#[test]
fn record_field_read_rejects_unknown_field() {
    let src = r#"
type Position:
    line: Int

fn bad(p: Position) -> Int:
    ret p.bogus
"#;
    assert_error_contains(src, "no field `bogus`");
}

#[test]
fn record_field_read_on_non_record_errors() {
    let src = r#"
fn bad(x: Int) -> Int:
    ret x.line
"#;
    assert_error_contains(src, "non-record type");
}

#[test]
fn record_spread_fills_missing_fields_from_base() {
    let src = r#"
type Position:
    line: Int
    col: Int

fn advance(p: Position) -> Position:
    ret Position { ..p, col = p.col + 1 }
"#;
    assert_no_errors(src);
}

#[test]
fn record_spread_rejects_unknown_field() {
    let src = r#"
type Position:
    line: Int
    col: Int

fn bad(p: Position) -> Position:
    ret Position { ..p, missing = 1 }
"#;
    assert_error_contains(src, "no field `missing`");
}

#[test]
fn record_spread_rejects_wrong_base_type() {
    let src = r#"
type A:
    x: Int

type B:
    x: Int

fn bad(a: A) -> B:
    ret B { ..a, x = 1 }
"#;
    assert_error_contains(src, "record-spread base must be a `B`");
}

// =========================================================================
// Method Syntax Dispatch Tests
// =========================================================================

/// Test 1: Simple user-defined method with Type_method naming convention
#[test]
fn method_dispatch_user_defined_string_trim() {
    let src = r#"
fn String_trim(s: String) -> String:
    ret s

fn main() -> String:
    let result = "  hello  ".trim()
    ret result
"#;
    assert_no_errors(src);
}

/// Test 2: User-defined method with arguments
#[test]
fn method_dispatch_user_defined_string_concat() {
    let src = r#"
fn String_concat(a: String, b: String) -> String:
    ret a

fn main() -> String:
    let result = "hello".concat(" world")
    ret result
"#;
    assert_no_errors(src);
}

/// Test 3: Chained method calls
#[test]
fn method_dispatch_chained_calls() {
    let src = r#"
fn String_trim(s: String) -> String:
    ret s

fn String_length(s: String) -> Int:
    ret 5

fn main() -> Int:
    let n = "  hello  ".trim().length()
    ret n
"#;
    assert_no_errors(src);
}

/// Test 4: Method on custom struct type
#[test]
fn method_dispatch_custom_struct() {
    let src = r#"
type Point:
    x: Int
    y: Int

fn Point_distance(p: Point) -> Float:
    ret 0.0

fn main() -> Float:
    let p = Point { x = 3, y = 4 }
    let d = p.distance()
    ret d
"#;
    assert_no_errors(src);
}

/// Test 5: Method with multiple arguments on custom type
#[test]
fn method_dispatch_custom_struct_with_args() {
    let src = r#"
type Rectangle:
    width: Int
    height: Int

fn Rectangle_scale(r: Rectangle, factor: Int) -> Rectangle:
    ret Rectangle { width = r.width * factor, height = r.height * factor }

fn main() -> Rectangle:
    let rect = Rectangle { width = 10, height = 20 }
    let scaled = rect.scale(2)
    ret scaled
"#;
    assert_no_errors(src);
}

/// Test 6: Method on generic List type
#[test]
fn method_dispatch_list_custom_method() {
    let src = r#"
fn List_sum(lst: List[Int]) -> Int:
    ret 0

fn main() -> Int:
    let nums = [1, 2, 3]
    let total = nums.sum()
    ret total
"#;
    assert_no_errors(src);
}

/// Test 7: Generic method naming convention with custom generic type
#[test]
fn method_dispatch_generic_vec_type() {
    let src = r#"
fn Vec_push(v: List[Int], item: Int) -> List[Int]:
    ret v

fn main() -> List[Int]:
    let v = [1, 2]
    let new_v = v.push(3)
    ret new_v
"#;
    assert_no_errors(src);
}

/// Test 8: Method not found error for user-defined type
#[test]
fn method_dispatch_unknown_method_error() {
    let src = r#"
type Point:
    x: Int
    y: Int

fn main() -> Int:
    let p = Point { x = 1, y = 2 }
    ret p.unknown_method()
"#;
    assert_error_contains(src, "has no method `unknown_method`");
}

// ---------------------------------------------------------------------------
// @verified annotation (ADR 0003 — tiered contracts, sub-issue #327)
// ---------------------------------------------------------------------------

/// `@verified` with at least one contract emits an "unimplemented;
/// falls back to runtime" warning per ADR 0003 implementation step 1.
/// The function still type-checks (no error), so downstream phases run.
#[test]
fn verified_with_contracts_emits_unimplemented_warning() {
    let src = r#"
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn clamp_nonneg(n: Int) -> Int:
    if n >= 0:
        n
    else:
        0
"#;
    assert_warning_contains(src, "static contract verification is unimplemented");
    // No hard errors — the warning is the load-bearing diagnostic.
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
    assert!(
        errors.is_empty(),
        "@verified with contracts should not produce errors at the launch tier; got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

/// `@verified` with no `@requires` and no `@ensures` is a checker
/// error: the verified tier exists to discharge contracts, so an empty
/// contract set is almost always a typo.
#[test]
fn verified_without_contracts_is_an_error() {
    let src = r#"
@verified
fn id(x: Int) -> Int:
    ret x
"#;
    assert_error_contains(
        src,
        "@verified function `id` has no `@requires` or `@ensures`",
    );
}

/// Functions without `@verified` continue to behave exactly as today
/// (no warning, no error from the verified-tier rules). This pins the
/// backwards-compatibility guarantee from ADR 0003 § Decision.
#[test]
fn unverified_function_unaffected_by_verified_rules() {
    let src = r#"
fn plain(x: Int) -> Int:
    ret x
"#;
    let all = check(src);
    assert!(
        all.iter().all(|e| !e.message.contains("@verified")),
        "plain functions must not be touched by the @verified rules; got: {:?}",
        all.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

/// `@verified` with only a `@requires` (no `@ensures`) is valid: ADR
/// 0003 only requires *at least one* predicate on a verified function.
#[test]
fn verified_with_only_requires_is_warning_not_error() {
    let src = r#"
@verified
@requires(n >= 0)
fn nonneg(n: Int) -> Int:
    ret n
"#;
    assert_warning_contains(src, "static contract verification is unimplemented");
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
}

/// `@verified` with only an `@ensures` (no `@requires`) is also valid.
#[test]
fn verified_with_only_ensures_is_warning_not_error() {
    let src = r#"
@verified
@ensures(result >= 0)
fn always_nonneg() -> Int:
    ret 0
"#;
    assert_warning_contains(src, "static contract verification is unimplemented");
    let all = check(src);
    let errors: Vec<_> = all.iter().filter(|e| !e.is_warning).collect();
    assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
}

// ---------------------------------------------------------------------------
// @verified Z3 discharge wiring (ADR 0003 step 3, sub-issue #329)
// ---------------------------------------------------------------------------

/// The launch-tier warning text now mentions the `GRADIENT_VC_VERIFY`
/// opt-in so users discover the new path. This pins the user-facing
/// surface: any rewording must stay backward-compatible with the
/// existing "static contract verification is unimplemented" prefix
/// for tools/agents that grep on it.
#[test]
fn verified_warning_mentions_gradient_vc_verify_opt_in() {
    let src = r#"
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn clamp_nonneg(n: Int) -> Int:
    if n >= 0:
        n
    else:
        0
"#;
    assert_warning_contains(src, "GRADIENT_VC_VERIFY=1");
}
