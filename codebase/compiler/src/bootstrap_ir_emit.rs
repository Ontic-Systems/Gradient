//! Issue #229: textual IR emission slice for the self-hosted compiler.
//!
//! This is the first executable codegen output the self-hosted compiler can
//! produce end-to-end. Given a module id allocated through the
//! `bootstrap_ir_*` runtime store (#227 / driven by `compiler/ir_builder.gr`'s
//! `lower_module`), [`bootstrap_ir_emit_text`] walks the store and produces a
//! stable, target-independent textual IR.
//!
//! This is intentionally narrow:
//! - Functions (name, params, return type, blocks) are emitted in module order.
//! - Instructions are emitted in block order with their operand kinds and
//!   result types preserved (matches the shape locked by #228's IR
//!   differential gate).
//! - No machine code yet — that is a follow-up issue. The slice gives
//!   downstream consumers (LSP, REPL, debug dumps, golden tests, future
//!   native backends delegated through this same boundary) a stable
//!   contract to read.
//!
//! Boundary contract: the self-hosted compiler holds the lowering logic
//! (`compiler/ir_builder.gr`) and dispatches to this Rust kernel for
//! emission. The kernel never reads AST or invokes the type-checker — it
//! only walks the runtime IR store. That keeps the kernel surface small
//! and makes the boundary easy to delete when the runtime can execute
//! emission natively.

use crate::bootstrap_ir_bridge::{
    bootstrap_ir_block_get_instr_at, bootstrap_ir_block_get_instr_count,
    bootstrap_ir_block_get_name, bootstrap_ir_function_get_block_at,
    bootstrap_ir_function_get_block_count, bootstrap_ir_function_get_name,
    bootstrap_ir_function_get_param_at, bootstrap_ir_function_get_param_count,
    bootstrap_ir_function_get_ret_type, bootstrap_ir_instr_get_cond, bootstrap_ir_instr_get_left,
    bootstrap_ir_instr_get_result, bootstrap_ir_instr_get_right, bootstrap_ir_instr_get_tag,
    bootstrap_ir_list_get, bootstrap_ir_list_len, bootstrap_ir_module_get_function_at,
    bootstrap_ir_module_get_function_count, bootstrap_ir_module_get_name,
    bootstrap_ir_param_get_name, bootstrap_ir_param_get_type, bootstrap_ir_type_get_name,
    bootstrap_ir_type_get_tag, bootstrap_ir_value_get_int, bootstrap_ir_value_get_slot,
    bootstrap_ir_value_get_tag, bootstrap_ir_value_get_text, bootstrap_ir_value_get_type,
    IrInstrTag, IrTypeTag, IrValueTag,
};

/// Emit canonical textual IR for the given module id.
///
/// Returns an empty string if `mod_id == 0`. Unknown ids encountered during
/// the walk are tolerated via the underlying store's safe defaults.
///
/// Format (one function at a time, blocks indented):
///
/// ```text
/// module <name>
///
/// fn <name>(<param>: <ty>, ...) -> <ret_ty>:
///     <label>:
///         <op> <result_type>, <operand>, <operand>, ...
///         ...
/// ```
///
/// Operand encoding:
/// - `i64 42` / `bool true` for constants
/// - `%<slot> : <ty>` for registers
/// - `%p<index> : <ty>` for params
/// - `@<name> : <ty>` for globals
/// - `[<op>, <op>, ...]` for value lists (call args)
/// - `<error: msg>` for error values
/// - `_` for none / unset operands
pub fn bootstrap_ir_emit_text(mod_id: i64) -> String {
    if mod_id == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mod_name = bootstrap_ir_module_get_name(mod_id);
    out.push_str(&format!("module {}\n", mod_name));

    let fn_count = bootstrap_ir_module_get_function_count(mod_id);
    for i in 0..fn_count {
        let fn_id = bootstrap_ir_module_get_function_at(mod_id, i);
        out.push('\n');
        emit_function(&mut out, fn_id);
    }
    out
}

fn emit_function(out: &mut String, fn_id: i64) {
    let name = bootstrap_ir_function_get_name(fn_id);
    let ret_ty_id = bootstrap_ir_function_get_ret_type(fn_id);
    let ret_ty = format_type(ret_ty_id);

    let pcount = bootstrap_ir_function_get_param_count(fn_id);
    let mut params = Vec::with_capacity(pcount as usize);
    for i in 0..pcount {
        let pid = bootstrap_ir_function_get_param_at(fn_id, i);
        let pname = bootstrap_ir_param_get_name(pid);
        let pty = format_type(bootstrap_ir_param_get_type(pid));
        params.push(format!("{}: {}", pname, pty));
    }
    out.push_str(&format!(
        "fn {}({}) -> {}:\n",
        name,
        params.join(", "),
        ret_ty
    ));

    let bcount = bootstrap_ir_function_get_block_count(fn_id);
    for i in 0..bcount {
        let bid = bootstrap_ir_function_get_block_at(fn_id, i);
        emit_block(out, bid);
    }
}

fn emit_block(out: &mut String, block_id: i64) {
    let label = bootstrap_ir_block_get_name(block_id);
    out.push_str(&format!("    {}:\n", label));
    let count = bootstrap_ir_block_get_instr_count(block_id);
    for i in 0..count {
        let instr_id = bootstrap_ir_block_get_instr_at(block_id, i);
        emit_instruction(out, instr_id);
    }
}

fn emit_instruction(out: &mut String, instr_id: i64) {
    let tag = bootstrap_ir_instr_get_tag(instr_id);
    let op = instr_tag_text(tag);
    let kind = IrInstrTag::from_i64(tag);

    let result_id = bootstrap_ir_instr_get_result(instr_id);
    let result_ty = if result_id != 0 {
        let ty = bootstrap_ir_value_get_type(result_id);
        if ty != 0 {
            Some(format_type(ty))
        } else {
            None
        }
    } else {
        None
    };

    let mut parts: Vec<String> = Vec::new();
    if let Some(rt) = result_ty {
        parts.push(rt);
    }

    match kind {
        IrInstrTag::Ret => {
            parts.push(format_value(bootstrap_ir_instr_get_cond(instr_id)));
        }
        IrInstrTag::RetVoid | IrInstrTag::Unreachable | IrInstrTag::Nop => {}
        IrInstrTag::Call | IrInstrTag::CallIndirect => {
            parts.push(format_value(bootstrap_ir_instr_get_left(instr_id)));
            parts.push(format_args_list(bootstrap_ir_instr_get_right(instr_id)));
        }
        IrInstrTag::Not => {
            parts.push(format_value(bootstrap_ir_instr_get_left(instr_id)));
        }
        _ => {
            parts.push(format_value(bootstrap_ir_instr_get_left(instr_id)));
            parts.push(format_value(bootstrap_ir_instr_get_right(instr_id)));
        }
    }

    if parts.is_empty() {
        out.push_str(&format!("        {}\n", op));
    } else {
        out.push_str(&format!("        {} {}\n", op, parts.join(", ")));
    }
}

fn format_args_list(handle: i64) -> String {
    let len = bootstrap_ir_list_len(handle);
    let mut items = Vec::with_capacity(len as usize);
    for i in 0..len {
        let id = bootstrap_ir_list_get(handle, i);
        items.push(format_value(id));
    }
    format!("[{}]", items.join(", "))
}

fn format_value(id: i64) -> String {
    if id == 0 {
        return "_".to_string();
    }
    let tag = bootstrap_ir_value_get_tag(id);
    let kind = IrValueTag::from_i64(tag);
    match kind {
        IrValueTag::ConstInt => {
            let ty = format_type(bootstrap_ir_value_get_type(id));
            let v = bootstrap_ir_value_get_int(id);
            format!("{} {}", ty, v)
        }
        IrValueTag::ConstBool => {
            let v = bootstrap_ir_value_get_int(id) != 0;
            format!("bool {}", v)
        }
        IrValueTag::Register => {
            let ty = format_type(bootstrap_ir_value_get_type(id));
            let slot = bootstrap_ir_value_get_slot(id);
            format!("%{} : {}", slot, ty)
        }
        IrValueTag::Param => {
            let ty = format_type(bootstrap_ir_value_get_type(id));
            let idx = bootstrap_ir_value_get_slot(id);
            format!("%p{} : {}", idx, ty)
        }
        IrValueTag::Global => {
            let ty = format_type(bootstrap_ir_value_get_type(id));
            let name = bootstrap_ir_value_get_text(id);
            format!("@{} : {}", name, ty)
        }
        IrValueTag::Error => format!("<error: {}>", bootstrap_ir_value_get_text(id)),
        _ => "_".to_string(),
    }
}

fn format_type(id: i64) -> String {
    if id == 0 {
        return "_".to_string();
    }
    let tag = bootstrap_ir_type_get_tag(id);
    let kind = IrTypeTag::from_i64(tag);
    match kind {
        IrTypeTag::Named => bootstrap_ir_type_get_name(id),
        _ => primitive_text(kind).to_string(),
    }
}

fn primitive_text(kind: IrTypeTag) -> &'static str {
    match kind {
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

fn instr_tag_text(tag: i64) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_ir_bridge::{
        bootstrap_ir_block_alloc, bootstrap_ir_block_append_instr, bootstrap_ir_function_alloc,
        bootstrap_ir_function_append_block, bootstrap_ir_function_append_param,
        bootstrap_ir_instr_alloc, bootstrap_ir_module_alloc, bootstrap_ir_module_append_function,
        bootstrap_ir_param_alloc, bootstrap_ir_type_alloc_primitive,
        bootstrap_ir_value_alloc_const_int, bootstrap_ir_value_alloc_param,
        bootstrap_ir_value_alloc_register, reset_ir_store, shared_test_lock,
    };

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        shared_test_lock()
    }

    #[test]
    fn empty_id_returns_empty_string() {
        let _g = lock();
        reset_ir_store();
        assert_eq!(bootstrap_ir_emit_text(0), "");
    }

    #[test]
    fn add_function_emits_canonical_text() {
        let _g = lock();
        reset_ir_store();
        let i64_ty = bootstrap_ir_type_alloc_primitive(IrTypeTag::I64 as i64);
        let m = bootstrap_ir_module_alloc("m");
        let f = bootstrap_ir_function_alloc("add", i64_ty);
        let pa = bootstrap_ir_param_alloc("x", i64_ty);
        let pb = bootstrap_ir_param_alloc("y", i64_ty);
        bootstrap_ir_function_append_param(f, pa);
        bootstrap_ir_function_append_param(f, pb);
        bootstrap_ir_module_append_function(m, f);
        let entry = bootstrap_ir_block_alloc("entry");
        bootstrap_ir_function_append_block(f, entry);
        let xv = bootstrap_ir_value_alloc_param(0, i64_ty);
        let yv = bootstrap_ir_value_alloc_param(1, i64_ty);
        let sum = bootstrap_ir_value_alloc_register(i64_ty);
        let add = bootstrap_ir_instr_alloc(IrInstrTag::Add as i64, i64_ty, xv, yv, 0, 0, 0, 0, sum);
        bootstrap_ir_block_append_instr(entry, add);
        let ret = bootstrap_ir_instr_alloc(IrInstrTag::Ret as i64, 0, 0, 0, sum, 0, 0, 0, 0);
        bootstrap_ir_block_append_instr(entry, ret);

        let text = bootstrap_ir_emit_text(m);
        let expected = "module m\n\nfn add(x: i64, y: i64) -> i64:\n    entry:\n        add i64, %p0 : i64, %p1 : i64\n        ret %1 : i64\n";
        assert_eq!(text, expected);
    }

    #[test]
    fn ret_void_and_const_emit_correctly() {
        let _g = lock();
        reset_ir_store();
        let i64_ty = bootstrap_ir_type_alloc_primitive(IrTypeTag::I64 as i64);
        let m = bootstrap_ir_module_alloc("m");
        let f = bootstrap_ir_function_alloc("answer", i64_ty);
        bootstrap_ir_module_append_function(m, f);
        let entry = bootstrap_ir_block_alloc("entry");
        bootstrap_ir_function_append_block(f, entry);
        let v = bootstrap_ir_value_alloc_const_int(i64_ty, 42);
        let ret = bootstrap_ir_instr_alloc(IrInstrTag::Ret as i64, 0, 0, 0, v, 0, 0, 0, 0);
        bootstrap_ir_block_append_instr(entry, ret);

        let text = bootstrap_ir_emit_text(m);
        assert!(text.contains("fn answer() -> i64:"));
        assert!(text.contains("ret i64 42"));
    }
}
