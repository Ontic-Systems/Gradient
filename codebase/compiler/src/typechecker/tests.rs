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
    assert!(
        !errors.is_empty(),
        "should report non-exhaustive match"
    );
    assert!(
        errors.iter().any(|e| e.message.contains("non-exhaustive") || e.message.contains("missing")),
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

// ---------------------------------------------------------------------------
// Multi-file module resolution and qualified calls
// ---------------------------------------------------------------------------

use super::checker::{check_module_with_imports, ImportedModules};
use super::env::FnSig;

/// Build an ImportedModules map with a single module containing the given functions.
fn make_imports(module_name: &str, fns: Vec<(&str, FnSig)>) -> ImportedModules {
    let mut module_fns = std::collections::HashMap::new();
    for (name, sig) in fns {
        module_fns.insert(name.to_string(), sig);
    }
    let mut imports = ImportedModules::new();
    imports.insert(module_name.to_string(), module_fns);
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
    let imports = make_imports("math", vec![
        ("add", FnSig {
                type_params: vec![],
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            ret: Ty::Int,
            effects: vec![],
        }),
    ]);

    let src = "\
use math

fn main() -> Int:
    ret math.add(3, 4)
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.is_empty(),
        "expected no type errors, got:\n{}",
        errors.iter().map(|e| format!("  - {}", e)).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn qualified_call_wrong_arg_type() {
    let imports = make_imports("math", vec![
        ("add", FnSig {
                type_params: vec![],
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            ret: Ty::Int,
            effects: vec![],
        }),
    ]);

    let src = "\
use math

fn main() -> Int:
    ret math.add(3, true)
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.iter().any(|e| e.message.contains("expected `Int`, found `Bool`")),
        "expected type mismatch error, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn qualified_call_wrong_arg_count() {
    let imports = make_imports("math", vec![
        ("add", FnSig {
                type_params: vec![],
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            ret: Ty::Int,
            effects: vec![],
        }),
    ]);

    let src = "\
use math

fn main() -> Int:
    ret math.add(3)
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.iter().any(|e| e.message.contains("expects 2 argument(s), but 1 were provided")),
        "expected arg count error, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn qualified_call_nonexistent_function() {
    let imports = make_imports("math", vec![
        ("add", FnSig {
                type_params: vec![],
            params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
            ret: Ty::Int,
            effects: vec![],
        }),
    ]);

    let src = "\
use math

fn main() -> Int:
    ret math.subtract(3, 4)
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.iter().any(|e| e.message.contains("module `math` has no function `subtract`")),
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
    assert!(
        !errors.is_empty(),
        "expected errors for unresolved module"
    );
}

#[test]
fn qualified_call_with_effects() {
    // Imported function with IO effect should require IO in caller.
    let imports = make_imports("io_mod", vec![
        ("write_line", FnSig {
                type_params: vec![],
            params: vec![("msg".to_string(), Ty::String)],
            ret: Ty::Unit,
            effects: vec!["IO".to_string()],
        }),
    ]);

    let src = "\
use io_mod

fn main() -> !{IO} ():
    io_mod.write_line(\"hello\")
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.is_empty(),
        "expected no type errors, got:\n{}",
        errors.iter().map(|e| format!("  - {}", e)).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn qualified_call_missing_effect() {
    // Calling an imported IO function without declaring IO should error.
    let imports = make_imports("io_mod", vec![
        ("write_line", FnSig {
                type_params: vec![],
            params: vec![("msg".to_string(), Ty::String)],
            ret: Ty::Unit,
            effects: vec!["IO".to_string()],
        }),
    ]);

    let src = "\
use io_mod

fn main():
    io_mod.write_line(\"hello\")
";
    let errors = check_with_imports(src, &imports);
    assert!(
        errors.iter().any(|e| e.message.contains("requires effect `IO`")),
        "expected effect error, got: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn qualified_call_return_type_used() {
    // The return type of a qualified call should be properly tracked.
    let imports = make_imports("math", vec![
        ("double", FnSig {
                type_params: vec![],
            params: vec![("x".to_string(), Ty::Int)],
            ret: Ty::Int,
            effects: vec![],
        }),
    ]);

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
        errors.iter().map(|e| format!("  - {}", e)).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn qualified_call_return_type_mismatch() {
    // Assigning a qualified call result to wrong type should error.
    let imports = make_imports("math", vec![
        ("double", FnSig {
                type_params: vec![],
            params: vec![("x".to_string(), Ty::Int)],
            ret: Ty::Int,
            effects: vec![],
        }),
    ]);

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

    let mut math_fns = std::collections::HashMap::new();
    math_fns.insert("add".to_string(), FnSig {
                type_params: vec![],
        params: vec![("a".to_string(), Ty::Int), ("b".to_string(), Ty::Int)],
        ret: Ty::Int,
        effects: vec![],
    });
    imports.insert("math".to_string(), math_fns);

    let mut str_fns = std::collections::HashMap::new();
    str_fns.insert("concat".to_string(), FnSig {
                type_params: vec![],
        params: vec![("a".to_string(), Ty::String), ("b".to_string(), Ty::String)],
        ret: Ty::String,
        effects: vec![],
    });
    imports.insert("str_utils".to_string(), str_fns);

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
        errors.iter().map(|e| format!("  - {}", e)).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn local_and_imported_coexist() {
    // Local functions and imported functions should coexist.
    let imports = make_imports("helper", vec![
        ("inc", FnSig {
                type_params: vec![],
            params: vec![("x".to_string(), Ty::Int)],
            ret: Ty::Int,
            effects: vec![],
        }),
    ]);

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
        errors.iter().map(|e| format!("  - {}", e)).collect::<Vec<_>>().join("\n")
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
    assert_no_errors(src);
}

#[test]
fn extern_fn_with_float_params() {
    let src = "\
@extern
fn sin(x: Float) -> Float
";
    assert_no_errors(src);
}

#[test]
fn extern_fn_with_bool_param() {
    let src = "\
@extern
fn check(b: Bool) -> Int
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
fn f(s: String) -> String:
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
    assert_error_contains(src, "tuple destructuring has 3 names but the tuple has 2 elements");
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
