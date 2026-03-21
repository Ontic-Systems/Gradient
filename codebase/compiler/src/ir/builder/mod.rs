//! IR builder: translates the parsed Gradient AST into SSA-based IR.
//!
//! This module is the bridge between the frontend (lexer/parser) and the
//! backend (Cranelift codegen). It walks an [`ast::Module`] and produces
//! an [`ir::Module`] whose functions consist of basic blocks of SSA
//! instructions.
//!
//! # Design
//!
//! - Every expression produces exactly one SSA [`Value`].
//! - Variables are tracked in a scope stack (`Vec<HashMap<String, Value>>`).
//! - `if`/`else` branches use `Branch`, `Jump`, and `Phi` instructions to
//!   merge values in proper SSA form.
//! - Short-circuit evaluation for `and`/`or` is lowered to conditional
//!   branches.
//! - For v0.1, all integers are [`Type::I64`] and all floats are [`Type::F64`].
//! - Errors are collected into a `Vec<String>` rather than panicking.

use crate::ast;
use super::{BasicBlock, Function, Instruction, Module, Type, Value, FuncRef, BlockRef, Literal, CmpOp};
use std::collections::{HashMap, HashSet};

/// The IR builder translates a parsed AST into the SSA-based IR.
///
/// # Usage
///
/// ```ignore
/// let (ir_module, errors) = IrBuilder::build_module(&ast_module);
/// ```
pub struct IrBuilder {
    /// Counter for generating fresh SSA values.
    next_value: u32,
    /// Counter for generating fresh block labels.
    next_block: u32,
    /// Counter for function references.
    next_func_ref: u32,
    /// Scope stack: each scope maps variable names to their current SSA value.
    variables: Vec<HashMap<String, Value>>,
    /// Map from function names to their [`FuncRef`].
    function_refs: HashMap<String, FuncRef>,
    /// Instructions in the current block being built.
    current_block: Vec<Instruction>,
    /// All completed blocks in the current function.
    completed_blocks: Vec<BasicBlock>,
    /// Label of the current block being built.
    current_block_label: BlockRef,
    /// Errors encountered during IR building.
    errors: Vec<String>,
    /// Set of SSA values known to be string-typed (Ptr to string data).
    /// Used to detect string concatenation (`+` on strings) and route it
    /// to a `string_concat` call instead of an `Add` instruction.
    string_values: HashSet<Value>,
    /// Set of variable names that are mutable (use alloca/load/store).
    mutable_vars: HashSet<String>,
    /// Maps mutable variable names to their alloca'd address (stack slot pointer).
    mutable_addrs: HashMap<String, Value>,
    /// Maps every SSA value to its IR type. Populated as values are created
    /// and copied into each [`Function`] when building completes.
    value_types: HashMap<Value, Type>,
    /// Maps function names to their declared return types, so that the
    /// builder can assign the correct type to `Call` result values.
    function_return_types: HashMap<String, Type>,
}

impl Default for IrBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl IrBuilder {
    // ── Construction ──────────────────────────────────────────────────

    /// Create a new, empty builder.
    pub fn new() -> Self {
        Self {
            next_value: 0,
            next_block: 0,
            next_func_ref: 0,
            variables: vec![HashMap::new()],
            function_refs: HashMap::new(),
            current_block: Vec::new(),
            completed_blocks: Vec::new(),
            current_block_label: BlockRef(0),
            errors: Vec::new(),
            string_values: HashSet::new(),
            mutable_vars: HashSet::new(),
            mutable_addrs: HashMap::new(),
            value_types: HashMap::new(),
            function_return_types: HashMap::new(),
        }
    }

    // ── Entry point ──────────────────────────────────────────────────

    /// Translate an AST module into an IR module.
    ///
    /// Returns the IR module and a list of any errors encountered during
    /// translation.
    pub fn build_module(ast_module: &ast::Module) -> (Module, Vec<String>) {
        let mut builder = IrBuilder::new();

        let module_name = ast_module
            .module_decl
            .as_ref()
            .map(|md| md.path.join("."))
            .unwrap_or_else(|| "main".to_string());

        // First pass: register all function names so that calls can be resolved.
        builder.register_functions(ast_module);

        // Second pass: build each item.
        let mut functions = Vec::new();
        for item in &ast_module.items {
            match &item.node {
                ast::ItemKind::FnDef(fn_def) => {
                    let func = builder.build_fn_def(fn_def);
                    functions.push(func);
                }
                ast::ItemKind::ExternFn(extern_fn) => {
                    // Extern functions are declared but have no body.
                    // We still register them (done in register_functions)
                    // and emit an empty function shell for the codegen layer
                    // to handle.
                    let func = builder.build_extern_fn(extern_fn);
                    functions.push(func);
                }
                ast::ItemKind::Let { name, value, mutable, .. } => {
                    // Top-level let bindings: evaluate the value and store in
                    // the global scope.  For now we just record the binding.
                    let val = builder.build_expr(value);
                    if *mutable {
                        builder.build_mutable_let(name, val);
                    } else {
                        builder.define_var(name, val);
                    }
                }
                ast::ItemKind::TypeDecl { .. } => {
                    // Type declarations have no runtime representation in v0.1.
                }
                ast::ItemKind::CapDecl { .. } => {
                    // Capability declarations are compile-time only.
                }
            }
        }

        // Build the reverse mapping from FuncRef -> function name for codegen.
        let func_refs: HashMap<String, super::FuncRef> = builder.function_refs.clone();
        let func_ref_map: HashMap<super::FuncRef, String> = func_refs
            .into_iter()
            .map(|(name, fref)| (fref, name))
            .collect();

        let ir_module = Module {
            name: module_name,
            functions,
            func_refs: func_ref_map,
        };

        (ir_module, builder.errors)
    }

    // ── Function registration ────────────────────────────────────────

    /// Pre-register all function names so that forward references resolve.
    fn register_functions(&mut self, ast_module: &ast::Module) {
        // Pre-register common external functions with their return types.
        self.register_func("print");
        self.function_return_types.insert("print".to_string(), Type::Void);
        self.register_func("println");
        self.function_return_types.insert("println".to_string(), Type::Void);
        self.register_func("print_int");
        self.function_return_types.insert("print_int".to_string(), Type::Void);
        self.register_func("print_float");
        self.function_return_types.insert("print_float".to_string(), Type::Void);
        self.register_func("print_bool");
        self.function_return_types.insert("print_bool".to_string(), Type::Void);
        self.register_func("int_to_string");
        self.function_return_types.insert("int_to_string".to_string(), Type::Ptr);
        self.register_func("abs");
        self.function_return_types.insert("abs".to_string(), Type::I64);
        self.register_func("min");
        self.function_return_types.insert("min".to_string(), Type::I64);
        self.register_func("max");
        self.function_return_types.insert("max".to_string(), Type::I64);
        self.register_func("mod_int");
        self.function_return_types.insert("mod_int".to_string(), Type::I64);
        self.register_func("string_concat");
        self.function_return_types.insert("string_concat".to_string(), Type::Ptr);

        for item in &ast_module.items {
            match &item.node {
                ast::ItemKind::FnDef(fn_def) => {
                    self.register_func(&fn_def.name);
                    let ret_ty = fn_def
                        .return_type
                        .as_ref()
                        .map(|rt| self.resolve_type(&rt.node))
                        .unwrap_or(Type::Void);
                    self.function_return_types.insert(fn_def.name.clone(), ret_ty);
                }
                ast::ItemKind::ExternFn(extern_fn) => {
                    self.register_func(&extern_fn.name);
                    let ret_ty = extern_fn
                        .return_type
                        .as_ref()
                        .map(|rt| self.resolve_type(&rt.node))
                        .unwrap_or(Type::Void);
                    self.function_return_types.insert(extern_fn.name.clone(), ret_ty);
                }
                _ => {}
            }
        }
    }

    /// Register a single function name, assigning it a fresh [`FuncRef`].
    fn register_func(&mut self, name: &str) {
        if !self.function_refs.contains_key(name) {
            let fref = FuncRef(self.next_func_ref);
            self.next_func_ref += 1;
            self.function_refs.insert(name.to_string(), fref);
        }
    }

    // ── Function building ────────────────────────────────────────────

    /// Build an IR function from an AST function definition.
    fn build_fn_def(&mut self, fn_def: &ast::FnDef) -> Function {
        // Reset per-function state.
        self.next_value = 0;
        self.next_block = 0;
        self.completed_blocks.clear();
        self.current_block.clear();
        self.variables = vec![HashMap::new()];
        self.string_values.clear();
        self.mutable_vars.clear();
        self.mutable_addrs.clear();
        self.value_types.clear();

        // Start the entry block.
        self.current_block_label = self.fresh_block();

        // Bind parameters as variables.
        let param_types: Vec<Type> = fn_def
            .params
            .iter()
            .map(|p| self.resolve_type(&p.type_ann.node))
            .collect();

        for (i, param) in fn_def.params.iter().enumerate() {
            let val = self.fresh_value(param_types[i].clone());
            // Emit a "parameter" as an Alloca + Store conceptually, but in
            // SSA form parameters are just fresh values.  We define the
            // variable to point directly at the parameter value.
            //
            // We use value IDs starting from 0 for parameters, which the
            // codegen layer will recognise as block parameters of the entry
            // block.
            self.define_var(&param.name, val);
        }

        let return_type = fn_def
            .return_type
            .as_ref()
            .map(|rt| self.resolve_type(&rt.node))
            .unwrap_or(Type::Void);

        // Build the function body, tracking the last expression value
        // so we can emit an implicit return for expression-bodied functions.
        let last_expr_val = self.build_fn_body(&fn_def.body);

        // If the last block has no terminator, add an implicit return.
        if !self.current_block_has_terminator() {
            if return_type == Type::Void {
                self.emit(Instruction::Ret(None));
            } else if let Some(val) = last_expr_val {
                // The last statement was an expression — return its value.
                self.emit(Instruction::Ret(Some(val)));
            } else {
                // Non-void function with no explicit or implicit return value.
                // The type checker should have caught this, so we record an
                // error and emit a fallback.
                self.errors.push(format!(
                    "function '{}' may not return a value on all paths",
                    fn_def.name
                ));
                let zero = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(zero, Literal::Int(0)));
                self.emit(Instruction::Ret(Some(zero)));
            }
        }

        // Seal the final block.
        self.seal_block();

        Function {
            name: fn_def.name.clone(),
            params: param_types,
            return_type,
            blocks: std::mem::take(&mut self.completed_blocks),
            value_types: self.value_types.clone(),
        }
    }

    /// Build an IR function shell for an extern function declaration.
    fn build_extern_fn(&mut self, extern_fn: &ast::ExternFnDecl) -> Function {
        let param_types: Vec<Type> = extern_fn
            .params
            .iter()
            .map(|p| self.resolve_type(&p.type_ann.node))
            .collect();

        let return_type = extern_fn
            .return_type
            .as_ref()
            .map(|rt| self.resolve_type(&rt.node))
            .unwrap_or(Type::Void);

        // Extern functions have no body — no blocks.
        Function {
            name: extern_fn.name.clone(),
            params: param_types,
            return_type,
            blocks: Vec::new(),
            value_types: HashMap::new(),
        }
    }

    // ── Block and statement building ─────────────────────────────────

    /// Build a function body, returning the value of the last expression
    /// statement if it exists. This enables implicit returns in
    /// expression-bodied functions (e.g. `fn f() -> i64: 42`).
    fn build_fn_body(&mut self, block: &ast::Block) -> Option<Value> {
        self.push_scope();
        let mut last_expr_val = None;
        for stmt in &block.node {
            // If we already emitted a terminator, stop processing.
            if self.current_block_has_terminator() {
                break;
            }
            match &stmt.node {
                ast::StmtKind::Let { name, value, mutable, .. } => {
                    let val = self.build_expr(value);
                    if *mutable {
                        self.build_mutable_let(name, val);
                    } else {
                        self.define_var(name, val);
                    }
                    last_expr_val = None;
                }
                ast::StmtKind::Assign { name, value } => {
                    let val = self.build_expr(value);
                    self.build_assign(name, val);
                    last_expr_val = None;
                }
                ast::StmtKind::Ret(expr) => {
                    let val = self.build_expr(expr);
                    self.emit(Instruction::Ret(Some(val)));
                    last_expr_val = None;
                }
                ast::StmtKind::Expr(expr) => {
                    let val = self.build_expr(expr);
                    last_expr_val = Some(val);
                }
            }
        }
        self.pop_scope();
        last_expr_val
    }

    /// Build a single statement.
    fn build_stmt(&mut self, stmt: &ast::Stmt) {
        match &stmt.node {
            ast::StmtKind::Let { name, value, mutable, .. } => {
                let val = self.build_expr(value);
                if *mutable {
                    self.build_mutable_let(name, val);
                } else {
                    self.define_var(name, val);
                }
            }
            ast::StmtKind::Assign { name, value } => {
                let val = self.build_expr(value);
                self.build_assign(name, val);
            }
            ast::StmtKind::Ret(expr) => {
                let val = self.build_expr(expr);
                self.emit(Instruction::Ret(Some(val)));
            }
            ast::StmtKind::Expr(expr) => {
                // Evaluate for side effects; discard the result value.
                let _val = self.build_expr(expr);
            }
        }
    }

    /// Build a mutable let binding: alloca + store + track the address.
    fn build_mutable_let(&mut self, name: &str, val: Value) {
        let val_ty = self.value_types.get(&val).cloned().unwrap_or(Type::I64);
        // Allocate a stack slot.
        let addr = self.fresh_value(Type::Ptr);
        self.emit(Instruction::Alloca(addr, val_ty));
        // Store the initial value.
        self.emit(Instruction::Store(val, addr));
        // Track as mutable.
        self.mutable_vars.insert(name.to_string());
        self.mutable_addrs.insert(name.to_string(), addr);
        // Also define in scope so lookup_var still works (maps to addr for tracking).
        self.define_var(name, addr);
    }

    /// Build an assignment to a mutable variable: store to the alloca'd address.
    fn build_assign(&mut self, name: &str, val: Value) {
        if let Some(addr) = self.mutable_addrs.get(name).copied() {
            self.emit(Instruction::Store(val, addr));
        } else {
            self.errors.push(format!("assignment to undefined or immutable variable: '{}'", name));
        }
    }

    // ── Expression building (core) ───────────────────────────────────

    /// Translate an expression into SSA instructions and return the
    /// resulting [`Value`].
    fn build_expr(&mut self, expr: &ast::Expr) -> Value {
        match &expr.node {
            ast::ExprKind::IntLit(n) => {
                let v = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(v, Literal::Int(*n)));
                v
            }
            ast::ExprKind::FloatLit(f) => {
                let v = self.fresh_value(Type::F64);
                self.emit(Instruction::Const(v, Literal::Float(*f)));
                v
            }
            ast::ExprKind::StringLit(s) => {
                let v = self.fresh_value(Type::Ptr);
                self.emit(Instruction::Const(v, Literal::Str(s.clone())));
                self.string_values.insert(v);
                v
            }
            ast::ExprKind::BoolLit(b) => {
                let v = self.fresh_value(Type::Bool);
                self.emit(Instruction::Const(v, Literal::Bool(*b)));
                v
            }
            ast::ExprKind::UnitLit => {
                // Unit has no runtime value. We produce a dummy const 0
                // so that every expression has a Value.
                let v = self.fresh_value(Type::Void);
                self.emit(Instruction::Const(v, Literal::Int(0)));
                v
            }
            ast::ExprKind::Ident(name) => {
                // If this is a mutable variable, load from its stack slot.
                if self.mutable_vars.contains(name.as_str()) {
                    if let Some(addr) = self.mutable_addrs.get(name.as_str()).copied() {
                        let result = self.fresh_value(Type::I64);
                        self.emit(Instruction::Load(result, addr));
                        return result;
                    }
                }
                match self.lookup_var(name) {
                    Some(val) => val,
                    None => {
                        self.errors.push(format!("undefined variable: '{}'", name));
                        // Return a dummy value so we can keep going.
                        let v = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(v, Literal::Int(0)));
                        v
                    }
                }
            }
            ast::ExprKind::TypedHole(label) => {
                let desc = label
                    .as_ref()
                    .map(|l| format!("?{}", l))
                    .unwrap_or_else(|| "?".to_string());
                self.errors
                    .push(format!("typed hole {} encountered during IR building", desc));
                let v = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(v, Literal::Int(0)));
                v
            }
            ast::ExprKind::BinaryOp { op, left, right } => {
                self.build_binary_op(*op, left, right)
            }
            ast::ExprKind::UnaryOp { op, operand } => {
                self.build_unary_op(*op, operand)
            }
            ast::ExprKind::Call { func, args } => {
                self.build_call(func, args)
            }
            ast::ExprKind::If {
                condition,
                then_block,
                else_ifs,
                else_block,
            } => self.build_if(condition, then_block, else_ifs, else_block),
            ast::ExprKind::FieldAccess { object, field } => {
                self.errors.push(format!(
                    "field access (.{}) is not yet supported in IR builder",
                    field
                ));
                let _obj = self.build_expr(object);
                let v = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(v, Literal::Int(0)));
                v
            }
            ast::ExprKind::For { var, iter, body } => {
                self.build_for(var, iter, body)
            }
            ast::ExprKind::While { condition, body } => {
                self.build_while(condition, body)
            }
            ast::ExprKind::Paren(inner) => {
                // Parentheses are purely syntactic — pass through.
                self.build_expr(inner)
            }
        }
    }

    // ── Binary operations ────────────────────────────────────────────

    /// Build a binary operation expression.
    fn build_binary_op(
        &mut self,
        op: ast::BinOp,
        left: &ast::Expr,
        right: &ast::Expr,
    ) -> Value {
        match op {
            // Arithmetic operators.
            // Special case: `+` on strings emits a call to `string_concat`.
            ast::BinOp::Add => {
                let v1 = self.build_expr(left);
                let v2 = self.build_expr(right);
                if self.string_values.contains(&v1) || self.string_values.contains(&v2) {
                    // String concatenation: call string_concat(a, b)
                    let func_ref = self.function_refs.get("string_concat").copied()
                        .expect("string_concat should be pre-registered");
                    let result = self.fresh_value(Type::Ptr);
                    self.emit(Instruction::Call(result, func_ref, vec![v1, v2]));
                    self.string_values.insert(result);
                    result
                } else {
                    // Use the type of the left operand for the result.
                    let operand_ty = self.value_types.get(&v1).cloned().unwrap_or(Type::I64);
                    let result = self.fresh_value(operand_ty);
                    self.emit(Instruction::Add(result, v1, v2));
                    result
                }
            }
            ast::BinOp::Sub => {
                let v1 = self.build_expr(left);
                let v2 = self.build_expr(right);
                let operand_ty = self.value_types.get(&v1).cloned().unwrap_or(Type::I64);
                let result = self.fresh_value(operand_ty);
                self.emit(Instruction::Sub(result, v1, v2));
                result
            }
            ast::BinOp::Mul => {
                let v1 = self.build_expr(left);
                let v2 = self.build_expr(right);
                let operand_ty = self.value_types.get(&v1).cloned().unwrap_or(Type::I64);
                let result = self.fresh_value(operand_ty);
                self.emit(Instruction::Mul(result, v1, v2));
                result
            }
            ast::BinOp::Div => {
                let v1 = self.build_expr(left);
                let v2 = self.build_expr(right);
                let operand_ty = self.value_types.get(&v1).cloned().unwrap_or(Type::I64);
                let result = self.fresh_value(operand_ty);
                self.emit(Instruction::Div(result, v1, v2));
                result
            }
            ast::BinOp::Mod => {
                // Modulo is not yet a first-class IR instruction.
                // For v0.1 we lower `a % b` as `a - (a / b) * b`.
                let v1 = self.build_expr(left);
                let v2 = self.build_expr(right);
                let operand_ty = self.value_types.get(&v1).cloned().unwrap_or(Type::I64);
                let div_result = self.fresh_value(operand_ty.clone());
                self.emit(Instruction::Div(div_result, v1, v2));
                let mul_result = self.fresh_value(operand_ty.clone());
                self.emit(Instruction::Mul(mul_result, div_result, v2));
                let result = self.fresh_value(operand_ty);
                self.emit(Instruction::Sub(result, v1, mul_result));
                result
            }

            // Comparison operators.
            ast::BinOp::Eq => self.build_cmp(CmpOp::Eq, left, right),
            ast::BinOp::Ne => self.build_cmp(CmpOp::Ne, left, right),
            ast::BinOp::Lt => self.build_cmp(CmpOp::Lt, left, right),
            ast::BinOp::Le => self.build_cmp(CmpOp::Le, left, right),
            ast::BinOp::Gt => self.build_cmp(CmpOp::Gt, left, right),
            ast::BinOp::Ge => self.build_cmp(CmpOp::Ge, left, right),

            // Short-circuit logical operators.
            ast::BinOp::And => self.build_short_circuit_and(left, right),
            ast::BinOp::Or => self.build_short_circuit_or(left, right),
        }
    }

    /// Build a comparison instruction.
    fn build_cmp(
        &mut self,
        op: CmpOp,
        left: &ast::Expr,
        right: &ast::Expr,
    ) -> Value {
        let v1 = self.build_expr(left);
        let v2 = self.build_expr(right);
        let result = self.fresh_value(Type::Bool);
        self.emit(Instruction::Cmp(result, op, v1, v2));
        result
    }

    /// Short-circuit AND: `left and right`.
    ///
    /// Lowered to:
    /// ```text
    ///   v_left = <build left>
    ///   branch v_left, right_block, merge_block
    /// right_block:
    ///   v_right = <build right>
    ///   jump merge_block
    /// merge_block:
    ///   result = phi [(current_block, v_left), (right_block, v_right)]
    /// ```
    fn build_short_circuit_and(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
    ) -> Value {
        let v_left = self.build_expr(left);

        let right_block = self.fresh_block();
        let merge_block = self.fresh_block();
        let left_block_ref = self.current_block_label;

        self.emit(Instruction::Branch(v_left, right_block, merge_block));
        self.seal_block();

        // right_block: evaluate right operand.
        self.current_block_label = right_block;
        let v_right = self.build_expr(right);
        let right_block_actual = self.current_block_label;
        self.emit(Instruction::Jump(merge_block));
        self.seal_block();

        // merge_block: phi to select the result.
        self.current_block_label = merge_block;
        let result = self.fresh_value(Type::Bool);
        self.emit(Instruction::Phi(
            result,
            vec![
                (left_block_ref, v_left),
                (right_block_actual, v_right),
            ],
        ));
        result
    }

    /// Short-circuit OR: `left or right`.
    ///
    /// Lowered to:
    /// ```text
    ///   v_left = <build left>
    ///   branch v_left, merge_block, right_block
    /// right_block:
    ///   v_right = <build right>
    ///   jump merge_block
    /// merge_block:
    ///   result = phi [(current_block, v_left), (right_block, v_right)]
    /// ```
    fn build_short_circuit_or(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
    ) -> Value {
        let v_left = self.build_expr(left);

        let right_block = self.fresh_block();
        let merge_block = self.fresh_block();
        let left_block_ref = self.current_block_label;

        // If left is true, skip to merge; otherwise evaluate right.
        self.emit(Instruction::Branch(v_left, merge_block, right_block));
        self.seal_block();

        // right_block: evaluate right operand.
        self.current_block_label = right_block;
        let v_right = self.build_expr(right);
        let right_block_actual = self.current_block_label;
        self.emit(Instruction::Jump(merge_block));
        self.seal_block();

        // merge_block: phi to select the result.
        self.current_block_label = merge_block;
        let result = self.fresh_value(Type::Bool);
        self.emit(Instruction::Phi(
            result,
            vec![
                (left_block_ref, v_left),
                (right_block_actual, v_right),
            ],
        ));
        result
    }

    // ── Unary operations ─────────────────────────────────────────────

    /// Build a unary operation expression.
    fn build_unary_op(
        &mut self,
        op: ast::UnaryOp,
        operand: &ast::Expr,
    ) -> Value {
        match op {
            ast::UnaryOp::Neg => {
                // -x  ==  0 - x
                let v = self.build_expr(operand);
                let operand_ty = self.value_types.get(&v).cloned().unwrap_or(Type::I64);
                let zero = self.fresh_value(operand_ty.clone());
                self.emit(Instruction::Const(zero, Literal::Int(0)));
                let result = self.fresh_value(operand_ty);
                self.emit(Instruction::Sub(result, zero, v));
                result
            }
            ast::UnaryOp::Not => {
                // not x  ==  x == false
                let v = self.build_expr(operand);
                let false_val = self.fresh_value(Type::Bool);
                self.emit(Instruction::Const(false_val, Literal::Bool(false)));
                let result = self.fresh_value(Type::Bool);
                self.emit(Instruction::Cmp(result, CmpOp::Eq, v, false_val));
                result
            }
        }
    }

    // ── Function calls ───────────────────────────────────────────────

    /// Build a function call expression.
    fn build_call(
        &mut self,
        func: &ast::Expr,
        args: &[ast::Expr],
    ) -> Value {
        // Build all argument expressions first.
        let arg_vals: Vec<Value> = args.iter().map(|a| self.build_expr(a)).collect();

        match &func.node {
            ast::ExprKind::Ident(name) => {
                match self.function_refs.get(name).copied() {
                    Some(func_ref) => {
                        let ret_ty = self.function_return_types
                            .get(name)
                            .cloned()
                            .unwrap_or(Type::I64);
                        let result = self.fresh_value(ret_ty);
                        self.emit(Instruction::Call(result, func_ref, arg_vals));
                        // Track string-returning builtins.
                        if name == "int_to_string" || name == "string_concat" {
                            self.string_values.insert(result);
                        }
                        result
                    }
                    None => {
                        self.errors
                            .push(format!("call to undefined function: '{}'", name));
                        let result = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(result, Literal::Int(0)));
                        result
                    }
                }
            }
            _ => {
                // Indirect calls / higher-order functions are not yet
                // supported in v0.1.
                self.errors.push(
                    "indirect function calls are not yet supported".to_string(),
                );
                let result = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(result, Literal::Int(0)));
                result
            }
        }
    }

    // ── If/else ──────────────────────────────────────────────────────

    /// Build an if/else-if/else expression with phi-node merges.
    ///
    /// The general strategy:
    ///   1. Evaluate the condition.
    ///   2. Branch to then_block or the next condition (else-if) / else /
    ///      merge.
    ///   3. Each arm produces a value and jumps to the merge block.
    ///   4. The merge block contains a phi node that selects the correct
    ///      value.
    fn build_if(
        &mut self,
        condition: &ast::Expr,
        then_block: &ast::Block,
        else_ifs: &[(ast::Expr, ast::Block)],
        else_block: &Option<ast::Block>,
    ) -> Value {
        let merge_block = self.fresh_block();
        let mut phi_entries: Vec<(BlockRef, Value)> = Vec::new();

        // ── Main if arm ──────────────────────────────────────────────
        let then_label = self.fresh_block();
        let else_label = self.fresh_block(); // first else-if or else or merge

        let cond_val = self.build_expr(condition);
        self.emit(Instruction::Branch(cond_val, then_label, else_label));
        self.seal_block();

        // Then arm.
        self.current_block_label = then_label;
        let then_val = self.build_block_expr(then_block);
        let then_exit_block = self.current_block_label;
        if !self.current_block_has_terminator() {
            self.emit(Instruction::Jump(merge_block));
        }
        phi_entries.push((then_exit_block, then_val));
        self.seal_block();

        // ── Else-if arms ─────────────────────────────────────────────
        let mut current_else_label = else_label;
        for (i, (elif_cond, elif_body)) in else_ifs.iter().enumerate() {
            self.current_block_label = current_else_label;

            let elif_then_label = self.fresh_block();
            let elif_else_label = if i + 1 < else_ifs.len() || else_block.is_some() {
                self.fresh_block()
            } else {
                merge_block
            };

            let elif_cond_val = self.build_expr(elif_cond);
            self.emit(Instruction::Branch(
                elif_cond_val,
                elif_then_label,
                elif_else_label,
            ));
            self.seal_block();

            // Else-if then arm.
            self.current_block_label = elif_then_label;
            let elif_val = self.build_block_expr(elif_body);
            let elif_exit_block = self.current_block_label;
            if !self.current_block_has_terminator() {
                self.emit(Instruction::Jump(merge_block));
            }
            phi_entries.push((elif_exit_block, elif_val));
            self.seal_block();

            current_else_label = elif_else_label;
        }

        // ── Else arm ─────────────────────────────────────────────────
        if let Some(else_body) = else_block {
            self.current_block_label = current_else_label;
            let else_val = self.build_block_expr(else_body);
            let else_exit_block = self.current_block_label;
            if !self.current_block_has_terminator() {
                self.emit(Instruction::Jump(merge_block));
            }
            phi_entries.push((else_exit_block, else_val));
            self.seal_block();
        } else {
            // No else arm.  If there are no else-ifs either, current_else_label
            // is unused and we need to route it to merge with a unit value.
            if current_else_label != merge_block {
                self.current_block_label = current_else_label;
                let unit_val = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(unit_val, Literal::Int(0)));
                self.emit(Instruction::Jump(merge_block));
                phi_entries.push((current_else_label, unit_val));
                self.seal_block();
            }
        }

        // ── Merge block ──────────────────────────────────────────────
        self.current_block_label = merge_block;
        // Use the type of the then-branch result for the phi.
        // When both branches terminate via `ret`, the dummy values now
        // carry the correct return type (set by build_block_expr).
        let phi_ty = self.value_types.get(&then_val).cloned().unwrap_or(Type::I64);
        let result = self.fresh_value(phi_ty);
        self.emit(Instruction::Phi(result, phi_entries));
        result
    }

    /// Build a block as an expression, returning the value of its last
    /// expression-statement (or a unit value if empty / ends with a let).
    fn build_block_expr(&mut self, block: &ast::Block) -> Value {
        self.push_scope();
        let mut last_val = None;
        // Track the type of a value returned via `ret` so that if the block
        // is terminated, the dummy value we produce carries the right type
        // (important for phi-node type inference in if/else).
        let mut ret_val_type = None;
        for stmt in &block.node {
            // If we already emitted a terminator (e.g. ret), stop
            // processing further statements in this block.
            if self.current_block_has_terminator() {
                break;
            }
            match &stmt.node {
                ast::StmtKind::Let { name, value, mutable, .. } => {
                    let val = self.build_expr(value);
                    if *mutable {
                        self.build_mutable_let(name, val);
                    } else {
                        self.define_var(name, val);
                    }
                    last_val = None;
                }
                ast::StmtKind::Assign { name, value } => {
                    let val = self.build_expr(value);
                    self.build_assign(name, val);
                    last_val = None;
                }
                ast::StmtKind::Ret(expr) => {
                    let val = self.build_expr(expr);
                    ret_val_type = self.value_types.get(&val).cloned();
                    self.emit(Instruction::Ret(Some(val)));
                    last_val = None;
                }
                ast::StmtKind::Expr(expr) => {
                    let val = self.build_expr(expr);
                    last_val = Some(val);
                }
            }
        }
        self.pop_scope();

        // If the block already has a terminator (e.g. from a `ret`),
        // we don't need a fallback value — just return a dummy.
        if self.current_block_has_terminator() {
            // The value won't actually be used since the block is terminated,
            // but we need to return *something*. Use the type of the returned
            // value so that phi nodes infer the correct type.
            let ty = ret_val_type.unwrap_or(Type::I64);
            let v = self.fresh_value(ty);
            // Don't emit the const — the block is already terminated.
            return v;
        }

        last_val.unwrap_or_else(|| {
            let v = self.fresh_value(Type::I64);
            self.emit(Instruction::Const(v, Literal::Int(0)));
            v
        })
    }

    // ── For loop ─────────────────────────────────────────────────────

    /// Build a for loop.
    ///
    /// For v0.1 this is a placeholder: we lower `for x in iter: body` as
    /// a simple counted loop from 0 to the iterator value (treating iter
    /// as an integer count).  A proper implementation would use iterator
    /// trait methods.
    fn build_for(
        &mut self,
        var: &str,
        iter: &ast::Expr,
        body: &ast::Block,
    ) -> Value {
        let iter_val = self.build_expr(iter);

        // Allocate the loop counter.
        let counter_init = self.fresh_value(Type::I64);
        self.emit(Instruction::Const(counter_init, Literal::Int(0)));

        let loop_header = self.fresh_block();
        let loop_body = self.fresh_block();
        let loop_exit = self.fresh_block();
        let entry_block = self.current_block_label;

        self.emit(Instruction::Jump(loop_header));
        self.seal_block();

        // Loop header: phi for the counter, then compare.
        self.current_block_label = loop_header;
        let counter = self.fresh_value(Type::I64);
        // The phi will be filled with (entry, counter_init) and
        // (loop_body_end, counter_next).
        // We emit a placeholder phi and fix it up after building the body.
        let phi_idx = self.current_block.len();
        self.emit(Instruction::Phi(
            counter,
            vec![(entry_block, counter_init)],
        ));

        let cmp_val = self.fresh_value(Type::Bool);
        self.emit(Instruction::Cmp(cmp_val, CmpOp::Lt, counter, iter_val));
        self.emit(Instruction::Branch(cmp_val, loop_body, loop_exit));
        self.seal_block();

        // Loop body.
        self.current_block_label = loop_body;
        self.push_scope();
        self.define_var(var, counter);
        for stmt in &body.node {
            self.build_stmt(stmt);
        }
        self.pop_scope();

        // Increment counter.
        let one = self.fresh_value(Type::I64);
        self.emit(Instruction::Const(one, Literal::Int(1)));
        let counter_next = self.fresh_value(Type::I64);
        self.emit(Instruction::Add(counter_next, counter, one));
        let body_end_block = self.current_block_label;
        self.emit(Instruction::Jump(loop_header));
        self.seal_block();

        // Patch the phi node in the header to include the back-edge.
        // The header block is already sealed, so we find it in
        // completed_blocks and mutate.
        for block in &mut self.completed_blocks {
            if block.label == loop_header {
                if let Some(Instruction::Phi(_, ref mut entries)) =
                    block.instructions.get_mut(phi_idx)
                {
                    entries.push((body_end_block, counter_next));
                }
                break;
            }
        }

        // Loop exit.
        self.current_block_label = loop_exit;
        // For loops produce a unit value.
        let result = self.fresh_value(Type::Void);
        self.emit(Instruction::Const(result, Literal::Int(0)));
        result
    }

    // ── While loop ──────────────────────────────────────────────────

    /// Build a while loop.
    ///
    /// Lowered to:
    /// ```text
    ///   jump loop_header
    /// loop_header:
    ///   cond = <build condition>
    ///   branch cond, loop_body, loop_exit
    /// loop_body:
    ///   <build body statements>
    ///   jump loop_header
    /// loop_exit:
    ///   result = unit value
    /// ```
    fn build_while(
        &mut self,
        condition: &ast::Expr,
        body: &ast::Block,
    ) -> Value {
        let loop_header = self.fresh_block();
        let loop_body = self.fresh_block();
        let loop_exit = self.fresh_block();

        // Jump from the current block to the loop header.
        self.emit(Instruction::Jump(loop_header));
        self.seal_block();

        // Loop header: evaluate condition, branch to body or exit.
        self.current_block_label = loop_header;
        let cond_val = self.build_expr(condition);
        self.emit(Instruction::Branch(cond_val, loop_body, loop_exit));
        self.seal_block();

        // Loop body: execute body statements, jump back to header.
        self.current_block_label = loop_body;
        self.push_scope();
        for stmt in &body.node {
            if self.current_block_has_terminator() {
                break;
            }
            self.build_stmt(stmt);
        }
        self.pop_scope();
        if !self.current_block_has_terminator() {
            self.emit(Instruction::Jump(loop_header));
        }
        self.seal_block();

        // Loop exit: while loops produce a unit value.
        self.current_block_label = loop_exit;
        let result = self.fresh_value(Type::Void);
        self.emit(Instruction::Const(result, Literal::Int(0)));
        result
    }

    // ── Helpers ──────────────────────────────────────────────────────

    /// Generate a fresh SSA value, recording its type in the value_types table.
    fn fresh_value(&mut self, ty: Type) -> Value {
        let v = Value(self.next_value);
        self.next_value += 1;
        self.value_types.insert(v, ty);
        v
    }

    /// Generate a fresh block label.
    fn fresh_block(&mut self) -> BlockRef {
        let b = BlockRef(self.next_block);
        self.next_block += 1;
        b
    }

    /// Append an instruction to the current block.
    fn emit(&mut self, instr: Instruction) {
        self.current_block.push(instr);
    }

    /// Finish the current block and prepare an empty block for subsequent
    /// instructions. The new block's label must be set by the caller via
    /// `self.current_block_label = ...` before emitting more instructions.
    fn seal_block(&mut self) {
        let block = BasicBlock {
            label: self.current_block_label,
            instructions: std::mem::take(&mut self.current_block),
        };
        self.completed_blocks.push(block);
    }

    /// Check whether the current block already ends with a terminator.
    fn current_block_has_terminator(&self) -> bool {
        self.current_block.last().is_some_and(|instr| {
            matches!(
                instr,
                Instruction::Ret(_)
                    | Instruction::Branch(_, _, _)
                    | Instruction::Jump(_)
            )
        })
    }

    /// Push a new variable scope.
    fn push_scope(&mut self) {
        self.variables.push(HashMap::new());
    }

    /// Pop the current variable scope.
    fn pop_scope(&mut self) {
        if self.variables.len() > 1 {
            self.variables.pop();
        }
    }

    /// Define a variable in the current scope.
    fn define_var(&mut self, name: &str, val: Value) {
        if let Some(scope) = self.variables.last_mut() {
            scope.insert(name.to_string(), val);
        }
    }

    /// Look up a variable by walking the scope stack from innermost to
    /// outermost.
    fn lookup_var(&self, name: &str) -> Option<Value> {
        for scope in self.variables.iter().rev() {
            if let Some(&val) = scope.get(name) {
                return Some(val);
            }
        }
        None
    }

    /// Convert an AST type expression to an IR type.
    fn resolve_type(&self, type_expr: &ast::TypeExpr) -> Type {
        match type_expr {
            ast::TypeExpr::Named(name) => self.resolve_named_type(name),
            ast::TypeExpr::Unit => Type::Void,
            ast::TypeExpr::Fn { .. } => {
                // Function types are pointers in v0.1.
                Type::Ptr
            }
        }
    }

    /// Map a named type string to its IR type.
    fn resolve_named_type(&self, name: &str) -> Type {
        if name == "Int" || name == "i64" {
            Type::I64
        } else if name == "i32" {
            Type::I32
        } else if name == "Float" || name == "f64" {
            Type::F64
        } else if name == "Bool" || name == "bool" {
            Type::Bool
        } else if name == "String" || name == "str" || name == "ptr" {
            Type::Ptr
        } else {
            // Default: treat unknown types as I64 for v0.1.
            Type::I64
        }
    }
}

#[cfg(test)]
mod tests;
