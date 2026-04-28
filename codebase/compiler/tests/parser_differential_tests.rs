//! Parser differential test — bootstrap subset gate (issue #196).
//!
//! This integration test enforces parity for the small "bootstrap subset" of
//! Gradient that the self-hosted parser must round-trip. Today the
//! self-hosted parser does not yet emit a NormalizedAst, so the gate is
//! anchored by Rust↔Rust round-trip baselines: each `.gr` snippet in the
//! corpus has a frozen `.json` baseline, and the test asserts:
//!
//!   1. The on-disk corpus is non-empty AND every `.gr` has a matching
//!      `.json` baseline (closes the "passes with 0 matches" hole).
//!   2. Parsing the snippet through the Rust parser, normalizing, and
//!      serializing to canonical JSON exactly matches the on-disk baseline.
//!   3. The in-memory `NormalizedAst` round-trips through JSON
//!      (serialize → deserialize → serialize) without changing.
//!
//! When the normalized form intentionally changes, regenerate baselines:
//!   cargo test -p gradient-compiler --test parser_differential_tests \
//!       regenerate_baselines -- --include-ignored
//!
//! Bootstrap subset (do not extend without updating the issue):
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
use gradient_compiler::lexer::Lexer;
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
    IntLit { value: i64 },
    BoolLit { value: bool },
    StringLit { value: String },
    Ident { name: String },
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
            reason: format!("item kind outside bootstrap subset: {}", item_kind_name(other)),
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
        TypeExpr::Named { name, cap: None } if matches!(name.as_str(), "Int" | "Bool" | "String") => {
            NormalizedType::Named { name: name.clone() }
        }
        TypeExpr::Named { name, cap } => NormalizedType::Unsupported {
            reason: format!(
                "named type outside bootstrap subset: {}{}",
                name,
                cap.as_ref().map(|c| format!(" (cap={})", c)).unwrap_or_default()
            ),
        },
        TypeExpr::Unit => NormalizedType::Unsupported { reason: "unit type".into() },
        TypeExpr::Fn { .. } => NormalizedType::Unsupported { reason: "fn type".into() },
        TypeExpr::Generic { name, .. } => NormalizedType::Unsupported {
            reason: format!("generic type {}", name),
        },
        TypeExpr::Tuple(_) => NormalizedType::Unsupported { reason: "tuple type".into() },
        TypeExpr::Record(_) => NormalizedType::Unsupported { reason: "record type".into() },
        TypeExpr::Linear(_) => NormalizedType::Unsupported { reason: "linear type".into() },
        TypeExpr::Type => NormalizedType::Unsupported { reason: "type-of-types".into() },
    }
}

fn normalize_block(b: &Block) -> Vec<NormalizedStmt> {
    b.node.iter().map(normalize_stmt).collect()
}

fn normalize_stmt(s: &Stmt) -> NormalizedStmt {
    match &s.node {
        StmtKind::Let { name, type_ann, value, mutable } => NormalizedStmt::Let {
            name: name.clone(),
            mutable: *mutable,
            ty: type_ann.as_ref().map(|t| normalize_type(&t.node)),
            value: normalize_expr(value),
        },
        StmtKind::Ret(e) => NormalizedStmt::Ret { value: normalize_expr(e) },
        StmtKind::Expr(e) => NormalizedStmt::Expr { value: normalize_expr(e) },
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
        ExprKind::If { condition, then_block, else_ifs, else_block } => {
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
            reason: format!("expression outside bootstrap subset: {}", expr_kind_name(other)),
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
    let value: serde_json::Value = serde_json::to_value(ast).expect("normalized ast is serialisable");
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

        // (b) Round-trip: serialize -> deserialize -> serialize. Catches
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

    if !failures.is_empty() {
        panic!(
            "parser differential gate failed ({} failures across {} comparisons):\n\n{}",
            failures.len(),
            comparisons,
            failures.join("\n\n")
        );
    }

    eprintln!(
        "parser differential gate: {} corpus snippets, {} comparisons, all pass",
        gr_files.len(),
        comparisons
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
