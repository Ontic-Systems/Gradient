//! Issue #230: end-to-end self-hosted compilation pipeline.
//!
//! Until #228 / #229 the self-hosted compiler had stubbed pipeline phases —
//! `compiler/compiler.gr::stage_lex / stage_parse / stage_codegen / stage_emit`
//! returned placeholder empty handles regardless of input. This module wires
//! the real phases together, driven by the same runtime-backed stores used
//! by the rest of the bootstrap surface.
//!
//! Phases:
//!
//! - **lex**: Rust lexer -> tokens (no separate handle yet; tokens stay
//!   in-memory because `compiler/lexer.gr` doesn't yet emit through a runtime
//!   store for full source text — it does for fixed token lists via
//!   `bootstrap_token_list_*`, which is exercised by the parser-token-access
//!   gate). The pipeline therefore caches the tokenized source by id and lets
//!   later phases consume it by id.
//! - **parse**: Rust parser -> `ast::Module`. We then flatten the module's
//!   items into a runtime-backed `bootstrap_module_item` list of function
//!   handles populated through the existing AST store (the same shape
//!   `parser.gr`'s direct path produces for #239). Returns the module-items
//!   list handle.
//! - **check**: Rust type-checker on the original `ast::Module`. Returns the
//!   number of real (non-warning) errors so the .gr-side pipeline can stop on
//!   > 0.
//! - **lower**: `bootstrap_ir_*` runtime store, driven by an adapter that
//!   mirrors `compiler/ir_builder.gr::lower_module` (same shape used by #228 /
//!   #229). Returns the IR module id.
//! - **emit**: delegates to `bootstrap_ir_emit::bootstrap_ir_emit_text`.
//!
//! Boundary contract: this module is the explicit Rust kernel boundary for
//! the bootstrap pipeline. The kernel holds tokenized source in a tiny
//! session table keyed by integer id, reuses existing AST / IR runtime stores
//! for cross-phase data, and never invents diagnostics — error counts come
//! from the actual type-checker, parse error list, and lexer error tokens.
//!
//! When the runtime can execute `compiler.gr` natively, this kernel can
//! shrink to just whichever externs `compiler.gr` cannot yet replace
//! directly.

use std::sync::Mutex;

use crate::ast::{ItemKind, Module};
use crate::bootstrap_ast_bridge::{
    bootstrap_function_alloc, bootstrap_module_item_alloc_function,
    bootstrap_module_item_list_alloc, bootstrap_node_list_append,
};
use crate::bootstrap_ir_bridge::{
    bootstrap_ir_block_alloc, bootstrap_ir_block_append_instr, bootstrap_ir_function_alloc,
    bootstrap_ir_function_append_block, bootstrap_ir_function_append_param,
    bootstrap_ir_instr_alloc, bootstrap_ir_list_append, bootstrap_ir_module_alloc,
    bootstrap_ir_module_append_function, bootstrap_ir_module_set_entry, bootstrap_ir_param_alloc,
    bootstrap_ir_type_alloc_named, bootstrap_ir_type_alloc_primitive,
    bootstrap_ir_value_alloc_const_bool, bootstrap_ir_value_alloc_const_int,
    bootstrap_ir_value_alloc_error, bootstrap_ir_value_alloc_global,
    bootstrap_ir_value_alloc_param, bootstrap_ir_value_alloc_register,
    bootstrap_ir_value_list_alloc, IrInstrTag, IrTypeTag,
};
use crate::bootstrap_ir_emit::bootstrap_ir_emit_text;
use crate::lexer::{Lexer, Token};
use crate::parser;
use crate::typechecker;

// ── Tag constants matching `compiler/parser.gr::TypeTag` ─────────────────

const TYPE_TAG_INT: i64 = 1;
const TYPE_TAG_FLOAT: i64 = 2;
const TYPE_TAG_BOOL: i64 = 3;
const TYPE_TAG_STRING: i64 = 4;
const TYPE_TAG_UNIT: i64 = 5;
const TYPE_TAG_NAMED: i64 = 6;

// ── Session table for tokenized source ──────────────────────────────────
//
// The lex phase needs to hand a cache id to the parse phase so the .gr
// pipeline can express phase chaining as integer-only `Int -> Int` calls.
// We store the tokens per session id; ids never wrap and never get reused
// in the lifetime of the process, mirroring the AST/IR store conventions.

#[derive(Default, Debug)]
struct PipelineSession {
    /// `0` means "not yet lexed".
    tokens: Vec<Token>,
    file_id: i64,
    /// `>= 0`. Set by parse phase. -1 sentinel means unset.
    parse_error_count: i64,
    /// `>= 0`. Set by check phase. -1 sentinel means unset.
    check_error_count: i64,
    /// AST module preserved for the check / lower phases.
    ast_module: Option<Module>,
}

#[derive(Default, Debug)]
struct PipelineStore {
    sessions: Vec<PipelineSession>,
}

impl PipelineStore {
    fn alloc(&mut self) -> i64 {
        let id = (self.sessions.len() as i64) + 1;
        self.sessions.push(PipelineSession {
            parse_error_count: -1,
            check_error_count: -1,
            ..Default::default()
        });
        id
    }

    fn get(&self, id: i64) -> Option<&PipelineSession> {
        if id <= 0 {
            return None;
        }
        self.sessions.get((id as usize) - 1)
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut PipelineSession> {
        if id <= 0 {
            return None;
        }
        self.sessions.get_mut((id as usize) - 1)
    }
}

fn store() -> &'static Mutex<PipelineStore> {
    use std::sync::OnceLock;
    static STORE: OnceLock<Mutex<PipelineStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(PipelineStore::default()))
}

fn with_store<R>(f: impl FnOnce(&mut PipelineStore) -> R) -> R {
    let mut s = store().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut s)
}

/// Reset the pipeline session table. Test-only: real callers never
/// reuse session ids so reset isn't needed in normal operation.
pub fn reset_pipeline_store() {
    with_store(|s| s.sessions.clear());
}

// ── Phase 1: Lex ─────────────────────────────────────────────────────────

/// Tokenize `source` for the given `file_id`. Allocates a new session id
/// the rest of the pipeline keys off, and returns it. Caller can then
/// pass the same id through `bootstrap_pipeline_parse` / `_check` /
/// `_lower` / `_emit`. Returns `0` if `source` is empty (the only error
/// the lexer surfaces at this layer; lex error tokens themselves are
/// surfaced by the parse phase as parse errors instead of being a
/// separate failure mode at this level).
pub fn bootstrap_pipeline_lex(source: &str, file_id: i64) -> i64 {
    if source.is_empty() {
        return 0;
    }
    let mut lexer = Lexer::new(source, file_id as u32);
    let tokens = lexer.tokenize();
    with_store(|s| {
        let id = s.alloc();
        // SAFETY: alloc just pushed; `get_mut` is guaranteed to succeed.
        let session = s.get_mut(id).expect("session just allocated");
        session.tokens = tokens;
        session.file_id = file_id;
        id
    })
}

/// Number of tokens in the session. Useful for the .gr pipeline to
/// confirm the lex phase produced real output (non-zero) before
/// advancing to parse.
pub fn bootstrap_pipeline_token_count(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| sess.tokens.len() as i64)
            .unwrap_or(0)
    })
}

// ── Phase 2: Parse ───────────────────────────────────────────────────────

/// Parse the tokens cached in `session_id` and flatten the resulting
/// AST module's `FnDef` items into a runtime-backed `bootstrap_module_item`
/// list. Returns the items-list handle (the same handle shape
/// `ir_builder.gr::lower_module` expects).
///
/// On parse error, returns `0` and increments the session's parse error
/// counter. Use `bootstrap_pipeline_parse_error_count(session_id)` to
/// inspect.
pub fn bootstrap_pipeline_parse(session_id: i64) -> i64 {
    let (tokens, file_id) = match with_store(|s| {
        s.get(session_id)
            .map(|sess| (sess.tokens.clone(), sess.file_id))
    }) {
        Some(t) => t,
        None => return 0,
    };
    if tokens.is_empty() {
        with_store(|s| {
            if let Some(sess) = s.get_mut(session_id) {
                sess.parse_error_count = 0;
            }
        });
        return 0;
    }
    let (module, errors) = parser::parse(tokens, file_id as u32);
    let err_count = errors.len() as i64;
    with_store(|s| {
        if let Some(sess) = s.get_mut(session_id) {
            sess.parse_error_count = err_count;
            sess.ast_module = Some(module.clone());
        }
    });
    if err_count > 0 {
        return 0;
    }
    // Flatten the module's FnDef items into the runtime-backed AST store as
    // module-item function handles. We use the existing function/list externs
    // directly — params/body bodies stay opaque (handle 0) because the
    // pipeline tests only need item identity for the lower phase, which has
    // its own AST adapter. Once `parser.gr` populates the AST store
    // end-to-end, this flattening collapses to the parser's own emission.
    let items_handle = bootstrap_module_item_list_alloc();
    for item in &module.items {
        if let ItemKind::FnDef(fn_def) = &item.node {
            // ret-type tag/name encoding mirrors compiler/parser.gr.
            let (ret_tag, ret_name) = match &fn_def.return_type {
                Some(t) => match &t.node {
                    crate::ast::types::TypeExpr::Named { name, .. } => match name.as_str() {
                        "Int" => (TYPE_TAG_INT, String::new()),
                        "Float" => (TYPE_TAG_FLOAT, String::new()),
                        "Bool" => (TYPE_TAG_BOOL, String::new()),
                        "String" => (TYPE_TAG_STRING, String::new()),
                        other => (TYPE_TAG_NAMED, other.to_string()),
                    },
                    crate::ast::types::TypeExpr::Unit => (TYPE_TAG_UNIT, String::new()),
                    _ => (TYPE_TAG_NAMED, String::new()),
                },
                None => (TYPE_TAG_UNIT, String::new()),
            };
            let fn_handle = bootstrap_function_alloc(
                &fn_def.name,
                /* params */ 0,
                ret_tag,
                &ret_name,
                /* body */ 0,
                /* is_pub */ 0,
                /* is_extern */ 0,
            );
            let item_handle = bootstrap_module_item_alloc_function(fn_handle);
            bootstrap_node_list_append(items_handle, item_handle);
        }
    }
    items_handle
}

/// Parse error count for the session, or `-1` if parse hasn't run yet.
pub fn bootstrap_pipeline_parse_error_count(session_id: i64) -> i64 {
    with_store(|s| {
        s.get(session_id)
            .map(|sess| sess.parse_error_count)
            .unwrap_or(-1)
    })
}

// ── Phase 3: Check ───────────────────────────────────────────────────────

/// Type-check the AST stored in `session_id`. Returns the number of real
/// (non-warning) type errors. `-1` if the session is unknown or hasn't
/// parsed yet (use `_parse_error_count` to distinguish).
pub fn bootstrap_pipeline_check(session_id: i64) -> i64 {
    let (module, file_id) = match with_store(|s| {
        s.get(session_id)
            .and_then(|sess| sess.ast_module.clone().map(|m| (m, sess.file_id)))
    }) {
        Some(p) => p,
        None => return -1,
    };
    let errors = typechecker::check_module(&module, file_id as u32);
    let real = errors.iter().filter(|e| !e.is_warning).count() as i64;
    with_store(|s| {
        if let Some(sess) = s.get_mut(session_id) {
            sess.check_error_count = real;
        }
    });
    real
}

// ── Phase 4: Lower ───────────────────────────────────────────────────────

/// Lower the parsed AST into a runtime-backed IR module via the same
/// shape `compiler/ir_builder.gr::lower_module` produces. Returns the IR
/// module id (`0` if no functions were lowered or session is unknown).
pub fn bootstrap_pipeline_lower(session_id: i64, mod_name: &str) -> i64 {
    let module = match with_store(|s| s.get(session_id).and_then(|sess| sess.ast_module.clone())) {
        Some(m) => m,
        None => return 0,
    };
    lower_module_via_externs(mod_name, &module)
}

// ── Phase 5: Emit ────────────────────────────────────────────────────────

/// Emit canonical textual IR for the given module id (delegates to
/// `bootstrap_ir_emit::bootstrap_ir_emit_text`). Provided here so the
/// .gr pipeline only needs one extern surface to wire all phases.
pub fn bootstrap_pipeline_emit(ir_module_id: i64) -> String {
    bootstrap_ir_emit_text(ir_module_id)
}

// ── Internal: AST -> IR adapter (mirrors ir_builder.gr) ─────────────────

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

fn lower_type_expr(t: &crate::ast::types::TypeExpr) -> i64 {
    use crate::ast::types::TypeExpr;
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

fn binop_to_instr_tag(op: crate::ast::expr::BinOp) -> i64 {
    use crate::ast::expr::BinOp;
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

fn binop_is_compare(op: crate::ast::expr::BinOp) -> bool {
    use crate::ast::expr::BinOp;
    matches!(
        op,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
    )
}

fn lower_expr(
    e: &crate::ast::expr::Expr,
    scope: &LoweringScope,
    block_id: i64,
    fallback_ty: i64,
) -> i64 {
    use crate::ast::expr::{ExprKind, UnaryOp};
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

fn lower_stmt(
    s: &crate::ast::stmt::Stmt,
    scope: &mut LoweringScope,
    block_id: i64,
    fallback_ty: i64,
) {
    use crate::ast::stmt::StmtKind;
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

fn lower_function_def(fn_def: &crate::ast::item::FnDef) -> i64 {
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
    // Count FnDef items first; if zero, return 0 to signal "no
    // bootstrap-subset functions to lower" so downstream callers
    // (driver, trust gate) can treat this as a real failure mode
    // instead of an empty placeholder success.
    let fn_count = m
        .items
        .iter()
        .filter(|i| matches!(i.node, ItemKind::FnDef(_)))
        .count();
    if fn_count == 0 {
        return 0;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_ast_bridge::reset_ast_store;
    use crate::bootstrap_ir_bridge::{reset_ir_store, shared_test_lock};

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        shared_test_lock()
    }

    fn reset_all() {
        reset_pipeline_store();
        reset_ast_store();
        reset_ir_store();
    }

    #[test]
    fn empty_source_lex_returns_zero() {
        let _g = lock();
        reset_all();
        assert_eq!(bootstrap_pipeline_lex("", 0), 0);
    }

    #[test]
    fn happy_path_advances_through_all_phases() {
        let _g = lock();
        reset_all();
        let src = "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n";

        let session = bootstrap_pipeline_lex(src, 0);
        assert!(session > 0, "lex should produce a session id");
        assert!(
            bootstrap_pipeline_token_count(session) > 5,
            "tokenization should produce real tokens"
        );

        let items = bootstrap_pipeline_parse(session);
        assert!(items > 0, "parse should produce real module items");
        assert_eq!(bootstrap_pipeline_parse_error_count(session), 0);

        let check_errs = bootstrap_pipeline_check(session);
        assert_eq!(check_errs, 0, "valid program must type-check");

        let ir = bootstrap_pipeline_lower(session, "demo");
        assert!(ir > 0, "lower should produce a real IR module");

        let text = bootstrap_pipeline_emit(ir);
        assert!(
            text.contains("fn add"),
            "emitted text must include the function"
        );
        assert!(
            text.contains("ret"),
            "emitted text must include a Ret terminator"
        );
    }

    #[test]
    fn unknown_session_id_returns_safe_defaults() {
        let _g = lock();
        reset_all();
        assert_eq!(bootstrap_pipeline_token_count(99999), 0);
        assert_eq!(bootstrap_pipeline_parse(99999), 0);
        assert_eq!(bootstrap_pipeline_parse_error_count(99999), -1);
        assert_eq!(bootstrap_pipeline_check(99999), -1);
        assert_eq!(bootstrap_pipeline_lower(99999, "x"), 0);
        assert_eq!(bootstrap_pipeline_emit(0), "");
    }

    #[test]
    fn type_error_stops_at_check() {
        let _g = lock();
        reset_all();
        // `bogus` is undefined — parse succeeds but check fails.
        let src = "fn f(x: Int) -> Int:\n    ret bogus\n";
        let session = bootstrap_pipeline_lex(src, 0);
        let items = bootstrap_pipeline_parse(session);
        assert!(items > 0, "parse should still succeed for type errors");
        assert_eq!(bootstrap_pipeline_parse_error_count(session), 0);
        let check_errs = bootstrap_pipeline_check(session);
        assert!(check_errs > 0, "check must report at least one error");
    }
}
