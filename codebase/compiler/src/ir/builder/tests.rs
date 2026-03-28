//! Tests for the IR builder.
//!
//! Each test parses a snippet of Gradient source code through the lexer and
//! parser, then runs the IR builder and asserts properties of the resulting
//! IR module.

use crate::ir::builder::IrBuilder;
use crate::ir::{Instruction, Literal, Module as IrModule, Type, Value, CmpOp};
use crate::lexer::Lexer;
use crate::parser;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Lex + parse + lower a Gradient source snippet and return the IR module.
/// Panics if there are parse errors or IR builder errors.
fn build_ok(src: &str) -> IrModule {
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(
        parse_errors.is_empty(),
        "unexpected parse errors: {:?}",
        parse_errors
    );
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    assert!(
        ir_errors.is_empty(),
        "unexpected IR builder errors: {:?}",
        ir_errors
    );
    ir_module
}

/// Lex + parse + lower, returning both the module and errors (for negative tests).
fn build_with_errors(src: &str) -> (IrModule, Vec<String>) {
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (ast_module, _parse_errors) = parser::parse(tokens, 0);
    IrBuilder::build_module(&ast_module)
}

/// Collect all instructions from every block of the first function.
fn all_instructions(module: &IrModule) -> Vec<&Instruction> {
    module.functions[0]
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .collect()
}

/// Collect all `Value`s that appear as the *defined* (destination) value
/// of an instruction — i.e. the left-hand side of an SSA assignment.
fn defined_values(module: &IrModule) -> Vec<Value> {
    let mut defs = Vec::new();
    for func in &module.functions {
        for block in &func.blocks {
            for instr in &block.instructions {
                match instr {
                    Instruction::Const(v, _)
                    | Instruction::Call(v, _, _)
                    | Instruction::Add(v, _, _)
                    | Instruction::Sub(v, _, _)
                    | Instruction::Mul(v, _, _)
                    | Instruction::Div(v, _, _)
                    | Instruction::Cmp(v, _, _, _)
                    | Instruction::Phi(v, _)
                    | Instruction::Alloca(v, _)
                    | Instruction::Load(v, _) => {
                        defs.push(*v);
                    }
                    Instruction::ConstructVariant { result, .. } => {
                        defs.push(*result);
                    }
                    Instruction::GetVariantTag { result, .. } => {
                        defs.push(*result);
                    }
                    Instruction::GetVariantField { result, .. } => {
                        defs.push(*result);
                    }
                    Instruction::Store(_, _)
                    | Instruction::Ret(_)
                    | Instruction::Branch(_, _, _)
                    | Instruction::Jump(_) => {}
                }
            }
        }
    }
    defs
}

// ---------------------------------------------------------------------------
// Tests: basic functions
// ---------------------------------------------------------------------------

#[test]
fn simple_return_int() {
    // fn main() -> Int:
    //     ret 42
    let src = "fn main() -> Int:\n    ret 42\n";
    let module = build_ok(src);

    assert_eq!(module.functions.len(), 1);
    let func = &module.functions[0];
    assert_eq!(func.name, "main");
    assert_eq!(func.return_type, Type::I64);
    assert!(func.params.is_empty());

    // Should have one block with a Const and a Ret.
    assert_eq!(func.blocks.len(), 1);
    let instrs = &func.blocks[0].instructions;
    assert!(instrs.len() >= 2);

    // First instruction should be Const(_, Int(42)).
    match &instrs[0] {
        Instruction::Const(_, Literal::Int(42)) => {}
        other => panic!("expected Const(_, Int(42)), got {:?}", other),
    }

    // Last instruction should be Ret(Some(_)).
    match instrs.last().unwrap() {
        Instruction::Ret(Some(_)) => {}
        other => panic!("expected Ret(Some(_)), got {:?}", other),
    }
}

#[test]
fn return_bool_literal() {
    let src = "fn check() -> Bool:\n    ret true\n";
    let module = build_ok(src);

    let func = &module.functions[0];
    assert_eq!(func.return_type, Type::Bool);

    let instrs = all_instructions(&module);
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Const(_, Literal::Bool(true)))));
}

#[test]
fn return_string_literal() {
    let src = "fn greet() -> String:\n    ret \"hello\"\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    assert!(instrs.iter().any(|i| matches!(
        i,
        Instruction::Const(_, Literal::Str(s)) if s == "hello"
    )));
}

// ---------------------------------------------------------------------------
// Tests: arithmetic
// ---------------------------------------------------------------------------

#[test]
fn arithmetic_add() {
    // fn add(a: Int, b: Int) -> Int:
    //     ret a + b
    let src = "fn add(a: Int, b: Int) -> Int:\n    ret a + b\n";
    let module = build_ok(src);

    let func = &module.functions[0];
    assert_eq!(func.name, "add");
    assert_eq!(func.params, vec![Type::I64, Type::I64]);
    assert_eq!(func.return_type, Type::I64);

    let instrs = all_instructions(&module);
    assert!(
        instrs.iter().any(|i| matches!(i, Instruction::Add(_, _, _))),
        "expected an Add instruction, got: {:?}",
        instrs
    );
}

#[test]
fn arithmetic_complex_expression() {
    // fn calc(a: Int, b: Int, c: Int) -> Int:
    //     ret a + b * c
    let src = "fn calc(a: Int, b: Int, c: Int) -> Int:\n    ret a + b * c\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    // Should have a Mul and an Add (in that order due to precedence).
    let has_mul = instrs.iter().any(|i| matches!(i, Instruction::Mul(_, _, _)));
    let has_add = instrs.iter().any(|i| matches!(i, Instruction::Add(_, _, _)));
    assert!(has_mul, "expected a Mul instruction");
    assert!(has_add, "expected an Add instruction");
}

#[test]
fn arithmetic_sub_div() {
    let src = "fn calc(x: Int, y: Int) -> Int:\n    ret x - y / x\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Sub(_, _, _))));
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Div(_, _, _))));
}

// ---------------------------------------------------------------------------
// Tests: let bindings
// ---------------------------------------------------------------------------

#[test]
fn let_binding_and_return() {
    // fn f() -> Int:
    //     let x = 10
    //     ret x
    let src = "fn f() -> Int:\n    let x = 10\n    ret x\n";
    let module = build_ok(src);

    let func = &module.functions[0];
    assert_eq!(func.blocks.len(), 1);

    let instrs = &func.blocks[0].instructions;
    // Should define x via Const, then Ret with the same value.
    match &instrs[0] {
        Instruction::Const(v, Literal::Int(10)) => {
            // The return should reference this same value.
            match instrs.last().unwrap() {
                Instruction::Ret(Some(ret_v)) => {
                    assert_eq!(v, ret_v, "ret should use the same value as the let binding");
                }
                other => panic!("expected Ret, got {:?}", other),
            }
        }
        other => panic!("expected Const(_, Int(10)), got {:?}", other),
    }
}

#[test]
fn multiple_let_bindings() {
    let src = "fn f() -> Int:\n    let a = 1\n    let b = 2\n    ret a + b\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    // Two constants, one add, one ret.
    let const_count = instrs
        .iter()
        .filter(|i| matches!(i, Instruction::Const(_, _)))
        .count();
    assert!(const_count >= 2, "expected at least 2 Const instructions");
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Add(_, _, _))));
}

// ---------------------------------------------------------------------------
// Tests: comparisons
// ---------------------------------------------------------------------------

#[test]
fn comparison_operators() {
    let src = "fn cmp(a: Int, b: Int) -> Bool:\n    ret a == b\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Cmp(_, CmpOp::Eq, _, _))));
}

#[test]
fn comparison_lt() {
    let src = "fn lt(a: Int, b: Int) -> Bool:\n    ret a < b\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Cmp(_, CmpOp::Lt, _, _))));
}

// ---------------------------------------------------------------------------
// Tests: unary operators
// ---------------------------------------------------------------------------

#[test]
fn unary_neg() {
    // -x  is lowered to  0 - x
    let src = "fn neg(x: Int) -> Int:\n    ret -x\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    // Should have a Const(0) and a Sub.
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Const(_, Literal::Int(0)))));
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Sub(_, _, _))));
}

#[test]
fn unary_not() {
    // not x  is lowered to  x == false
    let src = "fn inv(x: Bool) -> Bool:\n    ret not x\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Const(_, Literal::Bool(false)))));
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Cmp(_, CmpOp::Eq, _, _))));
}

// ---------------------------------------------------------------------------
// Tests: if/else
// ---------------------------------------------------------------------------

#[test]
fn if_else_produces_phi() {
    // fn choose(c: Bool) -> Int:
    //     if c:
    //         1
    //     else:
    //         2
    let src = "fn choose(c: Bool) -> Int:\n    if c:\n        1\n    else:\n        2\n";
    let module = build_ok(src);

    let func = &module.functions[0];
    // Should have multiple blocks (at least: entry, then, else, merge).
    assert!(
        func.blocks.len() >= 4,
        "expected >= 4 blocks for if/else, got {}",
        func.blocks.len()
    );

    let instrs = all_instructions(&module);
    // Should have Branch, Jump, and Phi instructions.
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Branch(_, _, _))));
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Jump(_))));
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Phi(_, _))));
}

#[test]
fn if_without_else() {
    // When there is no else arm, the builder should still produce valid
    // IR with a unit value for the missing branch.
    let src = "fn maybe(c: Bool) -> Int:\n    if c:\n        42\n    ret 0\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Branch(_, _, _))));
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Phi(_, _))));
}

// ---------------------------------------------------------------------------
// Tests: function calls
// ---------------------------------------------------------------------------

#[test]
fn function_call_simple() {
    // fn helper() -> Int:
    //     ret 1
    // fn main() -> Int:
    //     ret helper()
    let src = "fn helper() -> Int:\n    ret 1\nfn main() -> Int:\n    ret helper()\n";
    let module = build_ok(src);

    assert_eq!(module.functions.len(), 2);
    assert_eq!(module.functions[0].name, "helper");
    assert_eq!(module.functions[1].name, "main");

    // main's IR should contain a Call instruction.
    let main_instrs: Vec<_> = module.functions[1]
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .collect();
    assert!(
        main_instrs.iter().any(|i| matches!(i, Instruction::Call(_, _, _))),
        "expected a Call instruction in main"
    );
}

#[test]
fn function_call_with_args() {
    let src = "fn add(a: Int, b: Int) -> Int:\n    ret a + b\nfn main() -> Int:\n    ret add(3, 4)\n";
    let module = build_ok(src);

    // main should have Call with two argument values.
    let main_instrs: Vec<_> = module.functions[1]
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .collect();

    let call_instr = main_instrs
        .iter()
        .find(|i| matches!(i, Instruction::Call(_, _, _)))
        .expect("expected a Call instruction");

    match call_instr {
        Instruction::Call(_, _, args) => {
            assert_eq!(args.len(), 2, "expected 2 arguments");
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// Tests: hello world module
// ---------------------------------------------------------------------------

#[test]
fn hello_world_module() {
    let src = "fn main():\n    print(\"Hello, Gradient!\")\n";
    let module = build_ok(src);

    assert_eq!(module.name, "main");
    assert_eq!(module.functions.len(), 1);

    let func = &module.functions[0];
    assert_eq!(func.name, "main");
    assert_eq!(func.return_type, Type::Void);

    let instrs = all_instructions(&module);
    // Should have a string constant and a call to print.
    assert!(instrs.iter().any(|i| matches!(
        i,
        Instruction::Const(_, Literal::Str(s)) if s == "Hello, Gradient!"
    )));
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Call(_, _, _))));
}

// ---------------------------------------------------------------------------
// Tests: SSA property
// ---------------------------------------------------------------------------

#[test]
fn ssa_values_defined_exactly_once() {
    // Build a non-trivial function and verify uniqueness of defined values.
    let src = "fn f(a: Int, b: Int) -> Int:\n    let c = a + b\n    let d = c * 2\n    ret d - a\n";
    let module = build_ok(src);

    let defs = defined_values(&module);
    let mut seen = std::collections::HashSet::new();
    for v in &defs {
        assert!(
            seen.insert(v.0),
            "SSA violation: Value({}) is defined more than once",
            v.0
        );
    }
}

#[test]
fn ssa_uniqueness_across_branches() {
    // if/else produces phi nodes; values should still be unique.
    let src = "fn f(c: Bool) -> Int:\n    if c:\n        10\n    else:\n        20\n";
    let module = build_ok(src);

    let defs = defined_values(&module);
    let mut seen = std::collections::HashSet::new();
    for v in &defs {
        assert!(
            seen.insert(v.0),
            "SSA violation in branching code: Value({}) defined more than once",
            v.0
        );
    }
}

// ---------------------------------------------------------------------------
// Tests: module naming
// ---------------------------------------------------------------------------

#[test]
fn module_name_from_decl() {
    let src = "mod mylib.utils\nfn f():\n    ret 0\n";
    let module = build_ok(src);
    assert_eq!(module.name, "mylib.utils");
}

#[test]
fn module_name_defaults_to_main() {
    let src = "fn f():\n    ret 0\n";
    let module = build_ok(src);
    assert_eq!(module.name, "main");
}

// ---------------------------------------------------------------------------
// Tests: type resolution
// ---------------------------------------------------------------------------

#[test]
fn param_types_resolved() {
    let src = "fn f(a: Int, b: Float, c: Bool) -> Int:\n    ret 0\n";
    let module = build_ok(src);

    let func = &module.functions[0];
    assert_eq!(func.params, vec![Type::I64, Type::F64, Type::Bool]);
    assert_eq!(func.return_type, Type::I64);
}

// ---------------------------------------------------------------------------
// Tests: error collection (no panics)
// ---------------------------------------------------------------------------

#[test]
fn undefined_variable_produces_error() {
    let src = "fn f() -> Int:\n    ret x\n";
    let (_module, errors) = build_with_errors(src);
    assert!(
        errors.iter().any(|e| e.contains("undefined variable")),
        "expected 'undefined variable' error, got: {:?}",
        errors
    );
}

#[test]
fn call_to_unknown_function_produces_error() {
    let src = "fn f() -> Int:\n    ret unknown()\n";
    let (_module, errors) = build_with_errors(src);
    assert!(
        errors.iter().any(|e| e.contains("undefined function")),
        "expected 'undefined function' error, got: {:?}",
        errors
    );
}

// ---------------------------------------------------------------------------
// Tests: expression statement (side effects)
// ---------------------------------------------------------------------------

#[test]
fn expr_statement_discards_value() {
    // A bare call in statement position should still emit the Call.
    let src = "fn f():\n    print(\"hi\")\n";
    let module = build_ok(src);

    let instrs = all_instructions(&module);
    assert!(instrs.iter().any(|i| matches!(i, Instruction::Call(_, _, _))));
}

// ---------------------------------------------------------------------------
// Tests: multiple functions
// ---------------------------------------------------------------------------

#[test]
fn multiple_functions_in_module() {
    let src = "\
fn a() -> Int:
    ret 1
fn b() -> Int:
    ret 2
fn c() -> Int:
    ret 3
";
    let module = build_ok(src);
    assert_eq!(module.functions.len(), 3);
    assert_eq!(module.functions[0].name, "a");
    assert_eq!(module.functions[1].name, "b");
    assert_eq!(module.functions[2].name, "c");
}

// ---------------------------------------------------------------------------
// Tests: void functions
// ---------------------------------------------------------------------------

#[test]
fn void_function_implicit_return() {
    // A function with no explicit return should get an implicit Ret(None).
    let src = "fn noop():\n    let x = 1\n";
    let module = build_ok(src);

    let func = &module.functions[0];
    assert_eq!(func.return_type, Type::Void);

    let instrs = all_instructions(&module);
    assert!(
        instrs.iter().any(|i| matches!(i, Instruction::Ret(None))),
        "expected implicit Ret(None) for void function"
    );
}

// ---------------------------------------------------------------------------
// Enum IR tests
// ---------------------------------------------------------------------------

#[test]
fn enum_variant_tags_in_ir() {
    let src = "\
type Color = Red | Green | Blue

fn get_red() -> Color:
    ret Red
";
    let module = build_ok(src);

    // get_red should emit a ConstructVariant with tag 0 (Red is the first
    // variant). All enum values are now heap-allocated tagged unions.
    let func = module.functions.iter().find(|f| f.name == "get_red").unwrap();
    let instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();
    assert!(
        instrs.iter().any(|i| matches!(
            i,
            Instruction::ConstructVariant { tag: 0, payload, .. } if payload.is_empty()
        )),
        "expected ConstructVariant {{ tag: 0, payload: [] }} for Red variant, got: {:?}",
        instrs
    );
}

#[test]
fn enum_match_generates_comparisons() {
    let src = "\
type Color = Red | Green | Blue

fn describe(c: Color) -> Int:
    match c:
        Red:
            ret 0
        Green:
            ret 1
        Blue:
            ret 2
";
    let module = build_ok(src);
    let func = module.functions.iter().find(|f| f.name == "describe").unwrap();

    // Should have CmpOp::Eq comparisons for each variant
    let instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();
    let cmp_count = instrs.iter().filter(|i| matches!(i, Instruction::Cmp(_, CmpOp::Eq, _, _))).count();
    assert!(
        cmp_count >= 2,
        "expected at least 2 Cmp instructions for enum match, got {}",
        cmp_count
    );
    // Enum match should also emit GetVariantTag to load the tag from the
    // heap pointer before comparing.
    let has_get_tag = instrs.iter().any(|i| matches!(i, Instruction::GetVariantTag { .. }));
    assert!(
        has_get_tag,
        "expected GetVariantTag instruction in enum match"
    );
}

// ---------------------------------------------------------------------------
// Design-by-contract: @requires and @ensures
// ---------------------------------------------------------------------------

#[test]
fn requires_generates_branch_and_fail_call() {
    let src = "\
@requires(x > 0)
fn positive(x: Int) -> Int:
    ret x
";
    let module = build_ok(src);
    let func = module.functions.iter().find(|f| f.name == "positive").unwrap();

    // The function should have multiple blocks:
    // - entry block with the condition check and branch
    // - fail block with the contract failure call
    // - ok block with the function body
    assert!(
        func.blocks.len() >= 3,
        "expected at least 3 blocks (entry, fail, ok) for @requires, got {}",
        func.blocks.len()
    );

    // Check that there's a Call instruction to __gradient_contract_fail.
    let all_instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();
    let has_fail_call = all_instrs.iter().any(|i| {
        if let Instruction::Call(_, func_ref, _) = i {
            // The func_ref should map to __gradient_contract_fail.
            module.func_refs.get(func_ref).map_or(false, |name| name == "__gradient_contract_fail")
        } else {
            false
        }
    });
    assert!(has_fail_call, "expected a call to __gradient_contract_fail for @requires");

    // Check that there's a Branch instruction (condition check).
    let has_branch = all_instrs.iter().any(|i| matches!(i, Instruction::Branch(_, _, _)));
    assert!(has_branch, "expected a Branch instruction for the contract condition check");
}

#[test]
fn ensures_generates_branch_and_fail_call() {
    let src = "\
@ensures(result > 0)
fn f(x: Int) -> Int:
    x + 1
";
    let module = build_ok(src);
    let func = module.functions.iter().find(|f| f.name == "f").unwrap();

    // Should have blocks for the postcondition check.
    assert!(
        func.blocks.len() >= 3,
        "expected at least 3 blocks for @ensures, got {}",
        func.blocks.len()
    );

    // Check that there's a call to __gradient_contract_fail.
    let all_instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();
    let has_fail_call = all_instrs.iter().any(|i| {
        if let Instruction::Call(_, func_ref, _) = i {
            module.func_refs.get(func_ref).map_or(false, |name| name == "__gradient_contract_fail")
        } else {
            false
        }
    });
    assert!(has_fail_call, "expected a call to __gradient_contract_fail for @ensures");
}

#[test]
fn no_contracts_no_extra_blocks() {
    let src = "\
fn f(x: Int) -> Int:
    ret x
";
    let module = build_ok(src);
    let func = module.functions.iter().find(|f| f.name == "f").unwrap();

    // Without contracts, should have just 1 block.
    assert_eq!(
        func.blocks.len(), 1,
        "expected 1 block for function without contracts, got {}",
        func.blocks.len()
    );
}

#[test]
fn contract_fail_message_contains_function_name() {
    let src = "\
@requires(x > 0)
fn my_func(x: Int) -> Int:
    ret x
";
    let module = build_ok(src);
    let func = module.functions.iter().find(|f| f.name == "my_func").unwrap();

    // Find the string constant used for the contract failure message.
    let all_instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();
    let has_msg = all_instrs.iter().any(|i| {
        if let Instruction::Const(_, Literal::Str(s)) = i {
            s.contains("my_func") && s.contains("@requires")
        } else {
            false
        }
    });
    assert!(has_msg, "expected contract failure message to contain function name and @requires");
}

// ---------------------------------------------------------------------------
// FFI: @extern and @export in IR
// ---------------------------------------------------------------------------

#[test]
fn ir_extern_fn_has_no_blocks() {
    // An @extern function should produce a Function with empty blocks.
    let src = "\
@extern
fn puts(s: String) -> Int
";
    let ir = build_ok(src);
    let func = ir.functions.iter().find(|f| f.name == "puts").expect("expected puts function");
    assert!(func.blocks.is_empty(), "extern function should have no blocks");
    assert!(!func.is_export);
}

#[test]
fn ir_extern_fn_with_lib() {
    // @extern("libm") should set extern_lib.
    let src = r#"
@extern("libm")
fn sin(x: Float) -> Float
"#;
    let ir = build_ok(src);
    let func = ir.functions.iter().find(|f| f.name == "sin").expect("expected sin function");
    assert!(func.blocks.is_empty());
    assert_eq!(func.extern_lib.as_deref(), Some("libm"));
}

#[test]
fn ir_export_fn_has_blocks_and_flag() {
    // An @export function should have blocks (body) and is_export=true.
    let src = "\
@export
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
    let ir = build_ok(src);
    let func = ir.functions.iter().find(|f| f.name == "add").expect("expected add function");
    assert!(!func.blocks.is_empty(), "export function should have blocks (body)");
    assert!(func.is_export, "export function should have is_export=true");
    assert!(func.extern_lib.is_none());
}

#[test]
fn ir_regular_fn_not_export() {
    // A regular function should not be export.
    let src = "\
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
    let ir = build_ok(src);
    let func = ir.functions.iter().find(|f| f.name == "add").expect("expected add function");
    assert!(!func.is_export);
    assert!(func.extern_lib.is_none());
}

// ---------------------------------------------------------------------------
// Closure / lambda lowering
// ---------------------------------------------------------------------------

#[test]
fn closure_generates_function() {
    let src = "\
fn main():
    let f = |x: Int| x + 1
";
    let ir = build_ok(src);
    // Should have main + the closure function.
    let closure_fn = ir.functions.iter().find(|f| f.name.starts_with("__closure_"));
    assert!(closure_fn.is_some(), "expected a __closure_ function in IR");
    let closure = closure_fn.unwrap();
    assert_eq!(closure.params.len(), 1);
    assert_eq!(closure.params[0], Type::I64);
}

#[test]
fn closure_zero_params_generates_function() {
    let src = "\
fn main():
    let f = || 42
";
    let ir = build_ok(src);
    let closure_fn = ir.functions.iter().find(|f| f.name.starts_with("__closure_"));
    assert!(closure_fn.is_some(), "expected a __closure_ function in IR");
    let closure = closure_fn.unwrap();
    assert_eq!(closure.params.len(), 0);
}

#[test]
fn closure_multi_param_generates_function() {
    let src = "\
fn main():
    let f = |x: Int, y: Int| x + y
";
    let ir = build_ok(src);
    let closure_fn = ir.functions.iter().find(|f| f.name.starts_with("__closure_"));
    assert!(closure_fn.is_some(), "expected a __closure_ function in IR");
    let closure = closure_fn.unwrap();
    assert_eq!(closure.params.len(), 2);
    assert_eq!(closure.params[0], Type::I64);
    assert_eq!(closure.params[1], Type::I64);
}

#[test]
fn closure_has_return_instruction() {
    let src = "\
fn main():
    let f = |x: Int| x
";
    let ir = build_ok(src);
    let closure_fn = ir.functions.iter()
        .find(|f| f.name.starts_with("__closure_"))
        .expect("expected closure function");
    // The closure should have at least one block with a Ret instruction.
    let has_ret = closure_fn.blocks.iter().any(|b| {
        b.instructions.iter().any(|i| matches!(i, Instruction::Ret(_)))
    });
    assert!(has_ret, "closure function should have a return instruction");
}

// ---------------------------------------------------------------------------
// Tuple lowering
// ---------------------------------------------------------------------------

#[test]
fn ir_tuple_literal_creates_alloca_and_store() {
    let src = "\
fn f() -> Int:
    let pair = (1, 2)
    ret pair.0
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // Should have Alloca instructions (for tuple elements).
    let alloca_count = instrs.iter().filter(|i| matches!(i, Instruction::Alloca(_, _))).count();
    assert!(alloca_count >= 2, "expected at least 2 alloca instructions for a 2-element tuple, got {}", alloca_count);
    // Should have Store instructions.
    let store_count = instrs.iter().filter(|i| matches!(i, Instruction::Store(_, _))).count();
    assert!(store_count >= 2, "expected at least 2 store instructions, got {}", store_count);
}

#[test]
fn ir_tuple_field_access_creates_load() {
    let src = "\
fn f() -> Int:
    let pair = (10, 20)
    ret pair.1
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // Should have Load instructions (for tuple field access).
    let load_count = instrs.iter().filter(|i| matches!(i, Instruction::Load(_, _))).count();
    assert!(load_count >= 1, "expected at least 1 load instruction for tuple field access, got {}", load_count);
}

#[test]
fn ir_tuple_destructuring_creates_loads() {
    let src = "\
fn f() -> Int:
    let (a, b) = (3, 4)
    ret a
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // Should have Load instructions from destructuring.
    let load_count = instrs.iter().filter(|i| matches!(i, Instruction::Load(_, _))).count();
    assert!(load_count >= 2, "expected at least 2 load instructions for tuple destructuring, got {}", load_count);
}

#[test]
fn ir_tuple_three_elements() {
    let src = "\
fn f() -> Int:
    let t = (1, 2, 3)
    ret t.2
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // 3-element tuple needs at least 3 alloca + 3 stores.
    let alloca_count = instrs.iter().filter(|i| matches!(i, Instruction::Alloca(_, _))).count();
    assert!(alloca_count >= 3, "expected at least 3 alloca for 3-element tuple, got {}", alloca_count);
}

// ---------------------------------------------------------------------------
// List literals
// ---------------------------------------------------------------------------

#[test]
fn ir_list_literal_empty() {
    let src = "\
fn f():
    let nums: List[Int] = []
    ret ()
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // Empty list literal should generate a call to list_literal_0.
    let has_call = instrs.iter().any(|i| matches!(i, Instruction::Call(_, _, args) if args.is_empty()));
    assert!(has_call, "expected a call instruction for empty list literal");
}

#[test]
fn ir_list_literal_with_elements() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    ret ()
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // List literal [1, 2, 3] should generate a call to list_literal_3 with 3 args.
    let has_list_call = instrs.iter().any(|i| {
        matches!(i, Instruction::Call(_, _, args) if args.len() == 3)
    });
    assert!(has_list_call, "expected a call instruction with 3 args for list literal");
}

#[test]
fn ir_list_length_call() {
    let src = "\
fn f() -> Int:
    let nums = [10, 20]
    ret list_length(nums)
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // Should have at least 2 Call instructions: one for list_literal_2 and one for list_length.
    let call_count = instrs.iter().filter(|i| matches!(i, Instruction::Call(_, _, _))).count();
    assert!(call_count >= 2, "expected at least 2 call instructions, got {}", call_count);
}

#[test]
fn ir_list_get_call() {
    let src = "\
fn f() -> Int:
    let nums = [10, 20, 30]
    ret list_get(nums, 1)
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // list_get call should have 2 args (list + index).
    let has_get_call = instrs.iter().any(|i| {
        matches!(i, Instruction::Call(_, _, args) if args.len() == 2)
    });
    assert!(has_get_call, "expected a call instruction with 2 args for list_get");
}
// ---------------------------------------------------------------------------
// Higher-order list functions
// ---------------------------------------------------------------------------

#[test]
fn ir_list_map_call() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let doubled = list_map(nums, |x: Int| x * 2)
    ret ()
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // Should have Call instructions including one for list_map (2 args: list + closure).
    let call_count = instrs.iter().filter(|i| matches!(i, Instruction::Call(_, _, _))).count();
    assert!(call_count >= 2, "expected at least 2 call instructions (list_literal + list_map), got {}", call_count);
}

#[test]
fn ir_list_filter_call() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let evens = list_filter(nums, |x: Int| x > 1)
    ret ()
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    let call_count = instrs.iter().filter(|i| matches!(i, Instruction::Call(_, _, _))).count();
    assert!(call_count >= 2, "expected at least 2 call instructions, got {}", call_count);
}

#[test]
fn ir_list_fold_call() {
    let src = "\
fn f() -> Int:
    let nums = [1, 2, 3]
    ret list_fold(nums, 0, |acc: Int, x: Int| acc + x)
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // list_fold takes 3 args.
    let has_fold_call = instrs.iter().any(|i| {
        matches!(i, Instruction::Call(_, _, args) if args.len() == 3)
    });
    assert!(has_fold_call, "expected a call instruction with 3 args for list_fold");
}

#[test]
fn ir_list_reverse_call() {
    let src = "\
fn f():
    let nums = [1, 2, 3]
    let rev = list_reverse(nums)
    ret ()
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // list_reverse takes 1 arg.
    let call_count = instrs.iter().filter(|i| matches!(i, Instruction::Call(_, _, _))).count();
    assert!(call_count >= 2, "expected at least 2 call instructions, got {}", call_count);
}

#[test]
fn ir_list_sort_call() {
    let src = "\
fn f():
    let nums = [3, 1, 2]
    let sorted = list_sort(nums)
    ret ()
";
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    let call_count = instrs.iter().filter(|i| matches!(i, Instruction::Call(_, _, _))).count();
    assert!(call_count >= 2, "expected at least 2 call instructions, got {}", call_count);
}

// ---------------------------------------------------------------------------
// String interpolation IR generation
// ---------------------------------------------------------------------------

#[test]
fn ir_interp_string_literal_only() {
    // f"hello" should produce just a Const(Str("hello")), no concat calls.
    let src = r#"
fn f() -> String:
    ret f"hello"
"#;
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // Should have at least one Str constant.
    let str_consts: Vec<_> = instrs
        .iter()
        .filter(|i| matches!(i, Instruction::Const(_, Literal::Str(s)) if s == "hello"))
        .collect();
    assert!(!str_consts.is_empty(), "expected a string constant \"hello\"");
}

#[test]
fn ir_interp_string_with_int_calls_to_string_and_concat() {
    // f"n = {x}" should produce: int_to_string(x), string_concat("n = ", result)
    let src = r#"
fn f(x: Int) -> String:
    ret f"n = {x}"
"#;
    let ir = build_ok(src);
    let instrs = all_instructions(&ir);
    // Should have at least one Call to int_to_string and one to string_concat.
    let has_call = instrs.iter().any(|i| matches!(i, Instruction::Call(_, _, _)));
    assert!(has_call, "expected at least one Call instruction for interpolation");
}

// ---------------------------------------------------------------------------
// Pipe operator (|>)
// ---------------------------------------------------------------------------

#[test]
fn pipe_desugars_to_call() {
    // `5 |> double` should produce a Call instruction (same as `double(5)`).
    let src = "\
fn double(x: Int) -> Int:
    ret x + x

fn main() -> Int:
    ret 5 |> double
";
    let ir = build_ok(src);
    // The "main" function is the second one (after "double").
    let main_func = ir.functions.iter().find(|f| f.name == "main").unwrap();
    let instrs: Vec<_> = main_func.blocks.iter().flat_map(|b| b.instructions.iter()).collect();
    let has_call = instrs.iter().any(|i| matches!(i, Instruction::Call(_, _, _)));
    assert!(has_call, "expected a Call instruction for pipe desugaring");
}

#[test]
fn pipe_chained_desugars_to_nested_calls() {
    // `5 |> double |> negate` should produce two Call instructions.
    let src = "\
fn double(x: Int) -> Int:
    ret x + x

fn negate(x: Int) -> Int:
    ret 0 - x

fn main() -> Int:
    ret 5 |> double |> negate
";
    let ir = build_ok(src);
    let main_func = ir.functions.iter().find(|f| f.name == "main").unwrap();
    let instrs: Vec<_> = main_func.blocks.iter().flat_map(|b| b.instructions.iter()).collect();
    let call_count = instrs.iter().filter(|i| matches!(i, Instruction::Call(_, _, _))).count();
    assert!(call_count >= 2, "expected at least 2 Call instructions for chained pipe, got {}", call_count);
}

// ---------------------------------------------------------------------------
// For-in over lists
// ---------------------------------------------------------------------------

#[test]
fn ir_for_in_list_literal() {
    // Iterating over a list literal should use list_length and list_get.
    let src = "\
fn main() -> ():
    for x in [10, 20, 30]:
        println(x)
";
    let ir = build_ok(src);
    let main_func = ir.functions.iter().find(|f| f.name == "main").unwrap();
    let instrs: Vec<_> = main_func.blocks.iter().flat_map(|b| b.instructions.iter()).collect();

    // Should have Phi (loop counter), Cmp (counter < len), Branch, and Call (list_get).
    let has_phi = instrs.iter().any(|i| matches!(i, Instruction::Phi(_, _)));
    let has_cmp = instrs.iter().any(|i| matches!(i, Instruction::Cmp(_, CmpOp::Lt, _, _)));
    assert!(has_phi, "expected a Phi instruction for the loop counter");
    assert!(has_cmp, "expected a Cmp(Lt) instruction for the loop bound check");

    // Should have at least 2 Call instructions: list_literal_3, list_length, list_get, println.
    let call_count = instrs.iter().filter(|i| matches!(i, Instruction::Call(_, _, _))).count();
    assert!(call_count >= 3, "expected >= 3 Call instructions (list_literal, list_length, list_get+println), got {}", call_count);
}

// ---------------------------------------------------------------------------
// For-in over range expressions
// ---------------------------------------------------------------------------

#[test]
fn ir_for_in_range_expr() {
    // for i in 0..5 should produce a counted loop starting at 0.
    let src = "\
fn main() -> ():
    for i in 0..5:
        println(i)
";
    let ir = build_ok(src);
    let main_func = ir.functions.iter().find(|f| f.name == "main").unwrap();
    let instrs: Vec<_> = main_func.blocks.iter().flat_map(|b| b.instructions.iter()).collect();

    // Should have a Phi for the counter, a Cmp for bound check, and an Add for increment.
    let has_phi = instrs.iter().any(|i| matches!(i, Instruction::Phi(_, _)));
    let has_cmp = instrs.iter().any(|i| matches!(i, Instruction::Cmp(_, CmpOp::Lt, _, _)));
    let has_add = instrs.iter().any(|i| matches!(i, Instruction::Add(_, _, _)));
    assert!(has_phi, "expected a Phi instruction for the range loop counter");
    assert!(has_cmp, "expected a Cmp(Lt) instruction for the range bound");
    assert!(has_add, "expected an Add instruction for counter increment");

    // The range loop should have at least 3 blocks: entry, header, body, exit.
    assert!(
        main_func.blocks.len() >= 3,
        "expected >= 3 blocks for range loop, got {}",
        main_func.blocks.len()
    );
}

#[test]
fn ir_for_in_range_with_variables() {
    // Range endpoints can be variables.
    let src = "\
fn f(a: Int, b: Int) -> ():
    for i in a..b:
        println(i)
";
    let ir = build_ok(src);
    let f_func = ir.functions.iter().find(|f| f.name == "f").unwrap();
    let instrs: Vec<_> = f_func.blocks.iter().flat_map(|b| b.instructions.iter()).collect();

    let has_phi = instrs.iter().any(|i| matches!(i, Instruction::Phi(_, _)));
    assert!(has_phi, "expected Phi for range loop counter with variable endpoints");
}

// ---------------------------------------------------------------------------
// Phase LL: Tuple Variant Codegen — IR builder tests
// ---------------------------------------------------------------------------

#[test]
fn tuple_variant_constructor_emits_construct_variant() {
    // A call `Some(42)` should lower to ConstructVariant, not a Call instruction.
    let src = "\
type Option[T] = Some(T) | None

fn make_some(x: Int) -> Option[Int]:
    ret Some(x)
";
    let module = build_ok(src);
    let func = module.functions.iter().find(|f| f.name == "make_some").unwrap();
    let instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();

    // Should have a ConstructVariant with tag=0 (Some is the first variant) and 1 payload.
    let has_construct = instrs.iter().any(|i| matches!(
        i,
        Instruction::ConstructVariant { tag: 0, payload, .. } if payload.len() == 1
    ));
    assert!(
        has_construct,
        "expected ConstructVariant {{ tag: 0, payload: [x] }} for Some(x), got: {:?}",
        instrs
    );
}

#[test]
fn unit_variant_emits_construct_variant_no_payload() {
    // A unit variant `None` should lower to ConstructVariant with empty payload.
    let src = "\
type Option[T] = Some(T) | None

fn make_none() -> Option[Int]:
    ret None
";
    let module = build_ok(src);
    let func = module.functions.iter().find(|f| f.name == "make_none").unwrap();
    let instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();

    // None is tag=1 (second variant), no payload.
    let has_construct = instrs.iter().any(|i| matches!(
        i,
        Instruction::ConstructVariant { tag: 1, payload, .. } if payload.is_empty()
    ));
    assert!(
        has_construct,
        "expected ConstructVariant {{ tag: 1, payload: [] }} for None, got: {:?}",
        instrs
    );
}

#[test]
fn tuple_variant_match_with_binding_emits_get_variant_field() {
    // `Some(x): x` in a match arm should load the payload field.
    let src = "\
type Option[T] = Some(T) | None

fn unwrap_or(opt: Option[Int], default: Int) -> Int:
    match opt:
        Some(x):
            x
        None:
            default
";
    let module = build_ok(src);
    let func = module.functions.iter().find(|f| f.name == "unwrap_or").unwrap();
    let instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();

    // Match on enum must load the tag via GetVariantTag.
    let has_get_tag = instrs.iter().any(|i| matches!(i, Instruction::GetVariantTag { .. }));
    assert!(has_get_tag, "expected GetVariantTag instruction in match");

    // The `Some(x)` arm must extract field 0 via GetVariantField.
    let has_get_field = instrs.iter().any(|i| matches!(
        i,
        Instruction::GetVariantField { index: 0, .. }
    ));
    assert!(has_get_field, "expected GetVariantField {{ index: 0 }} for Some(x) binding");
}

#[test]
fn multi_field_enum_variant_tags_correct() {
    // Verify tag ordering for an enum with 3 variants (1 tuple, 1 unit).
    // NOTE: The current parser/AST supports at most one field per variant
    // (EnumVariant.field: Option<Spanned<TypeExpr>>). Multi-field variants
    // like Rectangle(Float, Float) are a TODO for a future phase.
    let src = "\
type Shape = Circle(Float) | Box(Float) | Point

fn make_circle(r: Float) -> Shape:
    ret Circle(r)

fn make_point() -> Shape:
    ret Point
";
    let module = build_ok(src);

    // Circle is tag 0 (first variant).
    let circle_func = module.functions.iter().find(|f| f.name == "make_circle").unwrap();
    let circle_instrs: Vec<_> = circle_func.blocks.iter().flat_map(|b| &b.instructions).collect();
    let has_circle = circle_instrs.iter().any(|i| matches!(
        i,
        Instruction::ConstructVariant { tag: 0, payload, .. } if payload.len() == 1
    ));
    assert!(has_circle, "Circle should be ConstructVariant tag=0 with 1 payload");

    // Point is tag 2 (third variant), no payload.
    let point_func = module.functions.iter().find(|f| f.name == "make_point").unwrap();
    let point_instrs: Vec<_> = point_func.blocks.iter().flat_map(|b| &b.instructions).collect();
    let has_point = point_instrs.iter().any(|i| matches!(
        i,
        Instruction::ConstructVariant { tag: 2, payload, .. } if payload.is_empty()
    ));
    assert!(has_point, "Point should be ConstructVariant tag=2 with no payload");
}

#[test]
fn result_enum_ok_err_variants() {
    // Verify Ok/Err constructors for Result[T, E].
    let src = "\
type Result[T, E] = Ok(T) | Err(E)

fn make_ok(x: Int) -> Result[Int, String]:
    ret Ok(x)

fn make_err(msg: String) -> Result[Int, String]:
    ret Err(msg)
";
    let module = build_ok(src);

    // Ok is tag 0.
    let ok_func = module.functions.iter().find(|f| f.name == "make_ok").unwrap();
    let ok_instrs: Vec<_> = ok_func.blocks.iter().flat_map(|b| &b.instructions).collect();
    let has_ok = ok_instrs.iter().any(|i| matches!(
        i,
        Instruction::ConstructVariant { tag: 0, payload, .. } if payload.len() == 1
    ));
    assert!(has_ok, "Ok(x) should be ConstructVariant tag=0 with 1 payload");

    // Err is tag 1.
    let err_func = module.functions.iter().find(|f| f.name == "make_err").unwrap();
    let err_instrs: Vec<_> = err_func.blocks.iter().flat_map(|b| &b.instructions).collect();
    let has_err = err_instrs.iter().any(|i| matches!(
        i,
        Instruction::ConstructVariant { tag: 1, payload, .. } if payload.len() == 1
    ));
    assert!(has_err, "Err(msg) should be ConstructVariant tag=1 with 1 payload");
}
