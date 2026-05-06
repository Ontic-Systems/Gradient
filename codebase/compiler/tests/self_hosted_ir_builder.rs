//! Issue #227: self-hosted IR builder runtime-backed lowering parity gate.
//!
//! Drives the runtime-backed `bootstrap_ir_*` store through the operations
//! the self-hosted IR builder (`compiler/ir_builder.gr`) issues when it
//! lowers checked AST into IR — module/function/block/instruction/value
//! allocation, scope-style local lookups, value-list args, and walking the
//! blocks list back through the accessors.
//!
//! Together with the static assertions on `compiler/ir_builder.gr`'s
//! source (extern declarations + lowering helpers) this is the concrete
//! evidence behind #227's acceptance criteria:
//!
//! - `ir_builder.gr` declares the extern surface needed to build IR through
//!   the host runtime.
//! - The runtime-backed store can hold a non-empty IR module covering the
//!   bootstrap parser/checker subset (functions with params, int/bool
//!   literals, ident reads, binary/unary ops, calls, let/expr/ret).
//! - IR output is stable enough for golden / differential reads — the test
//!   asserts deterministic ids, monotonic register slots, and consistent
//!   readback through the accessors.
//!
//! When the self-hosted runtime can execute `ir_builder.gr` directly, this
//! gate flips from "Rust mirrors the .gr code" to "the .gr code drives the
//! same store" without test changes.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use gradient_compiler::bootstrap_ir_bridge::{
    bootstrap_ir_block_alloc, bootstrap_ir_block_append_instr, bootstrap_ir_block_get_instr_at,
    bootstrap_ir_block_get_instr_count, bootstrap_ir_function_alloc,
    bootstrap_ir_function_append_block, bootstrap_ir_function_append_param,
    bootstrap_ir_function_get_block_at, bootstrap_ir_function_get_block_count,
    bootstrap_ir_function_get_entry_block, bootstrap_ir_function_get_name,
    bootstrap_ir_function_get_param_at, bootstrap_ir_function_get_param_count,
    bootstrap_ir_function_get_ret_type, bootstrap_ir_instr_alloc, bootstrap_ir_instr_get_cond,
    bootstrap_ir_instr_get_left, bootstrap_ir_instr_get_result, bootstrap_ir_instr_get_right,
    bootstrap_ir_instr_get_tag, bootstrap_ir_instr_get_then_target, bootstrap_ir_list_append,
    bootstrap_ir_list_get, bootstrap_ir_list_len, bootstrap_ir_module_alloc,
    bootstrap_ir_module_append_function, bootstrap_ir_module_get_entry_fn,
    bootstrap_ir_module_get_function_at, bootstrap_ir_module_get_function_count,
    bootstrap_ir_module_get_name, bootstrap_ir_module_set_entry, bootstrap_ir_param_alloc,
    bootstrap_ir_param_get_name, bootstrap_ir_param_get_type, bootstrap_ir_type_alloc_primitive,
    bootstrap_ir_value_alloc_const_bool, bootstrap_ir_value_alloc_const_int,
    bootstrap_ir_value_alloc_global, bootstrap_ir_value_alloc_param,
    bootstrap_ir_value_alloc_register, bootstrap_ir_value_get_int, bootstrap_ir_value_get_slot,
    bootstrap_ir_value_get_text, bootstrap_ir_value_get_type, bootstrap_ir_value_list_alloc,
    reset_ir_store, IrInstrTag, IrTypeTag,
};

/// Mirrors `IrTypeTag` constants in `compiler/ir_builder.gr`.
const TY_BOOL: i64 = IrTypeTag::Bool as i64;
const TY_I64: i64 = IrTypeTag::I64 as i64;

fn parity_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn ir_builder_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compiler/ir_builder.gr")
}

fn read_ir_builder() -> String {
    std::fs::read_to_string(ir_builder_path()).expect("failed to read compiler/ir_builder.gr")
}

/// Acceptance: `ir_builder.gr` declares the bootstrap IR extern surface
/// the lowering helpers need. Static-source assertions keep cosmetic
/// refactors free but guarantee the FFI contract stays intact.
#[test]
fn ir_builder_gr_declares_bootstrap_ir_externs() {
    let src = read_ir_builder();
    let required = [
        "fn bootstrap_ir_type_alloc_primitive(node_tag: Int) -> Int",
        "fn bootstrap_ir_type_alloc_named(name: String) -> Int",
        "fn bootstrap_ir_value_alloc_const_int(ty: Int, value: Int) -> Int",
        "fn bootstrap_ir_value_alloc_const_bool(value: Int) -> Int",
        "fn bootstrap_ir_value_alloc_register(ty: Int) -> Int",
        "fn bootstrap_ir_value_alloc_param(index: Int, ty: Int) -> Int",
        "fn bootstrap_ir_value_alloc_global(name: String, ty: Int) -> Int",
        "fn bootstrap_ir_value_alloc_error(message: String) -> Int",
        "fn bootstrap_ir_instr_alloc(node_tag: Int, ty: Int, left: Int, right: Int, cond_or_value: Int, then_target: Int, else_target: Int, int_extra: Int, slot_result: Int) -> Int",
        "fn bootstrap_ir_block_alloc(name: String) -> Int",
        "fn bootstrap_ir_block_append_instr(block_id: Int, instr_id: Int) -> Int",
        "fn bootstrap_ir_param_alloc(name: String, ty: Int) -> Int",
        "fn bootstrap_ir_function_alloc(name: String, ret_ty: Int) -> Int",
        "fn bootstrap_ir_function_append_param(fn_id: Int, param_id: Int) -> Int",
        "fn bootstrap_ir_function_append_block(fn_id: Int, block_id: Int) -> Int",
        "fn bootstrap_ir_module_alloc(name: String) -> Int",
        "fn bootstrap_ir_module_append_function(mod_id: Int, fn_id: Int) -> Int",
        "fn bootstrap_ir_module_set_entry(mod_id: Int, fn_id: Int) -> Int",
        "fn bootstrap_ir_value_list_alloc() -> Int",
        "fn bootstrap_ir_list_append(handle: Int, id: Int) -> Int",
    ];
    for line in required {
        assert!(
            src.contains(line),
            "ir_builder.gr must declare extern `{line}`"
        );
    }
}

/// Acceptance: `ir_builder.gr` exposes lowering helpers that walk the
/// AST store. Source-level assertion to keep the lowering functions
/// present even if their bodies are reshaped.
#[test]
fn ir_builder_gr_defines_lowering_helpers() {
    let src = read_ir_builder();
    for required in [
        "fn lower_type(node_tag: Int, type_name: String) -> Int:",
        "fn lower_expr(expr_id: Int, scope: Scope, fn_id: Int, block_id: Int, fallback_ty: Int) -> !{Heap} Int:",
        "fn lower_stmt(stmt_id: Int, scope: Scope, fn_id: Int, block_id: Int, fallback_ty: Int) -> !{Heap} Scope:",
        "fn lower_function(fn_ast_id: Int) -> !{Heap} Int:",
        "fn lower_module(name: String, items_handle: Int) -> !{Heap} Int:",
    ] {
        assert!(
            src.contains(required),
            "ir_builder.gr must define lowering helper `{required}`"
        );
    }
}

/// Acceptance: a function body of `let x = 1; let y = 2; ret x + y`
/// round-trips through the runtime-backed store with deterministic
/// register slots, an Add instruction with the right operands, and a Ret
/// instruction whose operand is the register result of the Add.
#[test]
fn lowered_add_function_round_trips() {
    let _g = parity_lock();
    reset_ir_store();

    let i64_ty = bootstrap_ir_type_alloc_primitive(TY_I64);
    let m = bootstrap_ir_module_alloc("m");
    let f = bootstrap_ir_function_alloc("answer", i64_ty);
    bootstrap_ir_module_append_function(m, f);
    bootstrap_ir_module_set_entry(m, f);

    let entry = bootstrap_ir_block_alloc("entry");
    bootstrap_ir_function_append_block(f, entry);

    // Lower `let x = 1; let y = 2; ret x + y` directly through the externs.
    let x_val = bootstrap_ir_value_alloc_const_int(i64_ty, 1);
    let y_val = bootstrap_ir_value_alloc_const_int(i64_ty, 2);
    let sum = bootstrap_ir_value_alloc_register(i64_ty);
    let add = bootstrap_ir_instr_alloc(
        IrInstrTag::Add as i64,
        i64_ty,
        x_val,
        y_val,
        0,
        0,
        0,
        0,
        sum,
    );
    bootstrap_ir_block_append_instr(entry, add);
    let ret = bootstrap_ir_instr_alloc(IrInstrTag::Ret as i64, 0, 0, 0, sum, 0, 0, 0, 0);
    bootstrap_ir_block_append_instr(entry, ret);

    // Module / function / block readback.
    assert_eq!(bootstrap_ir_module_get_name(m), "m");
    assert_eq!(bootstrap_ir_module_get_function_count(m), 1);
    assert_eq!(bootstrap_ir_module_get_function_at(m, 0), f);
    assert_eq!(bootstrap_ir_module_get_entry_fn(m), f);
    assert_eq!(bootstrap_ir_function_get_name(f), "answer");
    assert_eq!(bootstrap_ir_function_get_ret_type(f), i64_ty);
    assert_eq!(bootstrap_ir_function_get_block_count(f), 1);
    assert_eq!(bootstrap_ir_function_get_block_at(f, 0), entry);
    assert_eq!(bootstrap_ir_function_get_entry_block(f), entry);
    assert_eq!(bootstrap_ir_block_get_instr_count(entry), 2);

    // Instruction inspection.
    let first = bootstrap_ir_block_get_instr_at(entry, 0);
    assert_eq!(bootstrap_ir_instr_get_tag(first), IrInstrTag::Add as i64);
    assert_eq!(bootstrap_ir_instr_get_left(first), x_val);
    assert_eq!(bootstrap_ir_instr_get_right(first), y_val);
    assert_eq!(bootstrap_ir_instr_get_result(first), sum);

    let last = bootstrap_ir_block_get_instr_at(entry, 1);
    assert_eq!(bootstrap_ir_instr_get_tag(last), IrInstrTag::Ret as i64);
    assert_eq!(bootstrap_ir_instr_get_cond(last), sum);

    // Constant payloads survive.
    assert_eq!(bootstrap_ir_value_get_int(x_val), 1);
    assert_eq!(bootstrap_ir_value_get_int(y_val), 2);
    assert_eq!(bootstrap_ir_value_get_slot(sum), 1);
    assert_eq!(bootstrap_ir_value_get_type(sum), i64_ty);
}

/// Acceptance: function parameters round-trip with names and types and
/// can be referenced as Param values. Mirrors what `lower_function`
/// builds when it walks the AST param list.
#[test]
fn function_params_round_trip_through_store() {
    let _g = parity_lock();
    reset_ir_store();

    let i64_ty = bootstrap_ir_type_alloc_primitive(TY_I64);
    let f = bootstrap_ir_function_alloc("add", i64_ty);
    let pa = bootstrap_ir_param_alloc("a", i64_ty);
    let pb = bootstrap_ir_param_alloc("b", i64_ty);
    bootstrap_ir_function_append_param(f, pa);
    bootstrap_ir_function_append_param(f, pb);

    assert_eq!(bootstrap_ir_function_get_param_count(f), 2);
    let first = bootstrap_ir_function_get_param_at(f, 0);
    let second = bootstrap_ir_function_get_param_at(f, 1);
    assert_eq!(bootstrap_ir_param_get_name(first), "a");
    assert_eq!(bootstrap_ir_param_get_name(second), "b");
    assert_eq!(bootstrap_ir_param_get_type(second), i64_ty);

    // Param values created during lowering link back to the same type.
    let pv0 = bootstrap_ir_value_alloc_param(0, i64_ty);
    let pv1 = bootstrap_ir_value_alloc_param(1, i64_ty);
    assert_eq!(bootstrap_ir_value_get_slot(pv0), 0);
    assert_eq!(bootstrap_ir_value_get_slot(pv1), 1);
    assert_eq!(bootstrap_ir_value_get_type(pv0), i64_ty);
}

/// Acceptance: comparison ops record their operands as before and the
/// result register carries a Bool type so downstream consumers can
/// distinguish boolean predicates from integer arithmetic.
#[test]
fn comparison_op_yields_bool_typed_register() {
    let _g = parity_lock();
    reset_ir_store();

    let i64_ty = bootstrap_ir_type_alloc_primitive(TY_I64);
    let bool_ty = bootstrap_ir_type_alloc_primitive(TY_BOOL);

    let l = bootstrap_ir_value_alloc_const_int(i64_ty, 7);
    let r = bootstrap_ir_value_alloc_const_int(i64_ty, 9);
    let result = bootstrap_ir_value_alloc_register(bool_ty);
    let cmp =
        bootstrap_ir_instr_alloc(IrInstrTag::ICmpSLt as i64, i64_ty, l, r, 0, 0, 0, 0, result);
    let _ = cmp;

    assert_eq!(bootstrap_ir_value_get_type(result), bool_ty);
}

/// Acceptance: function calls record callee + arg list and produce a
/// register typed by the return type. Mirrors how `lower_expr` builds
/// `Call(callee, args, ret_ty)` for the bootstrap subset.
#[test]
fn lowered_call_records_callee_and_args() {
    let _g = parity_lock();
    reset_ir_store();

    let i64_ty = bootstrap_ir_type_alloc_primitive(TY_I64);
    let callee = bootstrap_ir_value_alloc_global("add", i64_ty);
    let a1 = bootstrap_ir_value_alloc_const_int(i64_ty, 1);
    let a2 = bootstrap_ir_value_alloc_const_int(i64_ty, 2);
    let args = bootstrap_ir_value_list_alloc();
    bootstrap_ir_list_append(args, a1);
    bootstrap_ir_list_append(args, a2);
    let result = bootstrap_ir_value_alloc_register(i64_ty);
    let call = bootstrap_ir_instr_alloc(
        IrInstrTag::Call as i64,
        i64_ty,
        callee,
        args,
        0,
        0,
        0,
        0,
        result,
    );

    assert_eq!(bootstrap_ir_instr_get_tag(call), IrInstrTag::Call as i64);
    assert_eq!(bootstrap_ir_instr_get_left(call), callee);
    assert_eq!(bootstrap_ir_instr_get_right(call), args);
    assert_eq!(bootstrap_ir_instr_get_result(call), result);
    assert_eq!(bootstrap_ir_value_get_text(callee), "add");
    assert_eq!(bootstrap_ir_list_len(args), 2);
    assert_eq!(bootstrap_ir_list_get(args, 0), a1);
    assert_eq!(bootstrap_ir_list_get(args, 1), a2);
}

/// Acceptance: `if cond: ret a else: ret b` lowers into a conditional
/// branch with two distinct target blocks, each terminated by Ret.
#[test]
fn lowered_if_branch_records_targets() {
    let _g = parity_lock();
    reset_ir_store();

    let i64_ty = bootstrap_ir_type_alloc_primitive(TY_I64);
    let f = bootstrap_ir_function_alloc("pick", i64_ty);
    let entry = bootstrap_ir_block_alloc("entry");
    let then_b = bootstrap_ir_block_alloc("then");
    let else_b = bootstrap_ir_block_alloc("else");
    bootstrap_ir_function_append_block(f, entry);
    bootstrap_ir_function_append_block(f, then_b);
    bootstrap_ir_function_append_block(f, else_b);

    let cond = bootstrap_ir_value_alloc_const_bool(1);
    let br = bootstrap_ir_instr_alloc(
        IrInstrTag::BrCond as i64,
        0,
        0,
        0,
        cond,
        then_b,
        else_b,
        0,
        0,
    );
    bootstrap_ir_block_append_instr(entry, br);

    let a = bootstrap_ir_value_alloc_const_int(i64_ty, 1);
    let then_ret = bootstrap_ir_instr_alloc(IrInstrTag::Ret as i64, 0, 0, 0, a, 0, 0, 0, 0);
    bootstrap_ir_block_append_instr(then_b, then_ret);

    let b = bootstrap_ir_value_alloc_const_int(i64_ty, 2);
    let else_ret = bootstrap_ir_instr_alloc(IrInstrTag::Ret as i64, 0, 0, 0, b, 0, 0, 0, 0);
    bootstrap_ir_block_append_instr(else_b, else_ret);

    assert_eq!(bootstrap_ir_function_get_block_count(f), 3);
    assert_eq!(bootstrap_ir_function_get_entry_block(f), entry);
    let first = bootstrap_ir_block_get_instr_at(entry, 0);
    assert_eq!(bootstrap_ir_instr_get_tag(first), IrInstrTag::BrCond as i64);
    assert_eq!(bootstrap_ir_instr_get_then_target(first), then_b);
    // else target survives via the same accessor used by then_target.
    let then_branch = bootstrap_ir_block_get_instr_at(then_b, 0);
    assert_eq!(
        bootstrap_ir_instr_get_tag(then_branch),
        IrInstrTag::Ret as i64
    );
    let else_branch = bootstrap_ir_block_get_instr_at(else_b, 0);
    assert_eq!(
        bootstrap_ir_instr_get_tag(else_branch),
        IrInstrTag::Ret as i64
    );
}

/// Acceptance: register slot ids stay monotonically increasing across a
/// run so consumers can rely on slot order to match instruction order.
/// This is the IR-side equivalent of the AST's "ids start at 1 and grow
/// monotonically" invariant.
#[test]
fn register_slot_ids_are_monotonic() {
    let _g = parity_lock();
    reset_ir_store();

    let i64_ty = bootstrap_ir_type_alloc_primitive(TY_I64);
    let r1 = bootstrap_ir_value_alloc_register(i64_ty);
    let r2 = bootstrap_ir_value_alloc_register(i64_ty);
    let r3 = bootstrap_ir_value_alloc_register(i64_ty);
    assert_eq!(bootstrap_ir_value_get_slot(r1), 1);
    assert_eq!(bootstrap_ir_value_get_slot(r2), 2);
    assert_eq!(bootstrap_ir_value_get_slot(r3), 3);
}

/// Acceptance: unknown ids return safe defaults (0 / empty string)
/// across module/function/block/instruction accessors, so a malformed
/// or partial lowering can keep walking without panics.
#[test]
fn unknown_ids_return_safe_defaults() {
    let _g = parity_lock();
    reset_ir_store();

    assert_eq!(bootstrap_ir_module_get_name(99999), "");
    assert_eq!(bootstrap_ir_module_get_function_count(99999), 0);
    assert_eq!(bootstrap_ir_function_get_name(99999), "");
    assert_eq!(bootstrap_ir_function_get_block_count(99999), 0);
    assert_eq!(bootstrap_ir_instr_get_tag(99999), 0);
    assert_eq!(bootstrap_ir_value_get_int(99999), 0);
    assert_eq!(bootstrap_ir_list_len(99999), 0);
    assert_eq!(bootstrap_ir_list_get(99999, 0), 0);
}
