//! Verification Condition (VC) intermediate representation and SMT-LIB
//! generator.
//!
//! This module is the bridge between the typechecked AST and an SMT
//! solver, anchored by ADR 0003 (tiered contracts).
//!
//! # Layers
//!
//! 1. **Data structures (sub-issue #327, landed via PR #435).**
//!    [`VerificationCondition`] / [`VerificationConditionSet`] package
//!    proof obligations with the metadata needed for counterexample
//!    diagnostics in #329.
//! 2. **VC generator (sub-issue #328, this PR).** [`VcEncoder`] lowers a
//!    `@verified` function's parameters, contracts, and a small body
//!    subset into a well-formed SMT-LIB 2 query string. Output is
//!    internal-only at this milestone — the encoder produces the text
//!    but no Z3 invocation occurs (that lands with #329). Set the
//!    environment variable `GRADIENT_DUMP_VC` to dump the generated
//!    `.smt2` text to `target/vc/<fn_name>.smt2` for inspection.
//! 3. **Z3 integration + counterexample translation (sub-issue #329).**
//!    Not implemented yet — the encoder's output here is the stable
//!    surface that #329 will feed to the solver and translate back.
//!
//! # Supported body subset (ADR 0003 § "VC generator")
//!
//! At this milestone the encoder handles a deliberately small,
//! tractable subset that matches the bootstrap stdlib's pure total
//! functions:
//!
//! - Function parameters of type `Int` / `Bool` → SMT-LIB free
//!   variables of sort `Int` / `Bool`.
//! - Integer arithmetic (`+`, `-`, `*`, `/`, `%`).
//! - Boolean logic (`and`, `or`, `not`).
//! - Comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`).
//! - `if`/`else` expressions (lowered to SMT-LIB `ite`).
//! - Function bodies that are a single tail expression — either an
//!   `if`/`else` chain or a literal/identifier/binary-op.
//! - `result` in `@ensures` is substituted with the body's tail
//!   expression.
//!
//! Anything outside this subset returns an [`EncodeError`] from
//! [`VcEncoder::encode_function`]. Downstream issues will widen the
//! subset; #328's contract is "well-formed SMT-LIB for the supported
//! subset; clean error otherwise".

use crate::ast::block::Block;
use crate::ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use crate::ast::item::{ContractKind, FnDef};
use crate::ast::span::Span;
use crate::ast::stmt::StmtKind;
use crate::ast::types::TypeExpr;
use std::fmt::Write as _;

/// A single proof obligation derived from one function contract.
///
/// In the launch tier, [`VerificationCondition`] carries the minimum
/// metadata the checker needs to record that an obligation would be
/// emitted; the SMT-LIB translation lives in [`VcEncoder`] (this PR).
#[derive(Debug, Clone, PartialEq)]
pub struct VerificationCondition {
    /// Which contract this obligation derives from (precondition vs
    /// postcondition).
    pub kind: ContractKind,
    /// The source span of the originating `@requires` or `@ensures`
    /// annotation. Used for counterexample diagnostics in #329.
    pub origin_span: Span,
    /// Whether the VC translation pipeline (#328) was wired for this
    /// obligation. `true` once [`VcEncoder`] successfully produces an
    /// SMT-LIB encoding for the function this VC belongs to.
    pub translated: bool,
}

/// All proof obligations derived from a single `@verified` function.
///
/// Wraps `Vec<VerificationCondition>` with the function name so that
/// downstream diagnostics can reference the function unambiguously.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VerificationConditionSet {
    /// The function this set of obligations belongs to.
    pub fn_name: String,
    /// One [`VerificationCondition`] per `@requires` / `@ensures` on
    /// the function. Empty for a `@verified` function with no
    /// contracts — which is itself a checker error per ADR 0003.
    pub conditions: Vec<VerificationCondition>,
}

impl VerificationConditionSet {
    /// Construct a new (empty) set for the named function.
    pub fn new(fn_name: impl Into<String>) -> Self {
        Self {
            fn_name: fn_name.into(),
            conditions: Vec::new(),
        }
    }

    /// Append a stub VC referencing the originating annotation span.
    ///
    /// `translated` defaults to `false`; flip via [`Self::mark_translated`]
    /// once [`VcEncoder`] has produced SMT-LIB for the owning function.
    pub fn add_stub(&mut self, kind: ContractKind, origin_span: Span) {
        self.conditions.push(VerificationCondition {
            kind,
            origin_span,
            translated: false,
        });
    }

    /// Mark every condition in the set as translated. Called after
    /// [`VcEncoder::encode_function`] succeeds for the owning function.
    pub fn mark_translated(&mut self) {
        for c in &mut self.conditions {
            c.translated = true;
        }
    }

    /// Number of obligations recorded.
    pub fn len(&self) -> usize {
        self.conditions.len()
    }

    /// Whether this set carries no obligations.
    pub fn is_empty(&self) -> bool {
        self.conditions.is_empty()
    }
}

// ── SMT-LIB sorts ────────────────────────────────────────────────────────

/// SMT-LIB sorts the encoder currently understands.
///
/// Restricted to the theories ADR 0003 § "VC generator" lists for the
/// initial milestone: linear integer arithmetic plus booleans. Wider
/// theories (bit-vectors, arrays, uninterpreted functions) follow with
/// later sub-issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtSort {
    /// SMT-LIB `Int` — used for Gradient `Int`.
    Int,
    /// SMT-LIB `Bool` — used for Gradient `Bool`.
    Bool,
}

impl SmtSort {
    fn smt_text(self) -> &'static str {
        match self {
            SmtSort::Int => "Int",
            SmtSort::Bool => "Bool",
        }
    }
}

// ── Encoder errors ───────────────────────────────────────────────────────

/// Why a `@verified` function could not be lowered to SMT-LIB at this
/// milestone.
///
/// These are *not* checker errors — they bubble up as a structured
/// "VC unimplemented" path that the checker can render into a
/// diagnostic when end-to-end verification is wired (#329). Until
/// then, the encoder swallows them silently and the launch-tier
/// "unimplemented; falls back to runtime" warning still fires.
#[derive(Debug, Clone, PartialEq)]
pub enum EncodeError {
    /// A parameter sort outside the supported subset (Int / Bool).
    UnsupportedParamType { name: String, type_text: String },
    /// An expression construct outside the supported subset.
    UnsupportedExpr { reason: String },
    /// A statement construct outside the supported subset (e.g. a
    /// non-tail `let` in a body the encoder doesn't yet model).
    UnsupportedStmt { reason: String },
    /// The body produced no tail expression to substitute for `result`.
    NoTailExpression,
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::UnsupportedParamType { name, type_text } => write!(
                f,
                "VC encoder: parameter `{name}` has unsupported type `{type_text}` (expected Int or Bool at this milestone)"
            ),
            EncodeError::UnsupportedExpr { reason } => {
                write!(f, "VC encoder: unsupported expression — {reason}")
            }
            EncodeError::UnsupportedStmt { reason } => {
                write!(f, "VC encoder: unsupported statement — {reason}")
            }
            EncodeError::NoTailExpression => {
                write!(f, "VC encoder: function body has no tail expression to bind `result`")
            }
        }
    }
}

impl std::error::Error for EncodeError {}

// ── SMT-LIB encoder ─────────────────────────────────────────────────────

/// Lowers a `@verified` function to a self-contained SMT-LIB 2 query
/// string.
///
/// The encoded query has this shape:
///
/// ```smt2
/// ; Verification condition for `<fn_name>`
/// ; Generated by gradient-compiler (sub-issue #328)
/// (set-logic ALL)
/// (declare-const <param0> <Sort>)
/// ...
/// (assert <requires0>)
/// ...
/// (assert (not <ensures0_with_result_substituted>))
/// (check-sat)
/// ```
///
/// One query is emitted per `@ensures` contract — this is the form
/// Z3 will consume in #329 (`unsat` ⇒ obligation discharged). When a
/// function has only `@requires` contracts, a single satisfiability
/// query is emitted (`unsat` ⇒ contradictory precondition, which is
/// itself a checker-detectable bug downstream).
pub struct VcEncoder;

/// Output of encoding a single `@verified` function.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedFunction {
    /// The function name.
    pub fn_name: String,
    /// One self-contained SMT-LIB query per proof obligation. Each
    /// entry is independently runnable through `z3 -in`.
    pub queries: Vec<EncodedQuery>,
}

/// A single `(check-sat)` query lowered for one contract obligation.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedQuery {
    /// Which contract this query discharges. `None` for the synthetic
    /// "preconditions are satisfiable" probe emitted when a function
    /// has only `@requires` contracts.
    pub kind: Option<ContractKind>,
    /// Zero-based index into the function's `contracts` vector. `None`
    /// for the synthetic probe.
    pub contract_index: Option<usize>,
    /// The full SMT-LIB 2 source. Stable, deterministic, and
    /// suitable for golden-file comparison.
    pub smtlib: String,
}

impl VcEncoder {
    /// Lower a `@verified` function to SMT-LIB queries.
    pub fn encode_function(fn_def: &FnDef) -> Result<EncodedFunction, EncodeError> {
        // 1. Resolve parameter sorts. Unsupported sorts abort.
        let mut params: Vec<(String, SmtSort)> = Vec::with_capacity(fn_def.params.len());
        for p in &fn_def.params {
            let sort = sort_of_type_expr(&p.type_ann.node).ok_or_else(|| {
                EncodeError::UnsupportedParamType {
                    name: p.name.clone(),
                    type_text: format!("{:?}", p.type_ann.node),
                }
            })?;
            params.push((p.name.clone(), sort));
        }

        // 2. Encode each `@requires` to a stand-alone Bool term so we
        //    can re-use them as assumptions in every obligation query.
        let mut requires_terms: Vec<String> = Vec::new();
        for c in &fn_def.contracts {
            if c.kind == ContractKind::Requires {
                let term = encode_bool_expr(&c.condition)?;
                requires_terms.push(term);
            }
        }

        // 3. Encode the body's tail expression once. Used both as the
        //    substitution for `result` in `@ensures` and as a
        //    sanity-check that the body is in-subset.
        let body_term = encode_block_tail(&fn_def.body)?;

        // 4. Build one query per `@ensures` (the load-bearing
        //    obligations). If there are none, emit a single
        //    satisfiability probe over the preconditions.
        let mut queries: Vec<EncodedQuery> = Vec::new();
        let return_sort = fn_def
            .return_type
            .as_ref()
            .and_then(|rt| sort_of_type_expr(&rt.node));

        let mut has_ensures = false;
        for (idx, c) in fn_def.contracts.iter().enumerate() {
            if c.kind != ContractKind::Ensures {
                continue;
            }
            has_ensures = true;
            // When we can bind `result` as a top-level SMT constant
            // (i.e. the function has a known return sort and the body
            // lowers to a value term), `result` references inside the
            // ensures clause stay symbolic and refer to that constant.
            // Otherwise, fall back to inline substitution of the body
            // term so the obligation still typechecks.
            let ensures_term = if return_sort.is_some() {
                // Pass `result` as its own substitution so the
                // identifier passes the `result-only-inside-ensures`
                // gate while staying as the symbolic constant.
                encode_expr(&c.condition, Some("result"), true)?
            } else {
                encode_bool_expr_with_result(&c.condition, &body_term)?
            };
            let smtlib = render_obligation_query(
                &fn_def.name,
                &params,
                return_sort,
                Some(&body_term),
                &requires_terms,
                Some(&ensures_term),
            );
            queries.push(EncodedQuery {
                kind: Some(ContractKind::Ensures),
                contract_index: Some(idx),
                smtlib,
            });
        }

        if !has_ensures {
            // No @ensures: emit a precondition-satisfiability probe.
            // `(check-sat)` returning `sat` proves at least one
            // parameter binding satisfies the conjunction.
            let smtlib = render_satisfiability_query(&fn_def.name, &params, &requires_terms);
            queries.push(EncodedQuery {
                kind: Some(ContractKind::Requires),
                contract_index: None,
                smtlib,
            });
        }

        Ok(EncodedFunction {
            fn_name: fn_def.name.clone(),
            queries,
        })
    }
}

// ── Internal helpers ────────────────────────────────────────────────────

fn sort_of_type_expr(t: &TypeExpr) -> Option<SmtSort> {
    if let TypeExpr::Named { name, .. } = t {
        match name.as_str() {
            "Int" => Some(SmtSort::Int),
            "Bool" => Some(SmtSort::Bool),
            _ => None,
        }
    } else {
        None
    }
}

/// Encode an expression that must produce a Bool term.
fn encode_bool_expr(expr: &Expr) -> Result<String, EncodeError> {
    encode_expr(expr, /*result_sub=*/ None, /*want_bool=*/ true)
}

/// Encode an `@ensures` expression, substituting any reference to the
/// special identifier `result` with the body's tail term.
fn encode_bool_expr_with_result(expr: &Expr, body_term: &str) -> Result<String, EncodeError> {
    encode_expr(expr, Some(body_term), /*want_bool=*/ true)
}

/// Encode a function body (a `Block`) down to its tail expression.
///
/// The supported body shape is intentionally narrow at this
/// milestone: a sequence of zero `let`s/`expr`s followed by either a
/// `Stmt::Ret(e)` or a `Stmt::Expr(e)` whose `e` is in the supported
/// expression subset. Anything else returns [`EncodeError`].
fn encode_block_tail(block: &Block) -> Result<String, EncodeError> {
    let stmts = &block.node;
    if stmts.is_empty() {
        return Err(EncodeError::NoTailExpression);
    }

    // For simplicity at this milestone, every statement except the
    // tail must be unsupported (we don't yet model `let` bindings as
    // SMT `let` forms — that's a planned widening). The tail must be
    // a `Ret(expr)` or `Expr(expr)`.
    if stmts.len() > 1 {
        return Err(EncodeError::UnsupportedStmt {
            reason: format!(
                "body contains {} statements; encoder currently models single-tail-expression bodies only",
                stmts.len()
            ),
        });
    }

    let tail = &stmts[stmts.len() - 1];
    match &tail.node {
        StmtKind::Ret(e) | StmtKind::Expr(e) => encode_value_expr(e),
        other => Err(EncodeError::UnsupportedStmt {
            reason: format!("tail statement kind not yet supported: {other:?}"),
        }),
    }
}

/// Encode a value-position expression. Returns its SMT-LIB term.
fn encode_value_expr(expr: &Expr) -> Result<String, EncodeError> {
    encode_expr(expr, /*result_sub=*/ None, /*want_bool=*/ false)
}

/// Core expression encoder.
///
/// `result_sub`: when `Some(term)`, the special identifier `result`
/// (used inside `@ensures`) is substituted with that term.
/// `want_bool`: a hint — when `true` and the expression is itself a
/// comparison/logical, encode it directly; when `false`, encode as an
/// arithmetic / value term. This lets us parse a bare `b: Bool`
/// identifier as either a Bool reference or, in an arithmetic
/// position, a type error (which falls through to the arithmetic
/// path and the SMT solver will type-check it).
#[allow(clippy::only_used_in_recursion)]
fn encode_expr(
    expr: &Expr,
    result_sub: Option<&str>,
    want_bool: bool,
) -> Result<String, EncodeError> {
    match &expr.node {
        ExprKind::IntLit(n) => Ok(format_int_literal(*n)),
        ExprKind::BoolLit(b) => Ok(if *b { "true".into() } else { "false".into() }),
        ExprKind::Ident(name) => {
            if name == "result" {
                if let Some(sub) = result_sub {
                    // The substitution is already a complete SMT-LIB
                    // term; emit it verbatim.
                    Ok(sub.to_string())
                } else {
                    Err(EncodeError::UnsupportedExpr {
                        reason: "`result` referenced outside an @ensures clause".into(),
                    })
                }
            } else {
                Ok(name.clone())
            }
        }
        ExprKind::Paren(inner) => encode_expr(inner, result_sub, want_bool),
        ExprKind::UnaryOp { op, operand } => {
            let inner = encode_expr(operand, result_sub, want_bool)?;
            match op {
                UnaryOp::Neg => Ok(format!("(- {inner})")),
                UnaryOp::Not => Ok(format!("(not {inner})")),
            }
        }
        ExprKind::BinaryOp { op, left, right } => {
            let want_bool_kids = matches!(op, BinOp::And | BinOp::Or);
            let l = encode_expr(left, result_sub, want_bool_kids)?;
            let r = encode_expr(right, result_sub, want_bool_kids)?;
            let smt_op = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "div",
                BinOp::Mod => "mod",
                BinOp::Eq => "=",
                BinOp::Ne => {
                    return Ok(format!("(not (= {l} {r}))"));
                }
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::And => "and",
                BinOp::Or => "or",
                BinOp::Pipe => {
                    return Err(EncodeError::UnsupportedExpr {
                        reason: "pipe operator `|>` not modelled in VC encoder".into(),
                    });
                }
            };
            Ok(format!("({smt_op} {l} {r})"))
        }
        ExprKind::If {
            condition,
            then_block,
            else_ifs,
            else_block,
        } => {
            let cond = encode_expr(condition, result_sub, true)?;
            let then_term = encode_block_tail_inner(then_block, result_sub)?;
            // Build the chain right-to-left so `else if` arms nest as
            // SMT-LIB `ite` forms.
            let mut tail = if let Some(b) = else_block {
                encode_block_tail_inner(b, result_sub)?
            } else {
                return Err(EncodeError::UnsupportedExpr {
                    reason: "if-expression without else: VC encoder requires total branches".into(),
                });
            };
            for (else_cond, else_body) in else_ifs.iter().rev() {
                let ec = encode_expr(else_cond, result_sub, true)?;
                let eb = encode_block_tail_inner(else_body, result_sub)?;
                tail = format!("(ite {ec} {eb} {tail})");
            }
            Ok(format!("(ite {cond} {then_term} {tail})"))
        }
        ExprKind::UnitLit
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::CharLit(_)
        | ExprKind::TypedHole(_)
        | ExprKind::Call { .. }
        | ExprKind::FieldAccess { .. }
        | ExprKind::For { .. }
        | ExprKind::While { .. }
        | ExprKind::Match { .. }
        | ExprKind::Tuple(_)
        | ExprKind::RecordLit { .. }
        | ExprKind::TypedExpr { .. }
        | ExprKind::Construct { .. }
        | ExprKind::TupleField { .. }
        | ExprKind::Spawn { .. }
        | ExprKind::Send { .. }
        | ExprKind::Ask { .. }
        | ExprKind::ListLit(_)
        | ExprKind::Closure { .. }
        | ExprKind::Range { .. }
        | ExprKind::Try(_)
        | ExprKind::Defer { .. }
        | ExprKind::StringInterp { .. }
        | ExprKind::ConcurrentScope { .. }
        | ExprKind::Supervisor { .. } => Err(EncodeError::UnsupportedExpr {
            reason: format!("expression kind not in VC subset: {:?}", expr.node),
        }),
    }
}

/// Helper: encode a block whose tail is the value of a branch arm.
fn encode_block_tail_inner(block: &Block, result_sub: Option<&str>) -> Result<String, EncodeError> {
    let stmts = &block.node;
    if stmts.is_empty() {
        return Err(EncodeError::NoTailExpression);
    }
    if stmts.len() > 1 {
        return Err(EncodeError::UnsupportedStmt {
            reason: format!(
                "branch body contains {} statements; encoder currently models single-tail-expression branches only",
                stmts.len()
            ),
        });
    }
    let tail = &stmts[0];
    match &tail.node {
        StmtKind::Ret(e) | StmtKind::Expr(e) => encode_expr(e, result_sub, false),
        other => Err(EncodeError::UnsupportedStmt {
            reason: format!("branch tail kind not yet supported: {other:?}"),
        }),
    }
}

/// SMT-LIB requires negative literals as `(- N)`, not `-N`.
fn format_int_literal(n: i64) -> String {
    if n < 0 {
        format!("(- {})", -(n as i128))
    } else {
        format!("{n}")
    }
}

fn render_header(out: &mut String, fn_name: &str) {
    let _ = writeln!(out, "; Verification condition for `{fn_name}`");
    let _ = writeln!(out, "; Generated by gradient-compiler (sub-issue #328)");
    let _ = writeln!(out, "(set-logic ALL)");
}

fn render_param_decls(out: &mut String, params: &[(String, SmtSort)]) {
    for (name, sort) in params {
        let _ = writeln!(out, "(declare-const {name} {})", sort.smt_text());
    }
}

fn render_obligation_query(
    fn_name: &str,
    params: &[(String, SmtSort)],
    return_sort: Option<SmtSort>,
    body_term: Option<&str>,
    requires: &[String],
    ensures_with_result: Option<&str>,
) -> String {
    let mut out = String::new();
    render_header(&mut out, fn_name);
    render_param_decls(&mut out, params);
    // Bind `result` as a fresh constant equal to the body's tail
    // expression. This keeps the obligation query readable and gives
    // the model returned in #329 a stable name to translate.
    if let (Some(sort), Some(body)) = (return_sort, body_term) {
        let _ = writeln!(out, "(declare-const result {})", sort.smt_text());
        let _ = writeln!(out, "(assert (= result {body}))");
    }
    for r in requires {
        let _ = writeln!(out, "(assert {r})");
    }
    if let Some(e) = ensures_with_result {
        let _ = writeln!(out, "(assert (not {e}))");
    }
    let _ = writeln!(out, "(check-sat)");
    out
}

fn render_satisfiability_query(
    fn_name: &str,
    params: &[(String, SmtSort)],
    requires: &[String],
) -> String {
    let mut out = String::new();
    render_header(&mut out, fn_name);
    render_param_decls(&mut out, params);
    for r in requires {
        let _ = writeln!(out, "(assert {r})");
    }
    let _ = writeln!(out, "(check-sat)");
    out
}

// ── Optional disk dump (gated by GRADIENT_DUMP_VC) ──────────────────────

/// Dump every encoded query to `target/vc/<fn_name>__<idx>.smt2` when
/// the `GRADIENT_DUMP_VC` environment variable is set to a non-empty
/// value. No-op otherwise. Errors during write are swallowed so a
/// permission issue doesn't block compilation.
pub fn maybe_dump(encoded: &EncodedFunction) {
    if std::env::var_os("GRADIENT_DUMP_VC")
        .map(|v| v.is_empty())
        .unwrap_or(true)
    {
        return;
    }
    let dir = std::path::Path::new("target").join("vc");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (idx, q) in encoded.queries.iter().enumerate() {
        let path = dir.join(format!("{}__{idx}.smt2", encoded.fn_name));
        let _ = std::fs::write(path, &q.smtlib);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::span::Position;

    fn dummy_span() -> Span {
        Span::new(0, Position::new(1, 1, 0), Position::new(1, 1, 0))
    }

    #[test]
    fn empty_set_is_empty() {
        let set = VerificationConditionSet::new("clamp_nonneg");
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert_eq!(set.fn_name, "clamp_nonneg");
    }

    #[test]
    fn add_stub_records_kind_and_span() {
        let mut set = VerificationConditionSet::new("clamp_nonneg");
        set.add_stub(ContractKind::Requires, dummy_span());
        set.add_stub(ContractKind::Ensures, dummy_span());
        assert_eq!(set.len(), 2);
        assert_eq!(set.conditions[0].kind, ContractKind::Requires);
        assert_eq!(set.conditions[1].kind, ContractKind::Ensures);
        assert!(!set.conditions[0].translated);
    }

    #[test]
    fn mark_translated_flips_all_conditions() {
        let mut set = VerificationConditionSet::new("f");
        set.add_stub(ContractKind::Requires, dummy_span());
        set.add_stub(ContractKind::Ensures, dummy_span());
        set.mark_translated();
        assert!(set.conditions.iter().all(|c| c.translated));
    }

    // ── Sort + literal encoding micro-tests ─────────────────────────

    #[test]
    fn sort_lookup_int_and_bool() {
        let int_t = TypeExpr::Named {
            name: "Int".into(),
            cap: None,
        };
        let bool_t = TypeExpr::Named {
            name: "Bool".into(),
            cap: None,
        };
        let str_t = TypeExpr::Named {
            name: "String".into(),
            cap: None,
        };
        assert_eq!(sort_of_type_expr(&int_t), Some(SmtSort::Int));
        assert_eq!(sort_of_type_expr(&bool_t), Some(SmtSort::Bool));
        assert_eq!(sort_of_type_expr(&str_t), None);
    }

    #[test]
    fn negative_integer_literal_uses_smtlib_form() {
        assert_eq!(format_int_literal(0), "0");
        assert_eq!(format_int_literal(7), "7");
        assert_eq!(format_int_literal(-1), "(- 1)");
        assert_eq!(format_int_literal(i64::MIN), "(- 9223372036854775808)");
    }

    // ── End-to-end encoder tests ────────────────────────────────────

    fn parse_first_fn(src: &str) -> FnDef {
        use crate::ast::item::ItemKind;
        use crate::lexer::Lexer;
        use crate::parser;
        let mut lexer = Lexer::new(src, 0);
        let tokens = lexer.tokenize();
        let (module, errs) = parser::parse(tokens, 0);
        assert!(errs.is_empty(), "parse errors: {errs:?}");
        match &module.items[0].node {
            ItemKind::FnDef(f) => f.clone(),
            other => panic!("expected FnDef, got {other:?}"),
        }
    }

    #[test]
    fn encode_simple_precondition_only_fn() {
        // Only @requires, no @ensures — emits a satisfiability probe.
        let src = "\
@verified
@requires(n >= 0)
fn nonneg(n: Int) -> Int:
    n
";
        let f = parse_first_fn(src);
        let encoded = VcEncoder::encode_function(&f).expect("encode succeeds");
        assert_eq!(encoded.fn_name, "nonneg");
        assert_eq!(encoded.queries.len(), 1);
        let q = &encoded.queries[0];
        assert_eq!(q.kind, Some(ContractKind::Requires));
        assert!(q.contract_index.is_none());
        assert!(q.smtlib.contains("(declare-const n Int)"));
        assert!(q.smtlib.contains("(assert (>= n 0))"));
        assert!(q.smtlib.trim_end().ends_with("(check-sat)"));
    }

    #[test]
    fn encode_clamp_nonneg_uses_ite_for_if_else() {
        let src = "\
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn clamp_nonneg(n: Int) -> Int:
    if n >= 0:
        n
    else:
        0
";
        let f = parse_first_fn(src);
        let encoded = VcEncoder::encode_function(&f).expect("encode succeeds");
        assert_eq!(encoded.queries.len(), 1);
        let q = &encoded.queries[0];
        assert_eq!(q.kind, Some(ContractKind::Ensures));
        assert_eq!(q.contract_index, Some(1));
        // The body must lower to an `ite` form.
        assert!(
            q.smtlib.contains("(ite (>= n 0) n 0)"),
            "expected ite form, got:\n{}",
            q.smtlib
        );
        // `result` must be bound to the body term.
        assert!(q.smtlib.contains("(declare-const result Int)"));
        assert!(q.smtlib.contains("(assert (= result"));
        // Postcondition must be negated.
        assert!(q.smtlib.contains("(assert (not (>= result 0)))"));
    }

    #[test]
    fn encode_emits_one_query_per_ensures() {
        let src = "\
@verified
@requires(n >= 0)
@ensures(result >= 0)
@ensures(result <= n)
fn id_nonneg(n: Int) -> Int:
    n
";
        let f = parse_first_fn(src);
        let encoded = VcEncoder::encode_function(&f).expect("encode succeeds");
        assert_eq!(encoded.queries.len(), 2);
        // First @ensures is at index 1 (after @requires at 0).
        assert_eq!(encoded.queries[0].contract_index, Some(1));
        assert_eq!(encoded.queries[1].contract_index, Some(2));
    }

    #[test]
    fn encode_rejects_unsupported_param_type() {
        let src = "\
@verified
@requires(true)
fn f(s: String) -> Int:
    0
";
        let f = parse_first_fn(src);
        let err = VcEncoder::encode_function(&f).expect_err("should fail");
        match err {
            EncodeError::UnsupportedParamType { name, .. } => assert_eq!(name, "s"),
            other => panic!("expected UnsupportedParamType, got {other:?}"),
        }
    }

    #[test]
    fn encode_rejects_call_in_body() {
        let src = "\
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn wraps(n: Int) -> Int:
    helper(n)
";
        let f = parse_first_fn(src);
        let err = VcEncoder::encode_function(&f).expect_err("should fail");
        assert!(
            matches!(err, EncodeError::UnsupportedExpr { .. }),
            "expected UnsupportedExpr for unmodeled call, got {err:?}"
        );
    }

    #[test]
    fn encode_handles_boolean_param() {
        let src = "\
@verified
@requires(b == true)
@ensures(result == b)
fn id_bool(b: Bool) -> Bool:
    b
";
        let f = parse_first_fn(src);
        let encoded = VcEncoder::encode_function(&f).expect("encode succeeds");
        assert_eq!(encoded.queries.len(), 1);
        let q = &encoded.queries[0];
        assert!(q.smtlib.contains("(declare-const b Bool)"));
        assert!(q.smtlib.contains("(declare-const result Bool)"));
    }

    #[test]
    fn encode_negative_literal_as_smtlib_form() {
        let src = "\
@verified
@requires(n > 0)
@ensures(result < 0)
fn neg(n: Int) -> Int:
    -1
";
        let f = parse_first_fn(src);
        let encoded = VcEncoder::encode_function(&f).expect("encode succeeds");
        assert!(encoded.queries[0].smtlib.contains("(- 1)"));
    }

    #[test]
    fn encode_else_if_chain_nests_ite() {
        let src = "\
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn sign(n: Int) -> Int:
    if n > 0:
        1
    else if n == 0:
        0
    else:
        0
";
        let f = parse_first_fn(src);
        let encoded = VcEncoder::encode_function(&f).expect("encode succeeds");
        let s = &encoded.queries[0].smtlib;
        // outer ite for `if n > 0` wrapping inner ite for `else if n == 0`.
        assert!(
            s.contains("(ite (> n 0) 1 (ite (= n 0) 0 0))"),
            "expected nested ite, got:\n{s}"
        );
    }
}
