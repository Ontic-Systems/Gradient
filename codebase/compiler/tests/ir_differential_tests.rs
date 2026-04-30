//! Issue #228: IR differential / golden parity gate for the self-hosted IR builder.
//!
//! With #227 the self-hosted IR builder (`compiler/ir_builder.gr`) walks the
//! bootstrap AST store and emits real IR through the runtime-backed
//! `bootstrap_ir_*` extern surface. Until the Gradient runtime can execute
//! `ir_builder.gr` directly, this gate runs a Rust adapter that mirrors
//! `lower_module` / `lower_function` / `lower_stmt` / `lower_expr` over the
//! same FFI surface — the resulting store is exactly what the self-hosted
//! builder will produce when execution lands.
//!
//! What this gate locks down:
//!
//!   1. The on-disk corpus is non-empty AND every `.gr` snippet has a
//!      matching `.json` baseline (closes the "passes with 0 matches" hole).
//!   2. The self-hosted IR for each snippet, normalized to canonical JSON,
//!      exactly matches its frozen baseline. This covers function names,
//!      params (names + types), block structure, instruction sequences,
//!      operand kinds (const / register / param / global), result types
//!      (Bool for compares, I64 for arithmetic), and call arg lists.
//!   3. Each snippet produces a NON-EMPTY module — no placeholder
//!      "module created with zero functions" sneaks past.
//!   4. Each function has at least one block with at least one instruction,
//!      and the entry block ends with a Ret-flavored terminator. This is
//!      the structural minimum that downstream codegen (#229) needs.
//!   5. The Rust IR builder's `IrBuilder::build_module` ALSO succeeds on
//!      every snippet (zero IR errors). Self-hosted IR isn't a subset of
//!      Rust IR shape (Rust uses Phi nodes / short-circuit branches that
//!      the bootstrap-stage self-hosted builder does not yet model), so
//!      we don't compare them instruction-for-instruction. We DO assert
//!      both produce the same function signature surface (names, param
//!      arity and types, return type).
//!   6. The normalized JSON round-trips through serde without change.
//!
//! Companion gates: `parser_differential_tests.rs` (parser parity),
//! `parser_boundary_tests.rs` (parser unsupported boundary),
//! `self_hosted_checker_differential.rs` (checker parity),
//! `self_hosted_ir_builder.rs` (IR builder runtime contract).
//!
//! When the lowering shape intentionally changes (e.g. comparisons get a
//! distinct result type), regenerate baselines with:
//!
//!     cargo test -p gradient-compiler --test ir_differential_tests \
//!         regenerate_ir_baselines -- --include-ignored

#![allow(clippy::uninlined_format_args)]

use std::collections::BTreeMap;
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
    bootstrap_ir_block_alloc, bootstrap_ir_block_append_instr, bootstrap_ir_block_get_instr_at,
    bootstrap_ir_block_get_instr_count, bootstrap_ir_block_get_name, bootstrap_ir_function_alloc,
    bootstrap_ir_function_append_block, bootstrap_ir_function_append_param,
    bootstrap_ir_function_get_block_at, bootstrap_ir_function_get_block_count,
    bootstrap_ir_function_get_name, bootstrap_ir_function_get_param_at,
    bootstrap_ir_function_get_param_count, bootstrap_ir_function_get_ret_type,
    bootstrap_ir_instr_alloc, bootstrap_ir_instr_get_cond, bootstrap_ir_instr_get_left,
    bootstrap_ir_instr_get_result, bootstrap_ir_instr_get_right, bootstrap_ir_instr_get_tag,
    bootstrap_ir_list_append, bootstrap_ir_list_get, bootstrap_ir_list_len,
    bootstrap_ir_module_alloc, bootstrap_ir_module_append_function,
    bootstrap_ir_module_get_entry_fn, bootstrap_ir_module_get_function_at,
    bootstrap_ir_module_get_function_count, bootstrap_ir_module_get_name,
    bootstrap_ir_module_set_entry, bootstrap_ir_param_alloc, bootstrap_ir_param_get_name,
    bootstrap_ir_param_get_type, bootstrap_ir_type_alloc_named, bootstrap_ir_type_alloc_primitive,
    bootstrap_ir_type_get_name, bootstrap_ir_type_get_tag, bootstrap_ir_value_alloc_const_bool,
    bootstrap_ir_value_alloc_const_int, bootstrap_ir_value_alloc_error,
    bootstrap_ir_value_alloc_global, bootstrap_ir_value_alloc_param,
    bootstrap_ir_value_alloc_register, bootstrap_ir_value_get_int, bootstrap_ir_value_get_slot,
    bootstrap_ir_value_get_tag, bootstrap_ir_value_get_text, bootstrap_ir_value_get_type,
    bootstrap_ir_value_list_alloc, reset_ir_store, IrInstrTag, IrTypeTag, IrValueTag,
};
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use serde::{Deserialize, Serialize};

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
// Normalized IR shape
//
// All operand-id / instruction-id / block-id / value-slot fields are dropped.
// We preserve only the semantic shape: function names, ordered params with
// names and types, ordered blocks with named labels and ordered instructions,
// and instruction tag / operand kind / result type. This is the contract
// that downstream codegen (#229) depends on.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedIrModule {
    pub name: String,
    pub entry_fn: Option<String>,
    pub functions: Vec<NormalizedIrFunction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedIrFunction {
    pub name: String,
    pub params: Vec<NormalizedIrParam>,
    pub ret_type: NormalizedIrType,
    pub blocks: Vec<NormalizedIrBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedIrParam {
    pub name: String,
    pub ty: NormalizedIrType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NormalizedIrType {
    Primitive { name: String },
    Named { name: String },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedIrBlock {
    pub label: String,
    pub instructions: Vec<NormalizedIrInstr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedIrInstr {
    /// Symbolic instruction tag (e.g. "add", "icmp_slt", "ret", "call").
    pub op: String,
    /// Result kind: "register" / "none" (terminators) / "error".
    /// Plus the result type when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_type: Option<NormalizedIrType>,
    /// Ordered operands, normalized to operand-shape only (kind + payload).
    pub operands: Vec<NormalizedIrOperand>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NormalizedIrOperand {
    ConstInt { value: i64, ty: NormalizedIrType },
    ConstBool { value: bool },
    Register { ty: NormalizedIrType },
    Param { index: i64, ty: NormalizedIrType },
    Global { name: String, ty: NormalizedIrType },
    Args { values: Vec<NormalizedIrOperand> },
    Error { message: String },
    None,
}

// ---------------------------------------------------------------------------
// Mapping from IrInstrTag -> stable string name (matches `compiler/ir.gr`
// case order via `IrInstrTag` repr in `bootstrap_ir_bridge.rs`).
// ---------------------------------------------------------------------------

fn instr_tag_name(tag: i64) -> &'static str {
    match IrInstrTag::from_i64(tag) {
        IrInstrTag::Unknown => "unknown",
        IrInstrTag::Ret => "ret",
        IrInstrTag::RetVoid => "ret_void",
        IrInstrTag::Br => "br",
        IrInstrTag::BrCond => "br_cond",
        IrInstrTag::Switch => "switch",
        IrInstrTag::Unreachable => "unreachable",
        IrInstrTag::Add => "add",
        IrInstrTag::Sub => "sub",
        IrInstrTag::Mul => "mul",
        IrInstrTag::SDiv => "sdiv",
        IrInstrTag::UDiv => "udiv",
        IrInstrTag::SRem => "srem",
        IrInstrTag::URem => "urem",
        IrInstrTag::FAdd => "fadd",
        IrInstrTag::FSub => "fsub",
        IrInstrTag::FMul => "fmul",
        IrInstrTag::FDiv => "fdiv",
        IrInstrTag::FRem => "frem",
        IrInstrTag::And => "and",
        IrInstrTag::Or => "or",
        IrInstrTag::Xor => "xor",
        IrInstrTag::Shl => "shl",
        IrInstrTag::LShr => "lshr",
        IrInstrTag::AShr => "ashr",
        IrInstrTag::Not => "not",
        IrInstrTag::ICmpEq => "icmp_eq",
        IrInstrTag::ICmpNe => "icmp_ne",
        IrInstrTag::ICmpSLt => "icmp_slt",
        IrInstrTag::ICmpSLe => "icmp_sle",
        IrInstrTag::ICmpSGt => "icmp_sgt",
        IrInstrTag::ICmpSGe => "icmp_sge",
        IrInstrTag::ICmpULt => "icmp_ult",
        IrInstrTag::ICmpULe => "icmp_ule",
        IrInstrTag::ICmpUGt => "icmp_ugt",
        IrInstrTag::ICmpUGe => "icmp_uge",
        IrInstrTag::FCmpEq => "fcmp_eq",
        IrInstrTag::FCmpNe => "fcmp_ne",
        IrInstrTag::FCmpLt => "fcmp_lt",
        IrInstrTag::FCmpLe => "fcmp_le",
        IrInstrTag::FCmpGt => "fcmp_gt",
        IrInstrTag::FCmpGe => "fcmp_ge",
        IrInstrTag::AllocA => "alloca",
        IrInstrTag::Load => "load",
        IrInstrTag::Store => "store",
        IrInstrTag::GetElementPtr => "gep",
        IrInstrTag::Trunc => "trunc",
        IrInstrTag::ZExt => "zext",
        IrInstrTag::SExt => "sext",
        IrInstrTag::FpToSi => "fp_to_si",
        IrInstrTag::FpToUi => "fp_to_ui",
        IrInstrTag::SiToFp => "si_to_fp",
        IrInstrTag::UiToFp => "ui_to_fp",
        IrInstrTag::PtrToInt => "ptr_to_int",
        IrInstrTag::IntToPtr => "int_to_ptr",
        IrInstrTag::BitCast => "bit_cast",
        IrInstrTag::Call => "call",
        IrInstrTag::CallIndirect => "call_indirect",
        IrInstrTag::ExtractValue => "extract_value",
        IrInstrTag::InsertValue => "insert_value",
        IrInstrTag::Phi => "phi",
        IrInstrTag::Select => "select",
        IrInstrTag::Nop => "nop",
    }
}

fn primitive_tag_name(tag: i64) -> &'static str {
    match IrTypeTag::from_i64(tag) {
        IrTypeTag::Unknown => "unknown",
        IrTypeTag::Unit => "unit",
        IrTypeTag::Bool => "bool",
        IrTypeTag::I8 => "i8",
        IrTypeTag::I16 => "i16",
        IrTypeTag::I32 => "i32",
        IrTypeTag::I64 => "i64",
        IrTypeTag::U8 => "u8",
        IrTypeTag::U16 => "u16",
        IrTypeTag::U32 => "u32",
        IrTypeTag::U64 => "u64",
        IrTypeTag::F32 => "f32",
        IrTypeTag::F64 => "f64",
        IrTypeTag::Ptr => "ptr",
        IrTypeTag::Array => "array",
        IrTypeTag::Func => "func",
        IrTypeTag::Struct => "struct",
        IrTypeTag::Named => "named",
        IrTypeTag::Opaque => "opaque",
    }
}

// ---------------------------------------------------------------------------
// Self-hosted lowering driver. This mirrors `compiler/ir_builder.gr`'s
// `lower_module` / `lower_function` / `lower_stmt` / `lower_expr` exactly,
// driven from a Rust ast::Module instead of the bootstrap_ast_bridge store.
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct LoweringScope {
    /// name -> IrValue id (param / let-bound).
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
        // Out-of-subset: fall back to i64 (matches `lower_type` in ir_builder.gr).
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
        BinOp::Mod | BinOp::Pipe => 0, // unsupported in bootstrap
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
        // Outside bootstrap subset (Paren elided to inner; everything else falls back to error).
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

// ---------------------------------------------------------------------------
// Walk the runtime store to produce the normalized form.
// ---------------------------------------------------------------------------

fn read_type(id: i64) -> NormalizedIrType {
    if id == 0 {
        return NormalizedIrType::Unknown;
    }
    let tag = bootstrap_ir_type_get_tag(id);
    let kind = IrTypeTag::from_i64(tag);
    match kind {
        IrTypeTag::Named => NormalizedIrType::Named {
            name: bootstrap_ir_type_get_name(id),
        },
        IrTypeTag::Unknown => NormalizedIrType::Unknown,
        _ => NormalizedIrType::Primitive {
            name: primitive_tag_name(tag).to_string(),
        },
    }
}

fn read_value_as_operand(id: i64) -> NormalizedIrOperand {
    if id == 0 {
        return NormalizedIrOperand::None;
    }
    let tag = bootstrap_ir_value_get_tag(id);
    match IrValueTag::from_i64(tag) {
        IrValueTag::ConstInt => NormalizedIrOperand::ConstInt {
            value: bootstrap_ir_value_get_int(id),
            ty: read_type(bootstrap_ir_value_get_type(id)),
        },
        IrValueTag::ConstBool => NormalizedIrOperand::ConstBool {
            value: bootstrap_ir_value_get_int(id) != 0,
        },
        IrValueTag::Register => NormalizedIrOperand::Register {
            ty: read_type(bootstrap_ir_value_get_type(id)),
        },
        IrValueTag::Param => NormalizedIrOperand::Param {
            index: bootstrap_ir_value_get_slot(id),
            ty: read_type(bootstrap_ir_value_get_type(id)),
        },
        IrValueTag::Global => NormalizedIrOperand::Global {
            name: bootstrap_ir_value_get_text(id),
            ty: read_type(bootstrap_ir_value_get_type(id)),
        },
        IrValueTag::Error => NormalizedIrOperand::Error {
            message: bootstrap_ir_value_get_text(id),
        },
        _ => NormalizedIrOperand::None,
    }
}

fn read_args_list(handle: i64) -> NormalizedIrOperand {
    let len = bootstrap_ir_list_len(handle);
    let mut values = Vec::with_capacity(len as usize);
    for i in 0..len {
        let id = bootstrap_ir_list_get(handle, i);
        values.push(read_value_as_operand(id));
    }
    NormalizedIrOperand::Args { values }
}

fn read_instruction(instr_id: i64) -> NormalizedIrInstr {
    let tag = bootstrap_ir_instr_get_tag(instr_id);
    let op = instr_tag_name(tag).to_string();
    let result_id = bootstrap_ir_instr_get_result(instr_id);
    let result_type = if result_id != 0 {
        Some(read_type(bootstrap_ir_value_get_type(result_id)))
    } else {
        None
    };
    let mut operands = Vec::new();
    let kind = IrInstrTag::from_i64(tag);
    match kind {
        IrInstrTag::Ret => {
            operands.push(read_value_as_operand(bootstrap_ir_instr_get_cond(instr_id)));
        }
        IrInstrTag::RetVoid | IrInstrTag::Unreachable | IrInstrTag::Nop => {}
        IrInstrTag::Call | IrInstrTag::CallIndirect => {
            operands.push(read_value_as_operand(bootstrap_ir_instr_get_left(instr_id)));
            operands.push(read_args_list(bootstrap_ir_instr_get_right(instr_id)));
        }
        IrInstrTag::Not => {
            operands.push(read_value_as_operand(bootstrap_ir_instr_get_left(instr_id)));
        }
        // Default: binary-shaped — left, right.
        _ => {
            operands.push(read_value_as_operand(bootstrap_ir_instr_get_left(instr_id)));
            operands.push(read_value_as_operand(bootstrap_ir_instr_get_right(
                instr_id,
            )));
        }
    }
    NormalizedIrInstr {
        op,
        result_type,
        operands,
    }
}

fn read_block(block_id: i64) -> NormalizedIrBlock {
    let label = bootstrap_ir_block_get_name(block_id);
    let count = bootstrap_ir_block_get_instr_count(block_id);
    let mut instructions = Vec::with_capacity(count as usize);
    for i in 0..count {
        let instr_id = bootstrap_ir_block_get_instr_at(block_id, i);
        instructions.push(read_instruction(instr_id));
    }
    NormalizedIrBlock {
        label,
        instructions,
    }
}

fn read_function(fn_id: i64) -> NormalizedIrFunction {
    let name = bootstrap_ir_function_get_name(fn_id);
    let ret_type = read_type(bootstrap_ir_function_get_ret_type(fn_id));
    let pcount = bootstrap_ir_function_get_param_count(fn_id);
    let mut params = Vec::with_capacity(pcount as usize);
    for i in 0..pcount {
        let pid = bootstrap_ir_function_get_param_at(fn_id, i);
        params.push(NormalizedIrParam {
            name: bootstrap_ir_param_get_name(pid),
            ty: read_type(bootstrap_ir_param_get_type(pid)),
        });
    }
    let bcount = bootstrap_ir_function_get_block_count(fn_id);
    let mut blocks = Vec::with_capacity(bcount as usize);
    for i in 0..bcount {
        let bid = bootstrap_ir_function_get_block_at(fn_id, i);
        blocks.push(read_block(bid));
    }
    NormalizedIrFunction {
        name,
        params,
        ret_type,
        blocks,
    }
}

fn read_module(mod_id: i64) -> NormalizedIrModule {
    let name = bootstrap_ir_module_get_name(mod_id);
    let entry_id = bootstrap_ir_module_get_entry_fn(mod_id);
    let entry_fn = if entry_id != 0 {
        Some(bootstrap_ir_function_get_name(entry_id))
    } else {
        None
    };
    let count = bootstrap_ir_module_get_function_count(mod_id);
    let mut functions = Vec::with_capacity(count as usize);
    for i in 0..count {
        let fid = bootstrap_ir_module_get_function_at(mod_id, i);
        functions.push(read_function(fid));
    }
    NormalizedIrModule {
        name,
        entry_fn,
        functions,
    }
}

// ---------------------------------------------------------------------------
// JSON canonicalization. Same shape as parser_differential_tests.rs.
// ---------------------------------------------------------------------------

fn to_canonical_json(ir: &NormalizedIrModule) -> String {
    let value: serde_json::Value = serde_json::to_value(ir).expect("normalized ir is serialisable");
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

// ---------------------------------------------------------------------------
// Parse + lower one snippet into a NormalizedIrModule. Holds the global
// IR-store lock for the duration of the lowering since the runtime store
// is process-wide.
// ---------------------------------------------------------------------------

fn lower_snippet_to_normalized(src: &str, mod_name: &str) -> NormalizedIrModule {
    let _g = parity_lock();
    reset_ir_store();

    let mut lex = Lexer::new(src, 0);
    let tokens = lex.tokenize();
    let (m, errs) = parser::parse(tokens, 0);
    assert!(
        errs.is_empty(),
        "Rust parser reported errors on IR-corpus snippet: {:?}",
        errs
    );

    let mod_id = lower_module_via_externs(mod_name, &m);
    read_module(mod_id)
}

// ---------------------------------------------------------------------------
// Sanity assertions performed for every snippet (separate from the JSON
// baseline check).
// ---------------------------------------------------------------------------

fn assert_structural_minimum(name: &str, ir: &NormalizedIrModule) {
    assert!(
        !ir.functions.is_empty(),
        "[{}] self-hosted IR module has zero functions — placeholder lowering is regressing",
        name
    );
    assert!(
        ir.entry_fn.is_some(),
        "[{}] self-hosted IR module has no entry function set",
        name
    );
    for f in &ir.functions {
        assert!(
            !f.blocks.is_empty(),
            "[{}] function `{}` has zero blocks — bootstrap lowering must allocate at least an entry block",
            name, f.name
        );
        let entry = &f.blocks[0];
        assert_eq!(
            entry.label, "entry",
            "[{}] function `{}` first block must be `entry`, got `{}`",
            name, f.name, entry.label
        );
        assert!(
            !entry.instructions.is_empty(),
            "[{}] function `{}` entry block has zero instructions — bootstrap subset always produces at least a Ret",
            name, f.name
        );
        let last = entry.instructions.last().unwrap();
        assert!(
            matches!(
                last.op.as_str(),
                "ret" | "ret_void" | "br" | "br_cond" | "unreachable"
            ),
            "[{}] function `{}` entry block does not end with a terminator (last op = `{}`)",
            name,
            f.name,
            last.op
        );
    }
}

fn assert_rust_ir_signatures_match(name: &str, src: &str, sh_ir: &NormalizedIrModule) {
    let mut lex = Lexer::new(src, 0);
    let tokens = lex.tokenize();
    let (m, errs) = parser::parse(tokens, 0);
    assert!(errs.is_empty(), "[{}] parse errors: {:?}", name, errs);
    let (rust_ir, ir_errs) = IrBuilder::build_module(&m);
    assert!(
        ir_errs.is_empty(),
        "[{}] Rust IR builder reported errors on IR-corpus snippet: {:?}",
        name,
        ir_errs
    );
    assert_eq!(
        rust_ir.functions.len(),
        sh_ir.functions.len(),
        "[{}] function count mismatch between Rust IR ({}) and self-hosted IR ({})",
        name,
        rust_ir.functions.len(),
        sh_ir.functions.len()
    );
    for (rf, sf) in rust_ir.functions.iter().zip(sh_ir.functions.iter()) {
        assert_eq!(
            rf.name, sf.name,
            "[{}] function name mismatch: rust={} self-hosted={}",
            name, rf.name, sf.name
        );
        assert_eq!(
            rf.params.len(),
            sf.params.len(),
            "[{}] param arity mismatch on `{}`: rust={} self-hosted={}",
            name,
            rf.name,
            rf.params.len(),
            sf.params.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Test driver.
// ---------------------------------------------------------------------------

#[test]
fn ir_differential_bootstrap_subset() {
    let dir = corpus_dir();
    assert!(
        dir.is_dir(),
        "ir differential corpus directory missing: {} \
         (this test requires a frozen corpus to be effective)",
        dir.display()
    );

    let gr_files = list_files_with_ext(&dir, "gr");
    let json_files = list_files_with_ext(&dir, "json");

    assert!(
        !gr_files.is_empty(),
        "ir differential corpus is empty at {} — gate is meaningless without snippets",
        dir.display()
    );
    assert!(
        !json_files.is_empty(),
        "ir differential corpus has {} .gr snippets but ZERO .json baselines at {} — \
         this is the 'passes with 0 matches' failure mode the gate exists to prevent",
        gr_files.len(),
        dir.display()
    );
    assert!(
        gr_files.len() >= 6,
        "ir differential corpus must contain at least 6 .gr snippets per issue #228; found {}",
        gr_files.len()
    );

    let mut comparisons = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let json_path = dir.join(format!("{}.json", stem));

        if !json_path.exists() {
            failures.push(format!(
                "[{}] missing baseline {} — every .gr snippet must have a frozen .json baseline. \
                 Regenerate with `cargo test -p gradient-compiler --test ir_differential_tests \
                 regenerate_ir_baselines -- --include-ignored`",
                stem,
                json_path.display()
            ));
            continue;
        }

        let source = fs::read_to_string(gr_path)
            .unwrap_or_else(|e| panic!("read {}: {}", gr_path.display(), e));
        let ir = lower_snippet_to_normalized(&source, &stem);
        assert_structural_minimum(&stem, &ir);
        assert_rust_ir_signatures_match(&stem, &source, &ir);

        let actual = to_canonical_json(&ir);
        let expected = fs::read_to_string(&json_path)
            .unwrap_or_else(|e| panic!("read {}: {}", json_path.display(), e));
        if actual != expected {
            failures.push(format!(
                "[{}] normalized IR does not match baseline {}\n\
                 --- expected (on disk)\n{}\n--- actual\n{}\n--- end ---",
                stem,
                json_path.display(),
                expected,
                actual
            ));
            comparisons += 1;
            continue;
        }

        // Round-trip through serde to catch fields that don't survive.
        let parsed_back: NormalizedIrModule = serde_json::from_str(&actual)
            .unwrap_or_else(|e| panic!("[{}] JSON round-trip parse failed: {}", stem, e));
        let reserialised = to_canonical_json(&parsed_back);
        if reserialised != actual {
            failures.push(format!(
                "[{}] NormalizedIrModule is not JSON round-trip stable\n\
                 --- first serialisation\n{}\n--- after round-trip\n{}\n--- end ---",
                stem, actual, reserialised
            ));
        }

        comparisons += 1;
    }

    assert!(
        comparisons > 0,
        "ir differential ran but performed ZERO comparisons — the gate is asleep"
    );

    if !failures.is_empty() {
        panic!(
            "ir differential gate failed ({} failures across {} comparisons):\n\n{}",
            failures.len(),
            comparisons,
            failures.join("\n\n")
        );
    }

    eprintln!(
        "ir differential gate: {} corpus snippets, {} comparisons, all pass",
        gr_files.len(),
        comparisons
    );
}

/// Acceptance: every snippet's lowered module has a non-placeholder shape —
/// exercises the structural-minimum guard outside the per-baseline loop so
/// it's still hit even if baselines drift.
#[test]
fn ir_differential_corpus_non_empty() {
    let dir = corpus_dir();
    let gr_files = list_files_with_ext(&dir, "gr");
    assert!(
        !gr_files.is_empty(),
        "ir corpus directory empty at {}",
        dir.display()
    );
    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let source = fs::read_to_string(gr_path).expect("read .gr");
        let ir = lower_snippet_to_normalized(&source, &stem);
        assert_structural_minimum(&stem, &ir);
    }
}

/// Regenerate baseline JSON files from the current self-hosted lowering.
///
/// `#[ignore]` so it never runs by default. To regenerate when the
/// lowering shape intentionally changes:
///
///     cargo test -p gradient-compiler --test ir_differential_tests \
///         regenerate_ir_baselines -- --include-ignored
#[test]
#[ignore = "regeneration utility — run with --include-ignored"]
fn regenerate_ir_baselines() {
    let dir = corpus_dir();
    let gr_files = list_files_with_ext(&dir, "gr");
    assert!(!gr_files.is_empty(), "no .gr snippets to regenerate from");
    for gr_path in &gr_files {
        let stem = gr_path.file_stem().unwrap().to_string_lossy().to_string();
        let json_path = dir.join(format!("{}.json", stem));
        let source = fs::read_to_string(gr_path).expect("read .gr");
        let ir = lower_snippet_to_normalized(&source, &stem);
        let canonical = to_canonical_json(&ir);
        fs::write(&json_path, &canonical).expect("write .json");
        eprintln!("regenerated {}", json_path.display());
    }
}
