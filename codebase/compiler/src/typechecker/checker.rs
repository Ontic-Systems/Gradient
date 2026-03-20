//! The main type checker for the Gradient programming language.
//!
//! The [`TypeChecker`] walks the AST produced by the parser, resolves names,
//! infers and checks types for all expressions and statements, validates
//! effect annotations, and collects structured type errors.
//!
//! # Design
//!
//! - The type checker does **not** modify the AST. It reads the AST and
//!   produces a list of [`TypeError`]s.
//! - [`Ty::Error`] is used as a sentinel for error recovery: once a type
//!   error is detected, `Error` propagates through dependent expressions
//!   without generating cascading diagnostics.
//! - For v0.1 there are no generics or Hindley-Milner unification. Type
//!   inference is limited to `let` bindings without explicit annotations.

use crate::ast::block::Block;
use crate::ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use crate::ast::item::{FnDef, ExternFnDecl, Item, ItemKind};
use crate::ast::module::Module;
use crate::ast::span::Span;
use crate::ast::stmt::{Stmt, StmtKind};
use crate::ast::types::TypeExpr;

use super::env::{FnSig, TypeEnv};
use super::error::TypeError;
use super::types::Ty;

/// The Gradient type checker.
///
/// Holds the type environment, accumulated errors, and the file id of the
/// module currently being checked.
pub struct TypeChecker {
    /// The type environment (scopes, function registry, context).
    env: TypeEnv,
    /// Type errors accumulated during checking.
    errors: Vec<TypeError>,
    /// The file id of the source file being checked (used in synthetic spans).
    #[allow(dead_code)]
    file_id: u32,
}

// =========================================================================
// Public entry point
// =========================================================================

/// Type-check a parsed module and return any type errors found.
///
/// This is the primary entry point for the type checker. It creates a fresh
/// [`TypeChecker`], registers all top-level function signatures, then checks
/// each item in the module.
pub fn check_module(module: &Module, file_id: u32) -> Vec<TypeError> {
    let mut checker = TypeChecker::new(file_id);
    checker.check_module(module);
    checker.errors
}

// =========================================================================
// Implementation
// =========================================================================

impl TypeChecker {
    /// Create a new type checker for the given file.
    fn new(file_id: u32) -> Self {
        Self {
            env: TypeEnv::new(),
            errors: Vec::new(),
            file_id,
        }
    }

    // ------------------------------------------------------------------
    // Module and items
    // ------------------------------------------------------------------

    /// Check an entire module: first register all function signatures (so that
    /// forward references work), then check each item's body.
    fn check_module(&mut self, module: &Module) {
        // First pass: register all function signatures.
        for item in &module.items {
            match &item.node {
                ItemKind::FnDef(fn_def) => {
                    let sig = self.fn_def_to_sig(fn_def);
                    self.env.define_fn(fn_def.name.clone(), sig);
                }
                ItemKind::ExternFn(decl) => {
                    let sig = self.extern_fn_to_sig(decl);
                    self.env.define_fn(decl.name.clone(), sig);
                }
                _ => {}
            }
        }

        // Second pass: check each item.
        for item in &module.items {
            self.check_item(item);
        }
    }

    /// Check a single top-level item.
    fn check_item(&mut self, item: &Item) {
        match &item.node {
            ItemKind::FnDef(fn_def) => self.check_fn_def(fn_def),
            ItemKind::ExternFn(decl) => self.check_extern_fn(decl),
            ItemKind::Let {
                name,
                type_ann,
                value,
            } => self.check_let(name, type_ann.as_ref(), value, item.span),
            ItemKind::TypeDecl { .. } => {
                // Type declarations are not semantically checked in v0.1.
            }
        }
    }

    /// Check a function definition: set up parameter bindings and return type
    /// context, then type-check the body.
    fn check_fn_def(&mut self, fn_def: &FnDef) {
        let ret_ty = fn_def
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(&t.node))
            .unwrap_or(Ty::Unit);

        let effects: Vec<String> = fn_def
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        self.env.set_current_fn_return(ret_ty.clone());
        self.env.set_current_effects(effects);
        self.env.push_scope();

        // Bind parameters.
        for param in &fn_def.params {
            let param_ty = self.resolve_type_expr(&param.type_ann.node);
            self.env.define(param.name.clone(), param_ty);
        }

        // Check the body.
        let body_ty = self.check_block(&fn_def.body);

        // If the function has an explicit return type, the body's type must
        // match. (We skip this check for Unit return types since trailing
        // expressions are often discarded.)
        if fn_def.return_type.is_some() && !body_ty.is_error() && !ret_ty.is_error() {
            // Allow Unit body when the return type is non-Unit only if there
            // are explicit `ret` statements. For v0.1, we just check that the
            // body type matches.
            if body_ty != ret_ty && body_ty != Ty::Unit {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "function `{}` body has type `{}`, expected `{}`",
                        fn_def.name, body_ty, ret_ty
                    ),
                    fn_def.body.span,
                    ret_ty,
                    body_ty,
                ));
            }
        }

        self.env.pop_scope();
        self.env.clear_current_fn_return();
        self.env.clear_current_effects();
    }

    /// Check an extern function declaration (no body to check, just validate
    /// that the signature is well-formed).
    fn check_extern_fn(&mut self, decl: &ExternFnDecl) {
        // Validate parameter types are resolvable.
        for param in &decl.params {
            let _ = self.resolve_type_expr(&param.type_ann.node);
        }
        // Validate return type.
        if let Some(ref rt) = decl.return_type {
            let _ = self.resolve_type_expr(&rt.node);
        }
    }

    // ------------------------------------------------------------------
    // Blocks and statements
    // ------------------------------------------------------------------

    /// Check a block of statements, returning the type of the last expression
    /// (or `Unit` if the block is empty or ends with a non-expression
    /// statement).
    fn check_block(&mut self, block: &Block) -> Ty {
        self.env.push_scope();

        let mut last_ty = Ty::Unit;
        for (i, stmt) in block.node.iter().enumerate() {
            let is_last = i == block.node.len() - 1;
            last_ty = self.check_stmt(stmt, is_last);
        }

        self.env.pop_scope();
        last_ty
    }

    /// Check a statement. Returns the type it contributes to the block: for
    /// an expression statement in tail position, this is the expression's type;
    /// otherwise `Unit`.
    fn check_stmt(&mut self, stmt: &Stmt, is_tail: bool) -> Ty {
        match &stmt.node {
            StmtKind::Let {
                name,
                type_ann,
                value,
            } => {
                self.check_let(name, type_ann.as_ref(), value, stmt.span);
                Ty::Unit
            }
            StmtKind::Ret(expr) => {
                let ty = self.check_expr(expr);
                if let Some(expected) = self.env.current_fn_return() {
                    let expected = expected.clone();
                    if !ty.is_error() && !expected.is_error() && ty != expected {
                        self.errors.push(TypeError::mismatch(
                            format!(
                                "`ret` type mismatch: expected `{}`, found `{}`",
                                expected, ty
                            ),
                            expr.span,
                            expected,
                            ty,
                        ));
                    }
                }
                Ty::Unit // ret doesn't contribute a value to the block
            }
            StmtKind::Expr(expr) => {
                let ty = self.check_expr(expr);
                if is_tail {
                    ty
                } else {
                    Ty::Unit
                }
            }
        }
    }

    /// Check a `let` binding: if there is a type annotation, verify the value
    /// matches; otherwise infer the type from the value.
    fn check_let(
        &mut self,
        name: &str,
        type_ann: Option<&crate::ast::span::Spanned<TypeExpr>>,
        value: &Expr,
        span: Span,
    ) {
        let value_ty = self.check_expr(value);

        if let Some(ann) = type_ann {
            let ann_ty = self.resolve_type_expr(&ann.node);
            if !value_ty.is_error() && !ann_ty.is_error() && value_ty != ann_ty {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "type mismatch in `let {}`: declared `{}`, but value has type `{}`",
                        name, ann_ty, value_ty
                    ),
                    span,
                    ann_ty.clone(),
                    value_ty.clone(),
                ));
            }
            // Use the annotation type even on mismatch so that the name is
            // usable in subsequent code.
            self.env.define(name.to_string(), ann_ty);
        } else {
            // Infer from the value.
            self.env.define(name.to_string(), value_ty);
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    /// Infer the type of an expression. This is the core of the type checker.
    fn check_expr(&mut self, expr: &Expr) -> Ty {
        match &expr.node {
            ExprKind::IntLit(_) => Ty::Int,
            ExprKind::FloatLit(_) => Ty::Float,
            ExprKind::StringLit(_) => Ty::String,
            ExprKind::BoolLit(_) => Ty::Bool,
            ExprKind::UnitLit => Ty::Unit,

            ExprKind::Ident(name) => {
                // First check local variables, then function names.
                if let Some(ty) = self.env.lookup(name) {
                    return ty.clone();
                }
                if let Some(sig) = self.env.lookup_fn(name) {
                    return Ty::Fn {
                        params: sig.params.iter().map(|(_, t)| t.clone()).collect(),
                        ret: Box::new(sig.ret.clone()),
                        effects: sig.effects.clone(),
                    };
                }
                self.errors.push(TypeError::new(
                    format!("undefined variable `{}`", name),
                    expr.span,
                ));
                Ty::Error
            }

            ExprKind::TypedHole(label) => {
                let label_str = label
                    .as_ref()
                    .map(|l| format!("?{}", l))
                    .unwrap_or_else(|| "?".to_string());
                self.errors.push(
                    TypeError::new(
                        format!("typed hole `{}` found", label_str),
                        expr.span,
                    )
                    .with_note("fill in the hole with a concrete expression".to_string()),
                );
                Ty::Error
            }

            ExprKind::BinaryOp { op, left, right } => {
                self.check_binary_op(*op, left, right, expr.span)
            }

            ExprKind::UnaryOp { op, operand } => {
                self.check_unary_op(*op, operand, expr.span)
            }

            ExprKind::Call { func, args } => self.check_call(func, args, expr.span),

            ExprKind::FieldAccess { object, field } => {
                let _obj_ty = self.check_expr(object);
                // Field access is not supported in v0.1 beyond type checking
                // the object. We report an error since there are no struct
                // types yet.
                self.errors.push(TypeError::new(
                    format!("field access `.{}` is not supported in v0.1", field),
                    expr.span,
                ));
                Ty::Error
            }

            ExprKind::If {
                condition,
                then_block,
                else_ifs,
                else_block,
            } => self.check_if(condition, then_block, else_ifs, else_block, expr.span),

            ExprKind::For { var, iter, body } => {
                // Check the iterator expression (we accept any type for v0.1).
                let _iter_ty = self.check_expr(iter);

                self.env.push_scope();
                // Bind the loop variable. For v0.1 we just give it Int type
                // (since `range` is the primary iterator).
                self.env.define(var.clone(), Ty::Int);
                let _body_ty = self.check_block(body);
                self.env.pop_scope();

                Ty::Unit
            }

            ExprKind::Paren(inner) => self.check_expr(inner),
        }
    }

    /// Type-check a binary operation.
    fn check_binary_op(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        span: Span,
    ) -> Ty {
        let left_ty = self.check_expr(left);
        let right_ty = self.check_expr(right);

        // If either side is Error, propagate without further diagnostics.
        if left_ty.is_error() || right_ty.is_error() {
            return Ty::Error;
        }

        match op {
            // Arithmetic: both sides must be the same numeric type.
            // Special case: `+` on String performs concatenation.
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                // String concatenation: "a" + "b"
                if op == BinOp::Add && left_ty == Ty::String && right_ty == Ty::String {
                    return Ty::String;
                }
                if !left_ty.is_numeric() {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "operator `{}` requires numeric operands, found `{}`",
                            binop_symbol(op),
                            left_ty
                        ),
                        left.span,
                        Ty::Int, // hint
                        left_ty.clone(),
                    ));
                    return Ty::Error;
                }
                if left_ty != right_ty {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "operands of `{}` must have the same type",
                            binop_symbol(op)
                        ),
                        span,
                        left_ty,
                        right_ty,
                    ));
                    return Ty::Error;
                }
                left_ty
            }

            // Ordering comparisons: numeric types only.
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if !left_ty.is_numeric() {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "operator `{}` requires numeric operands, found `{}`",
                            binop_symbol(op),
                            left_ty
                        ),
                        left.span,
                        Ty::Int,
                        left_ty.clone(),
                    ));
                    return Ty::Error;
                }
                if left_ty != right_ty {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "operands of `{}` must have the same type",
                            binop_symbol(op)
                        ),
                        span,
                        left_ty,
                        right_ty,
                    ));
                    return Ty::Error;
                }
                Ty::Bool
            }

            // Equality: any type, but both sides must match.
            BinOp::Eq | BinOp::Ne => {
                if left_ty != right_ty {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "operands of `{}` must have the same type",
                            binop_symbol(op)
                        ),
                        span,
                        left_ty,
                        right_ty,
                    ));
                    return Ty::Error;
                }
                Ty::Bool
            }

            // Logical operators: both sides must be Bool.
            BinOp::And | BinOp::Or => {
                if left_ty != Ty::Bool {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "operator `{}` requires Bool operands, found `{}`",
                            binop_symbol(op),
                            left_ty
                        ),
                        left.span,
                        Ty::Bool,
                        left_ty,
                    ));
                    return Ty::Error;
                }
                if right_ty != Ty::Bool {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "operator `{}` requires Bool operands, found `{}`",
                            binop_symbol(op),
                            right_ty
                        ),
                        right.span,
                        Ty::Bool,
                        right_ty,
                    ));
                    return Ty::Error;
                }
                Ty::Bool
            }
        }
    }

    /// Type-check a unary operation.
    fn check_unary_op(
        &mut self,
        op: UnaryOp,
        operand: &Expr,
        span: Span,
    ) -> Ty {
        let operand_ty = self.check_expr(operand);

        if operand_ty.is_error() {
            return Ty::Error;
        }

        match op {
            UnaryOp::Neg => {
                if !operand_ty.is_numeric() {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "unary `-` requires a numeric operand, found `{}`",
                            operand_ty
                        ),
                        span,
                        Ty::Int,
                        operand_ty,
                    ));
                    Ty::Error
                } else {
                    operand_ty
                }
            }
            UnaryOp::Not => {
                if operand_ty != Ty::Bool {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "`not` requires a Bool operand, found `{}`",
                            operand_ty
                        ),
                        span,
                        Ty::Bool,
                        operand_ty,
                    ));
                    Ty::Error
                } else {
                    Ty::Bool
                }
            }
        }
    }

    /// Type-check a function call expression.
    fn check_call(
        &mut self,
        func: &Expr,
        args: &[Expr],
        span: Span,
    ) -> Ty {
        // Resolve the function being called.
        let func_name = match &func.node {
            ExprKind::Ident(name) => Some(name.clone()),
            _ => None,
        };

        // Try to look up a known function signature by name.
        let sig = func_name
            .as_ref()
            .and_then(|n| self.env.lookup_fn(n))
            .cloned();

        if let Some(sig) = sig {
            // Check argument count.
            if args.len() != sig.params.len() {
                self.errors.push(
                    TypeError::new(
                        format!(
                            "function `{}` expects {} argument(s), but {} were provided",
                            func_name.as_deref().unwrap_or("<unknown>"),
                            sig.params.len(),
                            args.len()
                        ),
                        span,
                    )
                );
                return Ty::Error;
            }

            // Check each argument type.
            for (i, (arg, (param_name, param_ty))) in
                args.iter().zip(sig.params.iter()).enumerate()
            {
                let arg_ty = self.check_expr(arg);
                if !arg_ty.is_error() && !param_ty.is_error() && arg_ty != *param_ty {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "argument {} (`{}`) of `{}`: expected `{}`, found `{}`",
                            i + 1,
                            param_name,
                            func_name.as_deref().unwrap_or("<unknown>"),
                            param_ty,
                            arg_ty
                        ),
                        arg.span,
                        param_ty.clone(),
                        arg_ty,
                    ));
                }
            }

            // Effect checking: if the called function has effects, verify
            // they are available in the current context.
            if !sig.effects.is_empty() {
                let current = self.env.current_effects().to_vec();
                for effect in &sig.effects {
                    if !current.contains(effect) {
                        self.errors.push(
                            TypeError::new(
                                format!(
                                    "function `{}` requires effect `{}`, but the current context does not provide it",
                                    func_name.as_deref().unwrap_or("<unknown>"),
                                    effect
                                ),
                                span,
                            )
                            .with_note(format!(
                                "add `!{{{}}}` to the enclosing function's signature",
                                effect
                            )),
                        );
                    }
                }
            }

            sig.ret.clone()
        } else {
            // Not a known function. Try to check the callee expression as
            // a general expression.
            let callee_ty = self.check_expr(func);

            if callee_ty.is_error() {
                // Already reported an error (e.g. undefined variable).
                // Check args to find errors in them too.
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                return Ty::Error;
            }

            // If the callee resolved to a function type, check against it.
            if let Ty::Fn {
                params,
                ret,
                effects,
            } = &callee_ty
            {
                if args.len() != params.len() {
                    self.errors.push(TypeError::new(
                        format!(
                            "function expects {} argument(s), but {} were provided",
                            params.len(),
                            args.len()
                        ),
                        span,
                    ));
                    return Ty::Error;
                }

                for (arg, param_ty) in args.iter().zip(params.iter()) {
                    let arg_ty = self.check_expr(arg);
                    if !arg_ty.is_error() && !param_ty.is_error() && arg_ty != *param_ty {
                        self.errors.push(TypeError::mismatch(
                            "argument type mismatch".to_string(),
                            arg.span,
                            param_ty.clone(),
                            arg_ty,
                        ));
                    }
                }

                // Effect checking for function-typed values.
                if !effects.is_empty() {
                    let current = self.env.current_effects().to_vec();
                    for effect in effects {
                        if !current.contains(effect) {
                            self.errors.push(
                                TypeError::new(
                                    format!(
                                        "calling a function with effect `{}`, but the current context does not provide it",
                                        effect
                                    ),
                                    span,
                                )
                                .with_note(format!(
                                    "add `!{{{}}}` to the enclosing function's signature",
                                    effect
                                )),
                            );
                        }
                    }
                }

                return *ret.clone();
            }

            self.errors.push(TypeError::new(
                format!("expression of type `{}` is not callable", callee_ty),
                func.span,
            ));
            // Still check args.
            for arg in args {
                let _ = self.check_expr(arg);
            }
            Ty::Error
        }
    }

    /// Type-check an `if` / `else if` / `else` expression.
    fn check_if(
        &mut self,
        condition: &Expr,
        then_block: &Block,
        else_ifs: &[(Expr, Block)],
        else_block: &Option<Block>,
        _span: Span,
    ) -> Ty {
        // Condition must be Bool.
        let cond_ty = self.check_expr(condition);
        if !cond_ty.is_error() && cond_ty != Ty::Bool {
            self.errors.push(TypeError::mismatch(
                format!("`if` condition must be Bool, found `{}`", cond_ty),
                condition.span,
                Ty::Bool,
                cond_ty,
            ));
        }

        let then_ty = self.check_block(then_block);

        // Check else-if branches.
        for (elif_cond, elif_block) in else_ifs {
            let elif_cond_ty = self.check_expr(elif_cond);
            if !elif_cond_ty.is_error() && elif_cond_ty != Ty::Bool {
                self.errors.push(TypeError::mismatch(
                    format!("`else if` condition must be Bool, found `{}`", elif_cond_ty),
                    elif_cond.span,
                    Ty::Bool,
                    elif_cond_ty,
                ));
            }

            let elif_ty = self.check_block(elif_block);
            if !then_ty.is_error() && !elif_ty.is_error() && then_ty != elif_ty {
                self.errors.push(TypeError::mismatch(
                    "all branches of `if` expression must have the same type".to_string(),
                    elif_block.span,
                    then_ty.clone(),
                    elif_ty,
                ));
            }
        }

        // Check else block.
        if let Some(else_blk) = else_block {
            let else_ty = self.check_block(else_blk);
            if !then_ty.is_error() && !else_ty.is_error() && then_ty != else_ty {
                self.errors.push(TypeError::mismatch(
                    "all branches of `if` expression must have the same type".to_string(),
                    else_blk.span,
                    then_ty.clone(),
                    else_ty,
                ));
            }
            // The if expression produces the then-branch type (assuming all
            // branches agree or errors have been reported).
            if then_ty.is_error() {
                Ty::Error
            } else {
                then_ty
            }
        } else {
            // No else block: the expression type is Unit.
            Ty::Unit
        }
    }

    // ------------------------------------------------------------------
    // Type resolution
    // ------------------------------------------------------------------

    /// Convert an AST [`TypeExpr`] to an internal [`Ty`].
    fn resolve_type_expr(&self, te: &TypeExpr) -> Ty {
        match te {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" => Ty::Int,
                "Float" => Ty::Float,
                "String" => Ty::String,
                "Bool" => Ty::Bool,
                _ => {
                    // Unknown type name. In v0.1 we don't have user-defined
                    // types, so this is always an error. However, we return
                    // Error to avoid cascading issues.
                    Ty::Error
                }
            },
            TypeExpr::Unit => Ty::Unit,
            TypeExpr::Fn { params, ret } => {
                let param_tys: Vec<Ty> = params
                    .iter()
                    .map(|p| self.resolve_type_expr(&p.node))
                    .collect();
                let ret_ty = self.resolve_type_expr(&ret.node);
                Ty::Fn {
                    params: param_tys,
                    ret: Box::new(ret_ty),
                    effects: vec![],
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Build a [`FnSig`] from a parsed function definition.
    fn fn_def_to_sig(&self, fn_def: &FnDef) -> FnSig {
        let params: Vec<(String, Ty)> = fn_def
            .params
            .iter()
            .map(|p| (p.name.clone(), self.resolve_type_expr(&p.type_ann.node)))
            .collect();

        let ret = fn_def
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(&t.node))
            .unwrap_or(Ty::Unit);

        let effects = fn_def
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        FnSig {
            params,
            ret,
            effects,
        }
    }

    /// Build a [`FnSig`] from a parsed extern function declaration.
    fn extern_fn_to_sig(&self, decl: &ExternFnDecl) -> FnSig {
        let params: Vec<(String, Ty)> = decl
            .params
            .iter()
            .map(|p| (p.name.clone(), self.resolve_type_expr(&p.type_ann.node)))
            .collect();

        let ret = decl
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(&t.node))
            .unwrap_or(Ty::Unit);

        let effects = decl
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        FnSig {
            params,
            ret,
            effects,
        }
    }
}

// =========================================================================
// Formatting helpers
// =========================================================================

/// Return the human-readable symbol for a binary operator.
fn binop_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
    }
}
