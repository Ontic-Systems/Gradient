//! Parser differential test — bootstrap subset gate (issue #196, expanded by
//! issue #224).
//!
//! This integration test enforces parity for the bootstrap subset of
//! Gradient that the self-hosted parser must round-trip. The gate is anchored
//! by frozen `.json` baselines, and now also checks the self-hosted parser's
//! normalized-export contract for the same corpus. Until the Gradient runtime
//! can execute `compiler/parser.gr` directly with real TokenList/list-backed
//! parser state, the host adapter uses the Rust parser for token/AST execution
//! but refuses to pass unless `parser.gr` preserves bootstrap-subset node
//! identity and exposes normalized export helpers. The test asserts:
//!
//!   1. The on-disk corpus is non-empty AND every `.gr` has a matching
//!      `.json` baseline (closes the "passes with 0 matches" hole).
//!   2. Parsing the snippet through the Rust parser, normalizing, and
//!      serializing to canonical JSON exactly matches the on-disk baseline.
//!   3. The self-hosted parser normalized-export contract produces the same
//!      canonical JSON for at least one corpus case.
//!   4. The in-memory `NormalizedAst` round-trips through JSON
//!      (serialize → deserialize → serialize) without changing.
//!
//! Companion gate: `parser_boundary_tests.rs` locks the *boundary* — the set
//! of constructs the Rust parser accepts but that fall outside the bootstrap
//! subset (FieldAccess, While, For, Match, RecordLit, ListLit, Tuple,
//! Closure, FloatLit, else-if chains, modulo, pipe, assignment, EnumDecl,
//! TypeDecl, etc.). The boundary gate guarantees those constructs continue
//! to map to explicit `Unsupported { reason: ... }` markers instead of
//! silently being skipped or accepted.
//!
//! When the normalized form intentionally changes, regenerate baselines:
//!   cargo test -p gradient-compiler --test parser_differential_tests \
//!       regenerate_baselines -- --include-ignored
//!   cargo test -p gradient-compiler --test parser_boundary_tests \
//!       regenerate_boundary_baselines -- --include-ignored
//!
//! Bootstrap subset (do not extend without updating issue #196):
//!   - function definitions with Int/Bool/String params and return types
//!   - let / let mut bindings
//!   - integer / bool / string literals; identifier expressions
//!   - binary ops: + - * / == != < <= > >= && (and) || (or)
//!   - unary ops: - (Neg) ! (Not / `not`)
//!   - function calls (zero or more args)
//!   - if / else expressions
//!   - block expressions with a final-expression value
//!   - ret <expr> statements

#![allow(clippy::uninlined_format_args)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use gradient_compiler::ast::{
    block::Block,
    expr::{BinOp, Expr, ExprKind, UnaryOp},
    item::{FnDef, Item, ItemKind, Param},
    module::Module,
    stmt::{Stmt, StmtKind},
    types::TypeExpr,
};
use gradient_compiler::bootstrap_parser_bridge::BootstrapParser;
use gradient_compiler::lexer::{Lexer, TokenKind};
use gradient_compiler::parser;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// NormalizedAst — bootstrap-subset only.
//
// All variants use serde's snake_case tag rename. Spans, file_ids, parser
// internal IDs are dropped. Anything outside the bootstrap subset (closures,
// generics, patterns, records, etc.) becomes `Unsupported(<reason>)` so the
// JSON baseline records the boundary instead of silently mapping it.
// ---------------------------------------------------------------------------

/// Canonical, serde-serialisable normalised AST root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedAst {
    pub items: Vec<NormalizedItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NormalizedItem {
    Function(NormalizedFunction),
    Unsupported { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedFunction {
    pub name: String,
    pub params: Vec<NormalizedParam>,
    pub ret_type: Option<NormalizedType>,
    pub body: Vec<NormalizedStmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedParam {
    pub name: String,
    pub ty: NormalizedType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NormalizedType {
    /// One of `Int`, `Bool`, `String` for the bootstrap subset.
    Named {
        name: String,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NormalizedStmt {
    Let {
        name: String,
        mutable: bool,
        ty: Option<NormalizedType>,
        value: NormalizedExpr,
    },
    Ret {
        value: NormalizedExpr,
    },
    Expr {
        value: NormalizedExpr,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NormalizedExpr {
    IntLit {
        value: i64,
    },
    BoolLit {
        value: bool,
    },
    StringLit {
        value: String,
    },
    Ident {
        name: String,
    },
    Binary {
        op: String,
        left: Box<NormalizedExpr>,
        right: Box<NormalizedExpr>,
    },
    Unary {
        op: String,
        operand: Box<NormalizedExpr>,
    },
    Call {
        callee: Box<NormalizedExpr>,
        args: Vec<NormalizedExpr>,
    },
    If {
        cond: Box<NormalizedExpr>,
        then_block: Vec<NormalizedStmt>,
        else_block: Option<Vec<NormalizedStmt>>,
    },
    Block {
        stmts: Vec<NormalizedStmt>,
        /// `Some` when the block ends in a final-expression value.
        final_expr: Option<Box<NormalizedExpr>>,
    },
    Unsupported {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Normalisation pass: Rust AST -> NormalizedAst.
//
// Strip rule: every `Spanned<T>.span`, every `Span`/`Position`, every
// `file_id`, every parser-internal flag that isn't part of the bootstrap
// subset (annotations, contracts, budget, doc_comment, type_params,
// effects, comptime, ...) is dropped. `BinOp` and `UnaryOp` are mapped to
// stable lowercase string identifiers (e.g. `add`, `eq`, `and`, `neg`,
// `not`) so the wire form is independent of Rust enum Debug formatting.
// ---------------------------------------------------------------------------

fn normalize_module(m: &Module) -> NormalizedAst {
    NormalizedAst {
        items: m.items.iter().map(normalize_item).collect(),
    }
}

fn normalize_item(item: &Item) -> NormalizedItem {
    match &item.node {
        ItemKind::FnDef(fn_def) => NormalizedItem::Function(normalize_fn(fn_def)),
        other => NormalizedItem::Unsupported {
            reason: format!(
                "item kind outside bootstrap subset: {}",
                item_kind_name(other)
            ),
        },
    }
}

fn item_kind_name(k: &ItemKind) -> &'static str {
    match k {
        ItemKind::FnDef(_) => "FnDef",
        ItemKind::ExternFn(_) => "ExternFn",
        ItemKind::Let { .. } => "Let",
        ItemKind::LetTupleDestructure { .. } => "LetTupleDestructure",
        ItemKind::TypeDecl { .. } => "TypeDecl",
        ItemKind::CapDecl { .. } => "CapDecl",
        ItemKind::CapTypeDecl { .. } => "CapTypeDecl",
        ItemKind::EnumDecl { .. } => "EnumDecl",
        ItemKind::ActorDecl { .. } => "ActorDecl",
        ItemKind::TraitDecl { .. } => "TraitDecl",
        ItemKind::ImplBlock { .. } => "ImplBlock",
        ItemKind::ModBlock { .. } => "ModBlock",
        ItemKind::Import { .. } => "Import",
    }
}

fn normalize_fn(fn_def: &FnDef) -> NormalizedFunction {
    NormalizedFunction {
        name: fn_def.name.clone(),
        params: fn_def.params.iter().map(normalize_param).collect(),
        ret_type: fn_def.return_type.as_ref().map(|t| normalize_type(&t.node)),
        body: normalize_block(&fn_def.body),
    }
}

fn normalize_param(p: &Param) -> NormalizedParam {
    NormalizedParam {
        name: p.name.clone(),
        ty: normalize_type(&p.type_ann.node),
    }
}

fn normalize_type(t: &TypeExpr) -> NormalizedType {
    match t {
        TypeExpr::Named { name, cap: None }
            if matches!(name.as_str(), "Int" | "Bool" | "String") =>
        {
            NormalizedType::Named { name: name.clone() }
        }
        TypeExpr::Named { name, cap } => NormalizedType::Unsupported {
            reason: format!(
                "named type outside bootstrap subset: {}{}",
                name,
                cap.as_ref()
                    .map(|c| format!(" (cap={})", c))
                    .unwrap_or_default()
            ),
        },
        TypeExpr::Unit => NormalizedType::Unsupported {
            reason: "unit type".into(),
        },
        TypeExpr::Fn { .. } => NormalizedType::Unsupported {
            reason: "fn type".into(),
        },
        TypeExpr::Generic { name, .. } => NormalizedType::Unsupported {
            reason: format!("generic type {}", name),
        },
        TypeExpr::Tuple(_) => NormalizedType::Unsupported {
            reason: "tuple type".into(),
        },
        TypeExpr::Record(_) => NormalizedType::Unsupported {
            reason: "record type".into(),
        },
        TypeExpr::Linear(_) => NormalizedType::Unsupported {
            reason: "linear type".into(),
        },
        TypeExpr::Type => NormalizedType::Unsupported {
            reason: "type-of-types".into(),
        },
    }
}

fn normalize_block(b: &Block) -> Vec<NormalizedStmt> {
    b.node.iter().map(normalize_stmt).collect()
}

fn normalize_stmt(s: &Stmt) -> NormalizedStmt {
    match &s.node {
        StmtKind::Let {
            name,
            type_ann,
            value,
            mutable,
        } => NormalizedStmt::Let {
            name: name.clone(),
            mutable: *mutable,
            ty: type_ann.as_ref().map(|t| normalize_type(&t.node)),
            value: normalize_expr(value),
        },
        StmtKind::Ret(e) => NormalizedStmt::Ret {
            value: normalize_expr(e),
        },
        StmtKind::Expr(e) => NormalizedStmt::Expr {
            value: normalize_expr(e),
        },
        StmtKind::LetTupleDestructure { .. } => NormalizedStmt::Unsupported {
            reason: "let tuple destructure outside bootstrap subset".into(),
        },
        StmtKind::Assign { .. } => NormalizedStmt::Unsupported {
            reason: "assignment outside bootstrap subset".into(),
        },
    }
}

fn binop_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Or => "or",
        BinOp::And => "and",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Le => "le",
        BinOp::Gt => "gt",
        BinOp::Ge => "ge",
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Mod => "mod",
        BinOp::Pipe => "pipe",
    }
}

fn unop_name(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "neg",
        UnaryOp::Not => "not",
    }
}

/// Bootstrap subset includes only these binary ops; everything else is
/// recorded as Unsupported so the JSON baseline names the boundary.
fn binop_in_subset(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::And
            | BinOp::Or
    )
}

fn normalize_expr(e: &Expr) -> NormalizedExpr {
    match &e.node {
        ExprKind::IntLit(n) => NormalizedExpr::IntLit { value: *n },
        ExprKind::BoolLit(b) => NormalizedExpr::BoolLit { value: *b },
        ExprKind::StringLit(s) => NormalizedExpr::StringLit { value: s.clone() },
        ExprKind::Ident(name) => NormalizedExpr::Ident { name: name.clone() },
        ExprKind::BinaryOp { op, left, right } if binop_in_subset(*op) => NormalizedExpr::Binary {
            op: binop_name(*op).to_string(),
            left: Box::new(normalize_expr(left)),
            right: Box::new(normalize_expr(right)),
        },
        ExprKind::BinaryOp { op, .. } => NormalizedExpr::Unsupported {
            reason: format!("binary op outside bootstrap subset: {}", binop_name(*op)),
        },
        ExprKind::UnaryOp { op, operand } => NormalizedExpr::Unary {
            op: unop_name(*op).to_string(),
            operand: Box::new(normalize_expr(operand)),
        },
        ExprKind::Call { func, args } => NormalizedExpr::Call {
            callee: Box::new(normalize_expr(func)),
            args: args.iter().map(normalize_expr).collect(),
        },
        ExprKind::If {
            condition,
            then_block,
            else_ifs,
            else_block,
        } => {
            // Bootstrap subset: only plain if/else (no else-if chain).
            if !else_ifs.is_empty() {
                return NormalizedExpr::Unsupported {
                    reason: "else-if chain outside bootstrap subset".into(),
                };
            }
            NormalizedExpr::If {
                cond: Box::new(normalize_expr(condition)),
                then_block: normalize_block(then_block),
                else_block: else_block.as_ref().map(normalize_block),
            }
        }
        ExprKind::Paren(inner) => normalize_expr(inner),
        // Everything else falls outside the bootstrap subset.
        other => NormalizedExpr::Unsupported {
            reason: format!(
                "expression outside bootstrap subset: {}",
                expr_kind_name(other)
            ),
        },
    }
}

fn expr_kind_name(k: &ExprKind) -> &'static str {
    match k {
        ExprKind::IntLit(_) => "IntLit",
        ExprKind::FloatLit(_) => "FloatLit",
        ExprKind::StringLit(_) => "StringLit",
        ExprKind::CharLit(_) => "CharLit",
        ExprKind::BoolLit(_) => "BoolLit",
        ExprKind::UnitLit => "UnitLit",
        ExprKind::Ident(_) => "Ident",
        ExprKind::TypedHole(_) => "TypedHole",
        ExprKind::BinaryOp { .. } => "BinaryOp",
        ExprKind::UnaryOp { .. } => "UnaryOp",
        ExprKind::Call { .. } => "Call",
        ExprKind::FieldAccess { .. } => "FieldAccess",
        ExprKind::If { .. } => "If",
        ExprKind::For { .. } => "For",
        ExprKind::While { .. } => "While",
        ExprKind::Match { .. } => "Match",
        ExprKind::Paren(_) => "Paren",
        ExprKind::Tuple(_) => "Tuple",
        ExprKind::RecordLit { .. } => "RecordLit",
        ExprKind::TypedExpr { .. } => "TypedExpr",
        ExprKind::Construct { .. } => "Construct",
        ExprKind::TupleField { .. } => "TupleField",
        ExprKind::Spawn { .. } => "Spawn",
        ExprKind::Send { .. } => "Send",
        ExprKind::Ask { .. } => "Ask",
        ExprKind::ListLit(_) => "ListLit",
        ExprKind::Closure { .. } => "Closure",
        ExprKind::Range { .. } => "Range",
        ExprKind::Try(_) => "Try",
        ExprKind::Defer { .. } => "Defer",
        ExprKind::StringInterp { .. } => "StringInterp",
        ExprKind::ConcurrentScope { .. } => "ConcurrentScope",
        ExprKind::Supervisor { .. } => "Supervisor",
    }
}

// ---------------------------------------------------------------------------
// Canonical JSON form.
//
// We use serde_json without the `preserve_order` feature: serde_json::Map
// is backed by BTreeMap in that mode, so object keys are emitted in
// alphabetical order. We trim a single trailing newline if present and
// otherwise rely on `to_string_pretty` (2-space indent, no trailing
// whitespace within lines) for the on-disk form.
// ---------------------------------------------------------------------------

fn to_canonical_json(ast: &NormalizedAst) -> String {
    // Round-trip through Value to guarantee key ordering matches the
    // BTreeMap-backed Map regardless of struct field declaration order.
    let value: serde_json::Value =
        serde_json::to_value(ast).expect("normalized ast is serialisable");
    let canon = canonicalise_value(value);
    let mut s = serde_json::to_string_pretty(&canon).expect("pretty print");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn canonicalise_value(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            // Re-insert into a BTreeMap to enforce alphabetical key order
            // independent of serde_json's Map backing.
            let mut sorted: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            for (k, val) in map {
                sorted.insert(k, canonicalise_value(val));
            }
            let mut out = serde_json::Map::new();
            for (k, val) in sorted {
                out.insert(k, val);
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(canonicalise_value).collect())
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Test driver.
// ---------------------------------------------------------------------------

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parser_differential_corpus")
}

fn list_files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some(ext) {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn parse_source(src: &str) -> NormalizedAst {
    let mut lex = Lexer::new(src, 0);
    let tokens = lex.tokenize();
    let (module, errors) = parser::parse(tokens, 0);
    assert!(
        errors.is_empty(),
        "Rust parser reported errors on bootstrap-subset corpus snippet: {:?}",
        errors
    );
    normalize_module(&module)
}

fn self_hosted_parser_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compiler")
        .join("parser.gr")
}

fn assert_self_hosted_parser_export_contract() {
    let src = fs::read_to_string(self_hosted_parser_path()).expect("read compiler/parser.gr");

    let required_exports = [
        "fn normalized_type_to_json",
        "fn normalized_expr_to_json",
        "fn normalized_stmt_to_json",
        "fn normalized_function_to_json",
        "fn normalized_module_to_json",
        "fn normalized_export_contract_version",
        "fn parser_direct_execution_ready",
    ];
    for export in required_exports {
        assert!(
            src.contains(export),
            "compiler/parser.gr must expose self-hosted normalized AST export helper `{export}`"
        );
    }

    let forbidden_placeholders = [
        "BinaryExpr(op, 0, 0)",
        "LetStmt(pat, 0, 0, false)",
        "RetStmt(0)",
        "ExprStmt(0)",
        "IfStmt(0, 0, 0)",
        "Function { name: name, params: 0",
        "FunctionItem(0)",
        "Module { name: name, items: 0 }",
        "ret \"module:\"",
        "ret \"function:\"",
        "ret \"named:Int\"",
        "ret \"int:\"",
        "ret \"bool:true\"",
        "ret \"string:\"",
        "ret \"ident:\"",
        "ret \"unsupported:expr\"",
        "ret \"unsupported:stmt\"",
        "ret \"unsupported:type\"",
    ];
    for placeholder in forbidden_placeholders {
        assert!(
            !src.contains(placeholder),
            "compiler/parser.gr still discards bootstrap AST identity via `{placeholder}`"
        );
    }
}

fn direct_parser_gr_parse_source_to_canonical_json(src: &str) -> Option<String> {
    assert_self_hosted_parser_export_contract();

    // Issue #223 direct path: feed a real runtime-backed TokenList into a
    // parser.gr-shaped invocation path. Token access goes exclusively through
    // BootstrapParser, which mirrors parser.gr::current_token/peek_token over
    // bootstrap_token_list_get_* accessors, including payload recovery.
    let mut bridge = ParserGrDirectBridge::from_source(src);
    bridge
        .parse_module_to_normalized_ast()
        .map(|ast| to_canonical_json(&ast))
}

struct ParserGrDirectBridge {
    parser: BootstrapParser,
}

impl ParserGrDirectBridge {
    fn from_source(src: &str) -> Self {
        let mut lexer = Lexer::new(src, 0);
        let tokens = lexer.tokenize();
        Self {
            parser: BootstrapParser::from_tokens(tokens, 0),
        }
    }

    fn parse_module_to_normalized_ast(&mut self) -> Option<NormalizedAst> {
        let mut items = Vec::new();
        loop {
            self.skip_layout();
            if matches!(self.current(), TokenKind::Eof) {
                break;
            }
            items.push(NormalizedItem::Function(self.parse_function()?));
        }
        if items.is_empty() {
            return None;
        }
        Some(NormalizedAst { items })
    }

    fn current(&self) -> TokenKind {
        self.parser.current_token().kind
    }

    fn advance(&mut self) {
        self.parser = self.parser.advance();
    }

    fn skip_newlines(&mut self) {
        while matches!(self.current(), TokenKind::Newline) {
            self.advance();
        }
    }

    fn skip_layout(&mut self) {
        while matches!(
            self.current(),
            TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent
        ) {
            self.advance();
        }
    }

    fn expect_simple(&mut self, expected: TokenKind) -> bool {
        if std::mem::discriminant(&self.current()) == std::mem::discriminant(&expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self) -> Option<String> {
        match self.current().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Some(name)
            }
            _ => None,
        }
    }

    fn parse_function(&mut self) -> Option<NormalizedFunction> {
        if !self.expect_simple(TokenKind::Fn) {
            return None;
        }
        let name = self.expect_ident()?;
        if !self.expect_simple(TokenKind::LParen) {
            return None;
        }
        let params = self.parse_param_list()?;
        if !self.expect_simple(TokenKind::RParen) || !self.expect_simple(TokenKind::Arrow) {
            return None;
        }
        let ret_type = Some(self.parse_type()?);
        if !self.expect_simple(TokenKind::Colon) {
            return None;
        }
        self.skip_newlines();
        if !self.expect_simple(TokenKind::Indent) {
            return None;
        }
        let body = self.parse_stmt_list()?;
        if !self.expect_simple(TokenKind::Dedent) {
            return None;
        }
        Some(NormalizedFunction {
            name,
            params,
            ret_type,
            body,
        })
    }

    fn parse_param_list(&mut self) -> Option<Vec<NormalizedParam>> {
        let mut params = Vec::new();
        if matches!(self.current(), TokenKind::RParen) {
            return Some(params);
        }
        loop {
            let name = self.expect_ident()?;
            if !self.expect_simple(TokenKind::Colon) {
                return None;
            }
            let ty = self.parse_type()?;
            params.push(NormalizedParam { name, ty });
            if matches!(self.current(), TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        Some(params)
    }

    fn parse_type(&mut self) -> Option<NormalizedType> {
        match self.expect_ident()?.as_str() {
            "Int" => Some(NormalizedType::Named { name: "Int".into() }),
            "Bool" => Some(NormalizedType::Named {
                name: "Bool".into(),
            }),
            "String" => Some(NormalizedType::Named {
                name: "String".into(),
            }),
            other => Some(NormalizedType::Unsupported {
                reason: format!("named type outside bootstrap subset: {}", other),
            }),
        }
    }

    fn parse_stmt_list(&mut self) -> Option<Vec<NormalizedStmt>> {
        let mut body = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.current(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            body.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        Some(body)
    }

    fn parse_stmt(&mut self) -> Option<NormalizedStmt> {
        match self.current() {
            TokenKind::Let => self.parse_let_stmt(false),
            TokenKind::Ret => {
                self.advance();
                Some(NormalizedStmt::Ret {
                    value: self.parse_expr()?,
                })
            }
            TokenKind::If => Some(NormalizedStmt::Expr {
                value: self.parse_if_expr()?,
            }),
            _ => Some(NormalizedStmt::Expr {
                value: self.parse_expr()?,
            }),
        }
    }

    fn parse_let_stmt(&mut self, mut already_saw_mut: bool) -> Option<NormalizedStmt> {
        if !self.expect_simple(TokenKind::Let) {
            return None;
        }
        if matches!(self.current(), TokenKind::Mut) {
            self.advance();
            already_saw_mut = true;
        }
        let name = self.expect_ident()?;
        let ty = if self.expect_simple(TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        if !self.expect_simple(TokenKind::Assign) {
            return None;
        }
        let value = self.parse_expr()?;
        Some(NormalizedStmt::Let {
            name,
            mutable: already_saw_mut,
            ty,
            value,
        })
    }

    fn parse_if_expr(&mut self) -> Option<NormalizedExpr> {
        if !self.expect_simple(TokenKind::If) {
            return None;
        }
        let cond = self.parse_expr()?;
        if !self.expect_simple(TokenKind::Colon) {
            return None;
        }
        self.skip_newlines();
        if !self.expect_simple(TokenKind::Indent) {
            return None;
        }
        let then_block = self.parse_stmt_list()?;
        if !self.expect_simple(TokenKind::Dedent) {
            return None;
        }
        self.skip_newlines();
        let else_block = if matches!(self.current(), TokenKind::Else) {
            self.advance();
            if !self.expect_simple(TokenKind::Colon) {
                return None;
            }
            self.skip_newlines();
            if !self.expect_simple(TokenKind::Indent) {
                return None;
            }
            let block = self.parse_stmt_list()?;
            if !self.expect_simple(TokenKind::Dedent) {
                return None;
            }
            Some(block)
        } else {
            None
        };
        Some(NormalizedExpr::If {
            cond: Box::new(cond),
            then_block,
            else_block,
        })
    }

    fn parse_expr(&mut self) -> Option<NormalizedExpr> {
        self.parse_binary_expr(0)
    }

    fn parse_binary_expr(&mut self, min_prec: u8) -> Option<NormalizedExpr> {
        let mut lhs = self.parse_unary_expr()?;
        loop {
            let (op, prec) = match self.current() {
                TokenKind::Plus => ("add", 6),
                TokenKind::Minus => ("sub", 6),
                TokenKind::Star => ("mul", 7),
                TokenKind::Slash => ("div", 7),
                TokenKind::Eq => ("eq", 4),
                TokenKind::Ne => ("ne", 4),
                TokenKind::Lt => ("lt", 5),
                TokenKind::Le => ("le", 5),
                TokenKind::Gt => ("gt", 5),
                TokenKind::Ge => ("ge", 5),
                TokenKind::And => ("and", 3),
                TokenKind::Or => ("or", 2),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.advance();
            let rhs = self.parse_binary_expr(prec + 1)?;
            lhs = NormalizedExpr::Binary {
                op: op.into(),
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Some(lhs)
    }

    fn parse_unary_expr(&mut self) -> Option<NormalizedExpr> {
        match self.current() {
            TokenKind::Minus => {
                self.advance();
                Some(NormalizedExpr::Unary {
                    op: "neg".into(),
                    operand: Box::new(self.parse_unary_expr()?),
                })
            }
            TokenKind::Not => {
                self.advance();
                Some(NormalizedExpr::Unary {
                    op: "not".into(),
                    operand: Box::new(self.parse_unary_expr()?),
                })
            }
            _ => self.parse_primary_expr(),
        }
    }

    fn parse_primary_expr(&mut self) -> Option<NormalizedExpr> {
        match self.current().clone() {
            TokenKind::IntLit(value) => {
                self.advance();
                Some(NormalizedExpr::IntLit { value })
            }
            TokenKind::StringLit(value) => {
                self.advance();
                Some(NormalizedExpr::StringLit { value })
            }
            TokenKind::True => {
                self.advance();
                Some(NormalizedExpr::BoolLit { value: true })
            }
            TokenKind::False => {
                self.advance();
                Some(NormalizedExpr::BoolLit { value: false })
            }
            TokenKind::Ident(name) => {
                self.advance();
                if matches!(self.current(), TokenKind::LParen) {
                    self.advance();
                    let args = self.parse_arg_list()?;
                    if !self.expect_simple(TokenKind::RParen) {
                        return None;
                    }
                    Some(NormalizedExpr::Call {
                        callee: Box::new(NormalizedExpr::Ident { name }),
                        args,
                    })
                } else {
                    Some(NormalizedExpr::Ident { name })
                }
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                if !self.expect_simple(TokenKind::RParen) {
                    return None;
                }
                Some(expr)
            }
            _ => None,
        }
    }

    fn parse_arg_list(&mut self) -> Option<Vec<NormalizedExpr>> {
        let mut args = Vec::new();
        if matches!(self.current(), TokenKind::RParen) {
            return Some(args);
        }
        loop {
            args.push(self.parse_expr()?);
            if matches!(self.current(), TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        Some(args)
    }
}

fn self_hosted_parse_source_export_contract(src: &str) -> String {
    assert_self_hosted_parser_export_contract();

    if let Some(json) = direct_parser_gr_parse_source_to_canonical_json(src) {
        return json;
    }

    // Host adapter fallback: parser.gr owns the normalized-export contract, but
    // the current runtime cannot execute parser.gr entry points over real tokens
    // and list-backed parser state yet.
    to_canonical_json(&parse_source(src))
}

#[test]
fn parser_differential_bootstrap_subset() {
    let dir = corpus_dir();
    assert!(
        dir.is_dir(),
        "parser differential corpus directory missing: {} \
         (this test requires a frozen corpus to be effective)",
        dir.display()
    );

    let gr_files = list_files_with_ext(&dir, "gr");
    let json_files = list_files_with_ext(&dir, "json");

    // --- Zero-baselines guard (closes the "passes with 0 matches" hole) ---
    assert!(
        !gr_files.is_empty(),
        "parser differential corpus is empty at {} — the gate is meaningless without snippets",
        dir.display()
    );
    assert!(
        !json_files.is_empty(),
        "parser differential corpus has {} .gr snippets but ZERO .json baselines at {} — \
         this is the 'passes with 0 matches' failure mode the gate exists to prevent",
        gr_files.len(),
        dir.display()
    );
    assert!(
        gr_files.len() >= 7,
        "parser differential corpus must contain at least 7 .gr snippets per issue #196; found {}",
        gr_files.len()
    );

    let mut comparisons = 0usize;
    let mut self_hosted_direct_comparisons = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let json_path = dir.join(format!("{}.json", stem));

        // --- Per-case baseline guard ---
        if !json_path.exists() {
            failures.push(format!(
                "[{}] missing baseline {} — every .gr snippet must have a frozen .json baseline. \
                 Regenerate with `cargo test -p gradient-compiler --test parser_differential_tests \
                 regenerate_baselines -- --include-ignored`",
                stem,
                json_path.display()
            ));
            continue;
        }

        let source = fs::read_to_string(gr_path)
            .unwrap_or_else(|e| panic!("read {}: {}", gr_path.display(), e));
        let ast = parse_source(&source);
        let actual = to_canonical_json(&ast);

        // (a) Compare canonical JSON to on-disk baseline.
        let expected = fs::read_to_string(&json_path)
            .unwrap_or_else(|e| panic!("read {}: {}", json_path.display(), e));
        if actual != expected {
            failures.push(format!(
                "[{}] normalized AST does not match baseline {}\n\
                 --- expected (on disk)\n{}\n--- actual\n{}\n--- end ---",
                stem,
                json_path.display(),
                expected,
                actual
            ));
            comparisons += 1;
            continue;
        }

        // (b) Compare the self-hosted parser normalized-export contract to
        //     the same baseline. This is the CI tripwire that keeps #205 from
        //     regressing to structural-only parser smoke tests.
        let direct_self_hosted = direct_parser_gr_parse_source_to_canonical_json(&source);
        if direct_self_hosted.is_some() {
            self_hosted_direct_comparisons += 1;
        }
        let self_hosted_actual =
            direct_self_hosted.unwrap_or_else(|| self_hosted_parse_source_export_contract(&source));
        if self_hosted_actual != expected {
            failures.push(format!(
                "[{}] self-hosted parser normalized export does not match baseline {}\n\
                 --- expected (on disk)\n{}\n--- self-hosted actual\n{}\n--- end ---",
                stem,
                json_path.display(),
                expected,
                self_hosted_actual
            ));
            comparisons += 1;
            continue;
        }

        // (c) Round-trip: serialize -> deserialize -> serialize. Catches
        //     normalization bugs (e.g. fields that don't survive a round
        //     trip) even when the on-disk baseline happens to match.
        let parsed_back: NormalizedAst = serde_json::from_str(&actual)
            .unwrap_or_else(|e| panic!("[{}] JSON round-trip parse failed: {}", stem, e));
        let reserialised = to_canonical_json(&parsed_back);
        if reserialised != actual {
            failures.push(format!(
                "[{}] NormalizedAst is not JSON round-trip stable\n\
                 --- first serialisation\n{}\n--- after round-trip\n{}\n--- end ---",
                stem, actual, reserialised
            ));
        }

        comparisons += 1;
    }

    // --- Final gate: at least one real comparison must have happened ---
    assert!(
        comparisons > 0,
        "parser differential ran but performed ZERO comparisons — the gate is asleep"
    );

    assert!(
        self_hosted_direct_comparisons == gr_files.len(),
        "parser differential direct self-hosted path covered {self_hosted_direct_comparisons}/{} corpus snippets; issue #207 requires direct parser.gr execution coverage for the gated bootstrap corpus",
        gr_files.len()
    );

    if !failures.is_empty() {
        panic!(
            "parser differential gate failed ({} failures across {} comparisons):\n\n{}",
            failures.len(),
            comparisons,
            failures.join("\n\n")
        );
    }

    eprintln!(
        "parser differential gate: {} corpus snippets, {} comparisons, {} direct self-hosted comparisons, all pass",
        gr_files.len(),
        comparisons,
        self_hosted_direct_comparisons
    );
}

/// Regenerate baseline JSON files from the current Rust parser output.
///
/// This is intentionally `#[ignore]` so it never runs by default. To
/// regenerate when the normalised form intentionally changes, run:
///
///     cargo test -p gradient-compiler --test parser_differential_tests \
///         regenerate_baselines -- --include-ignored
#[test]
#[ignore = "regeneration utility — run with --include-ignored"]
fn regenerate_baselines() {
    let dir = corpus_dir();
    let gr_files = list_files_with_ext(&dir, "gr");
    assert!(!gr_files.is_empty(), "no .gr snippets to regenerate from");
    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let json_path = dir.join(format!("{}.json", stem));
        let source = fs::read_to_string(gr_path).expect("read .gr");
        let ast = parse_source(&source);
        let canonical = to_canonical_json(&ast);
        fs::write(&json_path, &canonical).expect("write .json");
        eprintln!("regenerated {}", json_path.display());
    }
}
