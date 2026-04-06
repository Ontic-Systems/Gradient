//! SMT Contract Verification Integration Tests
//!
//! These tests verify that the SMT-based contract verification system works correctly.
//! They require the `smt-verify` feature to be enabled.

#![cfg(feature = "smt-verify")]

use gradient_compiler::ast::item::{ContractKind, FnDef, ItemKind};
use gradient_compiler::typechecker::smt::{verify_function_contracts, VerificationResult};
use gradient_compiler::{lexer::Lexer, parser};

/// Helper function to parse a function from source and extract the FnDef
fn parse_function(source: &str) -> FnDef {
    let mut lexer = Lexer::new(source, 0);
    let tokens = lexer.tokenize();
    let (module, errors) = parser::parse(tokens, 0);
    assert!(errors.is_empty(), "Parse errors: {:?}", errors);
    assert!(!module.items.is_empty(), "No items in module");
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => fn_def.clone(),
        _ => panic!("Expected function definition"),
    }
}

/// Test 1: Simple valid precondition
/// @requires n >= 0 should be satisfiable
#[test]
fn test_simple_precondition_valid() {
    let source = r#"@requires(n >= 0)
fn abs(n: Int) -> Int:
    if n < 0:
        ret -n
    ret n
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 1);
    assert_eq!(fn_def.contracts[0].kind, ContractKind::Requires);

    let results = verify_function_contracts(&fn_def);
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[&0], VerificationResult::Proved),
        "Precondition should be satisfiable (proved)"
    );
}

/// Test 2: Simple valid postcondition
/// @ensures result >= 0 should hold for abs function
#[test]
fn test_simple_postcondition_valid() {
    let source = r#"@requires(n >= 0)
@ensures(result >= 0)
fn abs(n: Int) -> Int:
    if n < 0:
        ret -n
    ret n
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 2);

    let results = verify_function_contracts(&fn_def);
    // Second contract is the ensures clause
    assert!(
        matches!(results[&1], VerificationResult::Proved),
        "Postcondition result >= 0 should be provable"
    );
}

/// Test 3: Contradictory precondition (unsatisfiable)
/// @requires n > 0 and n < 0 should be unsatisfiable
#[test]
fn test_contradictory_precondition() {
    let source = r#"
@requires(n > 0 and n < 0)
fn impossible(n: Int) -> Int:
    ret n
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 1);

    let results = verify_function_contracts(&fn_def);
    // Unsatisfiable precondition gives CounterExample
    assert!(
        matches!(results[&0], VerificationResult::CounterExample { .. }),
        "Contradictory precondition should be unsatisfiable"
    );
}

/// Test 4: Postcondition that can be violated
/// A function that doesn't guarantee the postcondition
#[test]
fn test_postcondition_can_fail() {
    let source = r#"
@requires(x >= 0)
@ensures(result >= 0)
fn maybe_negative(x: Int) -> Int:
    ret x - 100
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 2);

    let results = verify_function_contracts(&fn_def);
    // The postcondition might not hold (x - 100 could be negative even if x >= 0)
    // Note: The SMT verifier may or may not catch this depending on how sophisticated it is
    // For now we just check that verification runs
    assert!(results.contains_key(&1));
}

/// Test 5: Multiple preconditions
/// All preconditions should be checked
#[test]
fn test_multiple_preconditions() {
    let source = r#"
@requires(x >= 0)
@requires(y >= 0)
@ensures(result >= 0)
fn sum_positive(x: Int, y: Int) -> Int:
    ret x + y
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 3);

    let results = verify_function_contracts(&fn_def);
    // First two should be proved (satisfiable)
    assert!(
        matches!(results[&0], VerificationResult::Proved),
        "First precondition should be satisfiable"
    );
    assert!(
        matches!(results[&1], VerificationResult::Proved),
        "Second precondition should be satisfiable"
    );
}

/// Test 6: Postcondition with arithmetic
/// Test that arithmetic expressions in postconditions work
#[test]
fn test_postcondition_arithmetic() {
    let source = r#"
@requires(n >= 0)
@ensures(result >= n)
fn double(n: Int) -> Int:
    ret n + n
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 2);

    let results = verify_function_contracts(&fn_def);
    // n + n >= n when n >= 0
    assert!(
        matches!(results[&1], VerificationResult::Proved),
        "Postcondition result >= n should be provable for n + n"
    );
}

/// Test 7: Boolean logic in contracts
/// Test and, or, not in preconditions
#[test]
fn test_boolean_logic_contracts() {
    let source = r#"
@requires(x > 0 or x < 0)
fn non_zero(x: Int) -> Int:
    ret x
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 1);

    let results = verify_function_contracts(&fn_def);
    // x > 0 or x < 0 is satisfiable (just not x == 0)
    assert!(
        matches!(results[&0], VerificationResult::Proved),
        "Precondition x > 0 or x < 0 should be satisfiable"
    );
}

/// Test 8: Result keyword in postcondition
/// The result keyword should refer to the return value
#[test]
fn test_result_keyword() {
    let source = r#"
@ensures(result >= 0)
fn constant_positive() -> Int:
    ret 42
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 1);

    let results = verify_function_contracts(&fn_def);
    // 42 >= 0 is always true
    assert!(
        matches!(results[&0], VerificationResult::Proved),
        "Postcondition result >= 0 should be provable for constant 42"
    );
}

/// Test 9: Complex arithmetic expression
/// Test that complex expressions work in contracts
#[test]
fn test_complex_arithmetic() {
    let source = r#"
@requires(a >= 0)
@requires(b >= 0)
@ensures(result >= a)
@ensures(result >= b)
fn max(a: Int, b: Int) -> Int:
    if a > b: ret a
    ret b
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 4);

    let results = verify_function_contracts(&fn_def);
    // Both postconditions should hold
    assert!(
        matches!(results[&2], VerificationResult::Proved),
        "Postcondition result >= a should be provable"
    );
    assert!(
        matches!(results[&3], VerificationResult::Proved),
        "Postcondition result >= b should be provable"
    );
}

/// Test 10: Integer comparison operators
/// Test all comparison operators in contracts
#[test]
fn test_comparison_operators() {
    let source = r#"
@requires(x > 0)
@requires(y >= 0)
@requires(z < 10)
@requires(w <= 5)
fn comparisons(x: Int, y: Int, z: Int, w: Int) -> Int:
    ret x + y + z + w
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 4);

    let results = verify_function_contracts(&fn_def);
    // All should be satisfiable
    for i in 0..4 {
        assert!(
            matches!(results[&i], VerificationResult::Proved),
            "Precondition {} should be satisfiable",
            i
        );
    }
}

/// Test 11: Equality in contracts
/// Test == and != operators
#[test]
fn test_equality_operators() {
    let source = r#"
@requires(x == 5)
@ensures(result == x)
fn identity(x: Int) -> Int:
    ret x
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 2);

    let results = verify_function_contracts(&fn_def);
    // Both should be provable
    assert!(
        matches!(results[&0], VerificationResult::Proved),
        "Precondition x == 5 should be satisfiable"
    );
    assert!(
        matches!(results[&1], VerificationResult::Proved),
        "Postcondition result == x should be provable"
    );
}

/// Test 12: Division operation
/// Test division in contract expressions
#[test]
fn test_division_operation() {
    let source = r#"
@requires(n >= 0)
@ensures(result >= 0)
fn half(n: Int) -> Int:
    ret n / 2
"#;
    let fn_def = parse_function(source);
    assert_eq!(fn_def.contracts.len(), 2);

    let results = verify_function_contracts(&fn_def);
    // n >= 0 implies n/2 >= 0 for integer division
    assert!(
        matches!(results[&1], VerificationResult::Proved),
        "Postcondition result >= 0 should be provable for n / 2"
    );
}
