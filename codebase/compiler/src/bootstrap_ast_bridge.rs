//! Runtime-backed AST node storage for the self-hosted parser.
//!
//! Issue #222: until the self-hosted runtime can execute `parser.gr` directly,
//! we model the bootstrap AST node store in Rust so the rewritten Gradient
//! parser exercises the same primitive FFI it will use once execution lands.
//! Each `bootstrap_*_alloc_*` extern declared in `compiler/parser.gr` maps to
//! a function on [`BootstrapAstStore`] that allocates a real node id and
//! returns it as the handle the parser then plumbs into parent nodes.
//!
//! Scope: the bootstrap parser corpus (single-function modules with binary
//! expressions, identifiers, integer/string/bool literals, calls, if/else,
//! blocks, let/expr/ret statements, function params, named/Int/Bool/String
//! return types). Variants outside that scope are stored as opaque payloads
//! so future issues can extend the store without breaking the Phase-0
//! contract.
//!
//! The accessor side (`bootstrap_*_get_*`) lets the normalized export walk
//! the stored tree rather than re-derive it from in-memory `Expr` / `Stmt`
//! values. Out-of-bounds and unknown ids return safe defaults (`tag = 0`,
//! empty string, child id `0`) so parser execution can keep advancing
//! without panicking.

use std::cell::RefCell;
use std::sync::Mutex;

/// Discriminator tags for stored expression nodes.
///
/// Tags match the case order of `Expr` in `compiler/parser.gr`, so the
/// self-hosted parser and the Rust mirror agree on the same encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum ExprTag {
    Unknown = 0,
    IntLit = 1,
    FloatLit = 2,
    StringLit = 3,
    BoolLit = 4,
    Ident = 5,
    Binary = 6,
    Unary = 7,
    Call = 8,
    If = 9,
    Block = 10,
    Match = 11,
    Lambda = 12,
    FieldAccess = 13,
    Index = 14,
    Assign = 15,
    Error = 16,
}

impl ExprTag {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => ExprTag::IntLit,
            2 => ExprTag::FloatLit,
            3 => ExprTag::StringLit,
            4 => ExprTag::BoolLit,
            5 => ExprTag::Ident,
            6 => ExprTag::Binary,
            7 => ExprTag::Unary,
            8 => ExprTag::Call,
            9 => ExprTag::If,
            10 => ExprTag::Block,
            11 => ExprTag::Match,
            12 => ExprTag::Lambda,
            13 => ExprTag::FieldAccess,
            14 => ExprTag::Index,
            15 => ExprTag::Assign,
            16 => ExprTag::Error,
            _ => ExprTag::Unknown,
        }
    }
}

/// Discriminator tags for stored statement nodes.
///
/// Tags match the case order of `Stmt` in `compiler/parser.gr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum StmtTag {
    Unknown = 0,
    Let = 1,
    Expr = 2,
    Ret = 3,
    If = 4,
    While = 5,
    For = 6,
    Match = 7,
    Break = 8,
    Continue = 9,
    Defer = 10,
    Assign = 11,
    Error = 12,
}

impl StmtTag {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => StmtTag::Let,
            2 => StmtTag::Expr,
            3 => StmtTag::Ret,
            4 => StmtTag::If,
            5 => StmtTag::While,
            6 => StmtTag::For,
            7 => StmtTag::Match,
            8 => StmtTag::Break,
            9 => StmtTag::Continue,
            10 => StmtTag::Defer,
            11 => StmtTag::Assign,
            12 => StmtTag::Error,
            _ => StmtTag::Unknown,
        }
    }
}

/// Discriminator tags for stored type expression nodes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum TypeTag {
    Unknown = 0,
    Int = 1,
    Float = 2,
    Bool = 3,
    String = 4,
    Unit = 5,
    Named = 6,
}

impl TypeTag {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => TypeTag::Int,
            2 => TypeTag::Float,
            3 => TypeTag::Bool,
            4 => TypeTag::String,
            5 => TypeTag::Unit,
            6 => TypeTag::Named,
            _ => TypeTag::Unknown,
        }
    }
}

/// Discriminator tags for stored module items.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum ModuleItemTag {
    Unknown = 0,
    Function = 1,
    Type = 2,
    Trait = 3,
    Impl = 4,
    Use = 5,
    Error = 6,
}

impl ModuleItemTag {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => ModuleItemTag::Function,
            2 => ModuleItemTag::Type,
            3 => ModuleItemTag::Trait,
            4 => ModuleItemTag::Impl,
            5 => ModuleItemTag::Use,
            6 => ModuleItemTag::Error,
            _ => ModuleItemTag::Unknown,
        }
    }
}

/// A single stored expression node. Slots are interpreted per [`ExprTag`].
///
/// Slot conventions (any unused slot is `0` / empty):
/// - IntLit: `int_value`
/// - BoolLit: `int_value` (0/1)
/// - StringLit: `text`
/// - Ident: `text = name`
/// - Binary: `int_value = op_tag`, `child_a = left_id`, `child_b = right_id`
/// - Unary: `int_value = op_tag`, `child_a = operand_id`
/// - Call: `child_a = callee_id`, `child_b = args_list_handle`
/// - If: `child_a = cond_id`, `child_b = then_id`, `child_c = else_id`
/// - Block: `child_a = stmt_list_handle`, `child_b = final_expr_id`
/// - FieldAccess: `child_a = obj_id`, `text = field`
/// - Index: `child_a = obj_id`, `child_b = index_id`
/// - Assign: `child_a = target_id`, `child_b = value_id`
/// - Error: `text = message`
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExprNode {
    pub tag: i64,
    pub int_value: i64,
    pub child_a: i64,
    pub child_b: i64,
    pub child_c: i64,
    pub text: String,
}

/// A single stored statement node.
///
/// Slot conventions:
/// - Let: `int_value = is_mut (0/1)`, `child_a = type_id`, `child_b = value_expr_id`,
///   `child_c = pattern_handle`, `text = pattern_name` (best-effort for Ident patterns).
/// - Expr / Ret / Defer: `child_a = expr_id`.
/// - If: `child_a = cond_id`, `child_b = then_block_handle`, `child_c = else_block_handle`.
/// - While: `child_a = cond_id`, `child_b = body_block_handle`.
/// - For: `child_a = iter_id`, `child_b = body_block_handle`, `child_c = pattern_handle`,
///   `text = pattern_name`.
/// - Assign: `child_a = target_id`, `child_b = value_id`.
/// - Break / Continue: no slots used.
/// - Error: `text = message`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StmtNode {
    pub tag: i64,
    pub int_value: i64,
    pub child_a: i64,
    pub child_b: i64,
    pub child_c: i64,
    pub text: String,
}

/// A stored function parameter. `type_tag` matches [`TypeTag`]; `type_name`
/// carries the named-type label for [`TypeTag::Named`]. `default_id` is `0`
/// when there is no default.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParamNode {
    pub name: String,
    pub type_tag: i64,
    pub type_name: String,
    pub default_id: i64,
}

/// A stored function definition.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionNode {
    pub name: String,
    pub params_handle: i64,
    pub ret_type_tag: i64,
    pub ret_type_name: String,
    pub body_handle: i64,
    pub is_pub: i64,
    pub is_extern: i64,
}

/// A stored module item.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModuleItemNode {
    pub tag: i64,
    pub function_id: i64,
    pub name: String,
}

/// Categories of node-id lists tracked by the AST store.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AstListKind {
    ExprList,
    StmtList,
    ParamList,
    ModuleItemList,
}

/// Generic node-id list backing the `bootstrap_*_list_*` externs.
#[derive(Clone, Debug, Default)]
pub struct AstList {
    pub kind: Option<AstListKind>,
    pub items: Vec<i64>,
}

/// In-memory store for runtime-backed AST nodes used by the self-hosted parser.
///
/// All ids are non-zero `i64`s starting at 1; id `0` is reserved as the
/// "no node" sentinel that propagates safely through the FFI when callers
/// branch on `child == 0`. Each kind has its own id space — expr ids are
/// distinct from stmt ids, etc. — so callers must use the matching getter
/// for the kind they appended.
#[derive(Clone, Debug, Default)]
pub struct BootstrapAstStore {
    exprs: Vec<ExprNode>,
    stmts: Vec<StmtNode>,
    params: Vec<ParamNode>,
    functions: Vec<FunctionNode>,
    module_items: Vec<ModuleItemNode>,
    lists: Vec<AstList>,
}

impl BootstrapAstStore {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Expression alloc / get ────────────────────────────────────────────

    pub fn alloc_expr(&mut self, node: ExprNode) -> i64 {
        self.exprs.push(node);
        self.exprs.len() as i64
    }

    pub fn get_expr(&self, id: i64) -> Option<&ExprNode> {
        if id <= 0 {
            return None;
        }
        self.exprs.get((id - 1) as usize)
    }

    pub fn expr_count(&self) -> i64 {
        self.exprs.len() as i64
    }

    // ── Statement alloc / get ─────────────────────────────────────────────

    pub fn alloc_stmt(&mut self, node: StmtNode) -> i64 {
        self.stmts.push(node);
        self.stmts.len() as i64
    }

    pub fn get_stmt(&self, id: i64) -> Option<&StmtNode> {
        if id <= 0 {
            return None;
        }
        self.stmts.get((id - 1) as usize)
    }

    pub fn stmt_count(&self) -> i64 {
        self.stmts.len() as i64
    }

    // ── Param alloc / get ─────────────────────────────────────────────────

    pub fn alloc_param(&mut self, node: ParamNode) -> i64 {
        self.params.push(node);
        self.params.len() as i64
    }

    pub fn get_param(&self, id: i64) -> Option<&ParamNode> {
        if id <= 0 {
            return None;
        }
        self.params.get((id - 1) as usize)
    }

    // ── Function alloc / get ──────────────────────────────────────────────

    pub fn alloc_function(&mut self, node: FunctionNode) -> i64 {
        self.functions.push(node);
        self.functions.len() as i64
    }

    pub fn get_function(&self, id: i64) -> Option<&FunctionNode> {
        if id <= 0 {
            return None;
        }
        self.functions.get((id - 1) as usize)
    }

    // ── Module item alloc / get ───────────────────────────────────────────

    pub fn alloc_module_item(&mut self, node: ModuleItemNode) -> i64 {
        self.module_items.push(node);
        self.module_items.len() as i64
    }

    pub fn get_module_item(&self, id: i64) -> Option<&ModuleItemNode> {
        if id <= 0 {
            return None;
        }
        self.module_items.get((id - 1) as usize)
    }

    // ── Generic node-id lists ─────────────────────────────────────────────

    pub fn list_alloc(&mut self, kind: AstListKind) -> i64 {
        self.lists.push(AstList {
            kind: Some(kind),
            items: Vec::new(),
        });
        self.lists.len() as i64
    }

    pub fn list_append(&mut self, handle: i64, id: i64) -> i64 {
        if let Some(list) = self.list_mut(handle) {
            list.items.push(id);
            list.items.len() as i64
        } else {
            0
        }
    }

    pub fn list_len(&self, handle: i64) -> i64 {
        self.list(handle)
            .map(|l| l.items.len() as i64)
            .unwrap_or(0)
    }

    pub fn list_get(&self, handle: i64, index: i64) -> i64 {
        if index < 0 {
            return 0;
        }
        self.list(handle)
            .and_then(|l| l.items.get(index as usize).copied())
            .unwrap_or(0)
    }

    pub fn list_kind(&self, handle: i64) -> Option<AstListKind> {
        self.list(handle).and_then(|l| l.kind)
    }

    fn list(&self, handle: i64) -> Option<&AstList> {
        if handle <= 0 {
            return None;
        }
        self.lists.get((handle - 1) as usize)
    }

    fn list_mut(&mut self, handle: i64) -> Option<&mut AstList> {
        if handle <= 0 {
            return None;
        }
        self.lists.get_mut((handle - 1) as usize)
    }
}

/// Process-wide ambient AST store used by the self-hosted parser bridge.
///
/// The self-hosted parser drives the bootstrap_* externs imperatively while
/// constructing nodes; the externs need somewhere to land their state. A
/// `Mutex<RefCell<...>>` keeps the contract single-threaded but explicit:
/// each test that exercises the bridge calls [`reset_ast_store`] before
/// running so prior state never leaks across cases.
fn ast_store() -> &'static Mutex<RefCell<BootstrapAstStore>> {
    use std::sync::OnceLock;
    static STORE: OnceLock<Mutex<RefCell<BootstrapAstStore>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(RefCell::new(BootstrapAstStore::new())))
}

/// Replace the ambient AST store with a fresh, empty one. Tests must call
/// this before exercising the bridge to keep id spaces isolated.
pub fn reset_ast_store() {
    let cell = ast_store().lock().expect("ast store lock poisoned");
    *cell.borrow_mut() = BootstrapAstStore::new();
}

/// Run `f` against a mutable view of the ambient AST store.
pub fn with_ast_store<R>(f: impl FnOnce(&mut BootstrapAstStore) -> R) -> R {
    let cell = ast_store().lock().expect("ast store lock poisoned");
    let mut store = cell.borrow_mut();
    f(&mut store)
}

/// Run `f` against a read-only view of the ambient AST store.
pub fn with_ast_store_ref<R>(f: impl FnOnce(&BootstrapAstStore) -> R) -> R {
    let cell = ast_store().lock().expect("ast store lock poisoned");
    let store = cell.borrow();
    f(&store)
}

// ── FFI-shaped accessors that mirror parser.gr externs ────────────────────
//
// Each extern declared in `compiler/parser.gr` has a corresponding free
// function here. Callers in tests (and, eventually, the self-hosted runtime)
// drive the same surface. The functions are intentionally narrow: they
// accept `Int`/`String`-shaped arguments (i64 / &str / String) and return
// the same types so the FFI contract is verifiable.

pub fn bootstrap_expr_alloc_int_lit(value: i64) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::IntLit as i64,
            int_value: value,
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_bool_lit(value: i64) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::BoolLit as i64,
            int_value: if value != 0 { 1 } else { 0 },
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_string_lit(value: &str) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::StringLit as i64,
            text: value.to_string(),
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_ident(name: &str) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::Ident as i64,
            text: name.to_string(),
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_binary(op_tag: i64, left: i64, right: i64) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::Binary as i64,
            int_value: op_tag,
            child_a: left,
            child_b: right,
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_unary(op_tag: i64, operand: i64) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::Unary as i64,
            int_value: op_tag,
            child_a: operand,
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_call(callee: i64, args_handle: i64) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::Call as i64,
            child_a: callee,
            child_b: args_handle,
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_if(cond: i64, then_b: i64, else_b: i64) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::If as i64,
            child_a: cond,
            child_b: then_b,
            child_c: else_b,
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_block(stmts_handle: i64, final_expr: i64) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::Block as i64,
            child_a: stmts_handle,
            child_b: final_expr,
            ..Default::default()
        })
    })
}

pub fn bootstrap_expr_alloc_error(message: &str) -> i64 {
    with_ast_store(|s| {
        s.alloc_expr(ExprNode {
            tag: ExprTag::Error as i64,
            text: message.to_string(),
            ..Default::default()
        })
    })
}

// Reader-side expression accessors. Out-of-bounds / kind-mismatched ids
// return safe defaults so the parser/normalizer can keep walking.

pub fn bootstrap_expr_get_tag(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_expr(id).map(|n| n.tag).unwrap_or(0))
}

pub fn bootstrap_expr_get_int_value(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_expr(id).map(|n| n.int_value).unwrap_or(0))
}

pub fn bootstrap_expr_get_text(id: i64) -> String {
    with_ast_store_ref(|s| s.get_expr(id).map(|n| n.text.clone()).unwrap_or_default())
}

pub fn bootstrap_expr_get_child_a(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_expr(id).map(|n| n.child_a).unwrap_or(0))
}

pub fn bootstrap_expr_get_child_b(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_expr(id).map(|n| n.child_b).unwrap_or(0))
}

pub fn bootstrap_expr_get_child_c(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_expr(id).map(|n| n.child_c).unwrap_or(0))
}

// ── Statement alloc / get ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn bootstrap_stmt_alloc(
    tag: i64,
    int_value: i64,
    child_a: i64,
    child_b: i64,
    child_c: i64,
    text: &str,
) -> i64 {
    with_ast_store(|s| {
        s.alloc_stmt(StmtNode {
            tag,
            int_value,
            child_a,
            child_b,
            child_c,
            text: text.to_string(),
        })
    })
}

pub fn bootstrap_stmt_get_tag(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_stmt(id).map(|n| n.tag).unwrap_or(0))
}

pub fn bootstrap_stmt_get_int_value(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_stmt(id).map(|n| n.int_value).unwrap_or(0))
}

pub fn bootstrap_stmt_get_text(id: i64) -> String {
    with_ast_store_ref(|s| s.get_stmt(id).map(|n| n.text.clone()).unwrap_or_default())
}

pub fn bootstrap_stmt_get_child_a(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_stmt(id).map(|n| n.child_a).unwrap_or(0))
}

pub fn bootstrap_stmt_get_child_b(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_stmt(id).map(|n| n.child_b).unwrap_or(0))
}

pub fn bootstrap_stmt_get_child_c(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_stmt(id).map(|n| n.child_c).unwrap_or(0))
}

// ── Param alloc / get ────────────────────────────────────────────────────

pub fn bootstrap_param_alloc(
    name: &str,
    type_tag: i64,
    type_name: &str,
    default_id: i64,
) -> i64 {
    with_ast_store(|s| {
        s.alloc_param(ParamNode {
            name: name.to_string(),
            type_tag,
            type_name: type_name.to_string(),
            default_id,
        })
    })
}

pub fn bootstrap_param_get_name(id: i64) -> String {
    with_ast_store_ref(|s| s.get_param(id).map(|p| p.name.clone()).unwrap_or_default())
}

pub fn bootstrap_param_get_type_tag(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_param(id).map(|p| p.type_tag).unwrap_or(0))
}

pub fn bootstrap_param_get_type_name(id: i64) -> String {
    with_ast_store_ref(|s| {
        s.get_param(id)
            .map(|p| p.type_name.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_param_get_default(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_param(id).map(|p| p.default_id).unwrap_or(0))
}

// ── Function alloc / get ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn bootstrap_function_alloc(
    name: &str,
    params_handle: i64,
    ret_type_tag: i64,
    ret_type_name: &str,
    body_handle: i64,
    is_pub: i64,
    is_extern: i64,
) -> i64 {
    with_ast_store(|s| {
        s.alloc_function(FunctionNode {
            name: name.to_string(),
            params_handle,
            ret_type_tag,
            ret_type_name: ret_type_name.to_string(),
            body_handle,
            is_pub,
            is_extern,
        })
    })
}

pub fn bootstrap_function_get_name(id: i64) -> String {
    with_ast_store_ref(|s| {
        s.get_function(id)
            .map(|f| f.name.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_function_get_params_handle(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_function(id).map(|f| f.params_handle).unwrap_or(0))
}

pub fn bootstrap_function_get_ret_type_tag(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_function(id).map(|f| f.ret_type_tag).unwrap_or(0))
}

pub fn bootstrap_function_get_ret_type_name(id: i64) -> String {
    with_ast_store_ref(|s| {
        s.get_function(id)
            .map(|f| f.ret_type_name.clone())
            .unwrap_or_default()
    })
}

pub fn bootstrap_function_get_body_handle(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_function(id).map(|f| f.body_handle).unwrap_or(0))
}

// ── Module item alloc / get ──────────────────────────────────────────────

pub fn bootstrap_module_item_alloc_function(function_id: i64) -> i64 {
    with_ast_store(|s| {
        s.alloc_module_item(ModuleItemNode {
            tag: ModuleItemTag::Function as i64,
            function_id,
            ..Default::default()
        })
    })
}

pub fn bootstrap_module_item_get_tag(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_module_item(id).map(|m| m.tag).unwrap_or(0))
}

pub fn bootstrap_module_item_get_function_id(id: i64) -> i64 {
    with_ast_store_ref(|s| s.get_module_item(id).map(|m| m.function_id).unwrap_or(0))
}

// ── Node-id list alloc / append / len / get ──────────────────────────────

pub fn bootstrap_expr_list_alloc() -> i64 {
    with_ast_store(|s| s.list_alloc(AstListKind::ExprList))
}

pub fn bootstrap_stmt_list_alloc() -> i64 {
    with_ast_store(|s| s.list_alloc(AstListKind::StmtList))
}

pub fn bootstrap_param_list_alloc() -> i64 {
    with_ast_store(|s| s.list_alloc(AstListKind::ParamList))
}

pub fn bootstrap_module_item_list_alloc() -> i64 {
    with_ast_store(|s| s.list_alloc(AstListKind::ModuleItemList))
}

pub fn bootstrap_node_list_append(handle: i64, id: i64) -> i64 {
    with_ast_store(|s| s.list_append(handle, id))
}

pub fn bootstrap_node_list_len(handle: i64) -> i64 {
    with_ast_store_ref(|s| s.list_len(handle))
}

pub fn bootstrap_node_list_get(handle: i64, index: i64) -> i64 {
    with_ast_store_ref(|s| s.list_get(handle, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> BootstrapAstStore {
        BootstrapAstStore::new()
    }

    #[test]
    fn nested_binary_round_trips_through_storage() {
        let mut s = fresh();
        // Build (a + b) * c
        let a = s.alloc_expr(ExprNode {
            tag: ExprTag::Ident as i64,
            text: "a".into(),
            ..Default::default()
        });
        let b = s.alloc_expr(ExprNode {
            tag: ExprTag::Ident as i64,
            text: "b".into(),
            ..Default::default()
        });
        let c = s.alloc_expr(ExprNode {
            tag: ExprTag::Ident as i64,
            text: "c".into(),
            ..Default::default()
        });
        let sum = s.alloc_expr(ExprNode {
            tag: ExprTag::Binary as i64,
            int_value: 1, // add
            child_a: a,
            child_b: b,
            ..Default::default()
        });
        let prod = s.alloc_expr(ExprNode {
            tag: ExprTag::Binary as i64,
            int_value: 3, // mul
            child_a: sum,
            child_b: c,
            ..Default::default()
        });

        let prod_node = s.get_expr(prod).expect("prod stored");
        assert_eq!(prod_node.tag, ExprTag::Binary as i64);
        let lhs = s.get_expr(prod_node.child_a).expect("lhs stored");
        assert_eq!(lhs.tag, ExprTag::Binary as i64);
        let rhs = s.get_expr(prod_node.child_b).expect("rhs stored");
        assert_eq!(rhs.tag, ExprTag::Ident as i64);
        assert_eq!(rhs.text, "c");
        let lhs_left = s.get_expr(lhs.child_a).expect("a stored");
        let lhs_right = s.get_expr(lhs.child_b).expect("b stored");
        assert_eq!(lhs_left.text, "a");
        assert_eq!(lhs_right.text, "b");
    }

    #[test]
    fn function_params_round_trip() {
        let mut s = fresh();
        let p1 = s.alloc_param(ParamNode {
            name: "a".into(),
            type_tag: TypeTag::Int as i64,
            type_name: String::new(),
            default_id: 0,
        });
        let p2 = s.alloc_param(ParamNode {
            name: "b".into(),
            type_tag: TypeTag::Int as i64,
            type_name: String::new(),
            default_id: 0,
        });
        let plist = s.list_alloc(AstListKind::ParamList);
        s.list_append(plist, p1);
        s.list_append(plist, p2);
        assert_eq!(s.list_len(plist), 2);
        let first = s.get_param(s.list_get(plist, 0)).unwrap();
        let second = s.get_param(s.list_get(plist, 1)).unwrap();
        assert_eq!(first.name, "a");
        assert_eq!(second.name, "b");
        assert_eq!(first.type_tag, TypeTag::Int as i64);
    }

    #[test]
    fn statement_body_round_trips_with_let_and_ret() {
        let mut s = fresh();
        let value = s.alloc_expr(ExprNode {
            tag: ExprTag::IntLit as i64,
            int_value: 42,
            ..Default::default()
        });
        let let_stmt = s.alloc_stmt(StmtNode {
            tag: StmtTag::Let as i64,
            int_value: 0,
            child_a: 0,
            child_b: value,
            child_c: 0,
            text: "x".into(),
        });
        let x_ref = s.alloc_expr(ExprNode {
            tag: ExprTag::Ident as i64,
            text: "x".into(),
            ..Default::default()
        });
        let ret_stmt = s.alloc_stmt(StmtNode {
            tag: StmtTag::Ret as i64,
            child_a: x_ref,
            ..Default::default()
        });
        let body = s.list_alloc(AstListKind::StmtList);
        s.list_append(body, let_stmt);
        s.list_append(body, ret_stmt);
        assert_eq!(s.list_len(body), 2);
        let first = s.get_stmt(s.list_get(body, 0)).unwrap();
        assert_eq!(first.tag, StmtTag::Let as i64);
        assert_eq!(first.text, "x");
        let first_value = s.get_expr(first.child_b).unwrap();
        assert_eq!(first_value.int_value, 42);
        let last = s.get_stmt(s.list_get(body, 1)).unwrap();
        assert_eq!(last.tag, StmtTag::Ret as i64);
        let last_value = s.get_expr(last.child_a).unwrap();
        assert_eq!(last_value.text, "x");
    }

    #[test]
    fn unknown_id_reads_return_safe_defaults() {
        let s = fresh();
        assert!(s.get_expr(0).is_none());
        assert!(s.get_expr(99).is_none());
        assert_eq!(s.list_len(0), 0);
        assert_eq!(s.list_get(0, 0), 0);
    }

    #[test]
    fn ambient_store_round_trips_through_externs() {
        let _guard = ambient_test_lock();
        reset_ast_store();
        let l = bootstrap_expr_alloc_ident("a");
        let r = bootstrap_expr_alloc_ident("b");
        let sum = bootstrap_expr_alloc_binary(1, l, r);
        assert_eq!(bootstrap_expr_get_tag(sum), ExprTag::Binary as i64);
        assert_eq!(bootstrap_expr_get_int_value(sum), 1);
        assert_eq!(bootstrap_expr_get_child_a(sum), l);
        assert_eq!(bootstrap_expr_get_child_b(sum), r);
        assert_eq!(bootstrap_expr_get_text(l), "a");
    }

    #[test]
    fn ambient_param_list_round_trips() {
        let _guard = ambient_test_lock();
        reset_ast_store();
        let pa = bootstrap_param_alloc("a", TypeTag::Int as i64, "", 0);
        let pb = bootstrap_param_alloc("b", TypeTag::Int as i64, "", 0);
        let plist = bootstrap_param_list_alloc();
        bootstrap_node_list_append(plist, pa);
        bootstrap_node_list_append(plist, pb);
        assert_eq!(bootstrap_node_list_len(plist), 2);
        assert_eq!(
            bootstrap_param_get_name(bootstrap_node_list_get(plist, 0)),
            "a"
        );
        assert_eq!(
            bootstrap_param_get_name(bootstrap_node_list_get(plist, 1)),
            "b"
        );
    }

    #[test]
    fn ambient_function_round_trips() {
        let _guard = ambient_test_lock();
        reset_ast_store();
        let body = bootstrap_stmt_list_alloc();
        let fid = bootstrap_function_alloc("f", 0, TypeTag::Int as i64, "", body, 0, 0);
        assert_eq!(bootstrap_function_get_name(fid), "f");
        assert_eq!(
            bootstrap_function_get_ret_type_tag(fid),
            TypeTag::Int as i64
        );
        assert_eq!(bootstrap_function_get_body_handle(fid), body);
    }

    /// Serialize tests that exercise the process-wide ambient store. Cargo
    /// runs tests in parallel by default, so without this guard one test's
    /// `reset_ast_store` can race against another's appended nodes.
    fn ambient_test_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }
}
