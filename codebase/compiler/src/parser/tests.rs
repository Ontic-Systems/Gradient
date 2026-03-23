//! Parser tests for the Gradient language.
//!
//! Each test constructs a token stream by hand and feeds it to the parser,
//! then asserts properties of the resulting AST. This avoids a dependency on
//! the lexer (which is being developed in parallel) while giving us thorough
//! coverage of every grammar rule.

use crate::ast::expr::{BinOp, ExprKind, Pattern, StringInterpPart, UnaryOp};
use crate::ast::item::{ContractKind, ItemKind, TypeParam};
use crate::ast::module::Module;
use crate::ast::stmt::StmtKind;
use crate::ast::types::TypeExpr;
use crate::ast::span::{Position, Span};
use crate::lexer::token::{InterpolationPart, Token, TokenKind};
use crate::parser::{parse, ParseError};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a token at a synthetic span. We use a simple scheme: each token
/// gets span `(line 1, col n) .. (line 1, col n+1)` where `n` is the
/// position in the token list. This makes the tests independent of actual
/// source layout while still exercising span merging.
fn tok(kind: TokenKind) -> Token {
    Token::new(kind, Span::new(0, Position::new(1, 1, 0), Position::new(1, 2, 1)))
}

/// Build a token with explicit span info (line, col).
fn tok_at(kind: TokenKind, line: u32, col: u32) -> Token {
    Token::new(
        kind,
        Span::new(
            0,
            Position::new(line, col, 0),
            Position::new(line, col + 1, 1),
        ),
    )
}

/// Parse a token stream and assert no errors.
fn parse_ok(tokens: Vec<Token>) -> Module {
    let (module, errors) = parse(tokens, 0);
    assert!(
        errors.is_empty(),
        "expected no parse errors, got: {:?}",
        errors
    );
    module
}

/// Parse a token stream and return both the module and errors.
fn parse_with_errors(tokens: Vec<Token>) -> (Module, Vec<ParseError>) {
    parse(tokens, 0)
}

// ---------------------------------------------------------------------------
// Module declaration
// ---------------------------------------------------------------------------

#[test]
fn parse_module_decl() {
    // mod std.io
    let tokens = vec![
        tok(TokenKind::Mod),
        tok(TokenKind::Ident("std".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("io".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let md = module.module_decl.expect("expected module_decl");
    assert_eq!(md.path, vec!["std", "io"]);
}

#[test]
fn parse_simple_module_decl() {
    // mod mymod
    let tokens = vec![
        tok(TokenKind::Mod),
        tok(TokenKind::Ident("mymod".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let md = module.module_decl.expect("expected module_decl");
    assert_eq!(md.path, vec!["mymod"]);
}

// ---------------------------------------------------------------------------
// Use declarations
// ---------------------------------------------------------------------------

#[test]
fn parse_use_decl_simple() {
    // use std.io
    let tokens = vec![
        tok(TokenKind::Use),
        tok(TokenKind::Ident("std".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("io".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.uses.len(), 1);
    let u = &module.uses[0];
    assert_eq!(u.path, vec!["std", "io"]);
    assert!(u.specific_imports.is_none());
}

#[test]
fn parse_use_decl_with_imports() {
    // use std.io.{read, write}
    let tokens = vec![
        tok(TokenKind::Use),
        tok(TokenKind::Ident("std".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("io".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::LBrace),
        tok(TokenKind::Ident("read".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::Ident("write".into())),
        tok(TokenKind::RBrace),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.uses.len(), 1);
    let u = &module.uses[0];
    assert_eq!(u.path, vec!["std", "io"]);
    assert_eq!(
        u.specific_imports.as_deref(),
        Some(&["read".to_string(), "write".to_string()][..])
    );
}

#[test]
fn parse_use_decl_trailing_comma() {
    // use std.io.{read,}
    let tokens = vec![
        tok(TokenKind::Use),
        tok(TokenKind::Ident("std".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("io".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::LBrace),
        tok(TokenKind::Ident("read".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::RBrace),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let u = &module.uses[0];
    assert_eq!(
        u.specific_imports.as_deref(),
        Some(&["read".to_string()][..])
    );
}

// ---------------------------------------------------------------------------
// Simple function definition
// ---------------------------------------------------------------------------

#[test]
fn parse_simple_fn_def() {
    // fn main():
    //     ret 0
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.items.len(), 1);

    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.name, "main");
            assert!(fd.params.is_empty());
            assert!(fd.return_type.is_none());
            assert!(fd.effects.is_none());
            assert_eq!(fd.body.node.len(), 1);
            match &fd.body.node[0].node {
                StmtKind::Ret(expr) => match &expr.node {
                    ExprKind::IntLit(0) => {}
                    other => panic!("expected IntLit(0), got {:?}", other),
                },
                other => panic!("expected Ret, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_fn_with_params_and_return() {
    // fn add(a: i32, b: i32) -> i32:
    //     ret a + b
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("add".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("i32".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("i32".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Arrow),
        tok(TokenKind::Ident("i32".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::Plus),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.name, "add");
            assert_eq!(fd.params.len(), 2);
            assert_eq!(fd.params[0].name, "a");
            assert_eq!(fd.params[1].name, "b");
            assert!(matches!(
                &fd.return_type.as_ref().unwrap().node,
                TypeExpr::Named(n) if n == "i32"
            ));
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_fn_with_effects() {
    // fn greet() -> !{IO} ():
    //     ret ()
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("greet".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Arrow),
        tok(TokenKind::Bang),
        tok(TokenKind::LBrace),
        tok(TokenKind::Ident("IO".into())),
        tok(TokenKind::RBrace),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.name, "greet");
            let eff = fd.effects.as_ref().expect("expected effect set");
            assert_eq!(eff.effects, vec!["IO"]);
            assert!(matches!(
                &fd.return_type.as_ref().unwrap().node,
                TypeExpr::Unit
            ));
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Extern function declarations
// ---------------------------------------------------------------------------

#[test]
fn parse_extern_fn_decl() {
    // fn puts(s: CStr) -> i32
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("puts".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("s".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("CStr".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Arrow),
        tok(TokenKind::Ident("i32".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::ExternFn(decl) => {
            assert_eq!(decl.name, "puts");
            assert_eq!(decl.params.len(), 1);
            assert_eq!(decl.params[0].name, "s");
            assert!(matches!(
                &decl.return_type.as_ref().unwrap().node,
                TypeExpr::Named(n) if n == "i32"
            ));
        }
        other => panic!("expected ExternFn, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Let bindings
// ---------------------------------------------------------------------------

#[test]
fn parse_let_with_type() {
    // let x: i32 = 42
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("i32".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(42)),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::Let {
            name,
            type_ann,
            value,
            ..
        } => {
            assert_eq!(name, "x");
            assert!(matches!(
                &type_ann.as_ref().unwrap().node,
                TypeExpr::Named(n) if n == "i32"
            ));
            assert!(matches!(&value.node, ExprKind::IntLit(42)));
        }
        other => panic!("expected Let, got {:?}", other),
    }
}

#[test]
fn parse_let_without_type() {
    // let y = "hello"
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::Ident("y".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::StringLit("hello".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::Let {
            name,
            type_ann,
            value,
            ..
        } => {
            assert_eq!(name, "y");
            assert!(type_ann.is_none());
            assert!(matches!(&value.node, ExprKind::StringLit(s) if s == "hello"));
        }
        other => panic!("expected Let, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// If/else expressions
// ---------------------------------------------------------------------------

#[test]
fn parse_if_else() {
    // fn f():
    //     if true:
    //         ret 1
    //     else:
    //         ret 2
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // if true:
        tok(TokenKind::If),
        tok(TokenKind::True),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // else:
        tok(TokenKind::Else),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    assert_eq!(fd.body.node.len(), 1);
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::If {
                condition,
                then_block,
                else_ifs,
                else_block,
            } => {
                assert!(matches!(&condition.node, ExprKind::BoolLit(true)));
                assert_eq!(then_block.node.len(), 1);
                assert!(else_ifs.is_empty());
                assert!(else_block.is_some());
            }
            other => panic!("expected If, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_if_else_if_else() {
    // fn f(x: i32):
    //     if x == 1:
    //         ret 10
    //     else if x == 2:
    //         ret 20
    //     else:
    //         ret 30
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("i32".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // if x == 1:
        tok(TokenKind::If),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Eq),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(10)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // else if x == 2:
        tok(TokenKind::Else),
        tok(TokenKind::If),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Eq),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(20)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // else:
        tok(TokenKind::Else),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(30)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::If {
                else_ifs,
                else_block,
                ..
            } => {
                assert_eq!(else_ifs.len(), 1);
                assert!(else_block.is_some());
            }
            other => panic!("expected If, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// For loops
// ---------------------------------------------------------------------------

#[test]
fn parse_for_loop() {
    // fn f():
    //     for x in items:
    //         print(x)
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::For),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::In),
        tok(TokenKind::Ident("items".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("print".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::For { var, iter, body } => {
                assert_eq!(var, "x");
                assert!(matches!(&iter.node, ExprKind::Ident(n) if n == "items"));
                assert_eq!(body.node.len(), 1);
            }
            other => panic!("expected For, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Expression precedence
// ---------------------------------------------------------------------------

#[test]
fn parse_arithmetic_precedence() {
    // Parse: 1 + 2 * 3
    // Expected AST: Add(1, Mul(2, 3))
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Plus),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::Star),
        tok(TokenKind::IntLit(3)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::BinaryOp { op, left, right } => {
                assert_eq!(*op, BinOp::Add);
                assert!(matches!(&left.node, ExprKind::IntLit(1)));
                match &right.node {
                    ExprKind::BinaryOp { op, left, right } => {
                        assert_eq!(*op, BinOp::Mul);
                        assert!(matches!(&left.node, ExprKind::IntLit(2)));
                        assert!(matches!(&right.node, ExprKind::IntLit(3)));
                    }
                    other => panic!("expected Mul, got {:?}", other),
                }
            }
            other => panic!("expected Add, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_left_associative_addition() {
    // Parse: 1 + 2 + 3
    // Expected AST: Add(Add(1, 2), 3)
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Plus),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::Plus),
        tok(TokenKind::IntLit(3)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::BinaryOp { op, left, right } => {
                assert_eq!(*op, BinOp::Add);
                // left should be Add(1, 2)
                match &left.node {
                    ExprKind::BinaryOp { op, left, right } => {
                        assert_eq!(*op, BinOp::Add);
                        assert!(matches!(&left.node, ExprKind::IntLit(1)));
                        assert!(matches!(&right.node, ExprKind::IntLit(2)));
                    }
                    other => panic!("expected inner Add, got {:?}", other),
                }
                assert!(matches!(&right.node, ExprKind::IntLit(3)));
            }
            other => panic!("expected outer Add, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_unary_negation() {
    // -x
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Minus),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::UnaryOp { op, operand } => {
                assert_eq!(*op, UnaryOp::Neg);
                assert!(matches!(&operand.node, ExprKind::Ident(n) if n == "x"));
            }
            other => panic!("expected UnaryOp::Neg, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_not_expression() {
    // not flag
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Not),
        tok(TokenKind::Ident("flag".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::UnaryOp { op, operand } => {
                assert_eq!(*op, UnaryOp::Not);
                assert!(matches!(&operand.node, ExprKind::Ident(n) if n == "flag"));
            }
            other => panic!("expected UnaryOp::Not, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_logical_and_or() {
    // a and b or c
    // Expected: Or(And(a, b), c) because `and` binds tighter than `or`
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::And),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::Or),
        tok(TokenKind::Ident("c".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::BinaryOp { op, left, right } => {
                assert_eq!(*op, BinOp::Or);
                match &left.node {
                    ExprKind::BinaryOp { op, .. } => assert_eq!(*op, BinOp::And),
                    other => panic!("expected And, got {:?}", other),
                }
                assert!(matches!(&right.node, ExprKind::Ident(n) if n == "c"));
            }
            other => panic!("expected Or, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Function calls
// ---------------------------------------------------------------------------

#[test]
fn parse_function_call() {
    // print("hello", 42)
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("print".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::StringLit("hello".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::IntLit(42)),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::Call { func, args } => {
                assert!(matches!(&func.node, ExprKind::Ident(n) if n == "print"));
                assert_eq!(args.len(), 2);
                assert!(matches!(&args[0].node, ExprKind::StringLit(s) if s == "hello"));
                assert!(matches!(&args[1].node, ExprKind::IntLit(42)));
            }
            other => panic!("expected Call, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_call_no_args() {
    // foo()
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("foo".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::Call { func, args } => {
                assert!(matches!(&func.node, ExprKind::Ident(n) if n == "foo"));
                assert!(args.is_empty());
            }
            other => panic!("expected Call, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_call_trailing_comma() {
    // foo(1,)
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("foo".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Comma),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::Call { args, .. } => {
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected Call, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Field access
// ---------------------------------------------------------------------------

#[test]
fn parse_field_access() {
    // obj.field
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("obj".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("field".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::FieldAccess { object, field } => {
                assert!(matches!(&object.node, ExprKind::Ident(n) if n == "obj"));
                assert_eq!(field, "field");
            }
            other => panic!("expected FieldAccess, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_chained_field_access_and_call() {
    // a.b.c()
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("c".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    // Should be Call(FieldAccess(FieldAccess(a, "b"), "c"), [])
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::Call { func, args } => {
                assert!(args.is_empty());
                match &func.node {
                    ExprKind::FieldAccess { object, field } => {
                        assert_eq!(field, "c");
                        match &object.node {
                            ExprKind::FieldAccess { object, field } => {
                                assert_eq!(field, "b");
                                assert!(
                                    matches!(&object.node, ExprKind::Ident(n) if n == "a")
                                );
                            }
                            other => panic!("expected inner FieldAccess, got {:?}", other),
                        }
                    }
                    other => panic!("expected outer FieldAccess, got {:?}", other),
                }
            }
            other => panic!("expected Call, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Typed holes
// ---------------------------------------------------------------------------

#[test]
fn parse_typed_hole_bare() {
    // ?
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Question),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => {
            assert!(matches!(&expr.node, ExprKind::TypedHole(None)));
        }
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_typed_hole_with_label() {
    // ?todo
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Question),
        tok(TokenKind::Ident("todo".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => {
            assert!(matches!(
                &expr.node,
                ExprKind::TypedHole(Some(name)) if name == "todo"
            ));
        }
        other => panic!("expected Expr, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Type declarations
// ---------------------------------------------------------------------------

#[test]
fn parse_type_decl() {
    // type Meters = f64
    let tokens = vec![
        tok(TokenKind::Type),
        tok(TokenKind::Ident("Meters".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::Ident("f64".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::TypeDecl { name, type_expr, .. } => {
            assert_eq!(name, "Meters");
            assert!(matches!(&type_expr.node, TypeExpr::Named(n) if n == "f64"));
        }
        other => panic!("expected TypeDecl, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------

#[test]
fn parse_annotation_on_fn() {
    // @inline
    // fn f():
    //     ret ()
    let tokens = vec![
        tok(TokenKind::At),
        tok(TokenKind::Ident("inline".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.annotations.len(), 1);
            assert_eq!(fd.annotations[0].name, "inline");
            assert!(fd.annotations[0].args.is_empty());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_annotation_with_args() {
    // @test("name")
    // fn f():
    //     ret ()
    let tokens = vec![
        tok(TokenKind::At),
        tok(TokenKind::Ident("test".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::StringLit("name".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.annotations.len(), 1);
            assert_eq!(fd.annotations[0].name, "test");
            assert_eq!(fd.annotations[0].args.len(), 1);
            assert!(matches!(
                &fd.annotations[0].args[0].node,
                ExprKind::StringLit(s) if s == "name"
            ));
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Parenthesized and unit expressions
// ---------------------------------------------------------------------------

#[test]
fn parse_paren_expr() {
    // (1 + 2)
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::LParen),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Plus),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::Paren(inner) => {
                assert!(matches!(&inner.node, ExprKind::BinaryOp { op: BinOp::Add, .. }));
            }
            other => panic!("expected Paren, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_unit_literal() {
    // ()
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => {
            assert!(matches!(&expr.node, ExprKind::UnitLit));
        }
        other => panic!("expected Expr(UnitLit), got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Comparison non-associativity
// ---------------------------------------------------------------------------

#[test]
fn parse_comparison_non_associative() {
    // a < b < c should produce an error
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::Lt),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::Lt),
        tok(TokenKind::Ident("c".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let (_module, errors) = parse_with_errors(tokens);
    assert!(
        !errors.is_empty(),
        "expected a parse error for chained comparisons"
    );
    assert!(errors[0]
        .message
        .contains("non-associative"));
}

// ---------------------------------------------------------------------------
// Complete hello.gr program
// ---------------------------------------------------------------------------

#[test]
fn parse_hello_gr_program() {
    // mod hello
    //
    // use std.io.{println}
    //
    // fn main() -> !{IO} ():
    //     let msg: String = "Hello, Gradient!"
    //     println(msg)
    let tokens = vec![
        // mod hello
        tok(TokenKind::Mod),
        tok(TokenKind::Ident("hello".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Newline),
        // use std.io.{println}
        tok(TokenKind::Use),
        tok(TokenKind::Ident("std".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("io".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::LBrace),
        tok(TokenKind::Ident("println".into())),
        tok(TokenKind::RBrace),
        tok(TokenKind::Newline),
        tok(TokenKind::Newline),
        // fn main() -> !{IO} ():
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Arrow),
        tok(TokenKind::Bang),
        tok(TokenKind::LBrace),
        tok(TokenKind::Ident("IO".into())),
        tok(TokenKind::RBrace),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        // block
        tok(TokenKind::Indent),
        //     let msg: String = "Hello, Gradient!"
        tok(TokenKind::Let),
        tok(TokenKind::Ident("msg".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("String".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::StringLit("Hello, Gradient!".into())),
        tok(TokenKind::Newline),
        //     println(msg)
        tok(TokenKind::Ident("println".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("msg".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);

    // Module declaration.
    let md = module.module_decl.as_ref().expect("expected mod decl");
    assert_eq!(md.path, vec!["hello"]);

    // Use declaration.
    assert_eq!(module.uses.len(), 1);
    assert_eq!(module.uses[0].path, vec!["std", "io"]);
    assert_eq!(
        module.uses[0].specific_imports.as_deref(),
        Some(&["println".to_string()][..])
    );

    // Function definition.
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.name, "main");
            assert!(fd.params.is_empty());
            assert!(fd.effects.is_some());
            assert_eq!(fd.effects.as_ref().unwrap().effects, vec!["IO"]);
            assert!(matches!(
                &fd.return_type.as_ref().unwrap().node,
                TypeExpr::Unit
            ));
            assert_eq!(fd.body.node.len(), 2);

            // let msg: String = "Hello, Gradient!"
            match &fd.body.node[0].node {
                StmtKind::Let {
                    name,
                    type_ann,
                    value,
                    ..
                } => {
                    assert_eq!(name, "msg");
                    assert!(matches!(
                        &type_ann.as_ref().unwrap().node,
                        TypeExpr::Named(n) if n == "String"
                    ));
                    assert!(matches!(
                        &value.node,
                        ExprKind::StringLit(s) if s == "Hello, Gradient!"
                    ));
                }
                other => panic!("expected Let, got {:?}", other),
            }

            // println(msg)
            match &fd.body.node[1].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Call { func, args } => {
                        assert!(
                            matches!(&func.node, ExprKind::Ident(n) if n == "println")
                        );
                        assert_eq!(args.len(), 1);
                        assert!(
                            matches!(&args[0].node, ExprKind::Ident(n) if n == "msg")
                        );
                    }
                    other => panic!("expected Call, got {:?}", other),
                },
                other => panic!("expected Expr, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Error recovery
// ---------------------------------------------------------------------------

#[test]
fn error_recovery_multiple_errors() {
    // Parse a file with two functions where the first has an error.
    // The parser should recover and still parse the second function.
    //
    // fn bad(:            <- error: expected parameter name
    //     ret 0
    // fn good():
    //     ret 1
    let tokens = vec![
        // fn bad(
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("bad".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Colon), // error — expected parameter name, got ':'
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // fn good():
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("good".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let (module, errors) = parse_with_errors(tokens);

    // We should have at least one error from the malformed first function.
    assert!(
        !errors.is_empty(),
        "expected parse errors for malformed function"
    );

    // Both functions should appear in the items list (error recovery).
    assert!(
        module.items.len() >= 2,
        "expected at least 2 items after recovery, got {}",
        module.items.len()
    );

    // The second function should be intact.
    let has_good = module.items.iter().any(|item| match &item.node {
        ItemKind::FnDef(fd) => fd.name == "good",
        _ => false,
    });
    assert!(has_good, "expected 'good' function after error recovery");
}

#[test]
fn error_recovery_reports_found_token() {
    // let = 42  <- missing variable name
    let tokens = vec![
        tok(TokenKind::Let),
        tok_at(TokenKind::Assign, 1, 5),
        tok(TokenKind::IntLit(42)),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let (_module, errors) = parse_with_errors(tokens);
    assert!(!errors.is_empty());
    // The first error should mention what was expected and what was found.
    assert!(
        errors[0].found.contains("="),
        "expected error to report '=' as found token, got: {}",
        errors[0].found
    );
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

#[test]
fn parse_float_literal() {
    // 3.14
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::FloatLit(3.14)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::FloatLit(v) => assert!((v - 3.14).abs() < f64::EPSILON),
            other => panic!("expected FloatLit, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_bool_literals() {
    // true, false
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::True),
        tok(TokenKind::Newline),
        tok(TokenKind::False),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    assert!(matches!(&fd.body.node[0].node, StmtKind::Expr(e) if matches!(&e.node, ExprKind::BoolLit(true))));
    assert!(matches!(&fd.body.node[1].node, StmtKind::Expr(e) if matches!(&e.node, ExprKind::BoolLit(false))));
}

// ---------------------------------------------------------------------------
// Let statement inside a block
// ---------------------------------------------------------------------------

#[test]
fn parse_let_in_block() {
    // fn f():
    //     let x = 10
    //     let y: i32 = 20
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Let),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(10)),
        tok(TokenKind::Newline),
        tok(TokenKind::Let),
        tok(TokenKind::Ident("y".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("i32".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(20)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    assert_eq!(fd.body.node.len(), 2);
    match &fd.body.node[0].node {
        StmtKind::Let { name, type_ann, .. } => {
            assert_eq!(name, "x");
            assert!(type_ann.is_none());
        }
        other => panic!("expected Let, got {:?}", other),
    }
    match &fd.body.node[1].node {
        StmtKind::Let { name, type_ann, .. } => {
            assert_eq!(name, "y");
            assert!(type_ann.is_some());
        }
        other => panic!("expected Let, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Multiple top-level items
// ---------------------------------------------------------------------------

#[test]
fn parse_multiple_items() {
    // type Meters = f64
    // let PI = 3.14
    // fn area(r: Meters) -> Meters:
    //     ret PI * r * r
    let tokens = vec![
        // type Meters = f64
        tok(TokenKind::Type),
        tok(TokenKind::Ident("Meters".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::Ident("f64".into())),
        tok(TokenKind::Newline),
        // let PI = 3.14
        tok(TokenKind::Let),
        tok(TokenKind::Ident("PI".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::FloatLit(3.14)),
        tok(TokenKind::Newline),
        // fn area(r: Meters) -> Meters:
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("area".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("r".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Meters".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Arrow),
        tok(TokenKind::Ident("Meters".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::Ident("PI".into())),
        tok(TokenKind::Star),
        tok(TokenKind::Ident("r".into())),
        tok(TokenKind::Star),
        tok(TokenKind::Ident("r".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.items.len(), 3);
    assert!(matches!(&module.items[0].node, ItemKind::TypeDecl { .. }));
    assert!(matches!(&module.items[1].node, ItemKind::Let { .. }));
    assert!(matches!(&module.items[2].node, ItemKind::FnDef(_)));
}

// ---------------------------------------------------------------------------
// Empty program
// ---------------------------------------------------------------------------

#[test]
fn parse_empty_program() {
    let tokens = vec![tok(TokenKind::Eof)];
    let module = parse_ok(tokens);
    assert!(module.module_decl.is_none());
    assert!(module.uses.is_empty());
    assert!(module.items.is_empty());
}

// ---------------------------------------------------------------------------
// Modulo / remainder operator
// ---------------------------------------------------------------------------

#[test]
fn parse_modulo_operator() {
    // 10 % 3
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::IntLit(10)),
        tok(TokenKind::Percent),
        tok(TokenKind::IntLit(3)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::BinaryOp { op, .. } => assert_eq!(*op, BinOp::Mod),
            other => panic!("expected BinaryOp(Mod), got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Trailing comma in param_list
// ---------------------------------------------------------------------------

#[test]
fn parse_fn_params_trailing_comma() {
    // fn f(a: i32,):
    //     ret a
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("i32".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.params.len(), 1);
            assert_eq!(fd.params[0].name, "a");
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Infinite-loop regression tests
//
// These tests verify that the parser terminates (with errors) on malformed
// input that previously caused infinite loops. The key scenarios are:
//
// 1. parse_atom encountering an unrecognized token (e.g. Dedent, Error) must
//    consume it before returning, otherwise parse_block loops forever.
// 2. A stray Dedent at the top level must be consumed after synchronize() so
//    parse_program does not loop.
// ---------------------------------------------------------------------------

#[test]
fn no_hang_fn_missing_colon_before_body() {
    // fn main()
    //     ret 0
    //
    // Missing the ':' between the signature and the body. The lexer still
    // produces INDENT/DEDENT for the indented block, so the parser sees:
    //   Fn Ident("main") LParen RParen Newline Indent Ret IntLit(0) Newline Dedent Eof
    //
    // Without the fix, the parser would treat the fn as an extern declaration
    // (no ':'), then hit Indent at the top level, fail to parse a top-level
    // item, call synchronize() which stops at Dedent without consuming it,
    // and loop forever.
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let (_module, errors) = parse_with_errors(tokens);
    // The parser must terminate and produce at least one error.
    assert!(
        !errors.is_empty(),
        "expected parse errors for fn missing ':', got none"
    );
}

#[test]
fn no_hang_completely_garbled_input() {
    // A stream of tokens that makes no syntactic sense at all. The parser
    // must not loop; it should record errors and eventually reach Eof.
    let tokens = vec![
        tok(TokenKind::Dedent),
        tok(TokenKind::Indent),
        tok(TokenKind::Dedent),
        tok(TokenKind::Bang),
        tok(TokenKind::RBrace),
        tok(TokenKind::Dedent),
        tok(TokenKind::Error("unexpected character '$'".into())),
        tok(TokenKind::Indent),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let (_module, errors) = parse_with_errors(tokens);
    assert!(
        !errors.is_empty(),
        "expected parse errors for garbled input, got none"
    );
}

#[test]
fn no_hang_error_token_in_expression() {
    // An Error token appearing where an expression is expected inside a
    // function body. Without the parse_atom fix, the parser would loop
    // because the Error token is never consumed.
    //
    // fn f():
    //     <error>
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Error("bad token".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let (_module, errors) = parse_with_errors(tokens);
    assert!(
        !errors.is_empty(),
        "expected parse errors for Error token in expression, got none"
    );
}

#[test]
fn no_hang_stray_dedent_at_top_level() {
    // A bare Dedent at the top level (no matching Indent). The parser must
    // consume it and move on, not loop forever.
    let tokens = vec![
        tok(TokenKind::Dedent),
        tok(TokenKind::Dedent),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let (_module, errors) = parse_with_errors(tokens);
    assert!(
        !errors.is_empty(),
        "expected parse errors for stray Dedent, got none"
    );
}

// ---------------------------------------------------------------------------
// JSON diagnostic output
// ---------------------------------------------------------------------------

#[test]
fn parse_error_json_output() {
    let tokens = vec![
        tok(TokenKind::Let),
        tok_at(TokenKind::Assign, 1, 5),
        tok(TokenKind::IntLit(42)),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let (_module, errors) = parse_with_errors(tokens);
    assert!(!errors.is_empty());
    let json = errors[0].to_json();
    assert!(json.contains("\"source_phase\":\"parser\""));
    assert!(json.contains("\"severity\":\"error\""));
}

// ---------------------------------------------------------------------------
// Mutable bindings
// ---------------------------------------------------------------------------

#[test]
fn parse_let_mut() {
    // let mut x: Int = 5
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::Mut),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(5)),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::Let { name, mutable, .. } => {
            assert_eq!(name, "x");
            assert!(*mutable, "expected mutable=true");
        }
        other => panic!("expected Let, got {:?}", other),
    }
}

#[test]
fn parse_let_immutable_default() {
    // let y = 10
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::Ident("y".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(10)),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::Let { name, mutable, .. } => {
            assert_eq!(name, "y");
            assert!(!*mutable, "expected mutable=false");
        }
        other => panic!("expected Let, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Assignment statements
// ---------------------------------------------------------------------------

#[test]
fn parse_assign_stmt() {
    // fn f():
    //     x = 10
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(10)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.body.node.len(), 1);
            match &fd.body.node[0].node {
                StmtKind::Assign { name, value } => {
                    assert_eq!(name, "x");
                    assert!(matches!(&value.node, ExprKind::IntLit(10)));
                }
                other => panic!("expected Assign, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// While loops
// ---------------------------------------------------------------------------

#[test]
fn parse_while_loop() {
    // fn f():
    //     while x > 0:
    //         x
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::While),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Gt),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fd) => {
            assert_eq!(fd.body.node.len(), 1);
            match &fd.body.node[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::While { condition, body } => {
                        assert!(matches!(
                            &condition.node,
                            ExprKind::BinaryOp { op: BinOp::Gt, .. }
                        ));
                        assert_eq!(body.node.len(), 1);
                    }
                    other => panic!("expected While, got {:?}", other),
                },
                other => panic!("expected Expr stmt, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Match expressions (using lexer for proper INDENT/DEDENT tokens)
// ---------------------------------------------------------------------------

/// Helper: lex + parse a source string and return the Module.
fn parse_source_ok(src: &str) -> Module {
    let mut lexer = crate::lexer::Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (module, errors) = parse(tokens, 0);
    assert!(
        errors.is_empty(),
        "expected no parse errors, got: {:?}",
        errors
    );
    module
}

#[test]
fn parse_match_int_patterns() {
    let src = "\
fn f(n: Int) -> String:
    match n:
        0:
            ret \"zero\"
        1:
            ret \"one\"
        _:
            ret \"other\"
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "f");
            // The body should contain a single match expression statement.
            assert_eq!(fn_def.body.node.len(), 1);
            match &fn_def.body.node[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Match { scrutinee, arms } => {
                        assert!(matches!(&scrutinee.node, ExprKind::Ident(n) if n == "n"));
                        assert_eq!(arms.len(), 3);
                        assert_eq!(arms[0].pattern, Pattern::IntLit(0));
                        assert_eq!(arms[1].pattern, Pattern::IntLit(1));
                        assert_eq!(arms[2].pattern, Pattern::Wildcard);
                    }
                    other => panic!("expected Match expr, got {:?}", other),
                },
                other => panic!("expected Expr stmt, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_match_bool_patterns() {
    let src = "\
fn f(b: Bool) -> String:
    match b:
        true:
            ret \"yes\"
        false:
            ret \"no\"
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.body.node.len(), 1);
            match &fn_def.body.node[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Match { scrutinee, arms } => {
                        assert!(matches!(&scrutinee.node, ExprKind::Ident(n) if n == "b"));
                        assert_eq!(arms.len(), 2);
                        assert_eq!(arms[0].pattern, Pattern::BoolLit(true));
                        assert_eq!(arms[1].pattern, Pattern::BoolLit(false));
                    }
                    other => panic!("expected Match expr, got {:?}", other),
                },
                other => panic!("expected Expr stmt, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_match_with_wildcard_only() {
    let src = "\
fn f(n: Int) -> Int:
    match n:
        _:
            ret 0
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            match &fn_def.body.node[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Match { arms, .. } => {
                        assert_eq!(arms.len(), 1);
                        assert_eq!(arms[0].pattern, Pattern::Wildcard);
                    }
                    other => panic!("expected Match expr, got {:?}", other),
                },
                other => panic!("expected Expr stmt, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Enum declarations
// ---------------------------------------------------------------------------

#[test]
fn parse_enum_unit_variants() {
    // type Color = Red | Green | Blue
    let tokens = vec![
        tok(TokenKind::Type),
        tok(TokenKind::Ident("Color".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::Ident("Red".into())),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("Green".into())),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("Blue".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::EnumDecl { name, variants, .. } => {
            assert_eq!(name, "Color");
            assert_eq!(variants.len(), 3);
            assert_eq!(variants[0].name, "Red");
            assert!(variants[0].field.is_none());
            assert_eq!(variants[1].name, "Green");
            assert!(variants[1].field.is_none());
            assert_eq!(variants[2].name, "Blue");
            assert!(variants[2].field.is_none());
        }
        other => panic!("expected EnumDecl, got {:?}", other),
    }
}

#[test]
fn parse_enum_with_tuple_variant() {
    // type Option = Some(Int) | None
    let tokens = vec![
        tok(TokenKind::Type),
        tok(TokenKind::Ident("Option".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::Ident("Some".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("None".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::EnumDecl { name, variants, .. } => {
            assert_eq!(name, "Option");
            assert_eq!(variants.len(), 2);
            assert_eq!(variants[0].name, "Some");
            assert!(variants[0].field.is_some());
            assert!(matches!(
                &variants[0].field.as_ref().unwrap().node,
                TypeExpr::Named(n) if n == "Int"
            ));
            assert_eq!(variants[1].name, "None");
            assert!(variants[1].field.is_none());
        }
        other => panic!("expected EnumDecl, got {:?}", other),
    }
}

#[test]
fn parse_match_with_variant_patterns() {
    // fn f(c: Color):
    //     match c:
    //         Red:
    //             ret 0
    //         Green:
    //             ret 1
    //         Blue:
    //             ret 2
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("c".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Color".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // match c:
        tok(TokenKind::Match),
        tok(TokenKind::Ident("c".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // Red:
        tok(TokenKind::Ident("Red".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // Green:
        tok(TokenKind::Ident("Green".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // Blue:
        tok(TokenKind::Ident("Blue".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // DEDENT DEDENT (match, fn body)
        tok(TokenKind::Dedent),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };

    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::Match { arms, .. } => {
                assert_eq!(arms.len(), 3);
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Variant { variant: "Red".into(), binding: None }
                );
                assert_eq!(
                    arms[1].pattern,
                    Pattern::Variant { variant: "Green".into(), binding: None }
                );
                assert_eq!(
                    arms[2].pattern,
                    Pattern::Variant { variant: "Blue".into(), binding: None }
                );
            }
            other => panic!("expected Match, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_match_with_tuple_variant_binding() {
    // fn f(o: Option):
    //     match o:
    //         Some(x):
    //             ret x
    //         None:
    //             ret 0
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("o".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Option".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // match o:
        tok(TokenKind::Match),
        tok(TokenKind::Ident("o".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // Some(x):
        tok(TokenKind::Ident("Some".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // None:
        tok(TokenKind::Ident("None".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        // DEDENT DEDENT
        tok(TokenKind::Dedent),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };

    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => match &expr.node {
            ExprKind::Match { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Variant { variant: "Some".into(), binding: Some("x".into()) }
                );
                assert_eq!(
                    arms[1].pattern,
                    Pattern::Variant { variant: "None".into(), binding: None }
                );
            }
            other => panic!("expected Match, got {:?}", other),
        },
        other => panic!("expected Expr, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Design-by-contract: @requires and @ensures
// ---------------------------------------------------------------------------

#[test]
fn parse_requires_annotation() {
    let src = "\
@requires(x > 0)
fn positive(x: Int) -> Int:
    ret x
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "positive");
            assert_eq!(fn_def.contracts.len(), 1);
            assert_eq!(fn_def.contracts[0].kind, ContractKind::Requires);
            match &fn_def.contracts[0].condition.node {
                ExprKind::BinaryOp { op, left, right } => {
                    assert_eq!(*op, BinOp::Gt);
                    assert!(matches!(&left.node, ExprKind::Ident(n) if n == "x"));
                    assert!(matches!(&right.node, ExprKind::IntLit(0)));
                }
                other => panic!("expected BinaryOp, got {:?}", other),
            }
            assert!(fn_def.annotations.is_empty());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_ensures_annotation() {
    let src = "\
@ensures(result >= 0)
fn abs_val(x: Int) -> Int:
    if x >= 0:
        x
    else:
        0 - x
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "abs_val");
            assert_eq!(fn_def.contracts.len(), 1);
            assert_eq!(fn_def.contracts[0].kind, ContractKind::Ensures);
            match &fn_def.contracts[0].condition.node {
                ExprKind::BinaryOp { op, left, .. } => {
                    assert_eq!(*op, BinOp::Ge);
                    assert!(matches!(&left.node, ExprKind::Ident(n) if n == "result"));
                }
                other => panic!("expected BinaryOp, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_multiple_contracts() {
    let src = "\
@requires(x > 0)
@requires(y > 0)
@ensures(result > 0)
fn multiply(x: Int, y: Int) -> Int:
    ret x * y
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.contracts.len(), 3);
            assert_eq!(fn_def.contracts[0].kind, ContractKind::Requires);
            assert_eq!(fn_def.contracts[1].kind, ContractKind::Requires);
            assert_eq!(fn_def.contracts[2].kind, ContractKind::Ensures);
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_contract_with_regular_annotation() {
    let tokens = vec![
        tok(TokenKind::At),
        tok(TokenKind::Ident("requires".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Gt),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::At),
        tok(TokenKind::Ident("inline".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Arrow),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.contracts.len(), 1);
            assert_eq!(fn_def.contracts[0].kind, ContractKind::Requires);
            assert_eq!(fn_def.annotations.len(), 1);
            assert_eq!(fn_def.annotations[0].name, "inline");
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_result_as_variable_name() {
    let src = "\
fn f() -> Int:
    let result: Int = 42
    ret result
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "f");
            assert!(fn_def.contracts.is_empty());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Generics syntax parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_generic_function_single_type_param() {
    let src = "\
fn identity[T](x: T) -> T:
    ret x
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "identity");
            assert_eq!(fn_def.type_params, vec![TypeParam { name: "T".to_string(), bounds: vec![] }]);
            assert_eq!(fn_def.params.len(), 1);
            assert_eq!(fn_def.params[0].type_ann.node, TypeExpr::Named("T".to_string()));
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_generic_function_multiple_type_params() {
    let src = "\
fn pair[T, U](x: T, y: U) -> T:
    ret x
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.type_params, vec![TypeParam { name: "T".to_string(), bounds: vec![] }, TypeParam { name: "U".to_string(), bounds: vec![] }]);
            assert_eq!(fn_def.params.len(), 2);
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_generic_enum_declaration() {
    let src = "\
type Option[T] = Some(Int) | None
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::EnumDecl { name, type_params, variants, .. } => {
            assert_eq!(name, "Option");
            assert_eq!(type_params, &vec!["T".to_string()]);
            assert_eq!(variants.len(), 2);
            assert_eq!(variants[0].name, "Some");
            assert_eq!(variants[1].name, "None");
        }
        other => panic!("expected EnumDecl, got {:?}", other),
    }
}

#[test]
fn parse_generic_type_in_annotation() {
    let src = "\
type Option[T] = Some(Int) | None
fn main() -> !{IO} ():
    let x: Option[Int] = Some(42)
    print_int(0)
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 2);
    // The `let` stmt inside main's body should have a Generic type annotation.
    match &module.items[1].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "main");
            let body = &fn_def.body.node;
            assert!(!body.is_empty());
            match &body[0].node {
                StmtKind::Let { type_ann, .. } => {
                    let ann = type_ann.as_ref().expect("should have type annotation");
                    match &ann.node {
                        TypeExpr::Generic { name, args } => {
                            assert_eq!(name, "Option");
                            assert_eq!(args.len(), 1);
                            assert_eq!(args[0].node, TypeExpr::Named("Int".to_string()));
                        }
                        other => panic!("expected Generic type, got {:?}", other),
                    }
                }
                other => panic!("expected Let, got {:?}", other),
            }
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_non_generic_function_has_empty_type_params() {
    let src = "\
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert!(fn_def.type_params.is_empty());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Runtime capability budgets: @budget
// ---------------------------------------------------------------------------

#[test]
fn parse_budget_annotation_both() {
    let src = "\
@budget(cpu: 5s, mem: 100mb)
fn process_data(data: String) -> !{IO} ():
    print(data)
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "process_data");
            let budget = fn_def.budget.as_ref().expect("expected budget annotation");
            assert_eq!(budget.cpu.as_deref(), Some("5s"));
            assert_eq!(budget.mem.as_deref(), Some("100mb"));
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_budget_cpu_only() {
    let src = "\
@budget(cpu: 10s)
fn compute(x: Int) -> Int:
    ret x * 2
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let budget = fn_def.budget.as_ref().expect("expected budget annotation");
            assert_eq!(budget.cpu.as_deref(), Some("10s"));
            assert!(budget.mem.is_none());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_budget_mem_only() {
    let src = "\
@budget(mem: 512mb)
fn allocate(n: Int) -> Int:
    ret n
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let budget = fn_def.budget.as_ref().expect("expected budget annotation");
            assert!(budget.cpu.is_none());
            assert_eq!(budget.mem.as_deref(), Some("512mb"));
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_budget_with_milliseconds() {
    let src = "\
@budget(cpu: 500ms, mem: 1gb)
fn fast_op(x: Int) -> Int:
    ret x
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let budget = fn_def.budget.as_ref().expect("expected budget annotation");
            assert_eq!(budget.cpu.as_deref(), Some("500ms"));
            assert_eq!(budget.mem.as_deref(), Some("1gb"));
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_no_budget_means_none() {
    let src = "\
fn plain(x: Int) -> Int:
    ret x
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert!(fn_def.budget.is_none());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// FFI: @extern and @export
// ---------------------------------------------------------------------------

#[test]
fn parse_extern_annotation_basic() {
    // @extern on a function with no body should produce ExternFn.
    let src = "\
@extern
fn puts(s: String) -> Int
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::ExternFn(decl) => {
            assert_eq!(decl.name, "puts");
            assert_eq!(decl.params.len(), 1);
            assert_eq!(decl.params[0].name, "s");
            assert!(decl.extern_lib.is_none());
        }
        other => panic!("expected ExternFn, got {:?}", other),
    }
}

#[test]
fn parse_extern_with_library_name() {
    // @extern("libm") should set extern_lib.
    let src = r#"
@extern("libm")
fn sin(x: Float) -> Float
"#;
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::ExternFn(decl) => {
            assert_eq!(decl.name, "sin");
            assert_eq!(decl.extern_lib.as_deref(), Some("libm"));
        }
        other => panic!("expected ExternFn, got {:?}", other),
    }
}

#[test]
fn parse_export_annotation() {
    // @export on a function with a body should produce FnDef with is_export=true.
    let src = "\
@export
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "add");
            assert!(fn_def.is_export);
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_fn_def_not_export_by_default() {
    // Regular functions should have is_export=false.
    let src = "\
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert!(!fn_def.is_export);
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// @test annotation
// ---------------------------------------------------------------------------

#[test]
fn parse_test_annotation() {
    // @test on a function with a body should produce FnDef with is_test=true.
    let src = "\
@test
fn test_add() -> Bool:
    1 + 1 == 2
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.name, "test_add");
            assert!(fn_def.is_test);
            // @test should not appear in regular annotations
            assert!(fn_def.annotations.is_empty());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_fn_def_not_test_by_default() {
    // Regular functions should have is_test=false.
    let src = "\
fn add(a: Int, b: Int) -> Int:
    ret a + b
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert!(!fn_def.is_test);
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_test_and_export_together() {
    // A function can be both @test and @export.
    let src = "\
@test
@export
fn test_exported() -> Bool:
    true
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert!(fn_def.is_test);
            assert!(fn_def.is_export);
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_test_with_unit_return() {
    let src = "\
@test
fn test_unit():
    let x: Int = 1
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert!(fn_def.is_test);
            assert!(fn_def.return_type.is_none());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Actor declarations
// ---------------------------------------------------------------------------

#[test]
fn parse_actor_decl_simple() {
    // actor Counter:
    //     state count: Int = 0
    //     on Increment:
    //         count = count + 1
    let tokens = vec![
        tok(TokenKind::Actor),
        tok(TokenKind::Ident("Counter".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // state count: Int = 0
        tok(TokenKind::State),
        tok(TokenKind::Ident("count".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Newline),
        // on Increment:
        tok(TokenKind::On),
        tok(TokenKind::Ident("Increment".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        // handler body
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("count".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::Ident("count".into())),
        tok(TokenKind::Plus),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::ActorDecl { name, state_fields, handlers, .. } => {
            assert_eq!(name, "Counter");
            assert_eq!(state_fields.len(), 1);
            assert_eq!(state_fields[0].name, "count");
            assert_eq!(handlers.len(), 1);
            assert_eq!(handlers[0].message_name, "Increment");
            assert!(handlers[0].return_type.is_none());
        }
        _ => panic!("expected ActorDecl"),
    }
}

#[test]
fn parse_actor_with_return_handler() {
    // actor Counter:
    //     state count: Int = 0
    //     on GetCount -> Int:
    //         ret count
    let tokens = vec![
        tok(TokenKind::Actor),
        tok(TokenKind::Ident("Counter".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // state count: Int = 0
        tok(TokenKind::State),
        tok(TokenKind::Ident("count".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Newline),
        // on GetCount -> Int:
        tok(TokenKind::On),
        tok(TokenKind::Ident("GetCount".into())),
        tok(TokenKind::Arrow),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        // handler body
        tok(TokenKind::Indent),
        tok(TokenKind::Ret),
        tok(TokenKind::Ident("count".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::ActorDecl { name, handlers, .. } => {
            assert_eq!(name, "Counter");
            assert_eq!(handlers.len(), 1);
            let h = &handlers[0];
            assert_eq!(h.message_name, "GetCount");
            assert!(h.return_type.is_some());
            match &h.return_type.as_ref().unwrap().node {
                TypeExpr::Named(n) => assert_eq!(n, "Int"),
                _ => panic!("expected Named type"),
            }
        }
        _ => panic!("expected ActorDecl"),
    }
}

// ---------------------------------------------------------------------------
// Spawn, Send, Ask expressions
// ---------------------------------------------------------------------------

#[test]
fn parse_spawn_expr() {
    // fn main():
    //     spawn Counter
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Spawn),
        tok(TokenKind::Ident("Counter".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmts = &fn_def.body.node;
            assert_eq!(stmts.len(), 1);
            match &stmts[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Spawn { actor_name } => {
                        assert_eq!(actor_name, "Counter");
                    }
                    _ => panic!("expected Spawn expr, got {:?}", expr.node),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_send_expr() {
    // fn main():
    //     send c Increment
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Send),
        tok(TokenKind::Ident("c".into())),
        tok(TokenKind::Ident("Increment".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmts = &fn_def.body.node;
            match &stmts[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Send { target, message } => {
                        assert_eq!(message, "Increment");
                        match &target.node {
                            ExprKind::Ident(name) => assert_eq!(name, "c"),
                            _ => panic!("expected Ident target"),
                        }
                    }
                    _ => panic!("expected Send expr"),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_ask_expr() {
    // fn main():
    //     ask c GetCount
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ask),
        tok(TokenKind::Ident("c".into())),
        tok(TokenKind::Ident("GetCount".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmts = &fn_def.body.node;
            match &stmts[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Ask { target, message } => {
                        assert_eq!(message, "GetCount");
                        match &target.node {
                            ExprKind::Ident(name) => assert_eq!(name, "c"),
                            _ => panic!("expected Ident target"),
                        }
                    }
                    _ => panic!("expected Ask expr"),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

// ---------------------------------------------------------------------------
// Closure / lambda expressions
// ---------------------------------------------------------------------------

#[test]
fn parse_closure_single_param() {
    // fn main():
    //   |x: Int| x + 1
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Plus),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmts = &fn_def.body.node;
            assert_eq!(stmts.len(), 1);
            match &stmts[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Closure { params, return_type, body } => {
                        assert_eq!(params.len(), 1);
                        assert_eq!(params[0].name, "x");
                        assert!(params[0].type_ann.is_some());
                        assert!(return_type.is_none());
                        // Body should be x + 1
                        assert!(matches!(&body.node, ExprKind::BinaryOp { op: BinOp::Add, .. }));
                    }
                    _ => panic!("expected Closure expr, got {:?}", expr.node),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_closure_zero_params() {
    // fn main():
    //   || 42
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Pipe),
        tok(TokenKind::Pipe),
        tok(TokenKind::IntLit(42)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmts = &fn_def.body.node;
            match &stmts[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Closure { params, return_type, body } => {
                        assert_eq!(params.len(), 0);
                        assert!(return_type.is_none());
                        assert!(matches!(&body.node, ExprKind::IntLit(42)));
                    }
                    _ => panic!("expected Closure expr"),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_closure_multi_params() {
    // fn main():
    //   |x: Int, y: Int| x + y
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::Ident("y".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Plus),
        tok(TokenKind::Ident("y".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmts = &fn_def.body.node;
            match &stmts[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Closure { params, return_type, body } => {
                        assert_eq!(params.len(), 2);
                        assert_eq!(params[0].name, "x");
                        assert_eq!(params[1].name, "y");
                        assert!(return_type.is_none());
                        assert!(matches!(&body.node, ExprKind::BinaryOp { op: BinOp::Add, .. }));
                    }
                    _ => panic!("expected Closure expr"),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_closure_with_return_type() {
    // fn main():
    //   |x: Int| -> Int: x + 1
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Pipe),
        tok(TokenKind::Arrow),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Plus),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmts = &fn_def.body.node;
            match &stmts[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Closure { params, return_type, body } => {
                        assert_eq!(params.len(), 1);
                        assert!(return_type.is_some());
                        let ret = return_type.as_ref().unwrap();
                        assert!(matches!(&ret.node, TypeExpr::Named(n) if n == "Int"));
                        assert!(matches!(&body.node, ExprKind::BinaryOp { .. }));
                    }
                    _ => panic!("expected Closure expr"),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_closure_untyped_param() {
    // fn main():
    //   |x| x
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("main".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Pipe),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmts = &fn_def.body.node;
            match &stmts[0].node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Closure { params, .. } => {
                        assert_eq!(params.len(), 1);
                        assert_eq!(params[0].name, "x");
                        assert!(params[0].type_ann.is_none());
                    }
                    _ => panic!("expected Closure expr"),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

// ---------------------------------------------------------------------------
// Tuple types, expressions, field access, and destructuring
// ---------------------------------------------------------------------------

#[test]
fn tuple_type_in_let_annotation() {
    // let x: (Int, String) = ...
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Colon),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("Int".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::Ident("String".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Assign),
        tok(TokenKind::IntLit(42)),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    let item = &module.items[0];
    match &item.node {
        ItemKind::Let { type_ann, .. } => {
            let ann = type_ann.as_ref().expect("expected type annotation");
            match &ann.node {
                TypeExpr::Tuple(elems) => {
                    assert_eq!(elems.len(), 2);
                    assert!(matches!(&elems[0].node, TypeExpr::Named(n) if n == "Int"));
                    assert!(matches!(&elems[1].node, TypeExpr::Named(n) if n == "String"));
                }
                _ => panic!("expected Tuple type, got {:?}", ann.node),
            }
        }
        _ => panic!("expected Let item"),
    }
}

#[test]
fn tuple_expression_two_elements() {
    // fn f(): (1, 2)
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::LParen),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Comma),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmt = &fn_def.body.node[0];
            match &stmt.node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::Tuple(elems) => {
                        assert_eq!(elems.len(), 2);
                        assert!(matches!(&elems[0].node, ExprKind::IntLit(1)));
                        assert!(matches!(&elems[1].node, ExprKind::IntLit(2)));
                    }
                    _ => panic!("expected Tuple expr, got {:?}", expr.node),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef item"),
    }
}

#[test]
fn tuple_field_access_numeric() {
    // fn f(): let x = (1, 2) \n x.0
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Let),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::LParen),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Comma),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::IntLit(0)),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            // Second statement should be x.0
            let stmt = &fn_def.body.node[1];
            match &stmt.node {
                StmtKind::Expr(expr) => match &expr.node {
                    ExprKind::TupleField { tuple, index } => {
                        assert!(matches!(&tuple.node, ExprKind::Ident(n) if n == "x"));
                        assert_eq!(*index, 0);
                    }
                    _ => panic!("expected TupleField expr, got {:?}", expr.node),
                },
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef item"),
    }
}

#[test]
fn tuple_destructuring_in_let() {
    // let (a, b) = (1, 2)
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Assign),
        tok(TokenKind::LParen),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Comma),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    let item = &module.items[0];
    match &item.node {
        ItemKind::LetTupleDestructure { names, value, .. } => {
            assert_eq!(names, &["a".to_string(), "b".to_string()]);
            assert!(matches!(&value.node, ExprKind::Tuple(elems) if elems.len() == 2));
        }
        _ => panic!("expected LetTupleDestructure item, got {:?}", item.node),
    }
}

#[test]
fn paren_expr_not_confused_with_tuple() {
    // (42) should be a Paren expression, not a tuple
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::LParen),
        tok(TokenKind::IntLit(42)),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmt = &fn_def.body.node[0];
            match &stmt.node {
                StmtKind::Expr(expr) => {
                    assert!(matches!(&expr.node, ExprKind::Paren(_)), "expected Paren, got {:?}", expr.node);
                }
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef item"),
    }
}

// ---------------------------------------------------------------------------
// Trait declarations
// ---------------------------------------------------------------------------

#[test]
fn parse_trait_decl_single_method() {
    let src = "\
trait Display:
    fn display(self) -> String
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::TraitDecl { name, methods, .. } => {
            assert_eq!(name, "Display");
            assert_eq!(methods.len(), 1);
            assert_eq!(methods[0].name, "display");
            assert_eq!(methods[0].params.len(), 1);
            assert_eq!(methods[0].params[0].name, "self");
            assert_eq!(methods[0].return_type.as_ref().unwrap().node, TypeExpr::Named("String".to_string()));
        }
        other => panic!("expected TraitDecl, got {:?}", other),
    }
}

#[test]
fn parse_trait_decl_multiple_methods() {
    let src = "\
trait Eq:
    fn equals(self, other: Int) -> Bool
    fn not_equals(self, other: Int) -> Bool
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::TraitDecl { name, methods, .. } => {
            assert_eq!(name, "Eq");
            assert_eq!(methods.len(), 2);
            assert_eq!(methods[0].name, "equals");
            assert_eq!(methods[1].name, "not_equals");
            // First method has self + other
            assert_eq!(methods[0].params.len(), 2);
            assert_eq!(methods[0].params[0].name, "self");
            assert_eq!(methods[0].params[1].name, "other");
        }
        other => panic!("expected TraitDecl, got {:?}", other),
    }
}

#[test]
fn parse_trait_decl_with_doc_comment() {
    let src = "\
/// A trait for display.
trait Display:
    fn display(self) -> String
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::TraitDecl { name, doc_comment, .. } => {
            assert_eq!(name, "Display");
            assert_eq!(doc_comment.as_deref(), Some("A trait for display."));
        }
        other => panic!("expected TraitDecl, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Impl blocks
// ---------------------------------------------------------------------------

#[test]
fn parse_impl_block_single_method() {
    let src = "\
impl Display for Int:
    fn display(self) -> String:
        ret int_to_string(self)
";
    let module = parse_source_ok(src);
    assert_eq!(module.items.len(), 1);
    match &module.items[0].node {
        ItemKind::ImplBlock { trait_name, target_type, methods } => {
            assert_eq!(trait_name, "Display");
            assert_eq!(target_type, "Int");
            assert_eq!(methods.len(), 1);
            assert_eq!(methods[0].name, "display");
            assert_eq!(methods[0].params.len(), 1);
            assert_eq!(methods[0].params[0].name, "self");
        }
        other => panic!("expected ImplBlock, got {:?}", other),
    }
}

#[test]
fn parse_impl_block_multiple_methods() {
    let src = "\
impl Eq for Int:
    fn equals(self, other: Int) -> Bool:
        ret self == other
    fn not_equals(self, other: Int) -> Bool:
        ret self != other
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::ImplBlock { trait_name, target_type, methods } => {
            assert_eq!(trait_name, "Eq");
            assert_eq!(target_type, "Int");
            assert_eq!(methods.len(), 2);
            assert_eq!(methods[0].name, "equals");
            assert_eq!(methods[1].name, "not_equals");
        }
        other => panic!("expected ImplBlock, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Trait bounds on generics
// ---------------------------------------------------------------------------

#[test]
fn parse_trait_bound_single() {
    let src = "\
fn print_value[T: Display](x: T) -> String:
    ret x
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.type_params.len(), 1);
            assert_eq!(fn_def.type_params[0].name, "T");
            assert_eq!(fn_def.type_params[0].bounds, vec!["Display".to_string()]);
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_trait_bound_multiple_params() {
    let src = "\
fn compare[T: Eq, U: Display](a: T, b: U) -> Bool:
    ret true
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.type_params.len(), 2);
            assert_eq!(fn_def.type_params[0].name, "T");
            assert_eq!(fn_def.type_params[0].bounds, vec!["Eq".to_string()]);
            assert_eq!(fn_def.type_params[1].name, "U");
            assert_eq!(fn_def.type_params[1].bounds, vec!["Display".to_string()]);
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}

#[test]
fn parse_trait_bound_no_bounds() {
    let src = "\
fn identity[T](x: T) -> T:
    ret x
";
    let module = parse_source_ok(src);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            assert_eq!(fn_def.type_params.len(), 1);
            assert_eq!(fn_def.type_params[0].name, "T");
            assert!(fn_def.type_params[0].bounds.is_empty());
        }
        other => panic!("expected FnDef, got {:?}", other),
    }
}
// ---------------------------------------------------------------------------
// Try operator (?)
// ---------------------------------------------------------------------------

#[test]
fn parse_try_operator_on_call() {
    // fn f():
    //     get_value()?
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("get_value".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Question),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => {
            assert!(
                matches!(&expr.node, ExprKind::Try(inner) if matches!(&inner.node, ExprKind::Call { .. })),
                "expected Try(Call), got {:?}",
                expr.node
            );
        }
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_try_operator_on_ident() {
    // fn f():
    //     x?
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Question),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => {
            assert!(
                matches!(&expr.node, ExprKind::Try(inner) if matches!(&inner.node, ExprKind::Ident(n) if n == "x")),
                "expected Try(Ident(\"x\")), got {:?}",
                expr.node
            );
        }
        other => panic!("expected Expr, got {:?}", other),
    }
}

#[test]
fn parse_try_operator_chained() {
    // fn f():
    //     a()?.b()?
    // This should parse as Try(Call(FieldAccess(Try(Call(a)), b)))
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        // a()?
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Question),
        // .b()
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        // ?
        tok(TokenKind::Question),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];

    let module = parse_ok(tokens);
    let fd = match &module.items[0].node {
        ItemKind::FnDef(fd) => fd,
        other => panic!("expected FnDef, got {:?}", other),
    };
    match &fd.body.node[0].node {
        StmtKind::Expr(expr) => {
            // Outermost should be Try
            assert!(
                matches!(&expr.node, ExprKind::Try(_)),
                "expected outer Try, got {:?}",
                expr.node
            );
        }
        other => panic!("expected Expr, got {:?}", other),
    }
}
// ---------------------------------------------------------------------------
// List literal parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_empty_list_literal() {
    // fn f(): []
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::LBracket),
        tok(TokenKind::RBracket),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmt = &fn_def.body.node[0];
            match &stmt.node {
                StmtKind::Expr(expr) => {
                    match &expr.node {
                        ExprKind::ListLit(elems) => {
                            assert!(elems.is_empty(), "expected empty list literal");
                        }
                        _ => panic!("expected ListLit, got {:?}", expr.node),
                    }
                }
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_list_literal_with_elements() {
    // fn f(): [1, 2, 3]
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::LBracket),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Comma),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::Comma),
        tok(TokenKind::IntLit(3)),
        tok(TokenKind::RBracket),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmt = &fn_def.body.node[0];
            match &stmt.node {
                StmtKind::Expr(expr) => {
                    match &expr.node {
                        ExprKind::ListLit(elems) => {
                            assert_eq!(elems.len(), 3, "expected 3 elements");
                            assert!(matches!(&elems[0].node, ExprKind::IntLit(1)));
                            assert!(matches!(&elems[1].node, ExprKind::IntLit(2)));
                            assert!(matches!(&elems[2].node, ExprKind::IntLit(3)));
                        }
                        _ => panic!("expected ListLit, got {:?}", expr.node),
                    }
                }
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_list_literal_single_element() {
    // fn f(): [42]
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::LBracket),
        tok(TokenKind::IntLit(42)),
        tok(TokenKind::RBracket),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmt = &fn_def.body.node[0];
            match &stmt.node {
                StmtKind::Expr(expr) => {
                    match &expr.node {
                        ExprKind::ListLit(elems) => {
                            assert_eq!(elems.len(), 1);
                            assert!(matches!(&elems[0].node, ExprKind::IntLit(42)));
                        }
                        _ => panic!("expected ListLit, got {:?}", expr.node),
                    }
                }
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}

#[test]
fn parse_list_literal_trailing_comma() {
    // fn f(): [1, 2,]
    let tokens = vec![
        tok(TokenKind::Fn),
        tok(TokenKind::Ident("f".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Colon),
        tok(TokenKind::Newline),
        tok(TokenKind::Indent),
        tok(TokenKind::LBracket),
        tok(TokenKind::IntLit(1)),
        tok(TokenKind::Comma),
        tok(TokenKind::IntLit(2)),
        tok(TokenKind::Comma),
        tok(TokenKind::RBracket),
        tok(TokenKind::Newline),
        tok(TokenKind::Dedent),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    match &module.items[0].node {
        ItemKind::FnDef(fn_def) => {
            let stmt = &fn_def.body.node[0];
            match &stmt.node {
                StmtKind::Expr(expr) => {
                    match &expr.node {
                        ExprKind::ListLit(elems) => {
                            assert_eq!(elems.len(), 2, "trailing comma should not create extra element");
                        }
                        _ => panic!("expected ListLit, got {:?}", expr.node),
                    }
                }
                _ => panic!("expected Expr stmt"),
            }
        }
        _ => panic!("expected FnDef"),
    }
}
// ---------------------------------------------------------------------------
// Interpolated strings
// ---------------------------------------------------------------------------

/// Helper: wrap an expression token in a `let x = <expr>` at the top level.
fn wrap_in_let(expr_token: Token) -> Vec<Token> {
    vec![
        tok(TokenKind::Let),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Assign),
        expr_token,
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ]
}

/// Extract the value expression from a top-level `let x = <expr>` item.
fn extract_let_value(module: &Module) -> &crate::ast::expr::Expr {
    match &module.items[0].node {
        ItemKind::Let { value, .. } => value,
        other => panic!("expected Let item, got {:?}", other),
    }
}

#[test]
fn interpolated_string_literal_only() {
    let tokens = wrap_in_let(tok(TokenKind::InterpolatedString(vec![
        InterpolationPart::Literal("hello".into()),
    ])));
    let module = parse_ok(tokens);
    let expr = extract_let_value(&module);
    match &expr.node {
        ExprKind::StringInterp { parts } => {
            assert_eq!(parts.len(), 1);
            assert!(matches!(&parts[0], StringInterpPart::Literal(s) if s == "hello"));
        }
        _ => panic!("expected StringInterp, got {:?}", expr.node),
    }
}

#[test]
fn interpolated_string_with_expr() {
    let tokens = wrap_in_let(tok(TokenKind::InterpolatedString(vec![
        InterpolationPart::Literal("hello ".into()),
        InterpolationPart::Expr("name".into()),
    ])));
    let module = parse_ok(tokens);
    let expr = extract_let_value(&module);
    match &expr.node {
        ExprKind::StringInterp { parts } => {
            assert_eq!(parts.len(), 2);
            assert!(matches!(&parts[0], StringInterpPart::Literal(s) if s == "hello "));
            assert!(matches!(&parts[1], StringInterpPart::Expr(e) if matches!(&e.node, ExprKind::Ident(n) if n == "name")));
        }
        _ => panic!("expected StringInterp, got {:?}", expr.node),
    }
}

#[test]
fn interpolated_string_with_binary_expr() {
    let tokens = wrap_in_let(tok(TokenKind::InterpolatedString(vec![
        InterpolationPart::Literal("result = ".into()),
        InterpolationPart::Expr("2 + 2".into()),
    ])));
    let module = parse_ok(tokens);
    let expr = extract_let_value(&module);
    match &expr.node {
        ExprKind::StringInterp { parts } => {
            assert_eq!(parts.len(), 2);
            assert!(matches!(&parts[1], StringInterpPart::Expr(e) if matches!(&e.node, ExprKind::BinaryOp { op: BinOp::Add, .. })));
        }
        _ => panic!("expected StringInterp, got {:?}", expr.node),
    }
}

// =========================================================================
// Method call syntax tests
// =========================================================================

/// `let x = obj.method()` should parse as Call { func: FieldAccess { object: obj, field: "method" }, args: [] }
#[test]
fn method_call_no_args_ast_structure() {
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::Ident("obj".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("method".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    let expr = extract_let_value(&module);
    match &expr.node {
        ExprKind::Call { func, args } => {
            assert!(args.is_empty(), "expected no arguments");
            match &func.node {
                ExprKind::FieldAccess { object, field } => {
                    assert!(matches!(&object.node, ExprKind::Ident(n) if n == "obj"));
                    assert_eq!(field, "method");
                }
                other => panic!("expected FieldAccess, got {:?}", other),
            }
        }
        other => panic!("expected Call, got {:?}", other),
    }
}

/// `let x = obj.method(a, b)` should parse as Call { func: FieldAccess, args: [a, b] }
#[test]
fn method_call_with_args_ast_structure() {
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::Ident("obj".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("method".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::Comma),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    let expr = extract_let_value(&module);
    match &expr.node {
        ExprKind::Call { func, args } => {
            assert_eq!(args.len(), 2, "expected two arguments");
            match &func.node {
                ExprKind::FieldAccess { object, field } => {
                    assert!(matches!(&object.node, ExprKind::Ident(n) if n == "obj"));
                    assert_eq!(field, "method");
                }
                other => panic!("expected FieldAccess, got {:?}", other),
            }
        }
        other => panic!("expected Call, got {:?}", other),
    }
}

/// Chained method call: `let x = obj.a().b()` should parse as nested Call(FieldAccess(Call(FieldAccess(...))))
#[test]
fn chained_method_call_ast_structure() {
    let tokens = vec![
        tok(TokenKind::Let),
        tok(TokenKind::Ident("x".into())),
        tok(TokenKind::Assign),
        tok(TokenKind::Ident("obj".into())),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("a".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Dot),
        tok(TokenKind::Ident("b".into())),
        tok(TokenKind::LParen),
        tok(TokenKind::RParen),
        tok(TokenKind::Newline),
        tok(TokenKind::Eof),
    ];
    let module = parse_ok(tokens);
    let expr = extract_let_value(&module);
    // Outer: Call { func: FieldAccess { object: Call { func: FieldAccess { object: obj, field: "a" } }, field: "b" }, args: [] }
    match &expr.node {
        ExprKind::Call { func: outer_func, args: outer_args } => {
            assert!(outer_args.is_empty());
            match &outer_func.node {
                ExprKind::FieldAccess { object: inner_call, field: outer_field } => {
                    assert_eq!(outer_field, "b");
                    match &inner_call.node {
                        ExprKind::Call { func: inner_func, args: inner_args } => {
                            assert!(inner_args.is_empty());
                            match &inner_func.node {
                                ExprKind::FieldAccess { object, field } => {
                                    assert!(matches!(&object.node, ExprKind::Ident(n) if n == "obj"));
                                    assert_eq!(field, "a");
                                }
                                other => panic!("expected inner FieldAccess, got {:?}", other),
                            }
                        }
                        other => panic!("expected inner Call, got {:?}", other),
                    }
                }
                other => panic!("expected outer FieldAccess, got {:?}", other),
            }
        }
        other => panic!("expected outer Call, got {:?}", other),
    }
}
