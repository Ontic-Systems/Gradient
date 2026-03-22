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

    // get_red should have a Const with tag 0 (Red is the first variant)
    let func = module.functions.iter().find(|f| f.name == "get_red").unwrap();
    let instrs: Vec<_> = func.blocks.iter().flat_map(|b| &b.instructions).collect();
    assert!(
        instrs.iter().any(|i| matches!(i, Instruction::Const(_, Literal::Int(0)))),
        "expected Const(_, Int(0)) for Red variant, got: {:?}",
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
