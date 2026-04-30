//! Issue #227: runtime-backed IR storage for the self-hosted IR builder.
//!
//! The self-hosted IR builder (`compiler/ir_builder.gr`) constructs an IR
//! representation of the checked AST. Until #227 the builder's helpers
//! returned dummy registers without recording instructions, blocks, or
//! functions anywhere — `build_add` allocated a fresh register and dropped
//! the operation on the floor.
//!
//! This module mirrors `bootstrap_ast_bridge.rs`: a process-wide store
//! reached through FFI-shaped `bootstrap_ir_*` free functions that the .gr
//! source declares as Phase 0 externs and the Rust host drives directly
//! from parity tests.
//!
//! Scope: lower the bootstrap parser corpus (single-function or multi-
//! function modules with int/bool literals, identifiers, unary/binary ops,
//! calls, if/else, blocks, let/expr/ret statements) into IR. Variants
//! outside that scope are stored as opaque payloads so future issues can
//! extend the store without breaking the Phase-0 contract.

use std::sync::{Mutex, MutexGuard, OnceLock};

// ── Tag enums ────────────────────────────────────────────────────────────

/// Discriminator tags for stored IR types.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum IrTypeTag {
    Unknown = 0,
    Unit = 1,
    Bool = 2,
    I8 = 3,
    I16 = 4,
    I32 = 5,
    I64 = 6,
    U8 = 7,
    U16 = 8,
    U32 = 9,
    U64 = 10,
    F32 = 11,
    F64 = 12,
    Ptr = 13,
    Array = 14,
    Func = 15,
    Struct = 16,
    Named = 17,
    Opaque = 18,
}

impl IrTypeTag {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => IrTypeTag::Unit,
            2 => IrTypeTag::Bool,
            3 => IrTypeTag::I8,
            4 => IrTypeTag::I16,
            5 => IrTypeTag::I32,
            6 => IrTypeTag::I64,
            7 => IrTypeTag::U8,
            8 => IrTypeTag::U16,
            9 => IrTypeTag::U32,
            10 => IrTypeTag::U64,
            11 => IrTypeTag::F32,
            12 => IrTypeTag::F64,
            13 => IrTypeTag::Ptr,
            14 => IrTypeTag::Array,
            15 => IrTypeTag::Func,
            16 => IrTypeTag::Struct,
            17 => IrTypeTag::Named,
            18 => IrTypeTag::Opaque,
            _ => IrTypeTag::Unknown,
        }
    }
}

/// Discriminator tags for stored IR values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum IrValueTag {
    Unknown = 0,
    ConstInt = 1,
    ConstFloat = 2,
    ConstBool = 3,
    ConstString = 4,
    ConstNull = 5,
    ConstUndef = 6,
    Register = 7,
    Global = 8,
    Param = 9,
    BlockAddr = 10,
    None = 11,
    Error = 12,
}

impl IrValueTag {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => IrValueTag::ConstInt,
            2 => IrValueTag::ConstFloat,
            3 => IrValueTag::ConstBool,
            4 => IrValueTag::ConstString,
            5 => IrValueTag::ConstNull,
            6 => IrValueTag::ConstUndef,
            7 => IrValueTag::Register,
            8 => IrValueTag::Global,
            9 => IrValueTag::Param,
            10 => IrValueTag::BlockAddr,
            11 => IrValueTag::None,
            12 => IrValueTag::Error,
            _ => IrValueTag::Unknown,
        }
    }
}

/// Discriminator tags for stored instructions. Match the case order of
/// `InstructionKind` in `compiler/ir.gr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum IrInstrTag {
    Unknown = 0,
    Ret = 1,
    RetVoid = 2,
    Br = 3,
    BrCond = 4,
    Switch = 5,
    Unreachable = 6,
    Add = 7,
    Sub = 8,
    Mul = 9,
    SDiv = 10,
    UDiv = 11,
    SRem = 12,
    URem = 13,
    FAdd = 14,
    FSub = 15,
    FMul = 16,
    FDiv = 17,
    FRem = 18,
    And = 19,
    Or = 20,
    Xor = 21,
    Shl = 22,
    LShr = 23,
    AShr = 24,
    Not = 25,
    ICmpEq = 26,
    ICmpNe = 27,
    ICmpSLt = 28,
    ICmpSLe = 29,
    ICmpSGt = 30,
    ICmpSGe = 31,
    ICmpULt = 32,
    ICmpULe = 33,
    ICmpUGt = 34,
    ICmpUGe = 35,
    FCmpEq = 36,
    FCmpNe = 37,
    FCmpLt = 38,
    FCmpLe = 39,
    FCmpGt = 40,
    FCmpGe = 41,
    AllocA = 42,
    Load = 43,
    Store = 44,
    GetElementPtr = 45,
    Trunc = 46,
    ZExt = 47,
    SExt = 48,
    FpToSi = 49,
    FpToUi = 50,
    SiToFp = 51,
    UiToFp = 52,
    PtrToInt = 53,
    IntToPtr = 54,
    BitCast = 55,
    Call = 56,
    CallIndirect = 57,
    ExtractValue = 58,
    InsertValue = 59,
    Phi = 60,
    Select = 61,
    Nop = 62,
}

impl IrInstrTag {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => IrInstrTag::Ret,
            2 => IrInstrTag::RetVoid,
            3 => IrInstrTag::Br,
            4 => IrInstrTag::BrCond,
            5 => IrInstrTag::Switch,
            6 => IrInstrTag::Unreachable,
            7 => IrInstrTag::Add,
            8 => IrInstrTag::Sub,
            9 => IrInstrTag::Mul,
            10 => IrInstrTag::SDiv,
            11 => IrInstrTag::UDiv,
            12 => IrInstrTag::SRem,
            13 => IrInstrTag::URem,
            14 => IrInstrTag::FAdd,
            15 => IrInstrTag::FSub,
            16 => IrInstrTag::FMul,
            17 => IrInstrTag::FDiv,
            18 => IrInstrTag::FRem,
            19 => IrInstrTag::And,
            20 => IrInstrTag::Or,
            21 => IrInstrTag::Xor,
            22 => IrInstrTag::Shl,
            23 => IrInstrTag::LShr,
            24 => IrInstrTag::AShr,
            25 => IrInstrTag::Not,
            26 => IrInstrTag::ICmpEq,
            27 => IrInstrTag::ICmpNe,
            28 => IrInstrTag::ICmpSLt,
            29 => IrInstrTag::ICmpSLe,
            30 => IrInstrTag::ICmpSGt,
            31 => IrInstrTag::ICmpSGe,
            32 => IrInstrTag::ICmpULt,
            33 => IrInstrTag::ICmpULe,
            34 => IrInstrTag::ICmpUGt,
            35 => IrInstrTag::ICmpUGe,
            36 => IrInstrTag::FCmpEq,
            37 => IrInstrTag::FCmpNe,
            38 => IrInstrTag::FCmpLt,
            39 => IrInstrTag::FCmpLe,
            40 => IrInstrTag::FCmpGt,
            41 => IrInstrTag::FCmpGe,
            42 => IrInstrTag::AllocA,
            43 => IrInstrTag::Load,
            44 => IrInstrTag::Store,
            45 => IrInstrTag::GetElementPtr,
            46 => IrInstrTag::Trunc,
            47 => IrInstrTag::ZExt,
            48 => IrInstrTag::SExt,
            49 => IrInstrTag::FpToSi,
            50 => IrInstrTag::FpToUi,
            51 => IrInstrTag::SiToFp,
            52 => IrInstrTag::UiToFp,
            53 => IrInstrTag::PtrToInt,
            54 => IrInstrTag::IntToPtr,
            55 => IrInstrTag::BitCast,
            56 => IrInstrTag::Call,
            57 => IrInstrTag::CallIndirect,
            58 => IrInstrTag::ExtractValue,
            59 => IrInstrTag::InsertValue,
            60 => IrInstrTag::Phi,
            61 => IrInstrTag::Select,
            62 => IrInstrTag::Nop,
            _ => IrInstrTag::Unknown,
        }
    }
}

/// Categories of generic id-lists tracked by the IR store.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum IrListKind {
    ValueList,
    InstrList,
    BlockList,
    ParamList,
    FunctionList,
    IntList,
}

// ── Stored records ────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct IrValueNode {
    pub tag: i64,
    /// For Register/Param: the slot id. For BlockAddr: block id.
    pub int_slot: i64,
    /// For ConstInt: integer payload. Otherwise unused.
    pub int_value: i64,
    /// For ConstFloat: float payload. Otherwise unused.
    pub float_value: f64,
    /// For ConstBool: 0 or 1. Otherwise unused.
    pub bool_value: i64,
    /// For ConstString / Global / Error: text payload.
    pub text: String,
    /// IR type id (or 0 if unknown).
    pub ty: i64,
}

/// Stored instruction record. Slots are interpreted per [`IrInstrTag`].
///
/// Slot conventions (any unused slot is `0` / empty):
/// - Ret: `value = operand IrValue id`.
/// - RetVoid / Unreachable / Nop: no slots used.
/// - Br: `target = block id`.
/// - BrCond: `cond = value id`, `then_target = block id`, `else_target = block id`.
/// - Binary arith / bitwise / cmp: `ty = ir type id`, `left = value id`, `right = value id`.
/// - Not: `ty`, `left = operand`.
/// - AllocA: `ty`, `int_extra = align`.
/// - Load: `ty`, `left = ptr value id`, `int_extra = align`.
/// - Store: `ty`, `left = value id`, `right = ptr value id`, `int_extra = align`.
/// - GEP: `ty`, `left = ptr id`, `right = indices list handle`.
/// - Trunc/ZExt/.../BitCast: `ty = to_ty`, `int_extra = from_ty`, `left = value id`.
/// - Call: `ty = ret_ty`, `left = callee value id`, `right = args list handle`.
/// - CallIndirect: same as Call but with `left = function pointer value id`.
/// - ExtractValue: `ty = agg_ty`, `int_extra = index`, `left = agg id`.
/// - InsertValue: `ty = agg_ty`, `int_extra = index`, `left = agg id`, `right = elem id`.
/// - Phi: `ty`, `right = incoming list handle`.
/// - Select: `cond_or_value = cond value id`, `left = true val`, `right = false val`, `ty`.
/// - Switch: `cond_or_value = scrutinee value id`, `then_target = default block id`,
///   `right = cases list handle`.
#[derive(Clone, Debug, Default)]
pub struct IrInstrNode {
    pub tag: i64,
    /// Most instructions: a type id. Some terminators: 0.
    pub ty: i64,
    /// First operand value id, or 0.
    pub left: i64,
    /// Second operand value id, or 0.
    pub right: i64,
    /// Cond value id (BrCond/Select/Switch) or third operand.
    pub cond_or_value: i64,
    /// Then/true branch block id (BrCond/Switch default).
    pub then_target: i64,
    /// Else/false branch block id (BrCond).
    pub else_target: i64,
    /// Extra integer payload (align, from_ty, index).
    pub int_extra: i64,
    /// Result IrValue id, or 0 for instructions that don't produce one.
    pub result: i64,
}

#[derive(Clone, Debug, Default)]
pub struct IrBlockNode {
    pub name: String,
    pub instrs: i64,
    pub preds: i64,
    pub succs: i64,
}

#[derive(Clone, Debug, Default)]
pub struct IrParamNode {
    pub name: String,
    pub ty: i64,
}

#[derive(Clone, Debug, Default)]
pub struct IrFunctionNode {
    pub name: String,
    pub params: i64,
    pub blocks: i64,
    pub ret_ty: i64,
    pub linkage: i64,
    pub is_variadic: i64,
    pub entry_block: i64,
}

#[derive(Clone, Debug, Default)]
pub struct IrModuleNode {
    pub name: String,
    pub functions: i64,
    pub entry_fn: i64,
}

#[derive(Clone, Debug, Default)]
pub struct IrTypeNode {
    pub tag: i64,
    /// For Ptr/Array: pointee/element type id.
    pub child: i64,
    /// For Array: size. For Func: ret type id.
    pub extra: i64,
    /// For Named/Opaque: name.
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct IrList {
    pub kind: Option<IrListKind>,
    pub items: Vec<i64>,
}

// ── Store ─────────────────────────────────────────────────────────────────

/// Process-wide runtime backing for the self-hosted IR builder.
#[derive(Debug, Default)]
pub struct BootstrapIrStore {
    types: Vec<IrTypeNode>,
    values: Vec<IrValueNode>,
    instrs: Vec<IrInstrNode>,
    blocks: Vec<IrBlockNode>,
    params: Vec<IrParamNode>,
    functions: Vec<IrFunctionNode>,
    modules: Vec<IrModuleNode>,
    lists: Vec<IrList>,
    next_register_slot: i64,
}

impl BootstrapIrStore {
    fn new() -> Self {
        Self {
            next_register_slot: 1,
            ..Self::default()
        }
    }

    fn alloc_type(&mut self, n: IrTypeNode) -> i64 {
        self.types.push(n);
        self.types.len() as i64
    }

    fn alloc_value(&mut self, n: IrValueNode) -> i64 {
        self.values.push(n);
        self.values.len() as i64
    }

    fn alloc_instr(&mut self, n: IrInstrNode) -> i64 {
        self.instrs.push(n);
        self.instrs.len() as i64
    }

    fn alloc_block(&mut self, n: IrBlockNode) -> i64 {
        self.blocks.push(n);
        self.blocks.len() as i64
    }

    fn alloc_param(&mut self, n: IrParamNode) -> i64 {
        self.params.push(n);
        self.params.len() as i64
    }

    fn alloc_function(&mut self, n: IrFunctionNode) -> i64 {
        self.functions.push(n);
        self.functions.len() as i64
    }

    fn alloc_module(&mut self, n: IrModuleNode) -> i64 {
        self.modules.push(n);
        self.modules.len() as i64
    }

    fn alloc_list(&mut self, kind: IrListKind) -> i64 {
        self.lists.push(IrList {
            kind: Some(kind),
            items: Vec::new(),
        });
        self.lists.len() as i64
    }

    fn list_append(&mut self, handle: i64, id: i64) -> i64 {
        if let Some(l) = self.list_mut(handle) {
            l.items.push(id);
            l.items.len() as i64
        } else {
            0
        }
    }

    fn list(&self, handle: i64) -> Option<&IrList> {
        if handle <= 0 {
            return None;
        }
        self.lists.get((handle - 1) as usize)
    }

    fn list_mut(&mut self, handle: i64) -> Option<&mut IrList> {
        if handle <= 0 {
            return None;
        }
        self.lists.get_mut((handle - 1) as usize)
    }

    pub fn type_count(&self) -> i64 {
        self.types.len() as i64
    }
    pub fn value_count(&self) -> i64 {
        self.values.len() as i64
    }
    pub fn instr_count(&self) -> i64 {
        self.instrs.len() as i64
    }
    pub fn block_count(&self) -> i64 {
        self.blocks.len() as i64
    }
    pub fn function_count(&self) -> i64 {
        self.functions.len() as i64
    }
    pub fn module_count(&self) -> i64 {
        self.modules.len() as i64
    }

    pub fn get_value(&self, id: i64) -> Option<&IrValueNode> {
        if id <= 0 {
            return None;
        }
        self.values.get((id - 1) as usize)
    }

    pub fn get_instr(&self, id: i64) -> Option<&IrInstrNode> {
        if id <= 0 {
            return None;
        }
        self.instrs.get((id - 1) as usize)
    }

    pub fn get_block(&self, id: i64) -> Option<&IrBlockNode> {
        if id <= 0 {
            return None;
        }
        self.blocks.get((id - 1) as usize)
    }

    pub fn get_param(&self, id: i64) -> Option<&IrParamNode> {
        if id <= 0 {
            return None;
        }
        self.params.get((id - 1) as usize)
    }

    pub fn get_function(&self, id: i64) -> Option<&IrFunctionNode> {
        if id <= 0 {
            return None;
        }
        self.functions.get((id - 1) as usize)
    }

    pub fn get_module(&self, id: i64) -> Option<&IrModuleNode> {
        if id <= 0 {
            return None;
        }
        self.modules.get((id - 1) as usize)
    }

    pub fn get_type(&self, id: i64) -> Option<&IrTypeNode> {
        if id <= 0 {
            return None;
        }
        self.types.get((id - 1) as usize)
    }

    pub fn list_len(&self, handle: i64) -> i64 {
        self.list(handle).map(|l| l.items.len() as i64).unwrap_or(0)
    }

    pub fn list_get(&self, handle: i64, index: i64) -> i64 {
        if index < 0 {
            return 0;
        }
        self.list(handle)
            .and_then(|l| l.items.get(index as usize).copied())
            .unwrap_or(0)
    }
}

fn store() -> &'static Mutex<BootstrapIrStore> {
    static STORE: OnceLock<Mutex<BootstrapIrStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BootstrapIrStore::new()))
}

fn lock() -> MutexGuard<'static, BootstrapIrStore> {
    store().lock().unwrap_or_else(|p| p.into_inner())
}

/// Reset the ambient IR store. Tests that drive the bridge must call this
/// before running so they see a clean slate.
pub fn reset_ir_store() {
    let mut s = lock();
    *s = BootstrapIrStore::new();
}

/// Run a closure with mutable access to the ambient store.
pub fn with_ir_store<R>(f: impl FnOnce(&mut BootstrapIrStore) -> R) -> R {
    let mut s = lock();
    f(&mut s)
}

/// Run a closure with shared access to the ambient store.
pub fn with_ir_store_ref<R>(f: impl FnOnce(&BootstrapIrStore) -> R) -> R {
    let s = lock();
    f(&s)
}

// ── Type alloc / get ─────────────────────────────────────────────────────

/// Process-wide lock shared across all `bootstrap_*` unit-test modules
/// that touch the same global stores (IR, AST, pipeline, driver). Tests
/// must acquire this lock instead of defining their own per-module
/// `Mutex<()>` — independent locks let parallel test runs from different
/// crates / modules race on the shared stores and produce flaky
/// failures. Holding `shared_test_lock()` while resetting + driving the
/// stores serialises every bootstrap test against every other one.
#[doc(hidden)]
pub fn shared_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

pub fn bootstrap_ir_type_alloc_primitive(tag: i64) -> i64 {
    with_ir_store(|s| {
        s.alloc_type(IrTypeNode {
            tag,
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_type_alloc_ptr(pointee: i64) -> i64 {
    with_ir_store(|s| {
        s.alloc_type(IrTypeNode {
            tag: IrTypeTag::Ptr as i64,
            child: pointee,
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_type_alloc_named(name: &str) -> i64 {
    with_ir_store(|s| {
        s.alloc_type(IrTypeNode {
            tag: IrTypeTag::Named as i64,
            name: name.to_string(),
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_type_get_tag(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_type(id).map(|t| t.tag).unwrap_or(0))
}

pub fn bootstrap_ir_type_get_child(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_type(id).map(|t| t.child).unwrap_or(0))
}

pub fn bootstrap_ir_type_get_name(id: i64) -> String {
    with_ir_store_ref(|s| s.get_type(id).map(|t| t.name.clone()).unwrap_or_default())
}

// ── Value alloc / get ────────────────────────────────────────────────────

pub fn bootstrap_ir_value_alloc_const_int(ty: i64, value: i64) -> i64 {
    with_ir_store(|s| {
        s.alloc_value(IrValueNode {
            tag: IrValueTag::ConstInt as i64,
            ty,
            int_value: value,
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_value_alloc_const_bool(value: i64) -> i64 {
    with_ir_store(|s| {
        s.alloc_value(IrValueNode {
            tag: IrValueTag::ConstBool as i64,
            ty: IrTypeTag::Bool as i64,
            bool_value: if value != 0 { 1 } else { 0 },
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_value_alloc_const_string(value: &str) -> i64 {
    with_ir_store(|s| {
        s.alloc_value(IrValueNode {
            tag: IrValueTag::ConstString as i64,
            text: value.to_string(),
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_value_alloc_const_float(ty: i64, value: f64) -> i64 {
    with_ir_store(|s| {
        s.alloc_value(IrValueNode {
            tag: IrValueTag::ConstFloat as i64,
            ty,
            float_value: value,
            ..Default::default()
        })
    })
}

/// Allocate a fresh register value with monotonically-increasing slot id.
pub fn bootstrap_ir_value_alloc_register(ty: i64) -> i64 {
    with_ir_store(|s| {
        let slot = s.next_register_slot;
        s.next_register_slot += 1;
        s.alloc_value(IrValueNode {
            tag: IrValueTag::Register as i64,
            ty,
            int_slot: slot,
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_value_alloc_param(index: i64, ty: i64) -> i64 {
    with_ir_store(|s| {
        s.alloc_value(IrValueNode {
            tag: IrValueTag::Param as i64,
            ty,
            int_slot: index,
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_value_alloc_global(name: &str, ty: i64) -> i64 {
    with_ir_store(|s| {
        s.alloc_value(IrValueNode {
            tag: IrValueTag::Global as i64,
            ty,
            text: name.to_string(),
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_value_alloc_undef(ty: i64) -> i64 {
    with_ir_store(|s| {
        s.alloc_value(IrValueNode {
            tag: IrValueTag::ConstUndef as i64,
            ty,
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_value_alloc_error(message: &str) -> i64 {
    with_ir_store(|s| {
        s.alloc_value(IrValueNode {
            tag: IrValueTag::Error as i64,
            text: message.to_string(),
            ..Default::default()
        })
    })
}

pub fn bootstrap_ir_value_get_tag(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_value(id).map(|v| v.tag).unwrap_or(0))
}

pub fn bootstrap_ir_value_get_type(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_value(id).map(|v| v.ty).unwrap_or(0))
}

pub fn bootstrap_ir_value_get_int(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_value(id).map(|v| v.int_value).unwrap_or(0))
}

pub fn bootstrap_ir_value_get_bool(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_value(id).map(|v| v.bool_value).unwrap_or(0))
}

pub fn bootstrap_ir_value_get_slot(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_value(id).map(|v| v.int_slot).unwrap_or(0))
}

pub fn bootstrap_ir_value_get_text(id: i64) -> String {
    with_ir_store_ref(|s| s.get_value(id).map(|v| v.text.clone()).unwrap_or_default())
}

// ── Instruction alloc / get ──────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn bootstrap_ir_instr_alloc(
    tag: i64,
    ty: i64,
    left: i64,
    right: i64,
    cond_or_value: i64,
    then_target: i64,
    else_target: i64,
    int_extra: i64,
    result: i64,
) -> i64 {
    with_ir_store(|s| {
        s.alloc_instr(IrInstrNode {
            tag,
            ty,
            left,
            right,
            cond_or_value,
            then_target,
            else_target,
            int_extra,
            result,
        })
    })
}

pub fn bootstrap_ir_instr_get_tag(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.tag).unwrap_or(0))
}

pub fn bootstrap_ir_instr_get_type(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.ty).unwrap_or(0))
}

pub fn bootstrap_ir_instr_get_left(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.left).unwrap_or(0))
}

pub fn bootstrap_ir_instr_get_right(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.right).unwrap_or(0))
}

pub fn bootstrap_ir_instr_get_cond(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.cond_or_value).unwrap_or(0))
}

pub fn bootstrap_ir_instr_get_then_target(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.then_target).unwrap_or(0))
}

pub fn bootstrap_ir_instr_get_else_target(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.else_target).unwrap_or(0))
}

pub fn bootstrap_ir_instr_get_int_extra(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.int_extra).unwrap_or(0))
}

pub fn bootstrap_ir_instr_get_result(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_instr(id).map(|i| i.result).unwrap_or(0))
}

// ── Block alloc / get ────────────────────────────────────────────────────

pub fn bootstrap_ir_block_alloc(name: &str) -> i64 {
    with_ir_store(|s| {
        let instrs = s.alloc_list(IrListKind::InstrList);
        let preds = s.alloc_list(IrListKind::IntList);
        let succs = s.alloc_list(IrListKind::IntList);
        s.alloc_block(IrBlockNode {
            name: name.to_string(),
            instrs,
            preds,
            succs,
        })
    })
}

pub fn bootstrap_ir_block_append_instr(block_id: i64, instr_id: i64) -> i64 {
    with_ir_store(|s| {
        let instrs = s.get_block(block_id).map(|b| b.instrs).unwrap_or(0);
        s.list_append(instrs, instr_id)
    })
}

pub fn bootstrap_ir_block_get_name(id: i64) -> String {
    with_ir_store_ref(|s| s.get_block(id).map(|b| b.name.clone()).unwrap_or_default())
}

pub fn bootstrap_ir_block_get_instrs(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_block(id).map(|b| b.instrs).unwrap_or(0))
}

pub fn bootstrap_ir_block_get_instr_count(id: i64) -> i64 {
    with_ir_store_ref(|s| {
        let h = s.get_block(id).map(|b| b.instrs).unwrap_or(0);
        s.list_len(h)
    })
}

pub fn bootstrap_ir_block_get_instr_at(id: i64, index: i64) -> i64 {
    with_ir_store_ref(|s| {
        let h = s.get_block(id).map(|b| b.instrs).unwrap_or(0);
        s.list_get(h, index)
    })
}

// ── Param alloc / get ────────────────────────────────────────────────────

pub fn bootstrap_ir_param_alloc(name: &str, ty: i64) -> i64 {
    with_ir_store(|s| {
        s.alloc_param(IrParamNode {
            name: name.to_string(),
            ty,
        })
    })
}

pub fn bootstrap_ir_param_get_name(id: i64) -> String {
    with_ir_store_ref(|s| s.get_param(id).map(|p| p.name.clone()).unwrap_or_default())
}

pub fn bootstrap_ir_param_get_type(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_param(id).map(|p| p.ty).unwrap_or(0))
}

// ── Function alloc / get ─────────────────────────────────────────────────

pub fn bootstrap_ir_function_alloc(name: &str, ret_ty: i64) -> i64 {
    with_ir_store(|s| {
        let params = s.alloc_list(IrListKind::ParamList);
        let blocks = s.alloc_list(IrListKind::BlockList);
        s.alloc_function(IrFunctionNode {
            name: name.to_string(),
            params,
            blocks,
            ret_ty,
            linkage: 0,
            is_variadic: 0,
            entry_block: 0,
        })
    })
}

pub fn bootstrap_ir_function_append_param(fn_id: i64, param_id: i64) -> i64 {
    with_ir_store(|s| {
        let h = s.get_function(fn_id).map(|f| f.params).unwrap_or(0);
        s.list_append(h, param_id)
    })
}

pub fn bootstrap_ir_function_append_block(fn_id: i64, block_id: i64) -> i64 {
    with_ir_store(|s| {
        let h = s.get_function(fn_id).map(|f| f.blocks).unwrap_or(0);
        let pos = s.list_append(h, block_id);
        // Record the first appended block as the entry block.
        if pos == 1 {
            if let Ok(idx) = usize::try_from(fn_id) {
                if idx > 0 {
                    if let Some(f) = s.functions.get_mut(idx - 1) {
                        f.entry_block = block_id;
                    }
                }
            }
        }
        pos
    })
}

pub fn bootstrap_ir_function_get_name(id: i64) -> String {
    with_ir_store_ref(|s| {
        s.get_function(id)
            .map(|f| f.name.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_ir_function_get_ret_type(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_function(id).map(|f| f.ret_ty).unwrap_or(0))
}

pub fn bootstrap_ir_function_get_params(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_function(id).map(|f| f.params).unwrap_or(0))
}

pub fn bootstrap_ir_function_get_blocks(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_function(id).map(|f| f.blocks).unwrap_or(0))
}

pub fn bootstrap_ir_function_get_param_count(id: i64) -> i64 {
    with_ir_store_ref(|s| {
        let h = s.get_function(id).map(|f| f.params).unwrap_or(0);
        s.list_len(h)
    })
}

pub fn bootstrap_ir_function_get_param_at(id: i64, index: i64) -> i64 {
    with_ir_store_ref(|s| {
        let h = s.get_function(id).map(|f| f.params).unwrap_or(0);
        s.list_get(h, index)
    })
}

pub fn bootstrap_ir_function_get_block_count(id: i64) -> i64 {
    with_ir_store_ref(|s| {
        let h = s.get_function(id).map(|f| f.blocks).unwrap_or(0);
        s.list_len(h)
    })
}

pub fn bootstrap_ir_function_get_block_at(id: i64, index: i64) -> i64 {
    with_ir_store_ref(|s| {
        let h = s.get_function(id).map(|f| f.blocks).unwrap_or(0);
        s.list_get(h, index)
    })
}

pub fn bootstrap_ir_function_get_entry_block(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_function(id).map(|f| f.entry_block).unwrap_or(0))
}

// ── Module alloc / get ───────────────────────────────────────────────────

pub fn bootstrap_ir_module_alloc(name: &str) -> i64 {
    with_ir_store(|s| {
        let functions = s.alloc_list(IrListKind::FunctionList);
        s.alloc_module(IrModuleNode {
            name: name.to_string(),
            functions,
            entry_fn: 0,
        })
    })
}

pub fn bootstrap_ir_module_append_function(mod_id: i64, fn_id: i64) -> i64 {
    with_ir_store(|s| {
        let h = s.get_module(mod_id).map(|m| m.functions).unwrap_or(0);
        s.list_append(h, fn_id)
    })
}

pub fn bootstrap_ir_module_set_entry(mod_id: i64, fn_id: i64) -> i64 {
    with_ir_store(|s| {
        if let Ok(idx) = usize::try_from(mod_id) {
            if idx > 0 {
                if let Some(m) = s.modules.get_mut(idx - 1) {
                    m.entry_fn = fn_id;
                    return 1;
                }
            }
        }
        0
    })
}

pub fn bootstrap_ir_module_get_name(id: i64) -> String {
    with_ir_store_ref(|s| s.get_module(id).map(|m| m.name.clone()).unwrap_or_default())
}

pub fn bootstrap_ir_module_get_functions(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_module(id).map(|m| m.functions).unwrap_or(0))
}

pub fn bootstrap_ir_module_get_entry_fn(id: i64) -> i64 {
    with_ir_store_ref(|s| s.get_module(id).map(|m| m.entry_fn).unwrap_or(0))
}

pub fn bootstrap_ir_module_get_function_count(id: i64) -> i64 {
    with_ir_store_ref(|s| {
        let h = s.get_module(id).map(|m| m.functions).unwrap_or(0);
        s.list_len(h)
    })
}

pub fn bootstrap_ir_module_get_function_at(id: i64, index: i64) -> i64 {
    with_ir_store_ref(|s| {
        let h = s.get_module(id).map(|m| m.functions).unwrap_or(0);
        s.list_get(h, index)
    })
}

// ── Generic value-list helpers (for call args, switch cases, etc.) ───────

pub fn bootstrap_ir_value_list_alloc() -> i64 {
    with_ir_store(|s| s.alloc_list(IrListKind::ValueList))
}

pub fn bootstrap_ir_int_list_alloc() -> i64 {
    with_ir_store(|s| s.alloc_list(IrListKind::IntList))
}

pub fn bootstrap_ir_list_append(handle: i64, id: i64) -> i64 {
    with_ir_store(|s| s.list_append(handle, id))
}

pub fn bootstrap_ir_list_len(handle: i64) -> i64 {
    with_ir_store_ref(|s| s.list_len(handle))
}

pub fn bootstrap_ir_list_get(handle: i64, index: i64) -> i64 {
    with_ir_store_ref(|s| s.list_get(handle, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn module_with_one_function_round_trips() {
        let _g = t_lock();
        reset_ir_store();
        let i32_ty = bootstrap_ir_type_alloc_primitive(IrTypeTag::I32 as i64);
        let m = bootstrap_ir_module_alloc("m");
        let f = bootstrap_ir_function_alloc("answer", i32_ty);
        bootstrap_ir_module_append_function(m, f);
        bootstrap_ir_module_set_entry(m, f);
        let entry = bootstrap_ir_block_alloc("entry");
        bootstrap_ir_function_append_block(f, entry);
        let v = bootstrap_ir_value_alloc_const_int(i32_ty, 42);
        let ret_real = bootstrap_ir_instr_alloc(IrInstrTag::Ret as i64, 0, 0, 0, v, 0, 0, 0, 0);
        bootstrap_ir_block_append_instr(entry, ret_real);

        assert_eq!(bootstrap_ir_module_get_name(m), "m");
        assert_eq!(bootstrap_ir_module_get_function_count(m), 1);
        assert_eq!(bootstrap_ir_module_get_entry_fn(m), f);
        assert_eq!(bootstrap_ir_function_get_name(f), "answer");
        assert_eq!(bootstrap_ir_function_get_block_count(f), 1);
        assert_eq!(bootstrap_ir_function_get_entry_block(f), entry);
        assert_eq!(bootstrap_ir_block_get_instr_count(entry), 1);
        let last = bootstrap_ir_block_get_instr_at(entry, 0);
        assert_eq!(bootstrap_ir_instr_get_tag(last), IrInstrTag::Ret as i64);
        assert_eq!(bootstrap_ir_instr_get_cond(last), v);
        assert_eq!(bootstrap_ir_value_get_int(v), 42);
    }

    #[test]
    fn unknown_ids_return_safe_defaults() {
        let _g = t_lock();
        reset_ir_store();
        assert_eq!(bootstrap_ir_module_get_name(99999), "");
        assert_eq!(bootstrap_ir_function_get_name(99999), "");
        assert_eq!(bootstrap_ir_block_get_name(99999), "");
        assert_eq!(bootstrap_ir_value_get_int(99999), 0);
        assert_eq!(bootstrap_ir_instr_get_tag(99999), 0);
        assert_eq!(bootstrap_ir_list_len(99999), 0);
        assert_eq!(bootstrap_ir_list_get(99999, 0), 0);
    }

    #[test]
    fn registers_have_monotonic_slot_ids() {
        let _g = t_lock();
        reset_ir_store();
        let i32_ty = bootstrap_ir_type_alloc_primitive(IrTypeTag::I32 as i64);
        let r1 = bootstrap_ir_value_alloc_register(i32_ty);
        let r2 = bootstrap_ir_value_alloc_register(i32_ty);
        let r3 = bootstrap_ir_value_alloc_register(i32_ty);
        assert_eq!(bootstrap_ir_value_get_slot(r1), 1);
        assert_eq!(bootstrap_ir_value_get_slot(r2), 2);
        assert_eq!(bootstrap_ir_value_get_slot(r3), 3);
    }

    #[test]
    fn binary_add_records_operands_and_result() {
        let _g = t_lock();
        reset_ir_store();
        let i32_ty = bootstrap_ir_type_alloc_primitive(IrTypeTag::I32 as i64);
        let a = bootstrap_ir_value_alloc_const_int(i32_ty, 1);
        let b = bootstrap_ir_value_alloc_const_int(i32_ty, 2);
        let r = bootstrap_ir_value_alloc_register(i32_ty);
        let i = bootstrap_ir_instr_alloc(IrInstrTag::Add as i64, i32_ty, a, b, 0, 0, 0, 0, r);
        assert_eq!(bootstrap_ir_instr_get_tag(i), IrInstrTag::Add as i64);
        assert_eq!(bootstrap_ir_instr_get_type(i), i32_ty);
        assert_eq!(bootstrap_ir_instr_get_left(i), a);
        assert_eq!(bootstrap_ir_instr_get_right(i), b);
        assert_eq!(bootstrap_ir_instr_get_result(i), r);
    }

    #[test]
    fn br_cond_records_targets() {
        let _g = t_lock();
        reset_ir_store();
        let cond = bootstrap_ir_value_alloc_const_bool(1);
        let then_b = bootstrap_ir_block_alloc("then");
        let else_b = bootstrap_ir_block_alloc("else");
        let i = bootstrap_ir_instr_alloc(
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
        assert_eq!(bootstrap_ir_instr_get_cond(i), cond);
        assert_eq!(bootstrap_ir_instr_get_then_target(i), then_b);
        assert_eq!(bootstrap_ir_instr_get_else_target(i), else_b);
    }

    #[test]
    fn function_params_round_trip_through_list() {
        let _g = t_lock();
        reset_ir_store();
        let i32_ty = bootstrap_ir_type_alloc_primitive(IrTypeTag::I32 as i64);
        let f = bootstrap_ir_function_alloc("add", i32_ty);
        let p1 = bootstrap_ir_param_alloc("a", i32_ty);
        let p2 = bootstrap_ir_param_alloc("b", i32_ty);
        bootstrap_ir_function_append_param(f, p1);
        bootstrap_ir_function_append_param(f, p2);
        assert_eq!(bootstrap_ir_function_get_param_count(f), 2);
        let first = bootstrap_ir_function_get_param_at(f, 0);
        assert_eq!(bootstrap_ir_param_get_name(first), "a");
        let second = bootstrap_ir_function_get_param_at(f, 1);
        assert_eq!(bootstrap_ir_param_get_name(second), "b");
        assert_eq!(bootstrap_ir_param_get_type(second), i32_ty);
    }

    #[test]
    fn call_records_args_through_value_list() {
        let _g = t_lock();
        reset_ir_store();
        let i32_ty = bootstrap_ir_type_alloc_primitive(IrTypeTag::I32 as i64);
        let callee = bootstrap_ir_value_alloc_global("add", i32_ty);
        let a = bootstrap_ir_value_alloc_const_int(i32_ty, 1);
        let b = bootstrap_ir_value_alloc_const_int(i32_ty, 2);
        let args = bootstrap_ir_value_list_alloc();
        bootstrap_ir_list_append(args, a);
        bootstrap_ir_list_append(args, b);
        let r = bootstrap_ir_value_alloc_register(i32_ty);
        let i =
            bootstrap_ir_instr_alloc(IrInstrTag::Call as i64, i32_ty, callee, args, 0, 0, 0, 0, r);
        assert_eq!(bootstrap_ir_instr_get_left(i), callee);
        assert_eq!(bootstrap_ir_instr_get_right(i), args);
        assert_eq!(bootstrap_ir_list_len(args), 2);
        assert_eq!(bootstrap_ir_list_get(args, 0), a);
        assert_eq!(bootstrap_ir_list_get(args, 1), b);
    }
}
