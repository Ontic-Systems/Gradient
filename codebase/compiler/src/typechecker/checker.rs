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
use crate::ast::expr::{BinOp, ClosureParam, Expr, ExprKind, MatchArm, Pattern, UnaryOp};
use crate::ast::item::{BudgetConstraint, ContractKind, FnDef, ExternFnDecl, Item, ItemKind};
use crate::ast::module::Module;
use crate::ast::span::{Span, Spanned};
use crate::ast::stmt::{Stmt, StmtKind};
use crate::ast::types::TypeExpr;

use super::effects::{self, EffectInfo, ModuleEffectSummary};
use super::env::TypeEnv;
use super::env::FnSig;
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
    /// Effects inferred for each function body during checking.
    /// Key: function name, Value: set of effects used in the body.
    inferred_effects: std::collections::HashMap<String, Vec<String>>,
    /// Effects currently being collected for the function being checked.
    current_inferred: Vec<String>,
    /// Module-level capability ceiling (from `@cap(...)` declaration).
    /// If set, no function in this module may use effects outside this set.
    module_capabilities: Option<Vec<String>>,
    /// Type parameters currently in scope (set during fn_def_to_sig and check_fn_def).
    /// Names listed here will resolve to Ty::TypeVar instead of "unknown type" errors.
    active_type_params: Vec<String>,
    /// Runtime capability budgets declared on functions via `@budget(...)`.
    /// Key: function name, Value: the budget constraint.
    function_budgets: std::collections::HashMap<String, BudgetConstraint>,
    /// Name of the function currently being checked (for budget containment).
    current_fn_name: Option<String>,
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

/// Type-check a parsed module and return both errors and effect analysis.
///
/// This is the agent-oriented entry point. In addition to type errors, it
/// returns a [`ModuleEffectSummary`] with per-function effect inference,
/// purity guarantees, and unused-effect warnings.
pub fn check_module_with_effects(
    module: &Module,
    file_id: u32,
) -> (Vec<TypeError>, ModuleEffectSummary) {
    let mut checker = TypeChecker::new(file_id);
    checker.check_module(module);

    let summary = checker.build_effect_summary(module);
    (checker.errors, summary)
}

/// A set of imported module function signatures, used for multi-file type checking.
///
/// Each entry maps a module name to a map of function name -> signature.
pub type ImportedModules = std::collections::HashMap<String, std::collections::HashMap<String, FnSig>>;

/// Type-check a parsed module with imported module signatures and return both
/// errors and effect analysis.
///
/// This is the multi-file entry point. The `imports` parameter provides the
/// function signatures from all modules referenced by `use` declarations.
pub fn check_module_with_imports(
    module: &Module,
    file_id: u32,
    imports: &ImportedModules,
) -> (Vec<TypeError>, ModuleEffectSummary) {
    let mut checker = TypeChecker::new(file_id);

    // Register imported module signatures.
    for (module_name, fns) in imports {
        checker.env.import_module(module_name.clone(), fns.clone());
    }

    checker.check_module(module);

    let summary = checker.build_effect_summary(module);
    (checker.errors, summary)
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
            inferred_effects: std::collections::HashMap::new(),
            current_inferred: Vec::new(),
            module_capabilities: None,
            active_type_params: Vec::new(),
            function_budgets: std::collections::HashMap::new(),
            current_fn_name: None,
        }
    }

    // ------------------------------------------------------------------
    // Module and items
    // ------------------------------------------------------------------

    /// Check an entire module: first register all function signatures (so that
    /// forward references work), then check each item's body.
    fn check_module(&mut self, module: &Module) {
        // Pre-pass: find and register module-level capability declarations.
        for item in &module.items {
            if let ItemKind::CapDecl { allowed_effects } = &item.node {
                // Validate effect names in the capability declaration.
                for eff in allowed_effects {
                    if !effects::is_known_effect(eff) {
                        self.errors.push(
                            TypeError::new(
                                format!("unknown effect `{}` in @cap declaration", eff),
                                item.span,
                            )
                            .with_note(format!(
                                "known effects: {}",
                                effects::KNOWN_EFFECTS.join(", ")
                            )),
                        );
                    }
                }
                self.module_capabilities = Some(allowed_effects.clone());
            }
        }

        // First pass: register all function signatures.
        for item in &module.items {
            match &item.node {
                ItemKind::FnDef(fn_def) => {
                    // If module has capability constraints, check that declared
                    // effects don't exceed the module ceiling.
                    if let Some(ref caps) = self.module_capabilities {
                        if let Some(ref effect_set) = fn_def.effects {
                            for eff in &effect_set.effects {
                                // Skip effect variables (lowercase) — they are
                                // resolved at call sites and may or may not
                                // exceed the ceiling depending on instantiation.
                                if !caps.contains(eff) && !effects::is_effect_variable(eff) {
                                    self.errors.push(
                                        TypeError::new(
                                            format!(
                                                "function `{}` declares effect `{}` which exceeds the module capability ceiling",
                                                fn_def.name, eff
                                            ),
                                            fn_def.body.span,
                                        )
                                        .with_note(format!(
                                            "module @cap allows: {}",
                                            caps.join(", ")
                                        )),
                                    );
                                }
                            }
                        }
                    }

                    let sig = self.fn_def_to_sig(fn_def);
                    self.env.define_fn(fn_def.name.clone(), sig);

                    // Pre-register budget constraints so containment
                    // checking works for forward references.
                    if let Some(ref budget) = fn_def.budget {
                        self.function_budgets.insert(fn_def.name.clone(), budget.clone());
                    }
                }
                ItemKind::ExternFn(decl) => {
                    let sig = self.extern_fn_to_sig(decl);
                    self.env.define_fn(decl.name.clone(), sig);
                }
                ItemKind::TypeDecl { name, type_expr, .. } => {
                    let ty = self.resolve_type_expr(&type_expr.node, type_expr.span);
                    self.env.define_type_alias(name.clone(), ty);
                }
                ItemKind::EnumDecl { name, variants, .. } => {
                    let mut ty_variants = Vec::new();
                    for v in variants {
                        let field_ty = v.field.as_ref().map(|f| {
                            self.resolve_type_expr(&f.node, f.span)
                        });
                        ty_variants.push((v.name.clone(), field_ty));
                    }
                    let enum_ty = Ty::Enum {
                        name: name.clone(),
                        variants: ty_variants.clone(),
                    };
                    self.env.define_enum(name.clone(), enum_ty.clone());

                    // Register unit variants as values of the enum type
                    // in the global scope, and tuple variants as functions.
                    for (vname, field) in &ty_variants {
                        match field {
                            None => {
                                // Unit variant: register as a variable with the enum type.
                                self.env.define(vname.clone(), enum_ty.clone());
                            }
                            Some(field_ty) => {
                                // Tuple variant: register as a function from field_ty to enum_ty.
                                self.env.define_fn(
                                    vname.clone(),
                                    FnSig {
                                        type_params: vec![],
                                        params: vec![("value".to_string(), field_ty.clone())],
                                        ret: enum_ty.clone(),
                                        effects: vec![],
                                    },
                                );
                            }
                        }
                    }
                }
                ItemKind::ActorDecl { name, state_fields, handlers, .. } => {
                    // Register actor type and its handler signatures.
                    let mut actor_state = Vec::new();
                    for sf in state_fields {
                        let ty = self.resolve_type_expr(&sf.type_ann.node, sf.type_ann.span);
                        actor_state.push((sf.name.clone(), ty));
                    }
                    let mut actor_handlers = Vec::new();
                    for h in handlers {
                        let ret_ty = h.return_type.as_ref()
                            .map(|t| self.resolve_type_expr(&t.node, t.span))
                            .unwrap_or(Ty::Unit);
                        actor_handlers.push((h.message_name.clone(), ret_ty));
                    }
                    self.env.define_actor(
                        name.clone(),
                        super::env::ActorInfo {
                            name: name.clone(),
                            state_fields: actor_state,
                            handlers: actor_handlers,
                        },
                    );
                }
                ItemKind::TraitDecl { name, methods, .. } => {
                    let mut trait_methods = Vec::new();
                    for m in methods {
                        // Resolve param types (skip self).
                        let params: Vec<(String, Ty)> = m.params.iter()
                            .filter(|p| p.name != "self")
                            .map(|p| (p.name.clone(), self.resolve_type_expr(&p.type_ann.node, p.type_ann.span)))
                            .collect();
                        let ret = m.return_type.as_ref()
                            .map(|t| self.resolve_type_expr(&t.node, t.span))
                            .unwrap_or(Ty::Unit);
                        let effects = m.effects.as_ref()
                            .map(|e| e.effects.clone())
                            .unwrap_or_default();
                        trait_methods.push(super::env::TraitMethodSig {
                            name: m.name.clone(),
                            params,
                            ret,
                            effects,
                        });
                    }
                    self.env.define_trait(
                        name.clone(),
                        super::env::TraitInfo {
                            name: name.clone(),
                            methods: trait_methods,
                        },
                    );
                }
                ItemKind::ImplBlock { trait_name, target_type, methods } => {
                    // Register the impl and its methods as functions.
                    self.env.register_impl(super::env::ImplInfo {
                        trait_name: trait_name.clone(),
                        target_type: target_type.clone(),
                    });

                    // Register each impl method as a function named
                    // `TraitName::method_name` for resolution.
                    for method in methods {
                        let sig = self.fn_def_to_sig(method);
                        let qualified_name = format!("{}::{}", target_type, method.name);
                        self.env.define_fn(qualified_name, sig);
                    }
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
                mutable,
            } => self.check_let(name, type_ann.as_ref(), value, item.span, *mutable),
            ItemKind::TypeDecl { .. } => {
                // Type aliases are resolved in the first pass (check_module).
            }
            ItemKind::EnumDecl { .. } => {
                // Enum declarations are resolved in the first pass (check_module).
            }
            ItemKind::CapDecl { .. } => {
                // Capability declarations are processed in check_module pre-pass.
            }
            ItemKind::LetTupleDestructure {
                names,
                type_ann,
                value,
            } => {
                self.check_let_tuple_destructure(names, type_ann.as_ref(), value, item.span);
            }
            ItemKind::ActorDecl { name, state_fields, handlers, .. } => {
                self.check_actor_decl(name, state_fields, handlers);
            }
            ItemKind::TraitDecl { .. } => {
                // Trait declarations are validated in the first pass.
            }
            ItemKind::ImplBlock { trait_name, target_type, methods, .. } => {
                self.check_impl_block(trait_name, target_type, methods);
            }
        }
    }

    /// Check a function definition: set up parameter bindings and return type
    /// context, then type-check the body. Also infers which effects the body
    /// actually requires and validates declared effect names.
    fn check_fn_def(&mut self, fn_def: &FnDef) {
        // Set active type parameters so resolve_type_expr produces TypeVar.
        let tp_names: Vec<String> = fn_def.type_params.iter().map(|tp| tp.name.clone()).collect();
        let saved_type_params = std::mem::replace(
            &mut self.active_type_params,
            tp_names,
        );

        let ret_ty = fn_def
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(&t.node, t.span))
            .unwrap_or(Ty::Unit);

        let declared_effects: Vec<String> = fn_def
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        // Validate declared effect names (skip effect variables — lowercase).
        if let Some(ref effect_set) = fn_def.effects {
            for eff_name in &effect_set.effects {
                if !effects::is_known_effect(eff_name) && !effects::is_effect_variable(eff_name) {
                    self.errors.push(
                        TypeError::new(
                            format!("unknown effect `{}`", eff_name),
                            fn_def.body.span,
                        )
                        .with_note(format!(
                            "known effects: {}",
                            effects::KNOWN_EFFECTS.join(", ")
                        )),
                    );
                }
            }
        }

        // Validate @budget annotation values if present.
        if let Some(ref budget) = fn_def.budget {
            self.validate_budget(budget, &fn_def.name);
            self.function_budgets.insert(fn_def.name.clone(), budget.clone());
        }

        // Validate FFI-compatible types for @export functions.
        if fn_def.is_export {
            for param in &fn_def.params {
                let ty = self.resolve_type_expr(&param.type_ann.node, param.type_ann.span);
                if !Self::is_ffi_compatible(&ty) {
                    self.errors.push(TypeError::new(
                        format!(
                            "parameter '{}' of @export function '{}' has type '{}' which is not FFI-compatible (allowed: Int, Float, Bool, String, ())",
                            param.name, fn_def.name, ty
                        ),
                        param.type_ann.span,
                    ));
                }
            }
            if !Self::is_ffi_compatible(&ret_ty) {
                let span = fn_def.return_type.as_ref().map(|t| t.span).unwrap_or(fn_def.body.span);
                self.errors.push(TypeError::new(
                    format!(
                        "return type of @export function '{}' is '{}' which is not FFI-compatible (allowed: Int, Float, Bool, String, ())",
                        fn_def.name, ret_ty
                    ),
                    span,
                ));
            }
        }

        // Validate @test functions: must take no parameters and return () or Bool.
        if fn_def.is_test {
            if !fn_def.params.is_empty() {
                self.errors.push(TypeError::new(
                    format!(
                        "@test function '{}' must take no parameters, but has {}",
                        fn_def.name,
                        fn_def.params.len()
                    ),
                    fn_def.body.span,
                ));
            }
            if ret_ty != Ty::Unit && ret_ty != Ty::Bool {
                let span = fn_def.return_type.as_ref().map(|t| t.span).unwrap_or(fn_def.body.span);
                self.errors.push(TypeError::new(
                    format!(
                        "@test function '{}' must return () or Bool, but returns '{}'",
                        fn_def.name, ret_ty
                    ),
                    span,
                ));
            }
        }

        self.env.set_current_fn_return(ret_ty.clone());
        self.env.set_current_effects(declared_effects.clone());
        self.env.push_scope();

        // Track current function name for budget containment checks.
        let saved_fn_name = self.current_fn_name.take();
        self.current_fn_name = Some(fn_def.name.clone());

        // Reset inferred effects for this function.
        let saved_inferred = std::mem::take(&mut self.current_inferred);

        // Bind parameters.
        for param in &fn_def.params {
            let param_ty = self.resolve_type_expr(&param.type_ann.node, param.type_ann.span);
            self.env.define(param.name.clone(), param_ty);
        }

        // Type-check @requires preconditions (parameters are in scope).
        for contract in &fn_def.contracts {
            if contract.kind == ContractKind::Requires {
                let cond_ty = self.check_expr(&contract.condition);
                if !cond_ty.is_error() && cond_ty != Ty::Bool {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "@requires condition must be Bool, found `{}`",
                            cond_ty
                        ),
                        contract.span,
                        Ty::Bool,
                        cond_ty,
                    ));
                }
            }
        }

        // Check the body.
        let body_ty = self.check_block(&fn_def.body);

        // If the function has an explicit return type, the body's type must
        // match. (We skip this check for Unit return types since trailing
        // expressions are often discarded, and for generic functions where
        // the return type contains type variables.)
        if fn_def.return_type.is_some()
            && !body_ty.is_error()
            && !ret_ty.is_error()
            && !ret_ty.is_type_var()
            && !body_ty.is_type_var()
            && body_ty != ret_ty
            && body_ty != Ty::Unit
        {
            self.errors.push(TypeError::mismatch(
                format!(
                    "function `{}` body has type `{}`, expected `{}`",
                    fn_def.name, body_ty, ret_ty
                ),
                fn_def.body.span,
                ret_ty.clone(),
                body_ty,
            ));
        }

        // Type-check @ensures postconditions.
        // `result` is bound to the function's return type in a nested scope.
        for contract in &fn_def.contracts {
            if contract.kind == ContractKind::Ensures {
                self.env.push_scope();
                self.env.define("result".to_string(), ret_ty.clone());
                let cond_ty = self.check_expr(&contract.condition);
                if !cond_ty.is_error() && cond_ty != Ty::Bool {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "@ensures condition must be Bool, found `{}`",
                            cond_ty
                        ),
                        contract.span,
                        Ty::Bool,
                        cond_ty,
                    ));
                }
                self.env.pop_scope();
            }
        }

        // Store inferred effects for this function.
        let mut inferred = std::mem::replace(&mut self.current_inferred, saved_inferred);
        inferred.sort();
        inferred.dedup();
        self.inferred_effects
            .insert(fn_def.name.clone(), inferred);

        self.env.pop_scope();
        self.env.clear_current_fn_return();
        self.env.clear_current_effects();
        self.active_type_params = saved_type_params;
        self.current_fn_name = saved_fn_name;
    }

    /// Check an extern function declaration (no body to check, just validate
    /// that the signature is well-formed and all types are FFI-compatible).
    fn check_extern_fn(&mut self, decl: &ExternFnDecl) {
        // Validate parameter types are resolvable and FFI-compatible.
        for param in &decl.params {
            let ty = self.resolve_type_expr(&param.type_ann.node, param.type_ann.span);
            if !Self::is_ffi_compatible(&ty) {
                self.errors.push(super::error::TypeError::new(
                    format!(
                        "parameter '{}' of extern function '{}' has type '{}' which is not FFI-compatible (allowed: Int, Float, Bool, String, ())",
                        param.name, decl.name, ty
                    ),
                    param.type_ann.span,
                ));
            }
        }
        // Validate return type is FFI-compatible.
        if let Some(ref rt) = decl.return_type {
            let ty = self.resolve_type_expr(&rt.node, rt.span);
            if !Self::is_ffi_compatible(&ty) {
                self.errors.push(super::error::TypeError::new(
                    format!(
                        "return type of extern function '{}' is '{}' which is not FFI-compatible (allowed: Int, Float, Bool, String, ())",
                        decl.name, ty
                    ),
                    rt.span,
                ));
            }
        }
    }

    /// Check whether a type is FFI-compatible for use in `@extern` / `@export`
    /// function signatures.
    ///
    /// FFI-compatible types are: Int, Float, Bool, String, Unit (void).
    fn is_ffi_compatible(ty: &Ty) -> bool {
        matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::String | Ty::Unit | Ty::Error)
    }

    // ------------------------------------------------------------------
    // Actor declarations
    // ------------------------------------------------------------------

    /// Check an actor declaration: validate state field types and default values,
    /// and type-check each handler body.
    fn check_actor_decl(
        &mut self,
        _name: &str,
        state_fields: &[crate::ast::item::StateField],
        handlers: &[crate::ast::item::MessageHandler],
    ) {
        // Check state fields: validate types and default value types.
        for sf in state_fields {
            let expected_ty = self.resolve_type_expr(&sf.type_ann.node, sf.type_ann.span);
            let actual_ty = self.check_expr(&sf.default_value);

            if !actual_ty.is_error() && !expected_ty.is_error() && actual_ty != expected_ty {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "state field `{}`: default value has type `{}`, expected `{}`",
                        sf.name, actual_ty, expected_ty
                    ),
                    sf.default_value.span,
                    expected_ty,
                    actual_ty,
                ));
            }
        }

        // Check handler bodies.
        for handler in handlers {
            let ret_ty = handler.return_type.as_ref()
                .map(|t| self.resolve_type_expr(&t.node, t.span))
                .unwrap_or(Ty::Unit);

            // Set up the handler context: state fields are in scope as
            // mutable variables, and the return type is set.
            self.env.push_scope();
            self.env.set_current_fn_return(ret_ty.clone());
            self.env.set_current_effects(vec!["Actor".to_string()]);

            // Bind state fields in the handler scope.
            for sf in state_fields {
                let ty = self.resolve_type_expr(&sf.type_ann.node, sf.type_ann.span);
                self.env.define_mutable(sf.name.clone(), ty);
            }

            let body_ty = self.check_block(&handler.body);

            // Validate that the body returns the correct type. Skip when
            // body_ty is Unit (a `ret` statement handles the validation
            // separately via current_fn_return).
            if ret_ty != Ty::Unit
                && !body_ty.is_error()
                && !ret_ty.is_error()
                && body_ty != ret_ty
                && body_ty != Ty::Unit
            {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "handler `{}` body returns `{}`, expected `{}`",
                        handler.message_name, body_ty, ret_ty
                    ),
                    handler.body.span,
                    ret_ty,
                    body_ty,
                ));
            }

            self.env.clear_current_fn_return();
            self.env.clear_current_effects();
            self.env.pop_scope();
        }
    }

    // ------------------------------------------------------------------
    // Trait impl checking
    // ------------------------------------------------------------------

    /// Check an impl block: validate that all required trait methods are
    /// implemented with matching signatures.
    fn check_impl_block(
        &mut self,
        trait_name: &str,
        target_type: &str,
        methods: &[FnDef],
    ) {
        // Look up the trait.
        let trait_info = match self.env.lookup_trait(trait_name) {
            Some(info) => info.clone(),
            None => {
                if let Some(first_method) = methods.first() {
                    self.errors.push(TypeError::new(
                        format!("unknown trait `{}`", trait_name),
                        first_method.body.span,
                    ));
                }
                return;
            }
        };

        // Resolve the target type for Self substitution.
        let self_ty = self.resolve_type_name(target_type);

        // Check that all trait methods are implemented.
        for trait_method in &trait_info.methods {
            let impl_method = methods.iter().find(|m| m.name == trait_method.name);
            match impl_method {
                None => {
                    if let Some(first_method) = methods.first() {
                        self.errors.push(TypeError::new(
                            format!(
                                "missing method `{}` required by trait `{}`",
                                trait_method.name, trait_name
                            ),
                            first_method.body.span,
                        ));
                    }
                }
                Some(impl_fn) => {
                    // Check that the parameter count matches (excluding self).
                    let impl_non_self_params: Vec<_> = impl_fn.params.iter()
                        .filter(|p| p.name != "self")
                        .collect();
                    if impl_non_self_params.len() != trait_method.params.len() {
                        self.errors.push(TypeError::new(
                            format!(
                                "method `{}` in impl for `{}` has {} parameter(s) (excluding self), expected {}",
                                trait_method.name, trait_name,
                                impl_non_self_params.len(), trait_method.params.len()
                            ),
                            impl_fn.body.span,
                        ));
                    } else {
                        // Check parameter types match (substituting Self -> target_type).
                        for (impl_p, (_, trait_ty)) in impl_non_self_params.iter().zip(trait_method.params.iter()) {
                            let impl_ty = self.resolve_type_expr(&impl_p.type_ann.node, impl_p.type_ann.span);
                            let expected_ty = Self::substitute_self(trait_ty, &self_ty);
                            if !impl_ty.is_error() && !expected_ty.is_error() && impl_ty != expected_ty {
                                self.errors.push(TypeError::mismatch(
                                    format!(
                                        "parameter `{}` in method `{}` has type `{}`, expected `{}`",
                                        impl_p.name, trait_method.name, impl_ty, expected_ty
                                    ),
                                    impl_p.span,
                                    expected_ty,
                                    impl_ty,
                                ));
                            }
                        }
                    }

                    // Check return type matches (substituting Self -> target_type).
                    let impl_ret = impl_fn.return_type.as_ref()
                        .map(|t| self.resolve_type_expr(&t.node, t.span))
                        .unwrap_or(Ty::Unit);
                    let expected_ret = Self::substitute_self(&trait_method.ret, &self_ty);
                    if !impl_ret.is_error() && !expected_ret.is_error() && impl_ret != expected_ret {
                        self.errors.push(TypeError::mismatch(
                            format!(
                                "method `{}` in impl for `{}` returns `{}`, expected `{}`",
                                trait_method.name, trait_name, impl_ret, expected_ret
                            ),
                            impl_fn.body.span,
                            expected_ret,
                            impl_ret,
                        ));
                    }
                }
            }
        }

        // Check for extra methods not in the trait.
        for impl_fn in methods {
            if !trait_info.methods.iter().any(|m| m.name == impl_fn.name) {
                self.errors.push(TypeError::new(
                    format!(
                        "method `{}` is not defined in trait `{}`",
                        impl_fn.name, trait_name
                    ),
                    impl_fn.body.span,
                ));
            }
        }

        // Type-check each method body.
        for method in methods {
            self.env.push_scope();

            // Bind `self` in the method scope with the target type.
            self.env.define("self".to_string(), self_ty.clone());

            // Set up type params, return type, and effects.
            let tp_names: Vec<String> = method.type_params.iter().map(|tp| tp.name.clone()).collect();
            let saved_type_params = std::mem::replace(&mut self.active_type_params, tp_names);

            let ret_ty = method.return_type.as_ref()
                .map(|t| self.resolve_type_expr(&t.node, t.span))
                .unwrap_or(Ty::Unit);
            self.env.set_current_fn_return(ret_ty);

            let effects: Vec<String> = method.effects
                .as_ref()
                .map(|e| e.effects.clone())
                .unwrap_or_default();
            self.env.set_current_effects(effects);

            // Bind non-self parameters.
            for param in &method.params {
                if param.name != "self" {
                    let ty = self.resolve_type_expr(&param.type_ann.node, param.type_ann.span);
                    self.env.define(param.name.clone(), ty);
                }
            }

            self.check_block(&method.body);

            self.env.clear_current_fn_return();
            self.env.clear_current_effects();
            self.env.pop_scope();
            self.active_type_params = saved_type_params;
        }
    }

    /// Substitute `Ty::TypeVar("Self")` with the concrete type in a Ty.
    fn substitute_self(ty: &Ty, self_ty: &Ty) -> Ty {
        match ty {
            Ty::TypeVar(name) if name == "Self" => self_ty.clone(),
            _ => ty.clone(),
        }
    }

    /// Resolve a type name string to a Ty. Used for resolving target_type in impls.
    fn resolve_type_name(&mut self, name: &str) -> Ty {
        match name {
            "Int" => Ty::Int,
            "Float" => Ty::Float,
            "String" => Ty::String,
            "Bool" => Ty::Bool,
            _ => {
                if let Some(ty) = self.env.lookup_type_alias(name) {
                    return ty.clone();
                }
                if let Some(ty) = self.env.lookup_enum(name) {
                    return ty.clone();
                }
                Ty::TypeVar(name.to_string())
            }
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
                mutable,
            } => {
                self.check_let(name, type_ann.as_ref(), value, stmt.span, *mutable);
                Ty::Unit
            }
            StmtKind::LetTupleDestructure {
                names,
                type_ann,
                value,
            } => {
                self.check_let_tuple_destructure(names, type_ann.as_ref(), value, stmt.span);
                Ty::Unit
            }
            StmtKind::Assign { name, value } => {
                self.check_assign(name, value, stmt.span);
                Ty::Unit
            }
            StmtKind::Ret(expr) => {
                let ty = self.check_expr(expr);
                if let Some(expected) = self.env.current_fn_return() {
                    let expected = expected.clone();
                    if !ty.is_error()
                        && !expected.is_error()
                        && !expected.is_type_var()
                        && !ty.is_type_var()
                        && ty != expected
                    {
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
        mutable: bool,
    ) {
        let value_ty = self.check_expr(value);

        if let Some(ann) = type_ann {
            let ann_ty = self.resolve_type_expr(&ann.node, ann.span);
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
            if mutable {
                self.env.define_mutable(name.to_string(), ann_ty);
            } else {
                self.env.define(name.to_string(), ann_ty);
            }
        } else {
            // Infer from the value.
            if mutable {
                self.env.define_mutable(name.to_string(), value_ty);
            } else {
                self.env.define(name.to_string(), value_ty);
            }
        }
    }

    /// Check a `let` tuple destructuring: verify the RHS has a matching
    /// `Ty::Tuple` and bind each name to its corresponding element type.
    fn check_let_tuple_destructure(
        &mut self,
        names: &[String],
        type_ann: Option<&crate::ast::span::Spanned<TypeExpr>>,
        value: &Expr,
        span: Span,
    ) {
        let value_ty = self.check_expr(value);

        // If there's a type annotation, resolve it and check against the value type.
        let tuple_ty = if let Some(ann) = type_ann {
            let ann_ty = self.resolve_type_expr(&ann.node, ann.span);
            if !value_ty.is_error() && !ann_ty.is_error() && value_ty != ann_ty {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "type mismatch in tuple destructuring: declared `{}`, but value has type `{}`",
                        ann_ty, value_ty
                    ),
                    span,
                    ann_ty.clone(),
                    value_ty.clone(),
                ));
            }
            ann_ty
        } else {
            value_ty
        };

        match &tuple_ty {
            Ty::Tuple(elems) => {
                if names.len() != elems.len() {
                    self.errors.push(TypeError::new(
                        format!(
                            "tuple destructuring has {} names but the tuple has {} elements",
                            names.len(),
                            elems.len()
                        ),
                        span,
                    ));
                    // Bind all names as Error to avoid cascading errors.
                    for name in names {
                        self.env.define(name.clone(), Ty::Error);
                    }
                } else {
                    for (name, ty) in names.iter().zip(elems.iter()) {
                        self.env.define(name.clone(), ty.clone());
                    }
                }
            }
            Ty::Error => {
                // Silently bind all names as Error.
                for name in names {
                    self.env.define(name.clone(), Ty::Error);
                }
            }
            _ => {
                self.errors.push(TypeError::new(
                    format!(
                        "cannot destructure non-tuple type `{}` in let binding",
                        tuple_ty
                    ),
                    span,
                ));
                for name in names {
                    self.env.define(name.clone(), Ty::Error);
                }
            }
        }
    }

    /// Check an assignment statement: look up the variable, check it's mutable,
    /// and verify the type matches.
    fn check_assign(&mut self, name: &str, value: &Expr, span: Span) {
        let value_ty = self.check_expr(value);

        // Check the variable exists.
        let var_ty = match self.env.lookup(name) {
            Some(ty) => ty.clone(),
            None => {
                self.errors.push(TypeError::new(
                    format!("undefined variable `{}`", name),
                    span,
                ));
                return;
            }
        };

        // Check it's mutable.
        if !self.env.is_mutable(name) {
            self.errors.push(TypeError::new(
                format!("cannot assign to immutable variable `{}`", name),
                span,
            ));
            return;
        }

        // Check type compatibility.
        if !value_ty.is_error() && !var_ty.is_error() && value_ty != var_ty {
            self.errors.push(TypeError::mismatch(
                format!(
                    "type mismatch in assignment to `{}`: expected `{}`, found `{}`",
                    name, var_ty, value_ty
                ),
                span,
                var_ty,
                value_ty,
            ));
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

                // Collect context: expected type (from function return) and
                // in-scope bindings that match, to help agents fill the hole.
                let expected_ty = self.env.current_fn_return().cloned();

                let mut error = TypeError::new(
                    format!("typed hole `{}` found", label_str),
                    expr.span,
                );

                if let Some(ref expected) = expected_ty {
                    error = error.with_note(format!("expected type: {}", expected));

                    // Collect all in-scope bindings whose type matches the expected type.
                    let all_bindings = self.env.all_bindings();
                    let matching: Vec<String> = all_bindings
                        .iter()
                        .filter(|(_, ty, _)| ty == expected)
                        .map(|(name, ty, _)| format!("`{}` ({})", name, ty))
                        .collect();

                    // Also check functions that return the expected type.
                    let matching_fns: Vec<String> = self.env.all_functions()
                        .iter()
                        .filter(|(_, sig)| sig.ret == *expected)
                        .map(|(name, sig)| {
                            let params = sig.params
                                .iter()
                                .map(|(pname, pty)| format!("{}: {}", pname, pty))
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!("`{}({})` -> {}", name, params, sig.ret)
                        })
                        .collect();

                    if !matching.is_empty() {
                        error = error.with_note(format!(
                            "matching bindings in scope: {}",
                            matching.join(", ")
                        ));
                    }
                    if !matching_fns.is_empty() {
                        error = error.with_note(format!(
                            "matching functions: {}",
                            matching_fns.join(", ")
                        ));
                    }
                    if matching.is_empty() && matching_fns.is_empty() {
                        error = error.with_note(
                            "no bindings or functions in scope match the expected type".to_string(),
                        );
                    }
                } else {
                    error = error.with_note(
                        "fill in the hole with a concrete expression".to_string(),
                    );
                }

                self.errors.push(error);
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
                // Check if this is a qualified module reference (e.g., `math.add`).
                if let ExprKind::Ident(module_name) = &object.node {
                    if self.env.is_imported_module(module_name) {
                        // Resolve as a qualified function reference.
                        if let Some(sig) = self.env.lookup_qualified_fn(module_name, field) {
                            return Ty::Fn {
                                params: sig.params.iter().map(|(_, t)| t.clone()).collect(),
                                ret: Box::new(sig.ret.clone()),
                                effects: sig.effects.clone(),
                            };
                        } else {
                            self.errors.push(TypeError::new(
                                format!(
                                    "module `{}` has no function `{}`",
                                    module_name, field
                                ),
                                expr.span,
                            ));
                            return Ty::Error;
                        }
                    }
                }

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

            ExprKind::While { condition, body } => {
                // Check the condition is Bool.
                let cond_ty = self.check_expr(condition);
                if !cond_ty.is_error() && cond_ty != Ty::Bool {
                    self.errors.push(TypeError::mismatch(
                        format!("`while` condition must be Bool, found `{}`", cond_ty),
                        condition.span,
                        Ty::Bool,
                        cond_ty,
                    ));
                }
                let _body_ty = self.check_block(body);
                Ty::Unit
            }

            ExprKind::Match { scrutinee, arms } => {
                self.check_match(scrutinee, arms, expr.span)
            }

            ExprKind::Paren(inner) => self.check_expr(inner),

            ExprKind::Tuple(elems) => {
                let elem_types: Vec<Ty> = elems.iter().map(|e| self.check_expr(e)).collect();
                Ty::Tuple(elem_types)
            }

            ExprKind::TupleField { tuple, index } => {
                let tuple_ty = self.check_expr(tuple);
                match &tuple_ty {
                    Ty::Tuple(elems) => {
                        if *index < elems.len() {
                            elems[*index].clone()
                        } else {
                            self.errors.push(TypeError::new(
                                format!(
                                    "tuple index `{}` out of bounds for tuple of {} elements",
                                    index,
                                    elems.len()
                                ),
                                expr.span,
                            ));
                            Ty::Error
                        }
                    }
                    Ty::Error => Ty::Error,
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "tuple field access `.{}` on non-tuple type `{}`",
                                index, tuple_ty
                            ),
                            expr.span,
                        ));
                        Ty::Error
                    }
                }
            }

            ExprKind::Spawn { actor_name } => {
                // Validate the actor type exists.
                if self.env.lookup_actor(actor_name).is_none() {
                    self.errors.push(TypeError::new(
                        format!("unknown actor type `{}`", actor_name),
                        expr.span,
                    ));
                    return Ty::Error;
                }
                // `spawn` requires the Actor effect.
                if !self.env.current_effects().contains(&"Actor".to_string()) {
                    self.current_inferred.push("Actor".to_string());
                    self.errors.push(
                        TypeError::new(
                            format!(
                                "`spawn {}` requires effect `Actor`, but the current function does not declare it",
                                actor_name
                            ),
                            expr.span,
                        )
                        .with_note("add `!{{Actor}}` to the function's effect annotation".to_string()),
                    );
                } else {
                    self.current_inferred.push("Actor".to_string());
                }
                Ty::Actor { name: actor_name.clone() }
            }

            ExprKind::Send { target, message } => {
                let target_ty = self.check_expr(target);

                match &target_ty {
                    Ty::Actor { name: actor_name } => {
                        // Validate the message is handled by this actor.
                        let actor_name = actor_name.clone();
                        let valid = self.env.lookup_actor(&actor_name)
                            .map(|info| info.handlers.iter().any(|(m, _)| m == message))
                            .unwrap_or(false);
                        if !valid {
                            self.errors.push(TypeError::new(
                                format!(
                                    "actor `{}` does not handle message `{}`",
                                    actor_name, message
                                ),
                                expr.span,
                            ));
                        }
                    }
                    Ty::Error => { /* propagate silently */ }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "`send` target must be an actor handle, found `{}`",
                                target_ty
                            ),
                            target.span,
                        ));
                    }
                }

                // `send` requires the Actor effect.
                if !self.env.current_effects().contains(&"Actor".to_string()) {
                    self.current_inferred.push("Actor".to_string());
                    self.errors.push(
                        TypeError::new(
                            "`send` requires effect `Actor`",
                            expr.span,
                        )
                        .with_note("add `!{{Actor}}` to the function's effect annotation".to_string()),
                    );
                } else {
                    self.current_inferred.push("Actor".to_string());
                }

                Ty::Unit
            }

            ExprKind::Ask { target, message } => {
                let target_ty = self.check_expr(target);

                let ret_ty = match &target_ty {
                    Ty::Actor { name: actor_name } => {
                        let actor_name = actor_name.clone();
                        let handler_ret = self.env.lookup_actor(&actor_name)
                            .and_then(|info| {
                                info.handlers.iter()
                                    .find(|(m, _)| m == message)
                                    .map(|(_, ret)| ret.clone())
                            });
                        match handler_ret {
                            Some(ret) => ret,
                            None => {
                                self.errors.push(TypeError::new(
                                    format!(
                                        "actor `{}` does not handle message `{}`",
                                        actor_name, message
                                    ),
                                    expr.span,
                                ));
                                Ty::Error
                            }
                        }
                    }
                    Ty::Error => Ty::Error,
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "`ask` target must be an actor handle, found `{}`",
                                target_ty
                            ),
                            target.span,
                        ));
                        Ty::Error
                    }
                };

                // `ask` requires the Actor effect.
                if !self.env.current_effects().contains(&"Actor".to_string()) {
                    self.current_inferred.push("Actor".to_string());
                    self.errors.push(
                        TypeError::new(
                            "`ask` requires effect `Actor`",
                            expr.span,
                        )
                        .with_note("add `!{{Actor}}` to the function's effect annotation".to_string()),
                    );
                } else {
                    self.current_inferred.push("Actor".to_string());
                }

                ret_ty
            }

            ExprKind::Closure { params, return_type, body } => {
                self.check_closure(params, return_type.as_ref(), body, expr.span)
            }

            ExprKind::Try(inner) => {
                let inner_ty = self.check_expr(inner);

                // The inner expression must be a Result[T, E] (an enum named "Result").
                match &inner_ty {
                    Ty::Enum { name, variants } if name == "Result" => {
                        // Extract the T from Ok(T) and E from Err(E).
                        let ok_ty = variants.iter()
                            .find(|(vn, _)| vn == "Ok")
                            .and_then(|(_, t)| t.clone());
                        let _err_ty = variants.iter()
                            .find(|(vn, _)| vn == "Err")
                            .and_then(|(_, t)| t.clone());

                        // Verify the enclosing function also returns a Result type.
                        if let Some(ret_ty) = self.env.current_fn_return().cloned() {
                            match &ret_ty {
                                Ty::Enum { name: ret_name, .. } if ret_name == "Result" => {
                                    // Valid: enclosing function returns Result.
                                }
                                _ => {
                                    self.errors.push(TypeError::new(
                                        format!(
                                            "`?` operator requires the enclosing function to return `Result`, but it returns `{}`",
                                            ret_ty
                                        ),
                                        expr.span,
                                    ));
                                }
                            }
                        } else {
                            self.errors.push(TypeError::new(
                                "`?` operator can only be used inside a function that returns `Result`".to_string(),
                                expr.span,
                            ));
                        }

                        // The type of `expr?` is T (the success type).
                        ok_ty.unwrap_or(Ty::Unit)
                    }
                    Ty::Error => Ty::Error,
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "`?` operator can only be applied to `Result` type, found `{}`",
                                inner_ty
                            ),
                            expr.span,
                        ));
                        Ty::Error
                    }
                }
            }
        }
    }

    /// Type-check a closure expression.
    ///
    /// Pushes a new scope, binds parameters, checks the body, and returns
    /// a `Ty::Fn` representing the closure's type.
    fn check_closure(
        &mut self,
        params: &[ClosureParam],
        return_type: Option<&Spanned<TypeExpr>>,
        body: &Expr,
        _span: Span,
    ) -> Ty {
        self.env.push_scope();

        // Bind each parameter in the new scope.
        let mut param_tys = Vec::new();
        for param in params {
            let ty = if let Some(ref type_ann) = param.type_ann {
                self.resolve_type_expr(&type_ann.node, type_ann.span)
            } else {
                // No type annotation -- for now, default to Int.
                // Full type inference for closure params is future work.
                Ty::Int
            };
            self.env.define(param.name.clone(), ty.clone());
            param_tys.push(ty);
        }

        // Check the body expression.
        let body_ty = self.check_expr(body);

        // If there's a return type annotation, check that the body type matches.
        let ret_ty = if let Some(ret_ann) = return_type {
            let declared_ret = self.resolve_type_expr(&ret_ann.node, ret_ann.span);
            if !body_ty.is_error() && !declared_ret.is_error() && body_ty != declared_ret {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "closure body type `{}` does not match declared return type `{}`",
                        body_ty, declared_ret
                    ),
                    ret_ann.span,
                    declared_ret.clone(),
                    body_ty,
                ));
            }
            declared_ret
        } else {
            body_ty
        };

        self.env.pop_scope();

        Ty::Fn {
            params: param_tys,
            ret: Box::new(ret_ty),
            effects: vec![],
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

    // ------------------------------------------------------------------
    // Effect polymorphism helpers
    // ------------------------------------------------------------------

    /// Check if a function signature has any effect variables.
    fn sig_has_effect_variables(sig: &FnSig) -> bool {
        for eff in &sig.effects {
            if effects::is_effect_variable(eff) {
                return true;
            }
        }
        for (_, ty) in &sig.params {
            if Self::ty_has_effect_variables(ty) {
                return true;
            }
        }
        false
    }

    /// Check if a Ty contains any effect variables.
    fn ty_has_effect_variables(ty: &Ty) -> bool {
        if let Ty::Fn { effects, .. } = ty {
            for eff in effects {
                if effects::is_effect_variable(eff) {
                    return true;
                }
            }
        }
        false
    }

    /// Match an argument type against a parameter type with effect variables.
    fn match_type_with_effect_vars(
        param_ty: &Ty,
        arg_ty: &Ty,
        effect_bindings: &mut std::collections::HashMap<String, Vec<String>>,
    ) -> bool {
        match (param_ty, arg_ty) {
            (
                Ty::Fn { params: p_params, ret: p_ret, effects: p_effects },
                Ty::Fn { params: a_params, ret: a_ret, effects: a_effects },
            ) => {
                if p_params.len() != a_params.len() { return false; }
                for (pp, ap) in p_params.iter().zip(a_params.iter()) {
                    if pp != ap && !pp.is_error() && !ap.is_error() { return false; }
                }
                if **p_ret != **a_ret && !p_ret.is_error() && !a_ret.is_error() { return false; }
                let p_vars: Vec<&String> = p_effects.iter().filter(|e| effects::is_effect_variable(e)).collect();
                let p_concrete: Vec<&String> = p_effects.iter().filter(|e| !effects::is_effect_variable(e)).collect();
                if p_vars.is_empty() { return p_effects == a_effects; }
                let remaining: Vec<String> = a_effects.iter().filter(|e| !p_concrete.contains(e)).cloned().collect();
                for var in &p_vars {
                    if let Some(prev) = effect_bindings.get(var.as_str()) {
                        if *prev != remaining { return false; }
                    } else {
                        effect_bindings.insert((*var).clone(), remaining.clone());
                    }
                }
                true
            }
            _ => param_ty == arg_ty || param_ty.is_error() || arg_ty.is_error(),
        }
    }

    /// Resolve effect variables using bindings.
    fn resolve_call_effects(
        sig_effects: &[String],
        effect_bindings: &std::collections::HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        let mut resolved = Vec::new();
        for eff in sig_effects {
            if effects::is_effect_variable(eff) {
                if let Some(bound) = effect_bindings.get(eff) {
                    for concrete in bound {
                        if !resolved.contains(concrete) { resolved.push(concrete.clone()); }
                    }
                }
            } else if !resolved.contains(eff) {
                resolved.push(eff.clone());
            }
        }
        resolved
    }

    /// Type-check a function call expression.
    fn check_call(
        &mut self,
        func: &Expr,
        args: &[Expr],
        span: Span,
    ) -> Ty {
        // Check for qualified function calls: `module.func(args)`.
        // The parser produces FieldAccess { object: Ident("module"), field: "func" }.
        if let ExprKind::FieldAccess { object, field } = &func.node {
            if let ExprKind::Ident(module_name) = &object.node {
                if self.env.is_imported_module(module_name) {
                    let sig = self
                        .env
                        .lookup_qualified_fn(module_name, field)
                        .cloned();
                    if let Some(sig) = sig {
                        let qualified_name = format!("{}.{}", module_name, field);
                        return self.check_call_with_sig(&qualified_name, &sig, args, span);
                    } else {
                        self.errors.push(TypeError::new(
                            format!(
                                "module `{}` has no function `{}`",
                                module_name, field
                            ),
                            func.span,
                        ));
                        // Still check args.
                        for arg in args {
                            let _ = self.check_expr(arg);
                        }
                        return Ty::Error;
                    }
                }
            }
        }

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
            // If the function is generic, handle it with type inference.
            if !sig.type_params.is_empty() {
                return self.check_generic_call(
                    func_name.as_deref().unwrap_or("<unknown>"),
                    &sig,
                    args,
                    span,
                );
            }

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

            // Determine if this call involves effect polymorphism.
            let has_effect_vars = Self::sig_has_effect_variables(&sig);
            let mut effect_bindings: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();

            // Check each argument type.
            for (i, (arg, (param_name, param_ty))) in
                args.iter().zip(sig.params.iter()).enumerate()
            {
                let arg_ty = self.check_expr(arg);
                if arg_ty.is_error() || param_ty.is_error() {
                    continue;
                }
                if has_effect_vars && Self::ty_has_effect_variables(param_ty) {
                    // Effect-polymorphic parameter: use effect-aware matching.
                    if !Self::match_type_with_effect_vars(param_ty, &arg_ty, &mut effect_bindings) {
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
                } else if arg_ty != *param_ty && !param_ty.is_type_var() {
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

            // Resolve the actual effects for this call.
            let resolved_effects = if has_effect_vars {
                Self::resolve_call_effects(&sig.effects, &effect_bindings)
            } else {
                sig.effects.clone()
            };

            // Effect checking: verify resolved effects are available in context.
            if !resolved_effects.is_empty() {
                for effect in &resolved_effects {
                    if !self.current_inferred.contains(effect) {
                        self.current_inferred.push(effect.clone());
                    }
                }

                let current = self.env.current_effects().to_vec();
                for effect in &resolved_effects {
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

            // Budget containment: if the caller has a budget, check that
            // the callee's budget fits within it.
            if let Some(ref caller_name) = self.current_fn_name {
                if let Some(ref callee_name) = func_name {
                    if self.function_budgets.contains_key(caller_name.as_str()) {
                        let cn = caller_name.clone();
                        let ce = callee_name.clone();
                        self.check_budget_containment(&cn, &ce, span);
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
                    for effect in effects {
                        if !self.current_inferred.contains(effect) {
                            self.current_inferred.push(effect.clone());
                        }
                    }

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

    /// Type-check a `match` expression.
    fn check_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        span: Span,
    ) -> Ty {
        let scrutinee_ty = self.check_expr(scrutinee);

        if arms.is_empty() {
            self.errors.push(TypeError::new(
                "match expression has no arms".to_string(),
                span,
            ));
            return Ty::Error;
        }

        let mut has_wildcard = false;
        let mut first_arm_ty: Option<Ty> = None;
        let mut matched_variants: Vec<String> = Vec::new();

        for arm in arms {
            // Check pattern compatibility with scrutinee type.
            if !scrutinee_ty.is_error() {
                match &arm.pattern {
                    Pattern::IntLit(_) => {
                        if scrutinee_ty != Ty::Int {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "integer pattern cannot match scrutinee of type `{}`",
                                    scrutinee_ty
                                ),
                                arm.span,
                                Ty::Int,
                                scrutinee_ty.clone(),
                            ));
                        }
                    }
                    Pattern::BoolLit(_) => {
                        if scrutinee_ty != Ty::Bool {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "boolean pattern cannot match scrutinee of type `{}`",
                                    scrutinee_ty
                                ),
                                arm.span,
                                Ty::Bool,
                                scrutinee_ty.clone(),
                            ));
                        }
                    }
                    Pattern::Wildcard => {
                        has_wildcard = true;
                    }
                    Pattern::Tuple(_) => {
                        // Tuple patterns are not supported in match arms (only in let bindings).
                        self.errors.push(TypeError::new(
                            "tuple patterns are only supported in `let` destructuring, not in `match` arms".to_string(),
                            arm.span,
                        ));
                    }
                    Pattern::Variant { variant, binding } => {
                        matched_variants.push(variant.clone());

                        // Check that the variant belongs to the scrutinee's enum type.
                        if let Ty::Enum { name: enum_name, variants } = &scrutinee_ty {
                            if let Some((_, field_ty)) = variants.iter().find(|(vn, _)| vn == variant) {
                                // If there's a binding, push a scope with the binding.
                                if let Some(bname) = binding {
                                    if let Some(fty) = field_ty {
                                        self.env.push_scope();
                                        self.env.define(bname.clone(), fty.clone());
                                    } else {
                                        self.errors.push(TypeError::new(
                                            format!(
                                                "variant `{}` of `{}` is a unit variant and cannot have a binding",
                                                variant, enum_name
                                            ),
                                            arm.span,
                                        ));
                                    }
                                }
                            } else {
                                self.errors.push(TypeError::new(
                                    format!(
                                        "variant `{}` is not a member of enum `{}`",
                                        variant, enum_name
                                    ),
                                    arm.span,
                                ));
                            }
                        } else {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "variant pattern `{}` cannot match scrutinee of type `{}`",
                                    variant, scrutinee_ty
                                ),
                                arm.span,
                                Ty::Error,
                                scrutinee_ty.clone(),
                            ));
                        }
                    }
                }
            }

            // Check the arm body.
            let arm_ty = self.check_block(&arm.body);

            // Pop scope for variant bindings (if we pushed one).
            if let Pattern::Variant { binding: Some(_), .. } = &arm.pattern {
                if let Ty::Enum { variants, .. } = &scrutinee_ty {
                    if let Pattern::Variant { variant, .. } = &arm.pattern {
                        if let Some((_, Some(_))) = variants.iter().find(|(vn, _)| vn == variant) {
                            self.env.pop_scope();
                        }
                    }
                }
            }

            // Compare with first arm's type.
            if let Some(ref expected) = first_arm_ty {
                if !arm_ty.is_error()
                    && !expected.is_error()
                    && arm_ty != *expected
                    // Allow Unit mismatch when arms use `ret`
                    && arm_ty != Ty::Unit
                    && *expected != Ty::Unit
                {
                    self.errors.push(TypeError::mismatch(
                        "all arms of `match` expression must have the same type".to_string(),
                        arm.body.span,
                        expected.clone(),
                        arm_ty,
                    ));
                }
            } else {
                first_arm_ty = Some(arm_ty);
            }
        }

        // Exhaustiveness checking.
        if !has_wildcard && !scrutinee_ty.is_error() {
            if let Ty::Enum { name: enum_name, variants } = &scrutinee_ty {
                // Check all variants are covered.
                let missing: Vec<&str> = variants
                    .iter()
                    .filter(|(vn, _)| !matched_variants.contains(vn))
                    .map(|(vn, _)| vn.as_str())
                    .collect();
                if !missing.is_empty() {
                    self.errors.push(
                        TypeError::new(
                            format!(
                                "non-exhaustive match on `{}`: missing variant(s): {}",
                                enum_name,
                                missing.join(", ")
                            ),
                            span,
                        )
                        .with_note("add the missing variant arms or a wildcard `_` arm".to_string()),
                    );
                }
            } else {
                self.errors.push(
                    TypeError::new(
                        "non-exhaustive match: consider adding a wildcard `_` arm".to_string(),
                        span,
                    )
                    .with_note("a `_` arm ensures all values are handled".to_string()),
                );
            }
        }

        first_arm_ty.unwrap_or(Ty::Error)
    }

    // ------------------------------------------------------------------
    // Generic function call inference
    // ------------------------------------------------------------------

    /// Type-check a call to a generic function.
    ///
    /// Infers type variable bindings from argument types, substitutes them
    /// into the return type and parameter types, and then checks that all
    /// arguments match the specialized signature.
    fn check_generic_call(
        &mut self,
        display_name: &str,
        sig: &FnSig,
        args: &[Expr],
        span: Span,
    ) -> Ty {
        // Check argument count first.
        if args.len() != sig.params.len() {
            self.errors.push(TypeError::new(
                format!(
                    "function `{}` expects {} argument(s), but {} were provided",
                    display_name,
                    sig.params.len(),
                    args.len()
                ),
                span,
            ));
            return Ty::Error;
        }

        // Evaluate all argument types.
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.check_expr(a)).collect();

        // Build type variable bindings by unifying param types with arg types.
        let mut bindings: std::collections::HashMap<String, Ty> =
            std::collections::HashMap::new();

        for ((_, param_ty), arg_ty) in sig.params.iter().zip(arg_tys.iter()) {
            if arg_ty.is_error() {
                continue;
            }
            Self::unify_types(param_ty, arg_ty, &mut bindings);
        }

        // Check that all type parameters got bound.
        for tp in &sig.type_params {
            if !bindings.contains_key(tp) {
                self.errors.push(TypeError::new(
                    format!(
                        "could not infer type parameter `{}` for function `{}`",
                        tp, display_name
                    ),
                    span,
                ));
                return Ty::Error;
            }
        }

        // Verify consistency: check that each arg type matches the
        // substituted param type.
        for (i, ((param_name, param_ty), (arg, arg_ty))) in sig
            .params
            .iter()
            .zip(args.iter().zip(arg_tys.iter()))
            .enumerate()
        {
            if arg_ty.is_error() {
                continue;
            }
            let specialized = Self::substitute_ty(param_ty, &bindings);
            if !specialized.is_error() && *arg_ty != specialized {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "argument {} (`{}`) of `{}`: expected `{}`, found `{}`",
                        i + 1,
                        param_name,
                        display_name,
                        specialized,
                        arg_ty
                    ),
                    arg.span,
                    specialized,
                    arg_ty.clone(),
                ));
            }
        }

        // Effect checking (same as non-generic calls).
        if !sig.effects.is_empty() {
            for effect in &sig.effects {
                if !self.current_inferred.contains(effect) {
                    self.current_inferred.push(effect.clone());
                }
            }
            let current = self.env.current_effects().to_vec();
            for effect in &sig.effects {
                if !current.contains(effect) {
                    self.errors.push(
                        TypeError::new(
                            format!(
                                "function `{}` requires effect `{}`, but the current context does not provide it",
                                display_name, effect
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

        // Substitute the return type.
        Self::substitute_ty(&sig.ret, &bindings)
    }

    /// Attempt to unify a parameter type (which may contain TypeVar) with a
    /// concrete argument type. Builds up bindings: TypeVar name -> concrete Ty.
    fn unify_types(
        param_ty: &Ty,
        arg_ty: &Ty,
        bindings: &mut std::collections::HashMap<String, Ty>,
    ) {
        match param_ty {
            Ty::TypeVar(name) => {
                if let Some(existing) = bindings.get(name) {
                    // Already bound — check consistency (we don't error here,
                    // the caller will detect mismatches during verification).
                    let _ = existing;
                } else {
                    bindings.insert(name.clone(), arg_ty.clone());
                }
            }
            Ty::Fn {
                params: p_params,
                ret: p_ret,
                ..
            } => {
                if let Ty::Fn {
                    params: a_params,
                    ret: a_ret,
                    ..
                } = arg_ty
                {
                    for (pp, ap) in p_params.iter().zip(a_params.iter()) {
                        Self::unify_types(pp, ap, bindings);
                    }
                    Self::unify_types(p_ret, a_ret, bindings);
                }
            }
            _ => {
                // Concrete types: no unification needed.
            }
        }
    }

    /// Substitute all TypeVar occurrences in a type using the given bindings.
    fn substitute_ty(
        ty: &Ty,
        bindings: &std::collections::HashMap<String, Ty>,
    ) -> Ty {
        match ty {
            Ty::TypeVar(name) => {
                bindings.get(name).cloned().unwrap_or_else(|| ty.clone())
            }
            Ty::Fn {
                params,
                ret,
                effects,
            } => Ty::Fn {
                params: params
                    .iter()
                    .map(|p| Self::substitute_ty(p, bindings))
                    .collect(),
                ret: Box::new(Self::substitute_ty(ret, bindings)),
                effects: effects.clone(),
            },
            Ty::Enum { name, variants } => Ty::Enum {
                name: name.clone(),
                variants: variants
                    .iter()
                    .map(|(vn, vt)| {
                        (
                            vn.clone(),
                            vt.as_ref().map(|t| Self::substitute_ty(t, bindings)),
                        )
                    })
                    .collect(),
            },
            _ => ty.clone(),
        }
    }

    // ------------------------------------------------------------------
    // Qualified call helper
    // ------------------------------------------------------------------

    /// Type-check a function call against a known signature.
    ///
    /// This is used for both direct calls (`add(1, 2)`) and qualified calls
    /// (`math.add(1, 2)`) once the signature has been resolved.
    fn check_call_with_sig(
        &mut self,
        display_name: &str,
        sig: &FnSig,
        args: &[Expr],
        span: Span,
    ) -> Ty {
        // Check argument count.
        if args.len() != sig.params.len() {
            self.errors.push(TypeError::new(
                format!(
                    "function `{}` expects {} argument(s), but {} were provided",
                    display_name,
                    sig.params.len(),
                    args.len()
                ),
                span,
            ));
            return Ty::Error;
        }

        // Determine if this call involves effect polymorphism.
        let has_effect_vars = Self::sig_has_effect_variables(sig);
        let mut effect_bindings: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        // Check each argument type.
        for (i, (arg, (param_name, param_ty))) in
            args.iter().zip(sig.params.iter()).enumerate()
        {
            let arg_ty = self.check_expr(arg);
            if arg_ty.is_error() || param_ty.is_error() {
                continue;
            }
            if has_effect_vars && Self::ty_has_effect_variables(param_ty) {
                if !Self::match_type_with_effect_vars(param_ty, &arg_ty, &mut effect_bindings) {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "argument {} (`{}`) of `{}`: expected `{}`, found `{}`",
                            i + 1,
                            param_name,
                            display_name,
                            param_ty,
                            arg_ty
                        ),
                        arg.span,
                        param_ty.clone(),
                        arg_ty,
                    ));
                }
            } else if arg_ty != *param_ty && !param_ty.is_type_var() {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "argument {} (`{}`) of `{}`: expected `{}`, found `{}`",
                        i + 1,
                        param_name,
                        display_name,
                        param_ty,
                        arg_ty
                    ),
                    arg.span,
                    param_ty.clone(),
                    arg_ty,
                ));
            }
        }

        // Resolve the actual effects for this call.
        let resolved_effects = if has_effect_vars {
            Self::resolve_call_effects(&sig.effects, &effect_bindings)
        } else {
            sig.effects.clone()
        };

        // Effect checking.
        if !resolved_effects.is_empty() {
            for effect in &resolved_effects {
                if !self.current_inferred.contains(effect) {
                    self.current_inferred.push(effect.clone());
                }
            }

            let current = self.env.current_effects().to_vec();
            for effect in &resolved_effects {
                if !current.contains(effect) {
                    self.errors.push(
                        TypeError::new(
                            format!(
                                "function `{}` requires effect `{}`, but the current context does not provide it",
                                display_name, effect
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
    }

    // ------------------------------------------------------------------
    // Type resolution
    // ------------------------------------------------------------------

    /// Convert an AST [`TypeExpr`] to an internal [`Ty`].
    ///
    /// The `span` parameter is used to report an error if the type name is
    /// not recognised. For v0.1 only the built-in types (`Int`, `Float`,
    /// `String`, `Bool`, `()`) are valid.
    fn resolve_type_expr(&mut self, te: &TypeExpr, span: Span) -> Ty {
        match te {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" => Ty::Int,
                "Float" => Ty::Float,
                "String" => Ty::String,
                "Bool" => Ty::Bool,
                "Self" => Ty::TypeVar("Self".to_string()),
                _ => {
                    // Check if it's a type parameter currently in scope.
                    if self.active_type_params.contains(name) {
                        return Ty::TypeVar(name.clone());
                    }
                    // Check type aliases before reporting an error.
                    if let Some(ty) = self.env.lookup_type_alias(name) {
                        return ty.clone();
                    }
                    // Check enum types.
                    if let Some(ty) = self.env.lookup_enum(name) {
                        return ty.clone();
                    }
                    self.errors.push(TypeError {
                        message: format!("unknown type `{}`", name),
                        span,
                        expected: None,
                        found: None,
                        notes: vec!["available types: Int, Float, String, Bool, ()".to_string()],
                    });
                    Ty::Error
                }
            },
            TypeExpr::Unit => Ty::Unit,
            TypeExpr::Fn { params, ret, effects } => {
                let param_tys: Vec<Ty> = params
                    .iter()
                    .map(|p| self.resolve_type_expr(&p.node, p.span))
                    .collect();
                let ret_ty = self.resolve_type_expr(&ret.node, ret.span);
                let eff_list = effects
                    .as_ref()
                    .map(|e| e.effects.clone())
                    .unwrap_or_default();
                Ty::Fn {
                    params: param_tys,
                    ret: Box::new(ret_ty),
                    effects: eff_list,
                }
            }
            TypeExpr::Generic { name, args } => {
                // Handle Actor[Name] type annotations.
                if name == "Actor" {
                    if args.len() == 1 {
                        if let TypeExpr::Named(actor_name) = &args[0].node {
                            return Ty::Actor { name: actor_name.clone() };
                        }
                    }
                    self.errors.push(TypeError {
                        message: "Actor type requires exactly one type argument, e.g. Actor[Counter]".to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                    });
                    return Ty::Error;
                }

                // Resolve the base type (must be an enum with type_params).
                // For now, resolve the base as if non-generic and resolve args
                // for error checking; full parameterized enum instantiation is
                // future work.
                for a in args {
                    let _ = self.resolve_type_expr(&a.node, a.span);
                }
                // Check if the base is a known enum.
                if let Some(ty) = self.env.lookup_enum(name) {
                    return ty.clone();
                }
                self.errors.push(TypeError {
                    message: format!("unknown generic type `{}`", name),
                    span,
                    expected: None,
                    found: None,
                    notes: vec![],
                });
                Ty::Error
            }
            TypeExpr::Tuple(elems) => {
                let elem_tys: Vec<Ty> = elems
                    .iter()
                    .map(|e| self.resolve_type_expr(&e.node, e.span))
                    .collect();
                Ty::Tuple(elem_tys)
            }
        }
    }

    // ------------------------------------------------------------------
    // Budget validation
    // ------------------------------------------------------------------

    /// Validate a `@budget(...)` annotation: check that cpu/mem values have
    /// recognised unit suffixes and can be parsed as quantities.
    fn validate_budget(&mut self, budget: &BudgetConstraint, fn_name: &str) {
        if let Some(ref cpu) = budget.cpu {
            if Self::parse_cpu_millis(cpu).is_none() {
                self.errors.push(
                    TypeError::new(
                        format!(
                            "invalid cpu budget `{}` on function `{}`; expected a value like `5s` or `100ms`",
                            cpu, fn_name
                        ),
                        budget.span,
                    ),
                );
            }
        }
        if let Some(ref mem) = budget.mem {
            if Self::parse_mem_bytes(mem).is_none() {
                self.errors.push(
                    TypeError::new(
                        format!(
                            "invalid mem budget `{}` on function `{}`; expected a value like `100mb` or `1gb`",
                            mem, fn_name
                        ),
                        budget.span,
                    ),
                );
            }
        }
        if budget.cpu.is_none() && budget.mem.is_none() {
            self.errors.push(
                TypeError::new(
                    format!(
                        "@budget on function `{}` must specify at least `cpu` or `mem`",
                        fn_name
                    ),
                    budget.span,
                ),
            );
        }
    }

    /// Check that a callee's budget fits within the caller's budget.
    ///
    /// If the caller has a budget and the callee also has a budget, every
    /// component of the callee's budget must be <= the corresponding component
    /// of the caller's budget.
    fn check_budget_containment(
        &mut self,
        caller: &str,
        callee: &str,
        span: Span,
    ) {
        let caller_budget = match self.function_budgets.get(caller) {
            Some(b) => b.clone(),
            None => return, // Caller has no budget — no constraint to check.
        };
        let callee_budget = match self.function_budgets.get(callee) {
            Some(b) => b.clone(),
            None => {
                // Caller has a budget but callee does not — warn that the
                // callee is unconstrained.
                self.errors.push(
                    TypeError::new(
                        format!(
                            "function `{}` has a @budget but calls `{}` which has no budget; \
                             inner calls should declare budgets for containment checking",
                            caller, callee
                        ),
                        span,
                    ),
                );
                return;
            }
        };

        // Check cpu containment.
        if let (Some(ref caller_cpu), Some(ref callee_cpu)) = (&caller_budget.cpu, &callee_budget.cpu) {
            if let (Some(caller_ms), Some(callee_ms)) =
                (Self::parse_cpu_millis(caller_cpu), Self::parse_cpu_millis(callee_cpu))
            {
                if callee_ms > caller_ms {
                    self.errors.push(
                        TypeError::new(
                            format!(
                                "callee `{}` cpu budget `{}` exceeds caller `{}` cpu budget `{}`",
                                callee, callee_cpu, caller, caller_cpu
                            ),
                            span,
                        ),
                    );
                }
            }
        }

        // Check mem containment.
        if let (Some(ref caller_mem), Some(ref callee_mem)) = (&caller_budget.mem, &callee_budget.mem) {
            if let (Some(caller_bytes), Some(callee_bytes)) =
                (Self::parse_mem_bytes(caller_mem), Self::parse_mem_bytes(callee_mem))
            {
                if callee_bytes > caller_bytes {
                    self.errors.push(
                        TypeError::new(
                            format!(
                                "callee `{}` mem budget `{}` exceeds caller `{}` mem budget `{}`",
                                callee, callee_mem, caller, caller_mem
                            ),
                            span,
                        ),
                    );
                }
            }
        }
    }

    /// Parse a CPU duration string like `"5s"` or `"100ms"` into milliseconds.
    fn parse_cpu_millis(s: &str) -> Option<u64> {
        if let Some(n) = s.strip_suffix("ms") {
            n.parse::<u64>().ok()
        } else if let Some(n) = s.strip_suffix("s") {
            n.parse::<u64>().ok().map(|v| v * 1000)
        } else {
            None
        }
    }

    /// Parse a memory size string like `"100mb"` or `"1gb"` into bytes.
    fn parse_mem_bytes(s: &str) -> Option<u64> {
        if let Some(n) = s.strip_suffix("gb") {
            n.parse::<u64>().ok().map(|v| v * 1024 * 1024 * 1024)
        } else if let Some(n) = s.strip_suffix("mb") {
            n.parse::<u64>().ok().map(|v| v * 1024 * 1024)
        } else if let Some(n) = s.strip_suffix("kb") {
            n.parse::<u64>().ok().map(|v| v * 1024)
        } else if let Some(n) = s.strip_suffix("b") {
            n.parse::<u64>().ok()
        } else {
            None
        }
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Build a [`FnSig`] from a parsed function definition.
    fn fn_def_to_sig(&mut self, fn_def: &FnDef) -> FnSig {
        // If the function has type parameters, temporarily register them so
        // resolve_type_expr will produce TypeVar instead of "unknown type" errors.
        let tp_names: Vec<String> = fn_def.type_params.iter().map(|tp| tp.name.clone()).collect();
        if !tp_names.is_empty() {
            self.active_type_params = tp_names.clone();
        }

        let params: Vec<(String, Ty)> = fn_def
            .params
            .iter()
            .map(|p| (p.name.clone(), self.resolve_type_expr(&p.type_ann.node, p.type_ann.span)))
            .collect();

        let ret = fn_def
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(&t.node, t.span))
            .unwrap_or(Ty::Unit);

        let effects = fn_def
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        self.active_type_params.clear();

        FnSig {
            type_params: tp_names,
            params,
            ret,
            effects,
        }
    }

    /// Build a [`FnSig`] from a parsed extern function declaration.
    fn extern_fn_to_sig(&mut self, decl: &ExternFnDecl) -> FnSig {
        let params: Vec<(String, Ty)> = decl
            .params
            .iter()
            .map(|p| (p.name.clone(), self.resolve_type_expr(&p.type_ann.node, p.type_ann.span)))
            .collect();

        let ret = decl
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(&t.node, t.span))
            .unwrap_or(Ty::Unit);

        let effects = decl
            .effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_default();

        FnSig {
            type_params: vec![],
            params,
            ret,
            effects,
        }
    }

    // ------------------------------------------------------------------
    // Effect analysis
    // ------------------------------------------------------------------

    /// Build a [`ModuleEffectSummary`] from the inferred effects collected
    /// during type checking.
    fn build_effect_summary(&self, module: &Module) -> ModuleEffectSummary {
        let mut functions = Vec::new();
        let mut all_effects: Vec<String> = Vec::new();

        for item in &module.items {
            match &item.node {
                ItemKind::FnDef(fn_def) => {
                    let declared: Vec<String> = fn_def
                        .effects
                        .as_ref()
                        .map(|e| e.effects.clone())
                        .unwrap_or_default();

                    let inferred = self
                        .inferred_effects
                        .get(&fn_def.name)
                        .cloned()
                        .unwrap_or_default();

                    // Effect variables (lowercase) are not "unused" — they are
                    // polymorphic and resolved at call sites.
                    let unused: Vec<String> = declared
                        .iter()
                        .filter(|d| !inferred.contains(d) && !effects::is_effect_variable(d))
                        .cloned()
                        .collect();

                    let missing: Vec<String> = inferred
                        .iter()
                        .filter(|i| !declared.contains(i))
                        .cloned()
                        .collect();

                    let is_pure = inferred.is_empty();

                    for eff in &inferred {
                        if !all_effects.contains(eff) {
                            all_effects.push(eff.clone());
                        }
                    }

                    functions.push(EffectInfo {
                        function: fn_def.name.clone(),
                        declared,
                        inferred,
                        is_pure,
                        unused,
                        missing,
                    });
                }
                ItemKind::ExternFn(decl) => {
                    let declared: Vec<String> = decl
                        .effects
                        .as_ref()
                        .map(|e| e.effects.clone())
                        .unwrap_or_default();

                    for eff in &declared {
                        if !all_effects.contains(eff) {
                            all_effects.push(eff.clone());
                        }
                    }

                    functions.push(EffectInfo {
                        function: decl.name.clone(),
                        declared: declared.clone(),
                        inferred: declared.clone(),
                        is_pure: declared.is_empty(),
                        unused: Vec::new(),
                        missing: Vec::new(),
                    });
                }
                _ => {}
            }
        }

        all_effects.sort();
        let pure_count = functions.iter().filter(|f| f.is_pure).count();
        let effectful_count = functions.len() - pure_count;

        ModuleEffectSummary {
            functions,
            pure_count,
            capability_ceiling: self.module_capabilities.clone(),
            effectful_count,
            effects_used: all_effects,
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
