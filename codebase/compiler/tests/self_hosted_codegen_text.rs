//! Issue #229: textual codegen/emission slice — golden text gate.
//!
//! With #227 the self-hosted IR builder lowers AST -> runtime-backed IR via
//! `bootstrap_ir_*` externs, and #228 locked down the structural IR shape
//! through a JSON differential gate. This test closes the next link in the
//! pipeline: take the same lowered IR and emit canonical textual IR via the
//! Rust kernel slice (`bootstrap_ir_emit::bootstrap_ir_emit_text`). The
//! emitted text is the first executable codegen output the self-hosted
//! compiler can produce end-to-end.
//!
//! Boundary contract: the self-hosted compiler holds the lowering logic
//! (`compiler/ir_builder.gr`); the Rust kernel only walks the runtime IR
//! store and emits text. The kernel never touches AST, parser, or the
//! type-checker. When the Gradient runtime can execute emission natively
//! the kernel's footprint here can shrink to zero — the text format and
//! corpus stay the same.
//!
//! What this gate locks down:
//!
//!   1. The on-disk corpus is non-empty AND every `.gr` snippet has a
//!      matching `.txt` baseline (closes the "passes with 0 matches" hole).
//!   2. The textual IR for each snippet exactly matches its frozen baseline.
//!   3. Each emission is non-empty (no placeholder "" output for supported
//!      fixtures — the issue's primary acceptance criterion).
//!   4. Each emitted module starts with `module <name>` and contains at
//!      least one `fn ` declaration ending in `:` followed by an indented
//!      `entry:` block. Stops empty/placeholder regressions even if a
//!      baseline drifts.
//!   5. Round-trip stability: emitting twice (after re-lowering the same
//!      AST) produces byte-identical output.
//!
//! Companion gates:
//! - `ir_differential_tests`: structural JSON shape (#228).
//! - `self_hosted_ir_builder`: runtime-backed IR contract (#227).
//! - parser/checker differential gates upstream.
//!
//! Regenerate baselines (only when the textual format intentionally moves):
//!
//!     cargo test -p gradient-compiler --test self_hosted_codegen_text \
//!         regenerate_codegen_text_baselines -- --include-ignored

#![allow(clippy::uninlined_format_args)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use gradient_compiler::ast::{
    expr::{BinOp, Expr, ExprKind, UnaryOp},
    item::{FnDef, ItemKind},
    module::Module,
    stmt::{Stmt, StmtKind},
    types::TypeExpr,
};
use gradient_compiler::bootstrap_ir_bridge::{
    bootstrap_ir_block_alloc, bootstrap_ir_block_append_instr, bootstrap_ir_function_alloc,
    bootstrap_ir_function_append_block, bootstrap_ir_function_append_param,
    bootstrap_ir_instr_alloc, bootstrap_ir_list_append, bootstrap_ir_module_alloc,
    bootstrap_ir_module_append_function, bootstrap_ir_module_set_entry, bootstrap_ir_param_alloc,
    bootstrap_ir_type_alloc_named, bootstrap_ir_type_alloc_primitive,
    bootstrap_ir_value_alloc_const_bool, bootstrap_ir_value_alloc_const_int,
    bootstrap_ir_value_alloc_error, bootstrap_ir_value_alloc_global,
    bootstrap_ir_value_alloc_param, bootstrap_ir_value_alloc_register,
    bootstrap_ir_value_list_alloc, reset_ir_store, IrInstrTag, IrTypeTag,
};
use gradient_compiler::bootstrap_ir_emit::bootstrap_ir_emit_text;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

fn parity_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("ir_differential_corpus")
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

// ---------------------------------------------------------------------------
// Self-hosted lowering driver. Mirrors `compiler/ir_builder.gr`'s
// `lower_module` / `lower_function` / `lower_stmt` / `lower_expr` exactly,
// over a Rust ast::Module — same adapter approach as ir_differential_tests.
// Kept inline so this gate stays self-contained and is unaffected by future
// refactors to the JSON gate's adapter.
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct LoweringScope {
    bindings: Vec<(String, i64)>,
}

impl LoweringScope {
    fn define(&mut self, name: &str, value_id: i64) {
        self.bindings.push((name.to_string(), value_id));
    }
    fn lookup(&self, name: &str) -> i64 {
        for (n, v) in self.bindings.iter().rev() {
            if n == name {
                return *v;
            }
        }
        0
    }
}

fn lower_type_expr(t: &TypeExpr) -> i64 {
    match t {
        TypeExpr::Named { name, .. } => match name.as_str() {
            "Int" => bootstrap_ir_type_alloc_primitive(IrTypeTag::I64 as i64),
            "Bool" => bootstrap_ir_type_alloc_primitive(IrTypeTag::Bool as i64),
            "Float" => bootstrap_ir_type_alloc_primitive(IrTypeTag::F64 as i64),
            "String" => bootstrap_ir_type_alloc_named(name),
            other => bootstrap_ir_type_alloc_named(other),
        },
        TypeExpr::Unit => bootstrap_ir_type_alloc_primitive(IrTypeTag::Unit as i64),
        _ => bootstrap_ir_type_alloc_primitive(IrTypeTag::I64 as i64),
    }
}

fn binop_to_instr_tag(op: BinOp) -> i64 {
    match op {
        BinOp::Add => IrInstrTag::Add as i64,
        BinOp::Sub => IrInstrTag::Sub as i64,
        BinOp::Mul => IrInstrTag::Mul as i64,
        BinOp::Div => IrInstrTag::SDiv as i64,
        BinOp::Eq => IrInstrTag::ICmpEq as i64,
        BinOp::Ne => IrInstrTag::ICmpNe as i64,
        BinOp::Lt => IrInstrTag::ICmpSLt as i64,
        BinOp::Le => IrInstrTag::ICmpSLe as i64,
        BinOp::Gt => IrInstrTag::ICmpSGt as i64,
        BinOp::Ge => IrInstrTag::ICmpSGe as i64,
        BinOp::And => IrInstrTag::And as i64,
        BinOp::Or => IrInstrTag::Or as i64,
        BinOp::Mod | BinOp::Pipe => 0,
    }
}

fn binop_is_compare(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
    )
}

fn lower_expr(e: &Expr, scope: &LoweringScope, block_id: i64, fallback_ty: i64) -> i64 {
    match &e.node {
        ExprKind::IntLit(n) => bootstrap_ir_value_alloc_const_int(fallback_ty, *n),
        ExprKind::BoolLit(b) => bootstrap_ir_value_alloc_const_bool(if *b { 1 } else { 0 }),
        ExprKind::Ident(name) => {
            let bound = scope.lookup(name);
            if bound != 0 {
                bound
            } else {
                bootstrap_ir_value_alloc_global(name, fallback_ty)
            }
        }
        ExprKind::BinaryOp { op, left, right } => {
            let left_val = lower_expr(left, scope, block_id, fallback_ty);
            let right_val = lower_expr(right, scope, block_id, fallback_ty);
            let instr_tag = binop_to_instr_tag(*op);
            if instr_tag == 0 {
                return bootstrap_ir_value_alloc_error("unsupported binary op");
            }
            let used_ty = if binop_is_compare(*op) {
                bootstrap_ir_type_alloc_primitive(IrTypeTag::Bool as i64)
            } else {
                fallback_ty
            };
            let result_val = bootstrap_ir_value_alloc_register(used_ty);
            let instr = bootstrap_ir_instr_alloc(
                instr_tag,
                fallback_ty,
                left_val,
                right_val,
                0,
                0,
                0,
                0,
                result_val,
            );
            bootstrap_ir_block_append_instr(block_id, instr);
            result_val
        }
        ExprKind::UnaryOp { op, operand } => {
            let operand_val = lower_expr(operand, scope, block_id, fallback_ty);
            match op {
                UnaryOp::Neg => {
                    let zero = bootstrap_ir_value_alloc_const_int(fallback_ty, 0);
                    let result_val = bootstrap_ir_value_alloc_register(fallback_ty);
                    let instr = bootstrap_ir_instr_alloc(
                        IrInstrTag::Sub as i64,
                        fallback_ty,
                        zero,
                        operand_val,
                        0,
                        0,
                        0,
                        0,
                        result_val,
                    );
                    bootstrap_ir_block_append_instr(block_id, instr);
                    result_val
                }
                UnaryOp::Not => {
                    let bool_ty = bootstrap_ir_type_alloc_primitive(IrTypeTag::Bool as i64);
                    let result_val = bootstrap_ir_value_alloc_register(bool_ty);
                    let instr = bootstrap_ir_instr_alloc(
                        IrInstrTag::Not as i64,
                        bool_ty,
                        operand_val,
                        0,
                        0,
                        0,
                        0,
                        0,
                        result_val,
                    );
                    bootstrap_ir_block_append_instr(block_id, instr);
                    result_val
                }
            }
        }
        ExprKind::Call { func, args } => {
            let callee_name = match &func.node {
                ExprKind::Ident(name) => name.clone(),
                _ => "<non-ident-callee>".to_string(),
            };
            let callee_val = bootstrap_ir_value_alloc_global(&callee_name, fallback_ty);
            let arg_list = bootstrap_ir_value_list_alloc();
            for a in args {
                let av = lower_expr(a, scope, block_id, fallback_ty);
                bootstrap_ir_list_append(arg_list, av);
            }
            let result_val = bootstrap_ir_value_alloc_register(fallback_ty);
            let instr = bootstrap_ir_instr_alloc(
                IrInstrTag::Call as i64,
                fallback_ty,
                callee_val,
                arg_list,
                0,
                0,
                0,
                0,
                result_val,
            );
            bootstrap_ir_block_append_instr(block_id, instr);
            result_val
        }
        ExprKind::Paren(inner) => lower_expr(inner, scope, block_id, fallback_ty),
        _ => bootstrap_ir_value_alloc_error("unsupported expr in bootstrap subset"),
    }
}

fn lower_stmt(s: &Stmt, scope: &mut LoweringScope, block_id: i64, fallback_ty: i64) {
    match &s.node {
        StmtKind::Let { name, value, .. } => {
            let val = lower_expr(value, scope, block_id, fallback_ty);
            scope.define(name, val);
        }
        StmtKind::Expr(e) => {
            let _ = lower_expr(e, scope, block_id, fallback_ty);
        }
        StmtKind::Ret(e) => {
            let val = lower_expr(e, scope, block_id, fallback_ty);
            let instr = bootstrap_ir_instr_alloc(IrInstrTag::Ret as i64, 0, 0, 0, val, 0, 0, 0, 0);
            bootstrap_ir_block_append_instr(block_id, instr);
        }
        _ => {
            let nop = bootstrap_ir_instr_alloc(IrInstrTag::Nop as i64, 0, 0, 0, 0, 0, 0, 0, 0);
            bootstrap_ir_block_append_instr(block_id, nop);
        }
    }
}

fn lower_function_def(fn_def: &FnDef) -> i64 {
    let ret_ty = fn_def
        .return_type
        .as_ref()
        .map(|t| lower_type_expr(&t.node))
        .unwrap_or_else(|| bootstrap_ir_type_alloc_primitive(IrTypeTag::Unit as i64));
    let fn_id = bootstrap_ir_function_alloc(&fn_def.name, ret_ty);
    let entry = bootstrap_ir_block_alloc("entry");
    bootstrap_ir_function_append_block(fn_id, entry);

    let mut scope = LoweringScope::default();
    for (idx, p) in fn_def.params.iter().enumerate() {
        let pty = lower_type_expr(&p.type_ann.node);
        let ir_param = bootstrap_ir_param_alloc(&p.name, pty);
        bootstrap_ir_function_append_param(fn_id, ir_param);
        let pval = bootstrap_ir_value_alloc_param(idx as i64, pty);
        scope.define(&p.name, pval);
    }
    for stmt in &fn_def.body.node {
        lower_stmt(stmt, &mut scope, entry, ret_ty);
    }
    fn_id
}

fn lower_module_via_externs(name: &str, m: &Module) -> i64 {
    let mod_id = bootstrap_ir_module_alloc(name);
    let mut first_fn = 0i64;
    for item in &m.items {
        if let ItemKind::FnDef(fn_def) = &item.node {
            let ir_fn = lower_function_def(fn_def);
            bootstrap_ir_module_append_function(mod_id, ir_fn);
            if first_fn == 0 {
                first_fn = ir_fn;
            }
        }
    }
    if first_fn != 0 {
        bootstrap_ir_module_set_entry(mod_id, first_fn);
    }
    mod_id
}

fn parse_lower_emit(src: &str, mod_name: &str) -> String {
    let _g = parity_lock();
    reset_ir_store();

    let mut lex = Lexer::new(src, 0);
    let tokens = lex.tokenize();
    let (m, errs) = parser::parse(tokens, 0);
    assert!(
        errs.is_empty(),
        "Rust parser reported errors on codegen-text snippet: {:?}",
        errs
    );

    let mod_id = lower_module_via_externs(mod_name, &m);
    bootstrap_ir_emit_text(mod_id)
}

// ---------------------------------------------------------------------------
// Structural minimums — enforced for every snippet alongside the per-baseline
// match. These would catch an empty / placeholder regression even if all
// baselines also drifted accidentally.
// ---------------------------------------------------------------------------

fn assert_structural_minimum(name: &str, text: &str) {
    assert!(
        !text.is_empty(),
        "[{}] textual emission produced an empty string — codegen slice is silently regressing to placeholder output",
        name
    );
    assert!(
        text.starts_with("module "),
        "[{}] textual emission must start with `module <name>`, got: {:?}",
        name,
        text.lines().next().unwrap_or("")
    );
    assert!(
        text.contains("\nfn ") || text.starts_with("fn "),
        "[{}] textual emission contains no `fn ` declarations — module appears empty",
        name
    );
    assert!(
        text.contains("    entry:"),
        "[{}] textual emission has no `entry:` block — bootstrap lowering must always allocate one",
        name
    );
    // Every function must end with a terminator-flavored line (ret / ret_void /
    // br / unreachable). We at minimum require one `ret` / `ret_void` in the
    // module, since the bootstrap subset never produces br-only modules.
    assert!(
        text.contains("\n        ret ") || text.contains("\n        ret_void"),
        "[{}] textual emission is missing a Ret terminator — last instruction in entry block isn't a return",
        name
    );
}

// ---------------------------------------------------------------------------
// Test driver.
// ---------------------------------------------------------------------------

#[test]
fn codegen_text_matches_golden_baselines() {
    let dir = corpus_dir();
    assert!(
        dir.is_dir(),
        "ir/codegen corpus directory missing: {} \
         (this test requires a frozen corpus to be effective)",
        dir.display()
    );

    let gr_files = list_files_with_ext(&dir, "gr");
    let txt_files = list_files_with_ext(&dir, "txt");

    assert!(
        !gr_files.is_empty(),
        "ir/codegen corpus is empty at {} — gate is meaningless without snippets",
        dir.display()
    );
    assert!(
        !txt_files.is_empty(),
        "ir/codegen corpus has {} .gr snippets but ZERO .txt baselines at {} — \
         this is the 'passes with 0 matches' failure mode the gate exists to prevent",
        gr_files.len(),
        dir.display()
    );
    assert!(
        gr_files.len() >= 6,
        "ir/codegen corpus must contain at least 6 .gr snippets per issue #229; found {}",
        gr_files.len()
    );

    let mut comparisons = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let txt_path = dir.join(format!("{}.txt", stem));

        if !txt_path.exists() {
            failures.push(format!(
                "[{}] missing baseline {} — every .gr snippet must have a frozen .txt baseline. \
                 Regenerate with `cargo test -p gradient-compiler --test self_hosted_codegen_text \
                 regenerate_codegen_text_baselines -- --include-ignored`",
                stem,
                txt_path.display()
            ));
            continue;
        }

        let source = fs::read_to_string(gr_path)
            .unwrap_or_else(|e| panic!("read {}: {}", gr_path.display(), e));
        let actual = parse_lower_emit(&source, &stem);
        assert_structural_minimum(&stem, &actual);

        let expected = fs::read_to_string(&txt_path)
            .unwrap_or_else(|e| panic!("read {}: {}", txt_path.display(), e));

        if actual != expected {
            failures.push(format!(
                "[{}] textual IR does not match baseline {}\n\
                 --- expected (on disk)\n{}\n--- actual\n{}\n--- end ---",
                stem,
                txt_path.display(),
                expected,
                actual
            ));
            comparisons += 1;
            continue;
        }

        // Round-trip stability: re-lower + re-emit produces identical text.
        let actual_again = parse_lower_emit(&source, &stem);
        if actual_again != actual {
            failures.push(format!(
                "[{}] textual IR is not deterministic across re-emission\n\
                 --- first emission\n{}\n--- second emission\n{}\n--- end ---",
                stem, actual, actual_again
            ));
        }

        comparisons += 1;
    }

    assert!(
        comparisons > 0,
        "codegen text gate ran but performed ZERO comparisons — the gate is asleep"
    );

    if !failures.is_empty() {
        panic!(
            "codegen text gate failed ({} failures across {} comparisons):\n\n{}",
            failures.len(),
            comparisons,
            failures.join("\n\n")
        );
    }

    eprintln!(
        "codegen text gate: {} corpus snippets, {} comparisons, all pass",
        gr_files.len(),
        comparisons
    );
}

/// Acceptance: every snippet's emission has a non-placeholder shape — the
/// structural-minimum guard runs even outside the per-baseline loop so a
/// silent regression to "" can't sneak past via baseline drift.
#[test]
fn codegen_text_corpus_non_empty() {
    let dir = corpus_dir();
    let gr_files = list_files_with_ext(&dir, "gr");
    assert!(
        !gr_files.is_empty(),
        "ir/codegen corpus directory empty at {}",
        dir.display()
    );
    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let source = fs::read_to_string(gr_path).expect("read .gr");
        let text = parse_lower_emit(&source, &stem);
        assert_structural_minimum(&stem, &text);
    }
}

/// Acceptance: emitting a zero module id returns "" and emitting an unknown
/// non-zero id does not panic. The kernel walks the runtime store with safe
/// defaults when ids are unknown — surface that contract here so future
/// changes to the store can't accidentally introduce panics on stale ids.
#[test]
fn codegen_text_unknown_module_id_is_safe() {
    let _g = parity_lock();
    reset_ir_store();
    assert_eq!(bootstrap_ir_emit_text(0), "");
    // Unknown non-zero id walks the store via safe defaults — no panic,
    // and the result starts with the `module ` prefix even though the
    // name is empty.
    let unknown = bootstrap_ir_emit_text(99999);
    assert!(
        unknown.starts_with("module "),
        "unknown module id should still produce `module <empty-name>` header, got {:?}",
        unknown
    );
    assert!(
        !unknown.contains("fn "),
        "unknown module id must not synthesize functions, got {:?}",
        unknown
    );
}

/// Regenerate baseline `.txt` files from the current self-hosted emission.
///
/// `#[ignore]` so it never runs by default. To regenerate when the textual
/// format intentionally changes:
///
///     cargo test -p gradient-compiler --test self_hosted_codegen_text \
///         regenerate_codegen_text_baselines -- --include-ignored
#[test]
#[ignore = "regeneration utility — run with --include-ignored"]
fn regenerate_codegen_text_baselines() {
    let dir = corpus_dir();
    let gr_files = list_files_with_ext(&dir, "gr");
    assert!(!gr_files.is_empty(), "no .gr snippets to regenerate from");
    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let txt_path = dir.join(format!("{}.txt", stem));
        let source = fs::read_to_string(gr_path).expect("read .gr");
        let text = parse_lower_emit(&source, &stem);
        fs::write(&txt_path, &text).expect("write .txt");
        eprintln!("regenerated {}", txt_path.display());
    }
}
