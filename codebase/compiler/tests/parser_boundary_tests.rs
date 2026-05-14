//! Parser boundary corpus gate (issue #224).
//!
//! Companion to `parser_differential_tests.rs`. The differential gate locks
//! the bootstrap subset; this gate locks the *boundary* — constructs that the
//! Rust parser accepts today but that fall outside the bootstrap subset the
//! self-hosted parser must reproduce. Each frozen `.json` baseline records
//! the exact `Unsupported { reason: ... }` shape produced by normalising the
//! Rust AST. The gate exists to:
//!
//!   1. Catch regressions where parser changes silently move the boundary
//!      (e.g. a feature starts producing parse errors, or its
//!      ExprKind/StmtKind/ItemKind changes name).
//!   2. Document and track the unsupported surface area without silently
//!      skipping snippets the way #196's bootstrap gate has to.
//!   3. Keep the differential corpus narrowly focused on the supported
//!      bootstrap subset while still expanding parity-relevant coverage.
//!
//! When the boundary intentionally moves (e.g. a feature joins the
//! bootstrap subset, or an unsupported reason text changes), regenerate
//! baselines with:
//!
//!   cargo test -p gradient-compiler --test parser_boundary_tests \
//!       regenerate_boundary_baselines -- --include-ignored
//!
//! Boundary corpus contract (do not relax silently):
//!   - Every snippet MUST parse without errors through the Rust parser.
//!   - Every snippet MUST produce at least one `Unsupported` reason somewhere
//!     in its normalized AST (otherwise it belongs in the differential
//!     bootstrap corpus instead).
//!   - Each `.gr` must have a frozen `.json` baseline.
//!   - Round-trip JSON serialise → deserialise → serialise must be stable.

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
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// NormalizedAst — same shape and rules as parser_differential_tests.rs.
// Duplicated intentionally so the boundary gate stays self-contained and
// changes to one cannot silently affect the other. Any normalisation drift
// between the two tests is itself a useful CI signal.
// ---------------------------------------------------------------------------

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
    Named { name: String },
    Unsupported { reason: String },
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
        final_expr: Option<Box<NormalizedExpr>>,
    },
    Unsupported {
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Normalisation pass — must stay byte-identical to
// parser_differential_tests.rs's version. Any divergence will produce
// noticeable diffs across both gates simultaneously.
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

fn to_canonical_json(ast: &NormalizedAst) -> String {
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

fn boundary_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parser_boundary_corpus")
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

fn parse_source_no_errors(src: &str, label: &str) -> NormalizedAst {
    let mut lex = Lexer::new(src, 0);
    let tokens = lex.tokenize();
    let (module, errors) = parser::parse(tokens, 0);
    assert!(
        errors.is_empty(),
        "[{}] boundary corpus snippet must parse without errors; got: {:?}",
        label,
        errors
    );
    normalize_module(&module)
}

/// True if any node in `ast` carries an `Unsupported` variant.
fn ast_records_unsupported(ast: &NormalizedAst) -> bool {
    fn in_expr(e: &NormalizedExpr) -> bool {
        match e {
            NormalizedExpr::Unsupported { .. } => true,
            NormalizedExpr::Binary { left, right, .. } => in_expr(left) || in_expr(right),
            NormalizedExpr::Unary { operand, .. } => in_expr(operand),
            NormalizedExpr::Call { callee, args } => in_expr(callee) || args.iter().any(in_expr),
            NormalizedExpr::If {
                cond,
                then_block,
                else_block,
            } => {
                in_expr(cond)
                    || then_block.iter().any(in_stmt)
                    || else_block
                        .as_ref()
                        .map(|b| b.iter().any(in_stmt))
                        .unwrap_or(false)
            }
            NormalizedExpr::Block { stmts, final_expr } => {
                stmts.iter().any(in_stmt) || final_expr.as_deref().map(in_expr).unwrap_or(false)
            }
            _ => false,
        }
    }
    fn in_stmt(s: &NormalizedStmt) -> bool {
        match s {
            NormalizedStmt::Unsupported { .. } => true,
            NormalizedStmt::Let { ty, value, .. } => {
                ty.as_ref()
                    .map(|t| matches!(t, NormalizedType::Unsupported { .. }))
                    .unwrap_or(false)
                    || in_expr(value)
            }
            NormalizedStmt::Ret { value } => in_expr(value),
            NormalizedStmt::Expr { value } => in_expr(value),
        }
    }
    ast.items.iter().any(|item| match item {
        NormalizedItem::Unsupported { .. } => true,
        NormalizedItem::Function(f) => {
            f.body.iter().any(in_stmt)
                || f.ret_type
                    .as_ref()
                    .map(|t| matches!(t, NormalizedType::Unsupported { .. }))
                    .unwrap_or(false)
                || f.params
                    .iter()
                    .any(|p| matches!(p.ty, NormalizedType::Unsupported { .. }))
        }
    })
}

#[test]
fn parser_boundary_corpus_locks_unsupported_surface() {
    let dir = boundary_corpus_dir();
    assert!(
        dir.is_dir(),
        "parser boundary corpus directory missing: {} \
         (this gate requires a frozen boundary corpus to be effective)",
        dir.display()
    );

    let gr_files = list_files_with_ext(&dir, "gr");
    let json_files = list_files_with_ext(&dir, "json");

    assert!(
        !gr_files.is_empty(),
        "parser boundary corpus is empty at {} — the gate is meaningless without snippets",
        dir.display()
    );
    assert!(
        !json_files.is_empty(),
        "parser boundary corpus has {} .gr snippets but ZERO .json baselines at {} — \
         this is the 'passes with 0 matches' failure mode the gate exists to prevent",
        gr_files.len(),
        dir.display()
    );
    assert!(
        gr_files.len() >= 10,
        "parser boundary corpus must contain at least 10 .gr snippets per issue #224; found {}",
        gr_files.len()
    );

    let mut comparisons = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let json_path = dir.join(format!("{}.json", stem));

        if !json_path.exists() {
            failures.push(format!(
                "[{}] missing baseline {} — every boundary .gr snippet must have a frozen \
                 .json baseline. Regenerate with `cargo test -p gradient-compiler \
                 --test parser_boundary_tests regenerate_boundary_baselines -- --include-ignored`",
                stem,
                json_path.display()
            ));
            continue;
        }

        let source = fs::read_to_string(gr_path)
            .unwrap_or_else(|e| panic!("read {}: {}", gr_path.display(), e));
        let ast = parse_source_no_errors(&source, &stem);
        let actual = to_canonical_json(&ast);

        // (a) Boundary contract: snippet MUST record at least one Unsupported
        // somewhere in its normalized AST. If it doesn't, it should live in
        // the differential bootstrap corpus instead.
        if !ast_records_unsupported(&ast) {
            failures.push(format!(
                "[{}] boundary snippet does not produce any `Unsupported` node — \
                 if the construct is now supported, move the snippet into \
                 parser_differential_corpus/ instead. Normalized JSON was:\n{}",
                stem, actual
            ));
            comparisons += 1;
            continue;
        }

        // (b) Compare canonical JSON to on-disk baseline.
        let expected = fs::read_to_string(&json_path)
            .unwrap_or_else(|e| panic!("read {}: {}", json_path.display(), e));
        if actual != expected {
            failures.push(format!(
                "[{}] boundary normalized AST does not match baseline {}\n\
                 --- expected (on disk)\n{}\n--- actual\n{}\n--- end ---",
                stem,
                json_path.display(),
                expected,
                actual
            ));
            comparisons += 1;
            continue;
        }

        // (c) Round-trip stability.
        let parsed_back: NormalizedAst = serde_json::from_str(&actual)
            .unwrap_or_else(|e| panic!("[{}] JSON round-trip parse failed: {}", stem, e));
        let reserialised = to_canonical_json(&parsed_back);
        if reserialised != actual {
            failures.push(format!(
                "[{}] boundary NormalizedAst is not JSON round-trip stable\n\
                 --- first serialisation\n{}\n--- after round-trip\n{}\n--- end ---",
                stem, actual, reserialised
            ));
        }

        comparisons += 1;
    }

    assert!(
        comparisons > 0,
        "parser boundary gate ran but performed ZERO comparisons — the gate is asleep"
    );

    if !failures.is_empty() {
        panic!(
            "parser boundary gate failed ({} failures across {} comparisons):\n\n{}",
            failures.len(),
            comparisons,
            failures.join("\n\n")
        );
    }

    eprintln!(
        "parser boundary gate: {} corpus snippets, {} comparisons, all pass",
        gr_files.len(),
        comparisons
    );
}

/// Regenerate boundary baseline JSON files from the current Rust parser
/// output. Ignored by default. Run with --include-ignored when the boundary
/// reasons intentionally change.
#[test]
#[ignore = "regeneration utility — run with --include-ignored"]
fn regenerate_boundary_baselines() {
    let dir = boundary_corpus_dir();
    let gr_files = list_files_with_ext(&dir, "gr");
    assert!(
        !gr_files.is_empty(),
        "no boundary .gr snippets to regenerate from"
    );
    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let json_path = dir.join(format!("{}.json", stem));
        let source = fs::read_to_string(gr_path).expect("read .gr");
        let ast = parse_source_no_errors(&source, &stem);
        let canonical = to_canonical_json(&ast);
        fs::write(&json_path, &canonical).expect("write .json");
        eprintln!("regenerated {}", json_path.display());
    }
}
