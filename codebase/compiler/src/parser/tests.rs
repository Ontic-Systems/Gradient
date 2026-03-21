//! Parser tests for the Gradient language.
//!
//! Each test constructs a token stream by hand and feeds it to the parser,
//! then asserts properties of the resulting AST. This avoids a dependency on
//! the lexer (which is being developed in parallel) while giving us thorough
//! coverage of every grammar rule.

use crate::ast::expr::{BinOp, ExprKind, MatchArm, Pattern, UnaryOp};
use crate::ast::item::ItemKind;
use crate::ast::module::Module;
use crate::ast::stmt::StmtKind;
use crate::ast::types::TypeExpr;
use crate::ast::span::{Position, Span};
use crate::lexer::token::{Token, TokenKind};
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
        ItemKind::TypeDecl { name, type_expr } => {
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
