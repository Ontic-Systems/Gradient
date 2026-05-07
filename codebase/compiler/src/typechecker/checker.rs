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
use crate::ast::expr::{
    BinOp, ClosureParam, Expr, ExprKind, MatchArm, Pattern, StringInterpPart, UnaryOp,
};
use crate::ast::item::{
    BudgetConstraint, ContractKind, ExternFnDecl, FnDef, Item, ItemKind, VariantField,
};
use crate::ast::module::Module;
use crate::ast::span::{Span, Spanned};
use crate::ast::stmt::{Stmt, StmtKind};
use crate::ast::types::TypeExpr;
use crate::comptime::{ComptimeEvaluator, ComptimeValue};

use super::effects::{self, EffectInfo, ModuleEffectSummary};
use super::env::FnSig;
use super::env::TypeEnv;
use super::error::TypeError;
use super::types::Ty;
use super::vc;

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
    /// Compile-time evaluator for comptime function evaluation.
    comptime_evaluator: ComptimeEvaluator,
    /// Expected type for bidirectional type inference.
    /// Set before checking expressions in contexts where the type is known
    /// (e.g., let bindings with annotations).
    expected_type: Option<Ty>,
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

/// Information about an imported module, including both functions and types.
#[derive(Debug, Clone, Default)]
pub struct ImportedModuleInfo {
    /// Function signatures exported by the module.
    pub functions: std::collections::HashMap<String, FnSig>,
    /// Type aliases exported by the module: name -> expanded type.
    pub type_aliases: std::collections::HashMap<String, Ty>,
    /// Enum types exported by the module: name -> Ty::Enum.
    pub enums: std::collections::HashMap<String, Ty>,
    /// Maps variant name -> (enum_type_name, variant_index) for enums in this module.
    pub variant_mappings: std::collections::HashMap<String, (String, usize)>,
    /// Type parameters for generic enums: name -> params.
    pub enum_type_params: std::collections::HashMap<String, Vec<String>>,
}

/// A set of imported module information, used for multi-file type checking.
///
/// Each entry maps a module name to its exported functions and types.
pub type ImportedModules = std::collections::HashMap<String, ImportedModuleInfo>;

/// Type-check a parsed module with imported module signatures and return both
/// errors and effect analysis.
///
/// This is the multi-file entry point. The `imports` parameter provides the
/// function signatures and type definitions from all modules referenced by `use` declarations.
pub fn check_module_with_imports(
    module: &Module,
    file_id: u32,
    imports: &ImportedModules,
) -> (Vec<TypeError>, ModuleEffectSummary) {
    let mut checker = TypeChecker::new(file_id);

    // Register imported module functions and types.
    for (module_name, info) in imports {
        checker
            .env
            .import_module_full(module_name.clone(), info.clone());
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
            comptime_evaluator: ComptimeEvaluator::new(),
            expected_type: None,
        }
    }

    /// Record a required effect at an expression/builtin site and report a
    /// missing-effect error if the current function does not declare it.
    fn require_effect(&mut self, effect: &str, site: &str, span: Span) {
        let effect_owned = effect.to_string();
        if !self.current_inferred.contains(&effect_owned) {
            self.current_inferred.push(effect_owned.clone());
        }

        if !self.env.current_effects().contains(&effect_owned) {
            self.errors.push(
                TypeError::new(format!("{} requires effect `{}`", site, effect), span).with_note(
                    format!(
                        "add `!{{{}}}` to the enclosing function's signature",
                        effect
                    ),
                ),
            );
        }
    }

    fn require_heap_effect(&mut self, site: &str, span: Span) {
        self.require_effect("Heap", site, span);
    }

    // ------------------------------------------------------------------
    // @untrusted source mode (#360)
    // ------------------------------------------------------------------

    /// Enforce the four @untrusted restrictions across all items in a
    /// module. Called from check_module() before any other pass when
    /// `module.trust.is_untrusted()`.
    fn check_untrusted_restrictions(&mut self, module: &Module) {
        for item in &module.items {
            match &item.node {
                ItemKind::ExternFn(decl) => {
                    // (2) No FFI.
                    self.errors.push(
                        TypeError::new(
                            format!(
                                "extern function `{}` is not allowed in @untrusted module",
                                decl.name
                            ),
                            item.span,
                        )
                        .with_note(
                            "FFI is banned in @untrusted modules — agent-emitted code may not call \
                             into native libraries. Move FFI declarations to a @trusted module.",
                        ),
                    );
                }
                ItemKind::FnDef(fn_def) => {
                    // (1) No comptime parameters.
                    for p in &fn_def.params {
                        if p.comptime {
                            self.errors.push(
                                TypeError::new(
                                    format!(
                                        "comptime parameter `{}` is not allowed in @untrusted module",
                                        p.name
                                    ),
                                    p.span,
                                )
                                .with_note(
                                    "comptime evaluation is disabled in @untrusted modules — \
                                     agent-emitted code may not run at compile time.",
                                ),
                            );
                        }
                    }
                    // (3) Explicit effects required.
                    if fn_def.effects.is_none() {
                        self.errors.push(
                            TypeError::new(
                                format!(
                                    "function `{}` must declare its effects in @untrusted module",
                                    fn_def.name
                                ),
                                fn_def.body.span,
                            )
                            .with_note(
                                "effect inference is disabled in @untrusted modules; add an \
                                 explicit effect annotation, e.g. `-> [IO] Int` or `-> [] Int` \
                                 for a pure function.",
                            ),
                        );
                    }
                    // (4) Explicit return type required.
                    if fn_def.return_type.is_none() {
                        self.errors.push(
                            TypeError::new(
                                format!(
                                    "function `{}` must declare its return type in @untrusted module",
                                    fn_def.name
                                ),
                                fn_def.body.span,
                            )
                            .with_note(
                                "return-type inference is disabled in @untrusted modules; add \
                                 an explicit `-> T` clause.",
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------
    // Module and items
    // ------------------------------------------------------------------

    /// Check an entire module: first register all function signatures (so that
    /// forward references work), then check each item's body.
    fn check_module(&mut self, module: &Module) {
        // @untrusted enforcement (#360, input-surface restriction).
        //
        // If the module is marked `@untrusted`, agent-emitted code must
        // operate inside a restricted subset:
        //   1. No comptime — `comptime { ... }` blocks rejected.
        //   2. No FFI — `@extern` rejected.
        //   3. Explicit effects — every fn must declare its effect set.
        //   4. No public-API inference — every fn must declare its
        //      return type (and parameter types, which are already
        //      required by the parser).
        //
        // We surface diagnostics here per-item rather than as a single
        // module-wide error so untrusted authors see exactly which
        // declaration violates which restriction.
        if module.trust.is_untrusted() {
            self.check_untrusted_restrictions(module);
        }

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

                    // Register function with comptime evaluator for compile-time evaluation.
                    self.comptime_evaluator.register_function(fn_def.clone());

                    // Pre-register budget constraints so containment
                    // checking works for forward references.
                    if let Some(ref budget) = fn_def.budget {
                        self.function_budgets
                            .insert(fn_def.name.clone(), budget.clone());
                    }
                }
                ItemKind::ExternFn(decl) => {
                    if let Some(ref caps) = self.module_capabilities {
                        for eff in self.extern_decl_effects(decl) {
                            if !caps.contains(&eff) {
                                self.errors.push(
                                    TypeError::new(
                                        format!(
                                            "extern function `{}` declares effect `{}` which exceeds the module capability ceiling",
                                            decl.name, eff
                                        ),
                                        item.span,
                                    )
                                    .with_note(format!(
                                        "module @cap allows: {}",
                                        caps.join(", ")
                                    )),
                                );
                            }
                        }
                    }
                    let sig = self.extern_fn_to_sig(decl);
                    self.env.define_fn(decl.name.clone(), sig);
                }
                ItemKind::TypeDecl {
                    name, type_expr, ..
                } => {
                    let mut ty = self.resolve_type_expr(&type_expr.node, type_expr.span);
                    // If the RHS was a record body, the resolver gave us a
                    // Struct with an empty name. Patch in the declared name
                    // so it round-trips through error messages and matches
                    // record-literal lookups.
                    if let Ty::Struct {
                        name: ref mut sname,
                        ..
                    } = ty
                    {
                        if sname.is_empty() {
                            *sname = name.clone();
                        }
                    }
                    self.env.define_type_alias(name.clone(), ty);
                }
                ItemKind::EnumDecl {
                    name,
                    type_params,
                    variants,
                    ..
                } => {
                    // Activate type parameters so variant fields like TypeVar("T") resolve correctly.
                    let saved_type_params =
                        std::mem::replace(&mut self.active_type_params, type_params.clone());

                    let mut ty_variants = Vec::new();
                    for v in variants {
                        let field_ty: Option<Ty> = v.fields.as_ref().map(|fields| {
                            let tys: Vec<Ty> = fields
                                .iter()
                                .map(|f| match f {
                                    VariantField::Named { type_expr, .. } => {
                                        self.resolve_type_expr(&type_expr.node, type_expr.span)
                                    }
                                    VariantField::Anonymous(type_expr) => {
                                        self.resolve_type_expr(&type_expr.node, type_expr.span)
                                    }
                                })
                                .collect();
                            // Convert multiple fields to a tuple type, or use single type directly
                            if tys.len() == 1 {
                                tys.into_iter().next().unwrap()
                            } else {
                                Ty::Tuple(tys)
                            }
                        });
                        ty_variants.push((v.name.clone(), field_ty));
                    }
                    let enum_ty = Ty::Enum {
                        name: name.clone(),
                        variants: ty_variants.clone(),
                    };
                    self.env.define_enum(name.clone(), enum_ty.clone());

                    // Register formal type params so generic instantiation (Option[Task]) works.
                    if !type_params.is_empty() {
                        self.env
                            .define_enum_type_params(name.clone(), type_params.clone());
                    }

                    self.active_type_params = saved_type_params;

                    // Register unit variants as values of the enum type
                    // in the global scope, and tuple variants as functions.
                    for (vname, field_ty) in &ty_variants {
                        match field_ty {
                            None => {
                                // Unit variant: register as a variable with the enum type.
                                self.env.define(vname.clone(), enum_ty.clone());
                            }
                            Some(Ty::Tuple(field_types)) => {
                                // Multi-field tuple variant: register as a function with one
                                // parameter per field so `Task(42, "hello", true)` type-checks.
                                let params: Vec<(String, Ty, bool)> = field_types
                                    .iter()
                                    .enumerate()
                                    .map(|(i, ty)| (format!("field{}", i), ty.clone(), false))
                                    .collect();
                                self.env.define_fn(
                                    vname.clone(),
                                    FnSig {
                                        type_params: vec![],
                                        params,
                                        ret: enum_ty.clone(),
                                        effects: vec![],
                                    },
                                );
                            }
                            Some(single_ty) => {
                                // Single-field tuple variant: register as a function from field_ty to enum_ty.
                                self.env.define_fn(
                                    vname.clone(),
                                    FnSig {
                                        type_params: vec![],
                                        params: vec![(
                                            "value".to_string(),
                                            single_ty.clone(),
                                            false,
                                        )],
                                        ret: enum_ty.clone(),
                                        effects: vec![],
                                    },
                                );
                            }
                        }
                    }
                }
                ItemKind::ActorDecl {
                    name,
                    state_fields,
                    handlers,
                    ..
                } => {
                    // Register actor type and its handler signatures.
                    let mut actor_state = Vec::new();
                    for sf in state_fields {
                        let ty = self.resolve_type_expr(&sf.type_ann.node, sf.type_ann.span);
                        actor_state.push((sf.name.clone(), ty));
                    }
                    let mut actor_handlers = Vec::new();
                    for h in handlers {
                        let ret_ty = h
                            .return_type
                            .as_ref()
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
                        let params: Vec<(String, Ty, bool)> = m
                            .params
                            .iter()
                            .filter(|p| p.name != "self")
                            .map(|p| {
                                (
                                    p.name.clone(),
                                    self.resolve_type_expr(&p.type_ann.node, p.type_ann.span),
                                    p.comptime,
                                )
                            })
                            .collect();
                        let ret = m
                            .return_type
                            .as_ref()
                            .map(|t| self.resolve_type_expr(&t.node, t.span))
                            .unwrap_or(Ty::Unit);
                        let effects = m
                            .effects
                            .as_ref()
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
                ItemKind::ImplBlock {
                    trait_name,
                    target_type,
                    methods,
                } => {
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
                ItemKind::ModBlock {
                    name: mod_name,
                    items: mod_items,
                    ..
                } => {
                    // First pass: register types and functions within mod block.
                    // These are namespaced under the module name.
                    for mod_item in mod_items {
                        match &mod_item.node {
                            ItemKind::TypeDecl {
                                name, type_expr, ..
                            } => {
                                let mut ty =
                                    self.resolve_type_expr(&type_expr.node, type_expr.span);
                                if let Ty::Struct {
                                    name: ref mut sname,
                                    ..
                                } = ty
                                {
                                    if sname.is_empty() {
                                        *sname = name.clone();
                                    }
                                }
                                // Register with qualified name: mod_name::type_name
                                let qualified_name = format!("{}::{}", mod_name, name);
                                self.env
                                    .define_type_alias(qualified_name.clone(), ty.clone());
                                // Also register unqualified for internal use within the mod
                                self.env.define_type_alias(name.clone(), ty);
                            }
                            ItemKind::EnumDecl {
                                name,
                                type_params,
                                variants,
                                ..
                            } => {
                                let saved_type_params = std::mem::replace(
                                    &mut self.active_type_params,
                                    type_params.clone(),
                                );
                                let mut ty_variants = Vec::new();
                                for v in variants {
                                    let field_ty: Option<Ty> = v.fields.as_ref().map(|fields| {
                                        let tys: Vec<Ty> = fields
                                            .iter()
                                            .map(|f| match f {
                                                VariantField::Named { type_expr, .. } => self
                                                    .resolve_type_expr(
                                                        &type_expr.node,
                                                        type_expr.span,
                                                    ),
                                                VariantField::Anonymous(type_expr) => self
                                                    .resolve_type_expr(
                                                        &type_expr.node,
                                                        type_expr.span,
                                                    ),
                                            })
                                            .collect();
                                        if tys.len() == 1 {
                                            tys.into_iter().next().unwrap()
                                        } else {
                                            Ty::Tuple(tys)
                                        }
                                    });
                                    ty_variants.push((v.name.clone(), field_ty));
                                }
                                self.active_type_params = saved_type_params;

                                // Build enum type with variants
                                let enum_ty = Ty::Enum {
                                    name: name.clone(),
                                    variants: ty_variants.clone(),
                                };

                                // Register with qualified name
                                let qualified_name = format!("{}::{}", mod_name, name);
                                self.env
                                    .define_enum(qualified_name.clone(), enum_ty.clone());
                                self.env
                                    .define_type_alias(qualified_name.clone(), enum_ty.clone());

                                // Also register unqualified for internal use
                                self.env.define_enum(name.clone(), enum_ty.clone());
                                self.env.define_type_alias(name.clone(), enum_ty.clone());

                                // Register unit variants as values of the enum type
                                // in the module scope, and tuple variants as functions.
                                for (vname, field_ty) in &ty_variants {
                                    match field_ty {
                                        None => {
                                            // Unit variant: register as a variable with the enum type.
                                            self.env.define(vname.clone(), enum_ty.clone());
                                        }
                                        Some(Ty::Tuple(field_types)) => {
                                            // Multi-field tuple variant: register as a function with one
                                            // parameter per field so `Task(42, "hello", true)` type-checks.
                                            let params: Vec<(String, Ty, bool)> = field_types
                                                .iter()
                                                .enumerate()
                                                .map(|(i, ty)| {
                                                    (format!("field{}", i), ty.clone(), false)
                                                })
                                                .collect();
                                            self.env.define_fn(
                                                vname.clone(),
                                                FnSig {
                                                    type_params: vec![],
                                                    params,
                                                    ret: enum_ty.clone(),
                                                    effects: vec![],
                                                },
                                            );
                                        }
                                        Some(single_ty) => {
                                            // Single-field tuple variant: register as a function from field_ty to enum_ty.
                                            self.env.define_fn(
                                                vname.clone(),
                                                FnSig {
                                                    type_params: vec![],
                                                    params: vec![(
                                                        "value".to_string(),
                                                        single_ty.clone(),
                                                        false,
                                                    )],
                                                    ret: enum_ty.clone(),
                                                    effects: vec![],
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                            ItemKind::FnDef(fn_def) => {
                                let sig = self.fn_def_to_sig(fn_def);
                                let qualified_name = format!("{}::{}", mod_name, fn_def.name);
                                self.env.define_fn(qualified_name, sig.clone());
                                // Also register unqualified for internal use
                                self.env.define_fn(fn_def.name.clone(), sig);
                            }
                            ItemKind::ExternFn(decl) => {
                                // Issue #261: register `extern fn` declarations
                                // inside `mod` blocks so calls from the same (and
                                // other) modules resolve. Mirrors the top-level
                                // `ExternFn` registration at ~line 243 plus the
                                // qualified+unqualified pattern used by `FnDef`
                                // above. Capability-ceiling validation also runs
                                // here so a mod-block extern can't escape its
                                // module's `@cap` ceiling.
                                //
                                // Critical: do NOT override an existing
                                // registration. The same `bootstrap_*` extern is
                                // often pre-registered in `TypeEnv::new()` with
                                // explicit effects: vec![] (no effects); a
                                // mod-block re-declaration without explicit
                                // effects defaults to the conservative
                                // EXTERN_DEFAULT_EFFECTS set, which would break
                                // pure callers. Preserve the existing entry.
                                if let Some(ref caps) = self.module_capabilities {
                                    for eff in self.extern_decl_effects(decl) {
                                        if !caps.contains(&eff) {
                                            self.errors.push(
                                                TypeError::new(
                                                    format!(
                                                        "extern function `{}` declares effect `{}` which exceeds the module capability ceiling",
                                                        decl.name, eff
                                                    ),
                                                    mod_item.span,
                                                )
                                                .with_note(format!(
                                                    "module @cap allows: {}",
                                                    caps.join(", ")
                                                )),
                                            );
                                        }
                                    }
                                }
                                let sig = self.extern_fn_to_sig(decl);
                                let qualified_name = format!("{}::{}", mod_name, decl.name);
                                if self.env.lookup_fn(&qualified_name).is_none() {
                                    self.env.define_fn(qualified_name, sig.clone());
                                }
                                if self.env.lookup_fn(&decl.name).is_none() {
                                    self.env.define_fn(decl.name.clone(), sig);
                                }
                            }
                            _ => {}
                        }
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
            ItemKind::ActorDecl {
                name,
                state_fields,
                handlers,
                ..
            } => {
                self.check_actor_decl(name, state_fields, handlers);
            }
            ItemKind::TraitDecl { .. } => {
                // Trait declarations are validated in the first pass.
            }
            ItemKind::ImplBlock {
                trait_name,
                target_type,
                methods,
                ..
            } => {
                self.check_impl_block(trait_name, target_type, methods);
            }
            ItemKind::ModBlock {
                items: mod_items, ..
            } => {
                // Check items within the module block recursively.
                for mod_item in mod_items {
                    self.check_item(mod_item);
                }
            }
            // Import declarations are processed in a pre-pass (module resolution).
            ItemKind::Import { .. } => {}
        }
    }

    /// Check a function definition: set up parameter bindings and return type
    /// context, then type-check the body. Also infers which effects the body
    /// actually requires and validates declared effect names.
    fn check_fn_def(&mut self, fn_def: &FnDef) {
        // Set active type parameters so resolve_type_expr produces TypeVar.
        let tp_names: Vec<String> = fn_def
            .type_params
            .iter()
            .map(|tp| tp.name.clone())
            .collect();
        let saved_type_params = std::mem::replace(&mut self.active_type_params, tp_names);

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
                if !effects::is_valid_effect_name(eff_name) {
                    self.errors.push(
                        TypeError::new(format!("unknown effect `{}`", eff_name), fn_def.body.span)
                            .with_note(format!(
                                "known effects: {}; parameterized effects: Throws(ErrorType)",
                                effects::KNOWN_EFFECTS.join(", ")
                            )),
                    );
                }
            }
        }

        // Validate @budget annotation values if present.
        if let Some(ref budget) = fn_def.budget {
            self.validate_budget(budget, &fn_def.name);
            self.function_budgets
                .insert(fn_def.name.clone(), budget.clone());
        }

        // Validate @verified annotation (ADR 0003 — tiered contracts).
        //
        // Launch tier (this PR / sub-issue #327):
        //   1. `@verified` without any `@requires` or `@ensures` is a
        //      checker error. The verified tier exists to discharge
        //      contracts; an empty contract set means there is nothing
        //      to verify, which is almost always a typo.
        //   2. `@verified` with at least one contract emits a warning
        //      that the static-discharge pipeline is not yet wired
        //      end-to-end (sub-issues #328 VC generator and #329 Z3
        //      integration land that). The contracts continue to behave
        //      as runtime checks for now.
        //
        // The construction of `VerificationConditionSet` here is a
        // structural placeholder that anchors the surface for #328/#329;
        // it is intentionally not yet stored on the checker (no
        // downstream consumer exists).
        if fn_def.is_verified {
            if fn_def.contracts.is_empty() {
                self.errors.push(TypeError::new(
                    format!(
                        "@verified function `{}` has no `@requires` or `@ensures` contracts; the verified tier requires at least one predicate to discharge (ADR 0003)",
                        fn_def.name
                    ),
                    fn_def.body.span,
                ).with_note(
                    "either add a `@requires(...)` / `@ensures(...)` annotation or remove `@verified`".to_string()
                ));
            } else {
                let mut vc_set = vc::VerificationConditionSet::new(fn_def.name.clone());
                for contract in &fn_def.contracts {
                    vc_set.add_stub(contract.kind, contract.span);
                }
                debug_assert_eq!(vc_set.len(), fn_def.contracts.len());

                // Sub-issue #328: lower the function to SMT-LIB.
                // Sub-issue #329 (this PR): when `GRADIENT_VC_VERIFY`
                // is set, pipe each generated query through Z3 and
                // translate `sat` results back into structured
                // counterexample diagnostics. Without the env opt-in,
                // we keep the launch-tier behaviour: warn that static
                // verification is not wired by default and fall back
                // to runtime contract checks. This staged rollout
                // matches ADR 0003 step 3 — Z3 is opt-in until the
                // build-system flag and stdlib pilot land.
                let encoded = match vc::VcEncoder::encode_function(fn_def) {
                    Ok(enc) => {
                        vc_set.mark_translated();
                        vc::maybe_dump(&enc);
                        Some(enc)
                    }
                    Err(_) => None,
                };

                let verify_opt_in = std::env::var_os("GRADIENT_VC_VERIFY")
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);

                let mut surfaced_diagnostic = false;
                if verify_opt_in {
                    if let Some(enc) = &encoded {
                        let discharger = vc::ContractDischarger::default();
                        if discharger.solver_available() {
                            match discharger.discharge_encoded(enc) {
                                Ok(report) => {
                                    surfaced_diagnostic =
                                        self.surface_discharge_report(fn_def, &report);
                                }
                                Err(e) => {
                                    self.errors.push(TypeError::warning(
                                        format!(
                                            "@verified function `{}`: discharger could not run ({e}); falling back to runtime enforcement",
                                            fn_def.name
                                        ),
                                        fn_def.body.span,
                                    ));
                                    surfaced_diagnostic = true;
                                }
                            }
                        } else {
                            self.errors.push(TypeError::warning(
                                format!(
                                    "@verified function `{}`: GRADIENT_VC_VERIFY is set but Z3 is not available on PATH; install z3 or set GRADIENT_Z3_BIN. Falling back to runtime enforcement.",
                                    fn_def.name
                                ),
                                fn_def.body.span,
                            ));
                            surfaced_diagnostic = true;
                        }
                    }
                }

                if !surfaced_diagnostic {
                    self.errors.push(TypeError::warning(
                        format!(
                            "@verified function `{}`: static contract verification is unimplemented; contracts fall back to runtime enforcement (set GRADIENT_VC_VERIFY=1 to opt into Z3 discharge)",
                            fn_def.name
                        ),
                        fn_def.body.span,
                    ));
                }
            }
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
                let span = fn_def
                    .return_type
                    .as_ref()
                    .map(|t| t.span)
                    .unwrap_or(fn_def.body.span);
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
                let span = fn_def
                    .return_type
                    .as_ref()
                    .map(|t| t.span)
                    .unwrap_or(fn_def.body.span);
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
            if param.comptime {
                self.env.define_comptime(param.name.clone(), param_ty);
            } else {
                self.env.define(param.name.clone(), param_ty);
            }
        }

        // Type-check @requires preconditions (parameters are in scope).
        for contract in &fn_def.contracts {
            if contract.kind == ContractKind::Requires {
                let cond_ty = self.check_expr(&contract.condition);
                if !cond_ty.is_error() && cond_ty != Ty::Bool {
                    self.errors.push(TypeError::mismatch(
                        format!("@requires condition must be Bool, found `{}`", cond_ty),
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
                        format!("@ensures condition must be Bool, found `{}`", cond_ty),
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
        self.inferred_effects.insert(fn_def.name.clone(), inferred);

        self.env.pop_scope();
        self.env.clear_current_fn_return();
        self.env.clear_current_effects();
        self.active_type_params = saved_type_params;
        self.current_fn_name = saved_fn_name;
    }

    /// Check an extern function declaration (no body to check, just validate
    /// that the signature is well-formed and all types are FFI-compatible).
    fn check_extern_fn(&mut self, decl: &ExternFnDecl) {
        let declared_effects = self.extern_decl_effects(decl);

        if decl.effects.is_none() {
            self.errors.push(
                TypeError::warning(
                    format!(
                        "extern function `{}` omits effects and defaults to the conservative set `{}`",
                        decl.name,
                        declared_effects.join(", ")
                    ),
                    decl.params
                        .first()
                        .map(|param| param.span)
                        .or_else(|| decl.return_type.as_ref().map(|ty| ty.span))
                        .unwrap_or(decl.annotations.first().map(|ann| ann.span).unwrap_or(Span::point(self.file_id, crate::ast::span::Position::new(1, 1, 0)))),
                )
                .with_note("declare explicit effects on `@extern` for a narrower contract".to_string()),
            );
        }

        for eff_name in &declared_effects {
            if !effects::is_known_effect(eff_name) {
                self.errors.push(
                    TypeError::new(
                        format!("unknown effect `{}`", eff_name),
                        decl.return_type
                            .as_ref()
                            .map(|ty| ty.span)
                            .unwrap_or_else(|| {
                                decl.params
                                    .first()
                                    .map(|param| param.span)
                                    .unwrap_or(Span::point(
                                        self.file_id,
                                        crate::ast::span::Position::new(1, 1, 0),
                                    ))
                            }),
                    )
                    .with_note(format!(
                        "known effects: {}",
                        effects::KNOWN_EFFECTS.join(", ")
                    )),
                );
            }
        }

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
        matches!(
            ty,
            Ty::Int | Ty::Float | Ty::Bool | Ty::String | Ty::Unit | Ty::Error
        )
    }

    // ------------------------------------------------------------------
    // Actor declarations
    // ------------------------------------------------------------------

    /// Check an actor declaration: validate state field types and default values,
    /// and type-check each handler body.
    fn check_actor_decl(
        &mut self,
        name: &str,
        state_fields: &[crate::ast::item::StateField],
        handlers: &[crate::ast::item::MessageHandler],
    ) {
        // ── Phase 1: Check state fields types and default value types ───────
        for sf in state_fields {
            let expected_ty = self.resolve_type_expr(&sf.type_ann.node, sf.type_ann.span);
            let actual_ty = self.check_expr(&sf.default_value);

            // ── Phase 2: Send-safety validation ────────────────────────────────
            // Actor state must be Send-safe: no raw pointers, thread-unsafe types
            if !self.is_send_safe(&expected_ty, sf.type_ann.span) {
                self.errors.push(TypeError::new(
                    format!(
                        "actor `{}` state field `{}` has type `{}` which is not Send-safe; \
                         actor state cannot contain raw pointers or thread-unsafe types",
                        name, sf.name, expected_ty
                    ),
                    sf.type_ann.span,
                ));
            }

            if !actual_ty.is_error() && !expected_ty.is_error() && actual_ty != expected_ty {
                self.errors.push(TypeError::mismatch(
                    format!(
                        "state field `{}`: default value has type `{}`, expected `{}`",
                        sf.name, actual_ty, expected_ty
                    ),
                    sf.default_value.span,
                    expected_ty.clone(),
                    actual_ty,
                ));
            }
        }

        // ── Phase 3: Check handler bodies with Send-safety validation ─────
        for handler in handlers {
            let ret_ty = handler
                .return_type
                .as_ref()
                .map(|t| self.resolve_type_expr(&t.node, t.span))
                .unwrap_or(Ty::Unit);

            // Validate return type is Send-safe for replies
            if !self.is_send_safe(&ret_ty, handler.body.span) {
                self.errors.push(TypeError::new(
                    format!(
                        "actor `{}` handler `{}` return type `{}` is not Send-safe; \
                         actor message replies must be thread-safe",
                        name, handler.message_name, ret_ty
                    ),
                    handler.body.span,
                ));
            }

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

    /// Check if a type is Send-safe for actor state/messages.
    /// Send-safe types can be safely moved between threads.
    fn is_send_safe(&self, ty: &Ty, _span: Span) -> bool {
        match ty {
            // Primitive types are always Send-safe
            Ty::Int | Ty::Float | Ty::Bool | Ty::Unit => true,

            // String is Send-safe (immutable reference counted)
            Ty::String => true,

            // Range is a primitive-like type
            Ty::Range => true,

            // List is Send-safe if element type is Send-safe
            Ty::List(elem) => self.is_send_safe(elem, _span),

            // Map is Send-safe if both key and value types are Send-safe
            Ty::Map(key, value) => self.is_send_safe(key, _span) && self.is_send_safe(value, _span),

            // Set is Send-safe if element type is Send-safe
            Ty::Set(elem) => self.is_send_safe(elem, _span),

            // Queue is Send-safe if element type is Send-safe
            Ty::Queue(elem) => self.is_send_safe(elem, _span),

            // Stack is Send-safe if element type is Send-safe
            Ty::Stack(elem) => self.is_send_safe(elem, _span),

            // Tuple is Send-safe if all elements are Send-safe
            Ty::Tuple(elems) => elems.iter().all(|e| self.is_send_safe(e, _span)),

            // Function types: check if all captured types are Send-safe
            // Note: Ty::Fn captures are not directly stored, but we assume function pointers are Send-safe
            Ty::Fn { .. } => true,

            // GenRef is NOT Send-safe (contains a raw pointer to heap memory)
            Ty::GenRef { .. } => false,

            // Linear types are NOT Send-safe by design (use-once semantics)
            Ty::Linear(_) => false,

            // Enum types are Send-safe if all their variants are Send-safe
            Ty::Enum { variants, .. } => variants.iter().all(|(_, field_ty)| {
                field_ty
                    .as_ref()
                    .map(|ty| self.is_send_safe(ty, _span))
                    .unwrap_or(true)
            }),

            // Actor handle is Send-safe (it's just an ID)
            Ty::Actor { .. } => true,

            // Type variables: conservatively assume not Send-safe until resolved
            Ty::TypeVar(_) => true, // Allow for now - generic types will be checked at instantiation

            // Error type is Send-safe (error recovery)
            Ty::Error => true,

            // Struct types are Send-safe if all fields are Send-safe and capability allows
            Ty::Struct { fields, cap, .. } => {
                // Struct is Send-safe if the capability allows sending (iso, val, tag)
                // and all field types are Send-safe
                if !cap.is_sendable() {
                    return false;
                }
                fields.iter().all(|(_, ty)| self.is_send_safe(ty, _span))
            }

            // Type values are compile-time only, not relevant for Send-safety
            Ty::Type => true,

            // HashMap is Send-safe if both key and value types are Send-safe
            Ty::HashMap(k, v) => self.is_send_safe(k, _span) && self.is_send_safe(v, _span),

            // Iterator is Send-safe if element type is Send-safe
            Ty::Iterator(elem) => self.is_send_safe(elem, _span),

            // StringBuilder is not Send-safe (contains mutable internal state)
            Ty::StringBuilder => false,
        }
    }

    // ------------------------------------------------------------------
    // Trait impl checking
    // ------------------------------------------------------------------

    /// Check an impl block: validate that all required trait methods are
    /// implemented with matching signatures.
    fn check_impl_block(&mut self, trait_name: &str, target_type: &str, methods: &[FnDef]) {
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
                    let impl_non_self_params: Vec<_> =
                        impl_fn.params.iter().filter(|p| p.name != "self").collect();
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
                        for (impl_p, (_, trait_ty, _)) in
                            impl_non_self_params.iter().zip(trait_method.params.iter())
                        {
                            let impl_ty =
                                self.resolve_type_expr(&impl_p.type_ann.node, impl_p.type_ann.span);
                            let expected_ty = Self::substitute_self(trait_ty, &self_ty);
                            if !impl_ty.is_error()
                                && !expected_ty.is_error()
                                && impl_ty != expected_ty
                            {
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
                    let impl_ret = impl_fn
                        .return_type
                        .as_ref()
                        .map(|t| self.resolve_type_expr(&t.node, t.span))
                        .unwrap_or(Ty::Unit);
                    let expected_ret = Self::substitute_self(&trait_method.ret, &self_ty);
                    if !impl_ret.is_error() && !expected_ret.is_error() && impl_ret != expected_ret
                    {
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
            let tp_names: Vec<String> = method
                .type_params
                .iter()
                .map(|tp| tp.name.clone())
                .collect();
            let saved_type_params = std::mem::replace(&mut self.active_type_params, tp_names);

            let ret_ty = method
                .return_type
                .as_ref()
                .map(|t| self.resolve_type_expr(&t.node, t.span))
                .unwrap_or(Ty::Unit);
            self.env.set_current_fn_return(ret_ty);

            let effects: Vec<String> = method
                .effects
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
                        && !Self::types_compatible_with_typevars(&ty, &expected)
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
        // For bidirectional type inference: if there's a type annotation,
        // set it as the expected type before checking the value expression.
        // This allows generic function calls to infer type parameters from
        // the expected return type (e.g., `let m: HashMap[String, Int] = hashmap_new()`).
        let saved_expected = self.expected_type.take();

        let ann_ty = type_ann.map(|ann| self.resolve_type_expr(&ann.node, ann.span));

        if let Some(ref ann) = ann_ty {
            self.expected_type = Some(ann.clone());
        }

        let value_ty = self.check_expr(value);

        // Clear the expected type after checking the expression.
        self.expected_type = saved_expected;

        if let Some(ann_ty) = ann_ty {
            // Allow assignment when the value type is structurally compatible
            // with the annotation modulo TypeVar wildcards.  This lets
            // `let m: Map[String, Int] = map_new()` work even though map_new()
            // returns Map[String, TypeVar("V")].
            let mismatch = !value_ty.is_error()
                && !ann_ty.is_error()
                && value_ty != ann_ty
                && !Self::types_compatible_with_typevars(&value_ty, &ann_ty);
            if mismatch {
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
            ExprKind::CharLit(_) => Ty::Int, // Characters are represented as integers
            ExprKind::BoolLit(_) => Ty::Bool,
            ExprKind::UnitLit => Ty::Unit,

            ExprKind::StringInterp { parts } => {
                for part in parts {
                    if let StringInterpPart::Expr(inner_expr) = part {
                        let ty = self.check_expr(inner_expr);
                        if !ty.is_error()
                            && ty != Ty::String
                            && ty != Ty::Int
                            && ty != Ty::Float
                            && ty != Ty::Bool
                        {
                            self.errors.push(TypeError::new(
                                format!(
                                    "type `{}` cannot be interpolated into a string (expected String, Int, Float, or Bool)",
                                    ty
                                ),
                                inner_expr.span,
                            ));
                        }
                    }
                }
                Ty::String
            }

            ExprKind::Ident(name) => {
                // First check local variables, then function names.
                if let Some(ty) = self.env.lookup(name) {
                    return ty.clone();
                }
                if let Some(sig) = self.env.lookup_fn(name) {
                    return Ty::Fn {
                        params: sig.params.iter().map(|(_, t, _)| t.clone()).collect(),
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

                let mut error =
                    TypeError::new(format!("typed hole `{}` found", label_str), expr.span);

                let mut hole_data = super::error::TypedHoleData {
                    label: label_str.clone(),
                    expected_type: None,
                    matching_bindings: Vec::new(),
                    matching_functions: Vec::new(),
                };

                if let Some(ref expected) = expected_ty {
                    let expected_str = format!("{}", expected);
                    error = error.with_note(format!("expected type: {}", expected_str));
                    hole_data.expected_type = Some(expected_str);

                    // Collect all in-scope bindings whose type matches the expected type.
                    let all_bindings = self.env.all_bindings();
                    let matching_data: Vec<super::error::HoleBindingData> = all_bindings
                        .iter()
                        .filter(|(_, ty, _)| ty == expected)
                        .map(|(name, ty, _)| super::error::HoleBindingData {
                            name: name.clone(),
                            ty: format!("{}", ty),
                        })
                        .collect();
                    let matching: Vec<String> = matching_data
                        .iter()
                        .map(|b| format!("`{}` ({})", b.name, b.ty))
                        .collect();

                    // Also check functions that return the expected type.
                    let matching_fns_data: Vec<super::error::HoleFunctionData> = self
                        .env
                        .all_functions()
                        .iter()
                        .filter(|(_, sig)| sig.ret == *expected)
                        .map(|(name, sig)| {
                            let params = sig
                                .params
                                .iter()
                                .map(|(pname, pty, _)| format!("{}: {}", pname, pty))
                                .collect::<Vec<_>>()
                                .join(", ");
                            super::error::HoleFunctionData {
                                name: name.clone(),
                                signature: format!("{}({}) -> {}", name, params, sig.ret),
                            }
                        })
                        .collect();
                    let matching_fns: Vec<String> = matching_fns_data
                        .iter()
                        .map(|f| {
                            // Format as "`name(params)` -> Ret" for the human-readable note.
                            let (sig, ret) = f
                                .signature
                                .split_once(" -> ")
                                .unwrap_or((f.signature.as_str(), ""));
                            format!("`{}` -> {}", sig, ret)
                        })
                        .collect();

                    if !matching.is_empty() {
                        error = error.with_note(format!(
                            "matching bindings in scope: {}",
                            matching.join(", ")
                        ));
                    }
                    if !matching_fns.is_empty() {
                        error = error
                            .with_note(format!("matching functions: {}", matching_fns.join(", ")));
                    }
                    if matching.is_empty() && matching_fns.is_empty() {
                        error = error.with_note(
                            "no bindings or functions in scope match the expected type".to_string(),
                        );
                    }

                    hole_data.matching_bindings = matching_data;
                    hole_data.matching_functions = matching_fns_data;
                } else {
                    error =
                        error.with_note("fill in the hole with a concrete expression".to_string());
                }

                error = error.with_hole_data(hole_data);
                self.errors.push(error);
                Ty::Error
            }

            ExprKind::BinaryOp {
                op: BinOp::Pipe,
                left,
                right,
            } => {
                // Desugar `left |> right` to `right(left)`.
                self.check_call(right, &[left.as_ref().clone()], expr.span)
            }

            ExprKind::BinaryOp { op, left, right } => {
                self.check_binary_op(*op, left, right, expr.span)
            }

            ExprKind::UnaryOp { op, operand } => self.check_unary_op(*op, operand, expr.span),

            ExprKind::Call { func, args } => self.check_call(func, args, expr.span),

            ExprKind::FieldAccess { object, field } => {
                // Check if this is a qualified module reference (e.g., `math.add`).
                if let ExprKind::Ident(module_name) = &object.node {
                    if self.env.is_imported_module(module_name) {
                        // Resolve as a qualified function reference.
                        if let Some(sig) = self.env.lookup_qualified_fn(module_name, field) {
                            return Ty::Fn {
                                params: sig.params.iter().map(|(_, t, _)| t.clone()).collect(),
                                ret: Box::new(sig.ret.clone()),
                                effects: sig.effects.clone(),
                            };
                        } else {
                            self.errors.push(TypeError::new(
                                format!("module `{}` has no function `{}`", module_name, field),
                                expr.span,
                            ));
                            return Ty::Error;
                        }
                    }
                }

                let obj_ty = self.check_expr(object);
                match &obj_ty {
                    Ty::Struct { name, fields, .. } => {
                        match fields.iter().find(|(n, _)| n == field) {
                            Some((_, fty)) => fty.clone(),
                            None => {
                                self.errors.push(TypeError::new(
                                    format!("struct `{}` has no field `{}`", name, field),
                                    expr.span,
                                ));
                                Ty::Error
                            }
                        }
                    }
                    Ty::Error => Ty::Error,
                    _ => {
                        self.errors.push(TypeError::new(
                            format!("field access `.{}` on non-record type `{}`", field, obj_ty),
                            expr.span,
                        ));
                        Ty::Error
                    }
                }
            }

            ExprKind::If {
                condition,
                then_block,
                else_ifs,
                else_block,
            } => self.check_if(condition, then_block, else_ifs, else_block, expr.span),

            ExprKind::For { var, iter, body } => {
                // Check the iterator expression and determine the element type.
                let iter_ty = self.check_expr(iter);

                let elem_ty = match &iter_ty {
                    Ty::List(elem) => *elem.clone(),
                    Ty::Range => Ty::Int,
                    // Backward compat: range() returns Unit in v0.1.
                    Ty::Unit => Ty::Int,
                    Ty::Error => Ty::Error,
                    other => {
                        self.errors.push(TypeError::mismatch(
                            format!("cannot iterate over type `{}`", other),
                            iter.span,
                            Ty::List(Box::new(Ty::Int)),
                            other.clone(),
                        ));
                        Ty::Error
                    }
                };

                self.env.push_scope();
                self.env.define(var.clone(), elem_ty);
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

            ExprKind::Match { scrutinee, arms } => self.check_match(scrutinee, arms, expr.span),

            ExprKind::Paren(inner) => self.check_expr(inner),

            ExprKind::ListLit(elements) => {
                self.require_heap_effect("list literal", expr.span);
                if elements.is_empty() {
                    // Empty list: return List[Unknown] so that any List[T] annotation is
                    // accepted without a type mismatch error.  TypeVar("_") acts as a
                    // wildcard that `types_compatible_with_typevars` matches against any type.
                    Ty::List(Box::new(Ty::TypeVar("_".to_string())))
                } else {
                    let first_ty = self.check_expr(&elements[0]);
                    for elem in elements.iter().skip(1) {
                        let elem_ty = self.check_expr(elem);
                        if !elem_ty.is_error() && !first_ty.is_error() && elem_ty != first_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "list element type mismatch: expected `{}`, found `{}`",
                                    first_ty, elem_ty
                                ),
                                elem.span,
                                first_ty.clone(),
                                elem_ty,
                            ));
                        }
                    }
                    if first_ty.is_error() {
                        Ty::Error
                    } else {
                        Ty::List(Box::new(first_ty))
                    }
                }
            }

            ExprKind::Tuple(elems) => {
                let elem_types: Vec<Ty> = elems.iter().map(|e| self.check_expr(e)).collect();
                Ty::Tuple(elem_types)
            }
            ExprKind::RecordLit {
                type_name,
                base,
                fields,
            } => {
                self.require_heap_effect("record literal", expr.span);
                // Look up the declared struct type by name. We always check
                // every field expression so type errors inside field values
                // are reported even if the surrounding type lookup fails.
                let provided: Vec<(String, Ty)> = fields
                    .iter()
                    .map(|(fname, fexpr)| (fname.clone(), self.check_expr(fexpr)))
                    .collect();

                // Check the spread base if present, so its type errors
                // surface even if the type lookup fails.
                let base_ty = base.as_ref().map(|b| self.check_expr(b));

                let declared = self.env.lookup_type_alias(type_name).cloned();

                match declared {
                    Some(Ty::Struct {
                        name: sname,
                        fields: decl_fields,
                        cap,
                    }) => {
                        // The base must have the same struct type as the
                        // declared name; otherwise we can't safely fill in
                        // missing fields from it.
                        if let Some(bty) = &base_ty {
                            let same_struct = matches!(
                                bty,
                                Ty::Struct { name: bname, .. } if bname == &sname
                            );
                            if !same_struct && !matches!(bty, Ty::Error) {
                                self.errors.push(TypeError::mismatch(
                                    format!("record-spread base must be a `{}`", sname),
                                    expr.span,
                                    Ty::Struct {
                                        name: sname.clone(),
                                        fields: decl_fields.clone(),
                                        cap,
                                    },
                                    bty.clone(),
                                ));
                            }
                        }

                        // Validate explicitly provided fields against the
                        // declared field set. We don't require the same
                        // order — record literals are by name.
                        for (pname, pty) in &provided {
                            match decl_fields.iter().find(|(n, _)| n == pname) {
                                Some((_, expected)) => {
                                    if expected != pty
                                        && !matches!(pty, Ty::Error)
                                        && !matches!(expected, Ty::Error)
                                    {
                                        self.errors.push(TypeError::mismatch(
                                            format!(
                                                "field `{}` of `{}` has wrong type",
                                                pname, sname
                                            ),
                                            expr.span,
                                            expected.clone(),
                                            pty.clone(),
                                        ));
                                    }
                                }
                                None => {
                                    self.errors.push(TypeError::new(
                                        format!("struct `{}` has no field `{}`", sname, pname),
                                        expr.span,
                                    ));
                                }
                            }
                        }
                        // If there's no base, every field must be provided.
                        // With a base, missing fields are taken from it.
                        if base.is_none() {
                            for (fname, _) in &decl_fields {
                                if !provided.iter().any(|(n, _)| n == fname) {
                                    self.errors.push(TypeError::new(
                                        format!("missing field `{}` in `{}` literal", fname, sname),
                                        expr.span,
                                    ));
                                }
                            }
                        }
                        Ty::Struct {
                            name: sname,
                            fields: decl_fields,
                            cap,
                        }
                    }
                    Some(other) => {
                        self.errors.push(TypeError::new(
                            format!("`{}` is not a record type (found `{}`)", type_name, other),
                            expr.span,
                        ));
                        Ty::Error
                    }
                    None => {
                        self.errors.push(TypeError::new(
                            format!("unknown record type `{}`", type_name),
                            expr.span,
                        ));
                        Ty::Error
                    }
                }
            }
            ExprKind::Construct { name, fields } => {
                self.require_heap_effect("constructor", expr.span);
                // Always type-check the supplied field expressions so downstream
                // diagnostics inside them are not lost regardless of validation outcome.
                let field_tys: Vec<Ty> = fields.iter().map(|(_, e)| self.check_expr(e)).collect();

                // Look up which enum (if any) this constructor name belongs to.
                let variant_lookup = self
                    .env
                    .lookup_variant(name)
                    .map(|(en, idx)| (en.clone(), *idx));

                if let Some((enum_name, variant_idx)) = variant_lookup {
                    let enum_ty = self.env.lookup_enum(&enum_name).cloned();
                    if let Some(Ty::Enum { variants, .. }) = enum_ty.as_ref().cloned() {
                        let payload = variants.get(variant_idx).and_then(|(_, p)| p.clone());

                        // Compute expected per-field payload types from the variant.
                        let expected_tys: Vec<Ty> = match &payload {
                            None => Vec::new(),
                            Some(Ty::Tuple(elems)) => elems.clone(),
                            Some(other) => vec![other.clone()],
                        };

                        if field_tys.len() != expected_tys.len() {
                            self.errors.push(TypeError::new(
                                format!(
                                    "enum variant `{}::{}` expects {} field(s), but {} were provided",
                                    enum_name,
                                    name,
                                    expected_tys.len(),
                                    field_tys.len()
                                ),
                                expr.span,
                            ));
                        } else {
                            for (i, ((fname, fexpr), exp_ty)) in
                                fields.iter().zip(expected_tys.iter()).enumerate()
                            {
                                let got = &field_tys[i];
                                // Skip checks when either side is unresolved/error or the
                                // payload slot is a generic type variable (lenient for generic enums).
                                if got.is_error()
                                    || exp_ty.is_error()
                                    || got.is_type_var()
                                    || matches!(exp_ty, Ty::TypeVar(_))
                                {
                                    continue;
                                }
                                if got != exp_ty
                                    && !Self::types_compatible_with_typevars(got, exp_ty)
                                {
                                    self.errors.push(TypeError::mismatch(
                                        format!(
                                            "field `{}` of enum variant `{}::{}` has type `{}`, expected `{}`",
                                            fname, enum_name, name, got, exp_ty
                                        ),
                                        fexpr.span,
                                        exp_ty.clone(),
                                        got.clone(),
                                    ));
                                }
                            }
                        }
                        return enum_ty.unwrap();
                    }
                    // Variant mapping pointed at an unknown enum; fall through to
                    // the legacy tuple shape as a defensive default.
                    return Ty::Tuple(field_tys);
                }

                // Unknown constructor name: surface a clear diagnostic instead
                // of silently returning a tuple type, which masked real errors.
                self.errors.push(TypeError::new(
                    format!("unknown constructor `{}`", name),
                    expr.span,
                ));
                Ty::Error
            }
            ExprKind::TypedExpr { type_expr, value } => {
                // Type-check the inner value and verify it's compatible with the
                // annotation. Mismatches produce a diagnostic but the annotated
                // type is still returned so that downstream checking proceeds.
                let value_ty = self.check_expr(value);
                let ann_ty = self.resolve_type_expr(type_expr, expr.span);
                if !value_ty.is_error()
                    && !ann_ty.is_error()
                    && !value_ty.is_type_var()
                    && !ann_ty.is_type_var()
                    && value_ty != ann_ty
                    && !Self::types_compatible_with_typevars(&value_ty, &ann_ty)
                {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "type annotation mismatch: value has type `{}`, but annotation requires `{}`",
                            value_ty, ann_ty
                        ),
                        expr.span,
                        ann_ty.clone(),
                        value_ty,
                    ));
                }
                ann_ty
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
                // `spawn` starts actor work across an async/send boundary.
                let site = format!("`spawn {}`", actor_name);
                self.require_effect("Actor", &site, expr.span);
                self.require_effect("Async", &site, expr.span);
                self.require_effect("Send", &site, expr.span);
                Ty::Actor {
                    name: actor_name.clone(),
                }
            }

            ExprKind::Send { target, message } => {
                let target_ty = self.check_expr(target);

                match &target_ty {
                    Ty::Actor { name: actor_name } => {
                        // Validate the message is handled by this actor.
                        let actor_name = actor_name.clone();
                        let valid = self
                            .env
                            .lookup_actor(&actor_name)
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

                // `send` transfers a message across an async/send boundary.
                self.require_effect("Actor", "`send`", expr.span);
                self.require_effect("Async", "`send`", expr.span);
                self.require_effect("Send", "`send`", expr.span);

                Ty::Unit
            }

            ExprKind::Ask { target, message } => {
                let target_ty = self.check_expr(target);

                let ret_ty = match &target_ty {
                    Ty::Actor { name: actor_name } => {
                        let actor_name = actor_name.clone();
                        let handler_ret = self.env.lookup_actor(&actor_name).and_then(|info| {
                            info.handlers
                                .iter()
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

                // `ask` transfers a request/response across an async/send boundary.
                self.require_effect("Actor", "`ask`", expr.span);
                self.require_effect("Async", "`ask`", expr.span);
                self.require_effect("Send", "`ask`", expr.span);

                ret_ty
            }

            ExprKind::Closure {
                params,
                return_type,
                body,
            } => self.check_closure(params, return_type.as_ref(), body, expr.span),

            ExprKind::Range { start, end } => {
                let start_ty = self.check_expr(start);
                let end_ty = self.check_expr(end);

                let start_err = start_ty.is_error();
                let end_err = end_ty.is_error();

                if !start_err && start_ty != Ty::Int {
                    self.errors.push(TypeError::mismatch(
                        format!("range start must be `Int`, found `{}`", start_ty),
                        start.span,
                        Ty::Int,
                        start_ty,
                    ));
                }
                if !end_err && end_ty != Ty::Int {
                    self.errors.push(TypeError::mismatch(
                        format!("range end must be `Int`, found `{}`", end_ty),
                        end.span,
                        Ty::Int,
                        end_ty,
                    ));
                }

                if start_err || end_err {
                    Ty::Error
                } else {
                    Ty::Range
                }
            }

            ExprKind::Try(inner) => {
                let inner_ty = self.check_expr(inner);

                // The inner expression must be a Result[T, E] (an enum named "Result").
                match &inner_ty {
                    Ty::Enum { name, variants } if name == "Result" => {
                        // Extract the T from Ok(T) and E from Err(E).
                        let ok_ty = variants
                            .iter()
                            .find(|(vn, _)| vn == "Ok")
                            .and_then(|(_, t)| t.clone());
                        let _err_ty = variants
                            .iter()
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

            ExprKind::Defer { body } => {
                // Type-check the deferred expression.
                // The defer expression itself evaluates to unit.
                let _body_ty = self.check_expr(body);
                Ty::Unit
            }

            ExprKind::ConcurrentScope { body } => {
                // Concurrent scope requires Actor effect since it manages actor lifetimes.
                if !self.env.current_effects().contains(&"Actor".to_string()) {
                    self.current_inferred.push("Actor".to_string());
                    self.errors.push(
                        TypeError::new(
                            "`concurrent_scope` requires effect `Actor`".to_string(),
                            expr.span,
                        )
                        .with_note(
                            "add `!{Actor}` to the function's effect annotation".to_string(),
                        ),
                    );
                } else {
                    self.current_inferred.push("Actor".to_string());
                }

                // Type-check the body in a new scope.
                self.env.push_scope();
                let _body_ty = self.check_block(body);
                self.env.pop_scope();

                // Concurrent scope returns unit (children are cancelled when scope exits).
                Ty::Unit
            }

            ExprKind::Supervisor {
                strategy: _,
                max_restarts,
                children,
            } => {
                // Supervisor requires Actor effect.
                if !self.env.current_effects().contains(&"Actor".to_string()) {
                    self.current_inferred.push("Actor".to_string());
                    self.errors.push(
                        TypeError::new(
                            "`supervisor` requires effect `Actor`".to_string(),
                            expr.span,
                        )
                        .with_note(
                            "add `!{Actor}` to the function's effect annotation".to_string(),
                        ),
                    );
                } else {
                    self.current_inferred.push("Actor".to_string());
                }

                // Validate that all children reference valid actor types.
                for child in children {
                    if self.env.lookup_actor(&child.actor_type).is_none() {
                        self.errors.push(TypeError::new(
                            format!(
                                "supervisor child references unknown actor type `{}`",
                                child.actor_type
                            ),
                            child.span,
                        ));
                    }
                }

                // Validate max_restarts is non-negative if provided.
                if let Some(max) = max_restarts {
                    if *max < 0 {
                        self.errors.push(TypeError::new(
                            format!("max_restarts must be non-negative, found {}", max),
                            expr.span,
                        ));
                    }
                }

                // Supervisor returns a handle to the supervisor actor (unit for now).
                Ty::Unit
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
    fn check_binary_op(&mut self, op: BinOp, left: &Expr, right: &Expr, span: Span) -> Ty {
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
                        format!("operands of `{}` must have the same type", binop_symbol(op)),
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
                        format!("operands of `{}` must have the same type", binop_symbol(op)),
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
                        format!("operands of `{}` must have the same type", binop_symbol(op)),
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

            // Pipe operator is desugared in check_expr before reaching here.
            BinOp::Pipe => unreachable!("`|>` is desugared in check_expr"),
        }
    }

    /// Type-check a unary operation.
    fn check_unary_op(&mut self, op: UnaryOp, operand: &Expr, span: Span) -> Ty {
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
                        format!("`not` requires a Bool operand, found `{}`", operand_ty),
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
        for (_, ty, _) in &sig.params {
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
                Ty::Fn {
                    params: p_params,
                    ret: p_ret,
                    effects: p_effects,
                },
                Ty::Fn {
                    params: a_params,
                    ret: a_ret,
                    effects: a_effects,
                },
            ) => {
                if p_params.len() != a_params.len() {
                    return false;
                }
                for (pp, ap) in p_params.iter().zip(a_params.iter()) {
                    if pp != ap && !pp.is_error() && !ap.is_error() {
                        return false;
                    }
                }
                if **p_ret != **a_ret && !p_ret.is_error() && !a_ret.is_error() {
                    return false;
                }
                let p_vars: Vec<&String> = p_effects
                    .iter()
                    .filter(|e| effects::is_effect_variable(e))
                    .collect();
                let p_concrete: Vec<&String> = p_effects
                    .iter()
                    .filter(|e| !effects::is_effect_variable(e))
                    .collect();
                if p_vars.is_empty() {
                    return p_effects == a_effects;
                }
                let remaining: Vec<String> = a_effects
                    .iter()
                    .filter(|e| !p_concrete.contains(e))
                    .cloned()
                    .collect();
                for var in &p_vars {
                    if let Some(prev) = effect_bindings.get(var.as_str()) {
                        if *prev != remaining {
                            return false;
                        }
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
                        if !resolved.contains(concrete) {
                            resolved.push(concrete.clone());
                        }
                    }
                }
            } else if !resolved.contains(eff) {
                resolved.push(eff.clone());
            }
        }
        resolved
    }

    /// Type-check a function call expression.
    fn check_call(&mut self, func: &Expr, args: &[Expr], span: Span) -> Ty {
        // Check for qualified function calls: `module.func(args)`.
        // The parser produces FieldAccess { object: Ident("module"), field: "func" }.
        if let ExprKind::FieldAccess { object, field } = &func.node {
            if let ExprKind::Ident(module_name) = &object.node {
                if self.env.is_imported_module(module_name) {
                    let sig = self.env.lookup_qualified_fn(module_name, field).cloned();
                    if let Some(sig) = sig {
                        let qualified_name = format!("{}.{}", module_name, field);
                        return self.check_call_with_sig(&qualified_name, &sig, args, span);
                    } else {
                        self.errors.push(TypeError::new(
                            format!("module `{}` has no function `{}`", module_name, field),
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

            // Method call syntax: `object.method(args)`.
            // Desugar to a function call with the object as the first argument.
            if let Some(result) = self.check_method_call(object, field, args, span) {
                return result;
            }
        }

        // Resolve the function being called.
        let func_name = match &func.node {
            ExprKind::Ident(name) => Some(name.clone()),
            _ => None,
        };

        // Handle list builtins specially (they are generic over element type).
        if let Some(ref name) = func_name {
            if let Some(ty) = self.check_list_builtin(name, args, span) {
                return ty;
            }
        }

        // Handle map builtins specially (they are generic over value type).
        if let Some(ref name) = func_name {
            if let Some(ty) = self.check_map_builtin(name, args, span) {
                return ty;
            }
        }

        // Try to look up a known function signature by name.
        let sig = func_name
            .as_ref()
            .and_then(|n| self.env.lookup_fn(n))
            .cloned();

        if let Some(sig) = sig {
            // If the function is generic, handle it with type inference.
            if !sig.type_params.is_empty() {
                // Clone expected type to avoid borrow issues
                let expected = self.expected_type.clone();
                return self.check_generic_call(
                    func_name.as_deref().unwrap_or("<unknown>"),
                    &sig,
                    args,
                    span,
                    expected.as_ref(),
                );
            }

            // Check argument count.
            if args.len() != sig.params.len() {
                self.errors.push(TypeError::new(
                    format!(
                        "function `{}` expects {} argument(s), but {} were provided",
                        func_name.as_deref().unwrap_or("<unknown>"),
                        sig.params.len(),
                        args.len()
                    ),
                    span,
                ));
                return Ty::Error;
            }

            // Determine if this call involves effect polymorphism.
            let has_effect_vars = Self::sig_has_effect_variables(&sig);
            let mut effect_bindings: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();

            // Check each argument type.
            for (i, (arg, (param_name, param_ty, is_comptime))) in
                args.iter().zip(sig.params.iter()).enumerate()
            {
                let arg_ty = self.check_expr(arg);
                if arg_ty.is_error() || param_ty.is_error() {
                    continue;
                }

                // Validate comptime parameters.
                if *is_comptime {
                    if let Err(e) = self.require_comptime(arg) {
                        // Enhance the error with comptime parameter context
                        let mut enhanced = TypeError::new(
                            format!(
                                "runtime value passed to comptime parameter `{}`",
                                param_name
                            ),
                            arg.span,
                        );
                        enhanced = enhanced.with_note(format!(
                            "parameter `{}` is marked `comptime` and requires a compile-time known value",
                            param_name
                        ));
                        // Include the original error message as an additional note
                        enhanced = enhanced.with_note(e.message);
                        enhanced = enhanced.with_note(
                            "to fix: either remove `comptime` from the parameter, or pass a literal or comptime-known value"
                        );
                        self.errors.push(enhanced);
                    }
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
                } else if arg_ty != *param_ty
                    && !param_ty.is_type_var()
                    && !Self::types_compatible_with_typevars(&arg_ty, param_ty)
                {
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

    // ------------------------------------------------------------------
    // Method call dispatch
    // ------------------------------------------------------------------

    /// Resolve a builtin method name on a given type to the corresponding
    /// free function name. Returns `None` if not a known builtin method.
    fn resolve_builtin_method(ty: &Ty, method: &str) -> Option<String> {
        match (ty, method) {
            // List methods
            (Ty::List(_), "length") => Some("list_length".into()),
            (Ty::List(_), "get") => Some("list_get".into()),
            (Ty::List(_), "push") => Some("list_push".into()),
            (Ty::List(_), "is_empty") => Some("list_is_empty".into()),
            (Ty::List(_), "head") => Some("list_head".into()),
            (Ty::List(_), "tail") => Some("list_tail".into()),
            (Ty::List(_), "concat") => Some("list_concat".into()),
            (Ty::List(_), "contains") => Some("list_contains".into()),
            // String methods
            (Ty::String, "length") => Some("string_length".into()),
            (Ty::String, "contains") => Some("string_contains".into()),
            (Ty::String, "starts_with") => Some("string_starts_with".into()),
            (Ty::String, "ends_with") => Some("string_ends_with".into()),
            (Ty::String, "trim") => Some("string_trim".into()),
            (Ty::String, "to_upper") => Some("string_to_upper".into()),
            (Ty::String, "to_lower") => Some("string_to_lower".into()),
            (Ty::String, "replace") => Some("string_replace".into()),
            (Ty::String, "index_of") => Some("string_index_of".into()),
            (Ty::String, "char_at") => Some("string_char_at".into()),
            (Ty::String, "substring") => Some("string_substring".into()),
            (Ty::String, "split") => Some("string_split".into()),
            // Map methods
            (Ty::Map(..), "set") => Some("map_set".into()),
            (Ty::Map(..), "get") => Some("map_get".into()),
            (Ty::Map(..), "contains") => Some("map_contains".into()),
            (Ty::Map(..), "remove") => Some("map_remove".into()),
            (Ty::Map(..), "size") => Some("map_size".into()),
            (Ty::Map(..), "keys") => Some("map_keys".into()),
            _ => None,
        }
    }

    /// Resolve a user-defined method for a given type using naming conventions.
    ///
    /// Tries the following naming patterns in order:
    /// 1. `Type_method` - for type-specific methods (e.g., `String_trim`)
    /// 2. `method` - for generic methods available on multiple types
    ///
    /// For generic types like `Vec[T]`, looks up `Vec_method`.
    fn resolve_user_defined_method(&self, ty: &Ty, method: &str) -> Option<String> {
        // Get the base type name for naming
        let type_name = self.type_name_for_method(ty);

        // Try Type_method naming convention first (e.g., String_trim)
        let type_prefixed = format!("{}_{}", type_name, method);
        if self.env.lookup_fn(&type_prefixed).is_some() {
            return Some(type_prefixed);
        }

        // Try just the method name (e.g., trim) for generic methods
        if self.env.lookup_fn(method).is_some() {
            return Some(method.to_string());
        }

        None
    }

    /// Get the type name for method resolution.
    /// For generic types, returns the base constructor name (e.g., "Vec" for Vec[T]).
    fn type_name_for_method(&self, ty: &Ty) -> String {
        match ty {
            Ty::Int => "Int".to_string(),
            Ty::Float => "Float".to_string(),
            Ty::String => "String".to_string(),
            Ty::Bool => "Bool".to_string(),
            Ty::Unit => "Unit".to_string(),
            Ty::List(_) => "List".to_string(),
            Ty::Map(_, _) => "Map".to_string(),
            Ty::HashMap(_, _) => "HashMap".to_string(),
            Ty::TypeVar(name) => name.clone(),
            Ty::Enum { name, .. } => name.clone(),
            Ty::Struct { name, .. } => name.clone(),
            Ty::Actor { name } => name.clone(),
            Ty::Iterator(_) => "Iterator".to_string(),
            Ty::Set(_) => "Set".to_string(),
            Ty::Queue(_) => "Queue".to_string(),
            Ty::Stack(_) => "Stack".to_string(),
            Ty::StringBuilder => "StringBuilder".to_string(),
            Ty::Range => "Range".to_string(),
            Ty::Linear(inner) => self.type_name_for_method(inner),
            Ty::GenRef { inner, .. } => self.type_name_for_method(inner),
            Ty::Tuple(_) => "Tuple".to_string(),
            Ty::Fn { .. } => "Fn".to_string(),
            Ty::Type => "Type".to_string(),
            Ty::Error => "Error".to_string(),
        }
    }

    /// Resolve a trait method for a given type. Returns the qualified function
    /// name (e.g. `Int::display`) and the trait method signature if found.
    fn resolve_trait_method(
        &self,
        ty: &Ty,
        method: &str,
    ) -> Option<(String, super::env::TraitMethodSig)> {
        let type_name = match ty {
            Ty::Int => "Int",
            Ty::Float => "Float",
            Ty::String => "String",
            Ty::Bool => "Bool",
            Ty::Unit => "Unit",
            Ty::List(_) => "List",
            Ty::Map(..) => "Map",
            Ty::Enum { name, .. } => name.as_str(),
            _ => return None,
        };
        for impl_info in self.env.all_impls() {
            if impl_info.target_type == type_name {
                if let Some(trait_info) = self.env.lookup_trait(&impl_info.trait_name) {
                    for m in &trait_info.methods {
                        if m.name == method {
                            let qualified = format!("{}::{}", type_name, method);
                            return Some((qualified, m.clone()));
                        }
                    }
                }
            }
        }
        None
    }

    /// Type-check a method call `object.method(args)`.
    ///
    /// Returns `Some(Ty)` if the method could be resolved (either as a builtin
    /// method or a trait method), or `None` to let the caller fall through to
    /// other resolution strategies.
    fn check_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<Ty> {
        let obj_ty = self.check_expr(object);
        if obj_ty.is_error() {
            // Still check args so we report errors in them.
            for arg in args {
                let _ = self.check_expr(arg);
            }
            return Some(Ty::Error);
        }

        // 1. Try builtin method resolution.
        if let Some(func_name) = Self::resolve_builtin_method(&obj_ty, method) {
            // Build a synthetic argument list: [object, args...]
            let mut full_args = vec![object.clone()];
            full_args.extend_from_slice(args);
            // Use the existing list/string/map builtin checking which handles
            // generic element types properly.
            if let Some(ty) = self.check_list_builtin(&func_name, &full_args, span) {
                return Some(ty);
            }
            if let Some(ty) = self.check_map_builtin(&func_name, &full_args, span) {
                return Some(ty);
            }
            // Not a list/map builtin (must be a string builtin); look up as a
            // normal function.
            if let Some(sig) = self.env.lookup_fn(&func_name).cloned() {
                return Some(self.check_method_call_with_sig(&func_name, &sig, object, args, span));
            }
        }

        // 2. Try user-defined method resolution with naming convention `Type_method`.
        if let Some(func_name) = self.resolve_user_defined_method(&obj_ty, method) {
            if let Some(sig) = self.env.lookup_fn(&func_name).cloned() {
                return Some(self.check_method_call_with_sig(&func_name, &sig, object, args, span));
            }
        }

        // 3. Try trait method resolution.
        if let Some((qualified_name, trait_method)) = self.resolve_trait_method(&obj_ty, method) {
            // The trait method signature excludes `self`. Build a FnSig with
            // self prepended so we can use check_call_with_sig.
            let mut params = vec![("self".into(), obj_ty.clone(), false)];
            params.extend(trait_method.params.iter().cloned());
            let sig = FnSig {
                type_params: vec![],
                params,
                ret: trait_method.ret.clone(),
                effects: trait_method.effects.clone(),
            };
            // Check against the full sig with object as first arg.
            let mut full_args = vec![object.clone()];
            full_args.extend_from_slice(args);
            return Some(self.check_call_with_sig(&qualified_name, &sig, &full_args, span));
        }

        // 4. No method found — report an error.
        let type_desc = format!("{}", obj_ty);
        // Still check args so we report errors in them.
        for arg in args {
            let _ = self.check_expr(arg);
        }
        self.errors.push(TypeError::new(
            format!("type `{}` has no method `{}`", type_desc, method),
            span,
        ));
        Some(Ty::Error)
    }

    /// Check a method call against a known function signature, where the
    /// object expression has already been type-checked.
    ///
    /// This avoids double-checking the object: it verifies the object type
    /// matches the first parameter, then checks the remaining arguments.
    fn check_method_call_with_sig(
        &mut self,
        display_name: &str,
        sig: &FnSig,
        object: &Expr,
        args: &[Expr],
        span: Span,
    ) -> Ty {
        // The full call has (1 + args.len()) arguments.
        let total_args = 1 + args.len();
        if total_args != sig.params.len() {
            self.errors.push(TypeError::new(
                format!(
                    "method `{}` expects {} argument(s), but {} were provided",
                    display_name,
                    sig.params.len() - 1, // exclude self
                    args.len()
                ),
                span,
            ));
            return Ty::Error;
        }

        // Build the full argument list and delegate.
        let mut full_args = vec![object.clone()];
        full_args.extend_from_slice(args);
        self.check_call_with_sig(display_name, sig, &full_args, span)
    }

    /// Type-check a list builtin function call. Returns `Some(Ty)` if the
    /// call is a recognized list builtin, `None` otherwise so the caller
    /// can fall through to normal function resolution.
    fn check_list_builtin(&mut self, name: &str, args: &[Expr], span: Span) -> Option<Ty> {
        match name {
            "list_length" => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_length` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let arg_ty = self.check_expr(first_arg);
                if arg_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(arg_ty, Ty::List(_)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `list_length`: expected a List type, found `{}`",
                            arg_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Int)
            }
            "list_get" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_get` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let idx_ty = self.check_expr(&args[1]);
                if list_ty.is_error() || idx_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !idx_ty.is_error() && idx_ty != Ty::Int {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "argument 2 of `list_get`: expected `Int`, found `{}`",
                            idx_ty
                        ),
                        args[1].span,
                        Ty::Int,
                        idx_ty,
                    ));
                }
                match list_ty {
                    Ty::List(elem) => Some(*elem),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_get`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_push" => {
                self.require_heap_effect("list_push", span);
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_push` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let elem_ty = self.check_expr(&args[1]);
                if list_ty.is_error() || elem_ty.is_error() {
                    return Some(Ty::Error);
                }
                match &list_ty {
                    Ty::List(expected_elem) => {
                        if !elem_ty.is_error() && elem_ty != **expected_elem {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `list_push`: expected `{}`, found `{}`",
                                    expected_elem, elem_ty
                                ),
                                args[1].span,
                                *expected_elem.clone(),
                                elem_ty,
                            ));
                        }
                        Some(list_ty.clone())
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_push`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_concat" => {
                self.require_heap_effect("list_concat", span);
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_concat` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let ty_a = self.check_expr(first_arg);
                let ty_b = self.check_expr(&args[1]);
                if ty_a.is_error() || ty_b.is_error() {
                    return Some(Ty::Error);
                }
                match (&ty_a, &ty_b) {
                    (Ty::List(_), Ty::List(_)) => {
                        if ty_a != ty_b {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "list_concat: both arguments must have the same list type, found `{}` and `{}`",
                                    ty_a, ty_b
                                ),
                                span,
                                ty_a.clone(),
                                ty_b,
                            ));
                        }
                        Some(ty_a)
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "list_concat: expected two List arguments, found `{}` and `{}`",
                                ty_a, ty_b
                            ),
                            span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_is_empty" => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_is_empty` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let arg_ty = self.check_expr(first_arg);
                if arg_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(arg_ty, Ty::List(_)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `list_is_empty`: expected a List type, found `{}`",
                            arg_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Bool)
            }
            "list_head" => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_head` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let arg_ty = self.check_expr(first_arg);
                if arg_ty.is_error() {
                    return Some(Ty::Error);
                }
                match arg_ty {
                    Ty::List(elem) => Some(*elem),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_head`: expected a List type, found `{}`",
                                arg_ty
                            ),
                            first_arg.span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_tail" => {
                self.require_heap_effect("list_tail", span);
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_tail` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let arg_ty = self.check_expr(first_arg);
                if arg_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(arg_ty, Ty::List(_)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `list_tail`: expected a List type, found `{}`",
                            arg_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(arg_ty)
            }
            "list_contains" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_contains` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let elem_ty = self.check_expr(&args[1]);
                if list_ty.is_error() || elem_ty.is_error() {
                    return Some(Ty::Error);
                }
                match &list_ty {
                    Ty::List(expected_elem) => {
                        if !elem_ty.is_error() && elem_ty != **expected_elem {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `list_contains`: expected `{}`, found `{}`",
                                    expected_elem, elem_ty
                                ),
                                args[1].span,
                                *expected_elem.clone(),
                                elem_ty,
                            ));
                        }
                        Some(Ty::Bool)
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_contains`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            // ── Higher-order list functions ────────────────────────────────
            "list_map" => {
                self.require_heap_effect("list_map", span);
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_map` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let fn_ty = self.check_expr(&args[1]);
                if list_ty.is_error() || fn_ty.is_error() {
                    return Some(Ty::Error);
                }
                let elem_ty = match &list_ty {
                    Ty::List(elem) => *elem.clone(),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_map`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        return Some(Ty::Error);
                    }
                };
                match &fn_ty {
                    Ty::Fn { params, ret, .. } => {
                        if params.len() != 1 {
                            self.errors.push(TypeError::new(
                                format!("argument 2 of `list_map`: expected a function with 1 parameter, found {} parameters", params.len()),
                                args[1].span,
                            ));
                            return Some(Ty::Error);
                        }
                        if params[0] != elem_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `list_map`: closure parameter type `{}` does not match list element type `{}`",
                                    params[0], elem_ty
                                ),
                                args[1].span,
                                elem_ty,
                                params[0].clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        Some(Ty::List(ret.clone()))
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 2 of `list_map`: expected a function type, found `{}`",
                                fn_ty
                            ),
                            args[1].span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_filter" => {
                self.require_heap_effect("list_filter", span);
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_filter` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let fn_ty = self.check_expr(&args[1]);
                if list_ty.is_error() || fn_ty.is_error() {
                    return Some(Ty::Error);
                }
                let elem_ty = match &list_ty {
                    Ty::List(elem) => *elem.clone(),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_filter`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        return Some(Ty::Error);
                    }
                };
                match &fn_ty {
                    Ty::Fn { params, ret, .. } => {
                        if params.len() != 1 {
                            self.errors.push(TypeError::new(
                                format!("argument 2 of `list_filter`: expected a function with 1 parameter, found {} parameters", params.len()),
                                args[1].span,
                            ));
                            return Some(Ty::Error);
                        }
                        if params[0] != elem_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `list_filter`: closure parameter type `{}` does not match list element type `{}`",
                                    params[0], elem_ty
                                ),
                                args[1].span,
                                elem_ty,
                                params[0].clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        if **ret != Ty::Bool {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `list_filter`: closure must return Bool, found `{}`",
                                    ret
                                ),
                                args[1].span,
                                Ty::Bool,
                                *ret.clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        Some(list_ty)
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 2 of `list_filter`: expected a function type, found `{}`",
                                fn_ty
                            ),
                            args[1].span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_foreach" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_foreach` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let fn_ty = self.check_expr(&args[1]);
                if list_ty.is_error() || fn_ty.is_error() {
                    return Some(Ty::Error);
                }
                let elem_ty = match &list_ty {
                    Ty::List(elem) => *elem.clone(),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_foreach`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        return Some(Ty::Error);
                    }
                };
                match &fn_ty {
                    Ty::Fn { params, .. } => {
                        if params.len() != 1 {
                            self.errors.push(TypeError::new(
                                format!("argument 2 of `list_foreach`: expected a function with 1 parameter, found {} parameters", params.len()),
                                args[1].span,
                            ));
                            return Some(Ty::Error);
                        }
                        if params[0] != elem_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `list_foreach`: closure parameter type `{}` does not match list element type `{}`",
                                    params[0], elem_ty
                                ),
                                args[1].span,
                                elem_ty,
                                params[0].clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        Some(Ty::Unit)
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!("argument 2 of `list_foreach`: expected a function type, found `{}`", fn_ty),
                            args[1].span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_fold" => {
                if args.len() != 3 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_fold` expects 3 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let init_ty = self.check_expr(&args[1]);
                let fn_ty = self.check_expr(&args[2]);
                if list_ty.is_error() || init_ty.is_error() || fn_ty.is_error() {
                    return Some(Ty::Error);
                }
                let elem_ty = match &list_ty {
                    Ty::List(elem) => *elem.clone(),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_fold`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        return Some(Ty::Error);
                    }
                };
                match &fn_ty {
                    Ty::Fn { params, ret, .. } => {
                        if params.len() != 2 {
                            self.errors.push(TypeError::new(
                                format!("argument 3 of `list_fold`: expected a function with 2 parameters, found {} parameters", params.len()),
                                args[2].span,
                            ));
                            return Some(Ty::Error);
                        }
                        if params[0] != init_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 3 of `list_fold`: first closure parameter type `{}` does not match accumulator type `{}`",
                                    params[0], init_ty
                                ),
                                args[2].span,
                                init_ty.clone(),
                                params[0].clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        if params[1] != elem_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 3 of `list_fold`: second closure parameter type `{}` does not match list element type `{}`",
                                    params[1], elem_ty
                                ),
                                args[2].span,
                                elem_ty,
                                params[1].clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        if **ret != init_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 3 of `list_fold`: closure return type `{}` does not match accumulator type `{}`",
                                    ret, init_ty
                                ),
                                args[2].span,
                                init_ty,
                                *ret.clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        Some(*ret.clone())
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 3 of `list_fold`: expected a function type, found `{}`",
                                fn_ty
                            ),
                            args[2].span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_any" | "list_all" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `{}` expects 2 argument(s), but {} were provided",
                            name,
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let fn_ty = self.check_expr(&args[1]);
                if list_ty.is_error() || fn_ty.is_error() {
                    return Some(Ty::Error);
                }
                let elem_ty = match &list_ty {
                    Ty::List(elem) => *elem.clone(),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `{}`: expected a List type, found `{}`",
                                name, list_ty
                            ),
                            first_arg.span,
                        ));
                        return Some(Ty::Error);
                    }
                };
                match &fn_ty {
                    Ty::Fn { params, ret, .. } => {
                        if params.len() != 1 {
                            self.errors.push(TypeError::new(
                                format!("argument 2 of `{}`: expected a function with 1 parameter, found {} parameters", name, params.len()),
                                args[1].span,
                            ));
                            return Some(Ty::Error);
                        }
                        if params[0] != elem_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `{}`: closure parameter type `{}` does not match list element type `{}`",
                                    name, params[0], elem_ty
                                ),
                                args[1].span,
                                elem_ty,
                                params[0].clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        if **ret != Ty::Bool {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `{}`: closure must return Bool, found `{}`",
                                    name, ret
                                ),
                                args[1].span,
                                Ty::Bool,
                                *ret.clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        Some(Ty::Bool)
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 2 of `{}`: expected a function type, found `{}`",
                                name, fn_ty
                            ),
                            args[1].span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_find" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_find` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                let fn_ty = self.check_expr(&args[1]);
                if list_ty.is_error() || fn_ty.is_error() {
                    return Some(Ty::Error);
                }
                let elem_ty = match &list_ty {
                    Ty::List(elem) => *elem.clone(),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_find`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        return Some(Ty::Error);
                    }
                };
                match &fn_ty {
                    Ty::Fn { params, ret, .. } => {
                        if params.len() != 1 {
                            self.errors.push(TypeError::new(
                                format!("argument 2 of `list_find`: expected a function with 1 parameter, found {} parameters", params.len()),
                                args[1].span,
                            ));
                            return Some(Ty::Error);
                        }
                        if params[0] != elem_ty {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `list_find`: closure parameter type `{}` does not match list element type `{}`",
                                    params[0], elem_ty
                                ),
                                args[1].span,
                                elem_ty.clone(),
                                params[0].clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        if **ret != Ty::Bool {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "argument 2 of `list_find`: closure must return Bool, found `{}`",
                                    ret
                                ),
                                args[1].span,
                                Ty::Bool,
                                *ret.clone(),
                            ));
                            return Some(Ty::Error);
                        }
                        Some(elem_ty)
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 2 of `list_find`: expected a function type, found `{}`",
                                fn_ty
                            ),
                            args[1].span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_sort" => {
                self.require_heap_effect("list_sort", span);
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_sort` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                if list_ty.is_error() {
                    return Some(Ty::Error);
                }
                match &list_ty {
                    Ty::List(elem) if **elem == Ty::Int => Some(list_ty),
                    Ty::List(elem) => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_sort`: expected List[Int], found List[{}]",
                                elem
                            ),
                            first_arg.span,
                        ));
                        Some(Ty::Error)
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `list_sort`: expected a List type, found `{}`",
                                list_ty
                            ),
                            first_arg.span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "list_reverse" => {
                self.require_heap_effect("list_reverse", span);
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `list_reverse` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let list_ty = self.check_expr(first_arg);
                if list_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(list_ty, Ty::List(_)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `list_reverse`: expected a List type, found `{}`",
                            list_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(list_ty)
            }
            _ => None,
        }
    }

    /// Type-check a map builtin function call.  Returns `Some(Ty)` if the
    /// call is a recognised map builtin, `None` otherwise.
    fn check_map_builtin(&mut self, name: &str, args: &[Expr], span: Span) -> Option<Ty> {
        // The generic Option enum type (TypeVar-based), consistent with the
        // registered "Option" enum in the environment.
        let option_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![
                ("Some".into(), Some(Ty::TypeVar("T".into()))),
                ("None".into(), None),
            ],
        };

        match name {
            "map_new" => {
                self.require_heap_effect("map_new", span);
                // map_new() -> Map[String, V]
                // Returns a type-variable-based map so that it is compatible
                // with any Map[String, _] annotation.  The annotation type is
                // used by `check_let` and wins over this inferred type.
                if !args.is_empty() {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `map_new` expects 0 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Map(
                    Box::new(Ty::String),
                    Box::new(Ty::TypeVar("V".into())),
                ))
            }
            "map_set" => {
                self.require_heap_effect("map_set", span);
                if args.len() != 3 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `map_set` expects 3 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let map_ty = self.check_expr(first_arg);
                let key_ty = self.check_expr(&args[1]);
                let val_ty = self.check_expr(&args[2]);
                if map_ty.is_error() || key_ty.is_error() || val_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(map_ty, Ty::Map(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `map_set`: expected a Map type, found `{}`",
                            map_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                if key_ty != Ty::String {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "argument 2 of `map_set`: expected `String`, found `{}`",
                            key_ty
                        ),
                        args[1].span,
                        Ty::String,
                        key_ty,
                    ));
                }
                Some(map_ty)
            }
            "map_get" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `map_get` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let map_ty = self.check_expr(first_arg);
                let key_ty = self.check_expr(&args[1]);
                if map_ty.is_error() || key_ty.is_error() {
                    return Some(Ty::Error);
                }
                if key_ty != Ty::String {
                    self.errors.push(TypeError::mismatch(
                        format!(
                            "argument 2 of `map_get`: expected `String`, found `{}`",
                            key_ty
                        ),
                        args[1].span,
                        Ty::String,
                        key_ty,
                    ));
                }
                match map_ty {
                    Ty::Map(..) => {
                        // Return the generic Option enum type, consistent with how
                        // Option[T] annotations are resolved from the environment.
                        Some(option_ty)
                    }
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `map_get`: expected a Map type, found `{}`",
                                map_ty
                            ),
                            first_arg.span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            "map_contains" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `map_contains` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let map_ty = self.check_expr(first_arg);
                let key_ty = self.check_expr(&args[1]);
                if map_ty.is_error() || key_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(map_ty, Ty::Map(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `map_contains`: expected a Map type, found `{}`",
                            map_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Bool)
            }
            "map_remove" => {
                self.require_heap_effect("map_remove", span);
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `map_remove` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let map_ty = self.check_expr(first_arg);
                let key_ty = self.check_expr(&args[1]);
                if map_ty.is_error() || key_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(map_ty, Ty::Map(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `map_remove`: expected a Map type, found `{}`",
                            map_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(map_ty)
            }
            "map_size" => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `map_size` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let map_ty = self.check_expr(first_arg);
                if map_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(map_ty, Ty::Map(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `map_size`: expected a Map type, found `{}`",
                            map_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Int)
            }
            "map_keys" => {
                self.require_heap_effect("map_keys", span);
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `map_keys` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let map_ty = self.check_expr(first_arg);
                if map_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(map_ty, Ty::Map(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `map_keys`: expected a Map type, found `{}`",
                            map_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::List(Box::new(Ty::String)))
            }

            // ── Phase PP: Set operations ────────────────────────────────────────
            "set_new" => {
                self.require_heap_effect("set_new", span);
                // set_new() -> Set[T]
                // Returns a type-variable-based set so that it is compatible
                // with any Set[_] annotation. The annotation type is
                // used by `check_let` and wins over this inferred type.
                if !args.is_empty() {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `set_new` expects 0 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Set(Box::new(Ty::TypeVar("T".into()))))
            }
            "set_add" => {
                self.require_heap_effect("set_add", span);
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `set_add` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let set_ty = self.check_expr(first_arg);
                let elem_ty = self.check_expr(&args[1]);
                if set_ty.is_error() || elem_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(set_ty, Ty::Set(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `set_add`: expected a Set type, found `{}`",
                            set_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                // Return the same set type
                Some(set_ty)
            }
            "set_remove" => {
                self.require_heap_effect("set_remove", span);
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `set_remove` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let set_ty = self.check_expr(first_arg);
                let elem_ty = self.check_expr(&args[1]);
                if set_ty.is_error() || elem_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(set_ty, Ty::Set(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `set_remove`: expected a Set type, found `{}`",
                            set_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(set_ty)
            }
            "set_contains" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `set_contains` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let set_ty = self.check_expr(first_arg);
                let elem_ty = self.check_expr(&args[1]);
                if set_ty.is_error() || elem_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(set_ty, Ty::Set(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `set_contains`: expected a Set type, found `{}`",
                            set_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Bool)
            }
            "set_size" => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `set_size` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let set_ty = self.check_expr(first_arg);
                if set_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(set_ty, Ty::Set(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `set_size`: expected a Set type, found `{}`",
                            set_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Int)
            }
            "set_union" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `set_union` expects 2 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let set_a_ty = self.check_expr(first_arg);
                let set_b_ty = self.check_expr(&args[1]);
                if set_a_ty.is_error() || set_b_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(set_a_ty, Ty::Set(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `set_union`: expected a Set type, found `{}`",
                            set_a_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                if !matches!(set_b_ty, Ty::Set(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 2 of `set_union`: expected a Set type, found `{}`",
                            set_b_ty
                        ),
                        args[1].span,
                    ));
                    return Some(Ty::Error);
                }
                // Return the first set's type (they should be compatible)
                Some(set_a_ty)
            }
            "set_intersection" => {
                if args.len() != 2 {
                    self.errors.push(TypeError::new(
                        format!("function `set_intersection` expects 2 argument(s), but {} were provided", args.len()),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let set_a_ty = self.check_expr(first_arg);
                let set_b_ty = self.check_expr(&args[1]);
                if set_a_ty.is_error() || set_b_ty.is_error() {
                    return Some(Ty::Error);
                }
                if !matches!(set_a_ty, Ty::Set(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 1 of `set_intersection`: expected a Set type, found `{}`",
                            set_a_ty
                        ),
                        first_arg.span,
                    ));
                    return Some(Ty::Error);
                }
                if !matches!(set_b_ty, Ty::Set(..)) {
                    self.errors.push(TypeError::new(
                        format!(
                            "argument 2 of `set_intersection`: expected a Set type, found `{}`",
                            set_b_ty
                        ),
                        args[1].span,
                    ));
                    return Some(Ty::Error);
                }
                Some(set_a_ty)
            }
            "set_to_list" => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        format!(
                            "function `set_to_list` expects 1 argument(s), but {} were provided",
                            args.len()
                        ),
                        span,
                    ));
                    return Some(Ty::Error);
                }
                let Some(first_arg) = args.first() else {
                    return Some(Ty::Error);
                };
                let set_ty = self.check_expr(first_arg);
                if set_ty.is_error() {
                    return Some(Ty::Error);
                }
                match &set_ty {
                    Ty::Set(elem_ty) => Some(Ty::List(elem_ty.clone())),
                    _ => {
                        self.errors.push(TypeError::new(
                            format!(
                                "argument 1 of `set_to_list`: expected a Set type, found `{}`",
                                set_ty
                            ),
                            first_arg.span,
                        ));
                        Some(Ty::Error)
                    }
                }
            }
            _ => None,
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
    fn check_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], span: Span) -> Ty {
        let scrutinee_ty = self.check_expr(scrutinee);

        if arms.is_empty() {
            self.errors.push(TypeError::new(
                "match expression has no arms".to_string(),
                span,
            ));
            return Ty::Error;
        }

        let mut has_wildcard = false;
        let mut wildcard_index: Option<usize> = None;
        let mut first_arm_ty: Option<Ty> = None;
        let mut matched_variants: Vec<String> = Vec::new();
        let mut matched_bool_true = false;
        let mut matched_bool_false = false;

        for (arm_idx, arm) in arms.iter().enumerate() {
            let _ = arm_idx; // used by exhaustiveness checks below
                             // Track whether we pushed a scope for this arm (for cleanup).
            let mut pushed_scope = false;

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
                    Pattern::BoolLit(val) => {
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
                        if *val {
                            // Check for duplicate true pattern.
                            if matched_bool_true {
                                self.errors.push(TypeError::warning(
                                    "unreachable pattern: `true` has already been matched"
                                        .to_string(),
                                    arm.span,
                                ));
                            }
                            matched_bool_true = true;
                        } else {
                            // Check for duplicate false pattern.
                            if matched_bool_false {
                                self.errors.push(TypeError::warning(
                                    "unreachable pattern: `false` has already been matched"
                                        .to_string(),
                                    arm.span,
                                ));
                            }
                            matched_bool_false = true;
                        }
                    }
                    Pattern::Wildcard => {
                        if !has_wildcard {
                            wildcard_index = Some(arm_idx);
                        }
                        has_wildcard = true;
                    }
                    Pattern::Tuple(_) => {
                        // Tuple patterns are not supported in match arms (only in let bindings).
                        self.errors.push(TypeError::new(
                            "tuple patterns are only supported in `let` destructuring, not in `match` arms".to_string(),
                            arm.span,
                        ));
                    }
                    Pattern::StringLit(_) => {
                        if scrutinee_ty != Ty::String {
                            self.errors.push(TypeError::mismatch(
                                format!(
                                    "string pattern cannot match scrutinee of type `{}`",
                                    scrutinee_ty
                                ),
                                arm.span,
                                Ty::String,
                                scrutinee_ty.clone(),
                            ));
                        }
                    }
                    Pattern::Variable(var_name) => {
                        // Variable patterns match any scrutinee type.
                        // A variable without a guard is essentially a wildcard.
                        if arm.guard.is_none() {
                            has_wildcard = true;
                        }
                        // Push a scope and bind the variable to the scrutinee's type.
                        self.env.push_scope();
                        self.env.define(var_name.clone(), scrutinee_ty.clone());
                        pushed_scope = true;
                    }
                    Pattern::Variant { variant, bindings } => {
                        // Check for duplicate variant patterns.
                        if matched_variants.contains(variant) {
                            self.errors.push(TypeError::warning(
                                format!(
                                    "unreachable pattern: variant `{}` has already been matched",
                                    variant
                                ),
                                arm.span,
                            ));
                        }
                        matched_variants.push(variant.clone());

                        // Check that the variant belongs to the scrutinee's enum type.
                        if let Ty::Enum {
                            name: enum_name,
                            variants,
                        } = &scrutinee_ty
                        {
                            if let Some((_, field_ty)) =
                                variants.iter().find(|(vn, _)| vn == variant)
                            {
                                if bindings.is_empty() {
                                    // Unit variant match — no bindings needed, OK.
                                } else {
                                    match field_ty {
                                        None => {
                                            self.errors.push(TypeError::new(
                                                format!(
                                                    "variant `{}` of `{}` is a unit variant and cannot have bindings",
                                                    variant, enum_name
                                                ),
                                                arm.span,
                                            ));
                                        }
                                        Some(Ty::Tuple(field_types)) => {
                                            // Multi-field variant: bind each name to its type.
                                            if bindings.len() != field_types.len() {
                                                self.errors.push(TypeError::new(
                                                    format!(
                                                        "variant `{}` has {} fields but {} bindings were given",
                                                        variant, field_types.len(), bindings.len()
                                                    ),
                                                    arm.span,
                                                ));
                                            } else {
                                                self.env.push_scope();
                                                for (bname, fty) in
                                                    bindings.iter().zip(field_types.iter())
                                                {
                                                    self.env.define(bname.clone(), fty.clone());
                                                }
                                                pushed_scope = true;
                                            }
                                        }
                                        Some(fty) => {
                                            // Single-field variant.
                                            if bindings.len() == 1 {
                                                self.env.push_scope();
                                                self.env.define(bindings[0].clone(), fty.clone());
                                                pushed_scope = true;
                                            } else {
                                                self.errors.push(TypeError::new(
                                                    format!(
                                                        "variant `{}` has 1 field but {} bindings were given",
                                                        variant, bindings.len()
                                                    ),
                                                    arm.span,
                                                ));
                                            }
                                        }
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
                    Pattern::Or(alternatives) => {
                        // Check each alternative pattern against the scrutinee type
                        for alt in alternatives {
                            // For now, just check that the pattern is a variant of the same enum
                            // In a full implementation, we'd need to verify all alternatives are
                            // of compatible types
                            if let Pattern::Variant { variant, .. } = alt {
                                if let Ty::Enum {
                                    name: enum_name,
                                    variants,
                                } = &scrutinee_ty
                                {
                                    if !variants.iter().any(|(vn, _)| vn == variant) {
                                        self.errors.push(TypeError::new(
                                            format!(
                                                "variant `{}` is not a member of enum `{}`",
                                                variant, enum_name
                                            ),
                                            arm.span,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // If the pattern is a variable and we haven't pushed a scope yet
            // (because scrutinee was error), we still need a scope for the
            // guard and body to avoid name resolution failures.
            if matches!(&arm.pattern, Pattern::Variable(_)) && !pushed_scope {
                self.env.push_scope();
                pushed_scope = true;
            }

            // Check the guard expression if present — it must be Bool.
            if let Some(ref guard_expr) = arm.guard {
                let guard_ty = self.check_expr(guard_expr);
                if !guard_ty.is_error() && guard_ty != Ty::Bool {
                    self.errors.push(TypeError::mismatch(
                        "match guard must be a `Bool` expression".to_string(),
                        guard_expr.span,
                        Ty::Bool,
                        guard_ty,
                    ));
                }
            }

            // Check the arm body.
            let arm_ty = self.check_block(&arm.body);

            // Pop scope if we pushed one.
            if pushed_scope {
                self.env.pop_scope();
            } else if let Pattern::Variant { bindings, .. } = &arm.pattern {
                if !bindings.is_empty() {
                    // Legacy path: variant binding scope pop for error-type scrutinee
                    // (no scope was pushed, so nothing to pop).
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

        // Unreachable arm detection: if a wildcard appears before the last arm,
        // all subsequent arms are unreachable.
        if let Some(wi) = wildcard_index {
            if wi < arms.len() - 1 {
                for arm in &arms[wi + 1..] {
                    self.errors.push(
                        TypeError::warning(
                            "unreachable pattern: wildcard `_` above already matches all values"
                                .to_string(),
                            arm.span,
                        )
                        .with_note("move this arm before the wildcard or remove it".to_string()),
                    );
                }
            }
        }

        // Exhaustiveness checking.
        if !has_wildcard && !scrutinee_ty.is_error() {
            if let Ty::Enum {
                name: enum_name,
                variants,
            } = &scrutinee_ty
            {
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
                        .with_note(
                            "add the missing variant arms or a wildcard `_` arm".to_string(),
                        ),
                    );
                }
            } else if matches!(scrutinee_ty, Ty::Bool) {
                // For Bool scrutinee: check both true and false are covered.
                if !matched_bool_true || !matched_bool_false {
                    let missing = if !matched_bool_true && !matched_bool_false {
                        "true, false"
                    } else if !matched_bool_true {
                        "true"
                    } else {
                        "false"
                    };
                    self.errors.push(
                        TypeError::new(
                            format!("non-exhaustive match on `Bool`: missing {}", missing),
                            span,
                        )
                        .with_note("add the missing arms or a wildcard `_` arm".to_string()),
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
    /// Infers type variable bindings from argument types and expected return type,
    /// substitutes them into the return type and parameter types, and then checks
    /// that all arguments match the specialized signature.
    fn check_generic_call(
        &mut self,
        display_name: &str,
        sig: &FnSig,
        args: &[Expr],
        span: Span,
        expected_ret: Option<&Ty>,
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
        let mut bindings: std::collections::HashMap<String, Ty> = std::collections::HashMap::new();

        for ((_, param_ty, _comptime), arg_ty) in sig.params.iter().zip(arg_tys.iter()) {
            if arg_ty.is_error() {
                continue;
            }
            Self::unify_types(param_ty, arg_ty, &mut bindings);
        }

        // Bidirectional type inference: if there's an expected return type,
        // unify it with the function's return type to infer more type parameters.
        if let Some(expected) = expected_ret {
            if !expected.is_error() {
                Self::unify_types(&sig.ret, expected, &mut bindings);
            }
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
        for (i, ((param_name, param_ty, _comptime), (arg, arg_ty))) in sig
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
            Ty::List(p_elem) => {
                if let Ty::List(a_elem) = arg_ty {
                    Self::unify_types(p_elem, a_elem, bindings);
                }
            }
            Ty::Map(p_k, p_v) => {
                if let Ty::Map(a_k, a_v) = arg_ty {
                    Self::unify_types(p_k, a_k, bindings);
                    Self::unify_types(p_v, a_v, bindings);
                }
            }
            Ty::HashMap(p_k, p_v) => {
                if let Ty::HashMap(a_k, a_v) = arg_ty {
                    Self::unify_types(p_k, a_k, bindings);
                    Self::unify_types(p_v, a_v, bindings);
                }
            }
            Ty::Iterator(p_elem) => {
                if let Ty::Iterator(a_elem) = arg_ty {
                    Self::unify_types(p_elem, a_elem, bindings);
                }
            }
            Ty::Enum {
                name: p_name,
                variants: p_vars,
            } => {
                if let Ty::Enum {
                    name: a_name,
                    variants: a_vars,
                } = arg_ty
                {
                    if p_name == a_name && p_vars.len() == a_vars.len() {
                        for ((_, p_payload), (_, a_payload)) in p_vars.iter().zip(a_vars.iter()) {
                            if let (Some(pt), Some(at)) = (p_payload, a_payload) {
                                Self::unify_types(pt, at, bindings);
                            }
                        }
                    }
                }
            }
            _ => {
                // Concrete types: no unification needed.
            }
        }
    }

    /// Check whether `value_ty` is structurally compatible with `ann_ty`,
    /// treating any `TypeVar` appearing in `value_ty` as a wildcard that can
    /// matching any corresponding type in `ann_ty`.  This allows generic
    /// builtins like `map_new()` (which returns `Map[String, TypeVar("V")]`)
    /// to be assigned to an annotated binding like `Map[String, Int]` without
    /// triggering a spurious type error.
    /// Recursively substitute type variables in `ty` using `subst`.
    fn substitute_type_vars(ty: &Ty, subst: &std::collections::HashMap<String, Ty>) -> Ty {
        match ty {
            Ty::TypeVar(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Ty::List(elem) => Ty::List(Box::new(Self::substitute_type_vars(elem, subst))),
            Ty::Map(k, v) => Ty::Map(
                Box::new(Self::substitute_type_vars(k, subst)),
                Box::new(Self::substitute_type_vars(v, subst)),
            ),
            Ty::Tuple(elems) => Ty::Tuple(
                elems
                    .iter()
                    .map(|e| Self::substitute_type_vars(e, subst))
                    .collect(),
            ),
            Ty::Enum { name, variants } => Ty::Enum {
                name: name.clone(),
                variants: variants
                    .iter()
                    .map(|(vn, vt)| {
                        (
                            vn.clone(),
                            vt.as_ref().map(|t| Self::substitute_type_vars(t, subst)),
                        )
                    })
                    .collect(),
            },
            Ty::Fn {
                params,
                ret,
                effects,
            } => Ty::Fn {
                params: params
                    .iter()
                    .map(|p| Self::substitute_type_vars(p, subst))
                    .collect(),
                ret: Box::new(Self::substitute_type_vars(ret, subst)),
                effects: effects.clone(),
            },
            Ty::Set(elem) => Ty::Set(Box::new(Self::substitute_type_vars(elem, subst))),
            Ty::Queue(elem) => Ty::Queue(Box::new(Self::substitute_type_vars(elem, subst))),
            Ty::Stack(elem) => Ty::Stack(Box::new(Self::substitute_type_vars(elem, subst))),
            Ty::HashMap(k, v) => Ty::HashMap(
                Box::new(Self::substitute_type_vars(k, subst)),
                Box::new(Self::substitute_type_vars(v, subst)),
            ),
            Ty::Iterator(elem) => Ty::Iterator(Box::new(Self::substitute_type_vars(elem, subst))),
            Ty::GenRef { inner, cap } => Ty::GenRef {
                inner: Box::new(Self::substitute_type_vars(inner, subst)),
                cap: *cap,
            },
            _ => ty.clone(),
        }
    }

    fn types_compatible_with_typevars(value_ty: &Ty, ann_ty: &Ty) -> bool {
        match (value_ty, ann_ty) {
            // A TypeVar in value position is a wildcard — always compatible.
            (Ty::TypeVar(_), _) => true,
            // Recurse into container types.
            (Ty::List(ve), Ty::List(ae)) => Self::types_compatible_with_typevars(ve, ae),
            (Ty::Map(vk, vv), Ty::Map(ak, av)) => {
                Self::types_compatible_with_typevars(vk, ak)
                    && Self::types_compatible_with_typevars(vv, av)
            }
            (Ty::HashMap(vk, vv), Ty::HashMap(ak, av)) => {
                Self::types_compatible_with_typevars(vk, ak)
                    && Self::types_compatible_with_typevars(vv, av)
            }
            (Ty::Iterator(ve), Ty::Iterator(ae)) => Self::types_compatible_with_typevars(ve, ae),
            (Ty::Set(ve), Ty::Set(ae)) => Self::types_compatible_with_typevars(ve, ae),
            // Enums: same name and compatible variant payloads.
            (
                Ty::Enum {
                    name: vn,
                    variants: vvars,
                },
                Ty::Enum {
                    name: an,
                    variants: avars,
                },
            ) => {
                if vn != an || vvars.len() != avars.len() {
                    return false;
                }
                vvars
                    .iter()
                    .zip(avars.iter())
                    .all(|((vvname, vvt), (avname, avt))| {
                        if vvname != avname {
                            return false;
                        }
                        match (vvt, avt) {
                            (None, None) => true,
                            (Some(vt), Some(at)) => Self::types_compatible_with_typevars(vt, at),
                            _ => false,
                        }
                    })
            }
            // For all other cases, fall back to strict equality.
            _ => value_ty == ann_ty,
        }
    }

    /// Substitute all TypeVar occurrences in a type using the given bindings.
    fn substitute_ty(ty: &Ty, bindings: &std::collections::HashMap<String, Ty>) -> Ty {
        match ty {
            Ty::TypeVar(name) => bindings.get(name).cloned().unwrap_or_else(|| ty.clone()),
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
            Ty::List(elem) => Ty::List(Box::new(Self::substitute_ty(elem, bindings))),
            Ty::Map(k, v) => Ty::Map(
                Box::new(Self::substitute_ty(k, bindings)),
                Box::new(Self::substitute_ty(v, bindings)),
            ),
            Ty::HashMap(k, v) => Ty::HashMap(
                Box::new(Self::substitute_ty(k, bindings)),
                Box::new(Self::substitute_ty(v, bindings)),
            ),
            Ty::Iterator(elem) => Ty::Iterator(Box::new(Self::substitute_ty(elem, bindings))),
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

        // Collect comptime arguments for potential evaluation.
        let mut comptime_args: std::collections::HashMap<String, ComptimeValue> =
            std::collections::HashMap::new();
        let mut has_comptime_params = false;

        // Check each argument type.
        for (i, (arg, (param_name, param_ty, is_comptime))) in
            args.iter().zip(sig.params.iter()).enumerate()
        {
            let arg_ty = self.check_expr(arg);
            if arg_ty.is_error() || param_ty.is_error() {
                continue;
            }

            // Validate comptime parameters.
            if *is_comptime {
                has_comptime_params = true;
                // Check that the argument is comptime-evaluable.
                if let Err(e) = self.require_comptime(arg) {
                    // Enhance the error with comptime parameter context
                    let mut enhanced = TypeError::new(
                        format!(
                            "runtime value passed to comptime parameter `{}`",
                            param_name
                        ),
                        arg.span,
                    );
                    enhanced = enhanced.with_note(format!(
                        "parameter `{}` is marked `comptime` and requires a compile-time known value",
                        param_name
                    ));
                    // Include the original error message as an additional note
                    enhanced = enhanced.with_note(e.message);
                    enhanced = enhanced.with_note(
                        "to fix: either remove `comptime` from the parameter, or pass a literal or comptime-known value"
                    );
                    self.errors.push(enhanced);
                } else {
                    // Try to evaluate the argument to a comptime value.
                    match self.try_eval_comptime(arg) {
                        Ok(value) => {
                            comptime_args.insert(param_name.clone(), value);
                        }
                        Err(_) => {
                            self.errors.push(TypeError::new(
                                format!(
                                    "cannot evaluate argument `{}` at compile time - expected a literal or comptime-known value",
                                    param_name
                                ),
                                arg.span,
                            ));
                        }
                    }
                }
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
            } else if arg_ty != *param_ty
                && !param_ty.is_type_var()
                && !Self::types_compatible_with_typevars(&arg_ty, param_ty)
            {
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

        // Handle comptime type instantiation: if the function returns Ty::Type,
        // evaluate it at compile time and return the resulting type.
        if sig.ret.is_comptime_only() && has_comptime_params {
            if let Some(fn_def) = self.comptime_evaluator.get_function(display_name).cloned() {
                match self.comptime_evaluator.eval_fn(&fn_def, comptime_args) {
                    Ok(ComptimeValue::Type(result_ty)) => return result_ty,
                    Ok(other) => {
                        // Function returned a non-type value - this is an error
                        self.errors.push(TypeError::new(
                            format!(
                                "function `{}` returns `Type` but evaluation produced `{}`",
                                display_name,
                                other.type_name()
                            ),
                            span,
                        ));
                    }
                    Err(e) => {
                        self.errors.push(TypeError::new(
                            format!(
                                "compile-time evaluation of `{}` failed: {}",
                                display_name, e
                            ),
                            span,
                        ));
                    }
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
            TypeExpr::Named { name, cap } => {
                let ty = match name.as_str() {
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
                            is_warning: false,
                        hole_data: None,
                        });
                        Ty::Error
                    }
                };
                // Apply capability annotation if present
                if let Some(cap) = cap {
                    let ref_cap = self.ast_cap_to_ref_cap(cap);
                    ty.with_capability(ref_cap)
                } else {
                    ty
                }
            }
            TypeExpr::Unit => Ty::Unit,
            TypeExpr::Fn {
                params,
                ret,
                effects,
            } => {
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
            TypeExpr::Generic { name, args, cap } => {
                // Handle List[T] type annotations.
                if name == "List" {
                    if args.len() == 1 {
                        let Some(first_arg) = args.first() else {
                            return Ty::Error;
                        };
                        let elem_ty = self.resolve_type_expr(&first_arg.node, first_arg.span);
                        return Ty::List(Box::new(elem_ty));
                    }
                    self.errors.push(TypeError {
                        message: "List type requires exactly one type argument, e.g. List[Int]"
                            .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle Map[K, V] type annotations.
                if name == "Map" {
                    if args.len() == 2 {
                        let Some(first_arg) = args.first() else {
                            return Ty::Error;
                        };
                        let key_ty = self.resolve_type_expr(&first_arg.node, first_arg.span);
                        let val_ty = self.resolve_type_expr(&args[1].node, args[1].span);
                        return Ty::Map(Box::new(key_ty), Box::new(val_ty));
                    }
                    self.errors.push(TypeError {
                        message:
                            "Map type requires exactly two type arguments, e.g. Map[String, Int]"
                                .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle HashMap[K, V] type annotations (Self-Hosting Phase 1.1).
                if name == "HashMap" {
                    if args.len() == 2 {
                        let Some(first_arg) = args.first() else {
                            return Ty::Error;
                        };
                        let key_ty = self.resolve_type_expr(&first_arg.node, first_arg.span);
                        let val_ty = self.resolve_type_expr(&args[1].node, args[1].span);
                        return Ty::HashMap(Box::new(key_ty), Box::new(val_ty));
                    }
                    self.errors.push(TypeError {
                        message: "HashMap type requires exactly two type arguments, e.g. HashMap[String, Int]"
                            .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                    hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle Set[T] type annotations.
                if name == "Set" {
                    if args.len() == 1 {
                        let Some(first_arg) = args.first() else {
                            return Ty::Error;
                        };
                        let elem_ty = self.resolve_type_expr(&first_arg.node, first_arg.span);
                        return Ty::Set(Box::new(elem_ty));
                    }
                    self.errors.push(TypeError {
                        message: "Set type requires exactly one type argument, e.g. Set[Int]"
                            .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle Queue[T] type annotations.
                if name == "Queue" {
                    if args.len() == 1 {
                        let Some(first_arg) = args.first() else {
                            return Ty::Error;
                        };
                        let elem_ty = self.resolve_type_expr(&first_arg.node, first_arg.span);
                        return Ty::Queue(Box::new(elem_ty));
                    }
                    self.errors.push(TypeError {
                        message: "Queue type requires exactly one type argument, e.g. Queue[Int]"
                            .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle Stack[T] type annotations.
                if name == "Stack" {
                    if args.len() == 1 {
                        let Some(first_arg) = args.first() else {
                            return Ty::Error;
                        };
                        let elem_ty = self.resolve_type_expr(&first_arg.node, first_arg.span);
                        return Ty::Stack(Box::new(elem_ty));
                    }
                    self.errors.push(TypeError {
                        message: "Stack type requires exactly one type argument, e.g. Stack[Int]"
                            .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle GenRef[T] type annotations with optional capability.
                if name == "GenRef" {
                    if args.len() == 1 {
                        let Some(first_arg) = args.first() else {
                            return Ty::Error;
                        };
                        let elem_ty = self.resolve_type_expr(&first_arg.node, first_arg.span);
                        let ref_cap = cap
                            .as_ref()
                            .map(|c| self.ast_cap_to_ref_cap(c))
                            .unwrap_or(super::types::RefCap::Ref);
                        return Ty::GenRef {
                            inner: Box::new(elem_ty),
                            cap: ref_cap,
                        };
                    }
                    self.errors.push(TypeError {
                        message: "GenRef type requires exactly one type argument, e.g. GenRef[Int]"
                            .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle Iterator[T] type annotations (Self-Hosting Phase 1.2).
                if name == "Iterator" {
                    if args.len() == 1 {
                        let Some(first_arg) = args.first() else {
                            return Ty::Error;
                        };
                        let elem_ty = self.resolve_type_expr(&first_arg.node, first_arg.span);
                        return Ty::Iterator(Box::new(elem_ty));
                    }
                    self.errors.push(TypeError {
                        message:
                            "Iterator type requires exactly one type argument, e.g. Iterator[Int]"
                                .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle StringBuilder type annotations (Self-Hosting Phase 1.3).
                if name == "StringBuilder" {
                    if args.is_empty() {
                        return Ty::StringBuilder;
                    }
                    self.errors.push(TypeError {
                        message: "StringBuilder type does not take type arguments".to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Handle Actor[Name] type annotations.
                if name == "Actor" {
                    if args.len() == 1 {
                        if let TypeExpr::Named {
                            name: actor_name, ..
                        } = &args.first().expect("args.len() == 1 checked above").node
                        {
                            return Ty::Actor {
                                name: actor_name.clone(),
                            };
                        }
                    }
                    self.errors.push(TypeError {
                        message:
                            "Actor type requires exactly one type argument, e.g. Actor[Counter]"
                                .to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                        hole_data: None,
                    });
                    return Ty::Error;
                }

                // Generic enum instantiation: e.g. Option[Task] -> substitute T=Task in Option variants.
                let actual_args: Vec<Ty> = args
                    .iter()
                    .map(|a| self.resolve_type_expr(&a.node, a.span))
                    .collect();
                if let Some(base_ty) = self.env.lookup_enum(name).cloned() {
                    if let Some(type_param_names) = self.env.lookup_enum_type_params(name).cloned()
                    {
                        let subst: std::collections::HashMap<String, Ty> = type_param_names
                            .iter()
                            .zip(actual_args.iter())
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        return Self::substitute_type_vars(&base_ty, &subst);
                    }
                    return base_ty;
                }
                self.errors.push(TypeError {
                    message: format!("unknown generic type `{}`", name),
                    span,
                    expected: None,
                    found: None,
                    notes: vec![],
                    is_warning: false,
                    hole_data: None,
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
            TypeExpr::Record(fields) => {
                // A record TypeExpr produced by `type Name:` declarations.
                // The wrapping ItemKind::TypeDecl provides the name; here we
                // only have anonymous fields, so we emit a Struct with an
                // empty name. The TypeDecl handler patches the name in.
                let field_tys: Vec<(String, Ty)> = fields
                    .iter()
                    .map(|(fname, fty)| {
                        (fname.clone(), self.resolve_type_expr(&fty.node, fty.span))
                    })
                    .collect();
                Ty::Struct {
                    name: String::new(),
                    fields: field_tys,
                    cap: crate::typechecker::types::RefCap::default_struct(),
                }
            }
            TypeExpr::Linear(inner) => {
                let inner_ty = self.resolve_type_expr(&inner.node, inner.span);
                Ty::Linear(Box::new(inner_ty))
            }
            TypeExpr::Type => Ty::Type,
        }
    }

    // ------------------------------------------------------------------
    // @verified contract discharge surfacing (sub-issue #329)
    // ------------------------------------------------------------------

    /// Translate a [`vc::FunctionDischargeReport`] into checker
    /// diagnostics (errors for `Counterexample`, warnings for
    /// `Unknown`/`Timeout`/`SolverError`, and a single info-level
    /// note when every obligation is `Discharged`).
    ///
    /// Returns `true` if at least one diagnostic was emitted.
    fn surface_discharge_report(
        &mut self,
        fn_def: &FnDef,
        report: &vc::FunctionDischargeReport,
    ) -> bool {
        let mut emitted = false;
        let mut all_discharged = !report.outcomes.is_empty();

        for q in &report.outcomes {
            // Resolve the originating contract span when we have an
            // index — this gives the diagnostic the precise
            // `@requires` / `@ensures` source location instead of the
            // function body span.
            let span = q
                .contract_index
                .and_then(|idx| fn_def.contracts.get(idx))
                .map(|c| c.span)
                .unwrap_or(fn_def.body.span);
            let contract_label = match (q.kind, q.contract_index) {
                (Some(ContractKind::Requires), Some(i)) => {
                    format!("@requires #{i}")
                }
                (Some(ContractKind::Requires), None) => "preconditions".to_string(),
                (Some(ContractKind::Ensures), Some(i)) => {
                    format!("@ensures #{i}")
                }
                (Some(ContractKind::Ensures), None) => "postcondition".to_string(),
                (None, _) => "contract".to_string(),
            };

            match &q.outcome {
                vc::DischargeOutcome::Discharged => {
                    // No diagnostic per discharged obligation —
                    // collect to summarise once below.
                }
                vc::DischargeOutcome::Counterexample { bindings } => {
                    all_discharged = false;
                    let summary = if bindings.is_empty() {
                        "z3 returned a model but the discharger could not parse it".to_string()
                    } else {
                        let parts: Vec<String> = bindings
                            .iter()
                            .map(|b| format!("{} = {}", b.name, b.value))
                            .collect();
                        format!("counterexample: {}", parts.join(", "))
                    };
                    let mut err = TypeError::new(
                        format!(
                            "@verified function `{}` violates {}: {}",
                            fn_def.name, contract_label, summary
                        ),
                        span,
                    );
                    err = err.with_note(format!(
                        "Z3 found inputs that satisfy the preconditions but falsify the postcondition for `{}`",
                        fn_def.name
                    ));
                    self.errors.push(err);
                    emitted = true;
                }
                vc::DischargeOutcome::Unknown => {
                    all_discharged = false;
                    self.errors.push(
                        TypeError::warning(
                            format!(
                                "@verified function `{}`: solver returned `unknown` for {} (cannot prove or refute within the timeout)",
                                fn_def.name, contract_label
                            ),
                            span,
                        )
                        .with_note(
                            "increase the timeout via the discharger config or simplify the contract".to_string(),
                        ),
                    );
                    emitted = true;
                }
                vc::DischargeOutcome::Timeout => {
                    all_discharged = false;
                    self.errors.push(TypeError::warning(
                        format!(
                            "@verified function `{}`: discharger timed out on {}",
                            fn_def.name, contract_label
                        ),
                        span,
                    ));
                    emitted = true;
                }
                vc::DischargeOutcome::SolverError { detail } => {
                    all_discharged = false;
                    self.errors.push(TypeError::warning(
                        format!(
                            "@verified function `{}`: Z3 returned an error on {}: {}",
                            fn_def.name, contract_label, detail
                        ),
                        span,
                    ));
                    emitted = true;
                }
            }
        }

        if all_discharged && !report.outcomes.is_empty() {
            // A single positive-outcome warning so the agent / user
            // gets visible feedback that verification ran. Once #330
            // / #332 land we can downgrade this to a non-diagnostic
            // log line, but the user-visible surface is the contract
            // here, not stdout.
            self.errors.push(TypeError::warning(
                format!(
                    "@verified function `{}`: all {} contract obligation(s) discharged by Z3",
                    fn_def.name,
                    report.outcomes.len()
                ),
                fn_def.body.span,
            ));
            emitted = true;
        }

        emitted
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
            self.errors.push(TypeError::new(
                format!(
                    "@budget on function `{}` must specify at least `cpu` or `mem`",
                    fn_name
                ),
                budget.span,
            ));
        }
    }

    /// Check that a callee's budget fits within the caller's budget.
    ///
    /// If the caller has a budget and the callee also has a budget, every
    /// component of the callee's budget must be <= the corresponding component
    /// of the caller's budget.
    fn check_budget_containment(&mut self, caller: &str, callee: &str, span: Span) {
        let caller_budget = match self.function_budgets.get(caller) {
            Some(b) => b.clone(),
            None => return, // Caller has no budget — no constraint to check.
        };
        let callee_budget = match self.function_budgets.get(callee) {
            Some(b) => b.clone(),
            None => {
                // Caller has a budget but callee does not — warn that the
                // callee is unconstrained.
                self.errors.push(TypeError::new(
                    format!(
                        "function `{}` has a @budget but calls `{}` which has no budget; \
                             inner calls should declare budgets for containment checking",
                        caller, callee
                    ),
                    span,
                ));
                return;
            }
        };

        // Check cpu containment.
        if let (Some(ref caller_cpu), Some(ref callee_cpu)) =
            (&caller_budget.cpu, &callee_budget.cpu)
        {
            if let (Some(caller_ms), Some(callee_ms)) = (
                Self::parse_cpu_millis(caller_cpu),
                Self::parse_cpu_millis(callee_cpu),
            ) {
                if callee_ms > caller_ms {
                    self.errors.push(TypeError::new(
                        format!(
                            "callee `{}` cpu budget `{}` exceeds caller `{}` cpu budget `{}`",
                            callee, callee_cpu, caller, caller_cpu
                        ),
                        span,
                    ));
                }
            }
        }

        // Check mem containment.
        if let (Some(ref caller_mem), Some(ref callee_mem)) =
            (&caller_budget.mem, &callee_budget.mem)
        {
            if let (Some(caller_bytes), Some(callee_bytes)) = (
                Self::parse_mem_bytes(caller_mem),
                Self::parse_mem_bytes(callee_mem),
            ) {
                if callee_bytes > caller_bytes {
                    self.errors.push(TypeError::new(
                        format!(
                            "callee `{}` mem budget `{}` exceeds caller `{}` mem budget `{}`",
                            callee, callee_mem, caller, caller_mem
                        ),
                        span,
                    ));
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
        let tp_names: Vec<String> = fn_def
            .type_params
            .iter()
            .map(|tp| tp.name.clone())
            .collect();
        if !tp_names.is_empty() {
            self.active_type_params = tp_names.clone();
        }

        let params: Vec<(String, Ty, bool)> = fn_def
            .params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    self.resolve_type_expr(&p.type_ann.node, p.type_ann.span),
                    p.comptime,
                )
            })
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
        let params: Vec<(String, Ty, bool)> = decl
            .params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    self.resolve_type_expr(&p.type_ann.node, p.type_ann.span),
                    p.comptime,
                )
            })
            .collect();

        let ret = decl
            .return_type
            .as_ref()
            .map(|t| self.resolve_type_expr(&t.node, t.span))
            .unwrap_or(Ty::Unit);

        let effects = self.extern_decl_effects(decl);

        FnSig {
            type_params: vec![],
            params,
            ret,
            effects,
        }
    }

    fn extern_decl_effects(&self, decl: &ExternFnDecl) -> Vec<String> {
        decl.effects
            .as_ref()
            .map(|e| e.effects.clone())
            .unwrap_or_else(effects::extern_default_effects)
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
                    let declared = self.extern_decl_effects(decl);

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

    /// Convert an AST Capability to an internal RefCap.
    fn ast_cap_to_ref_cap(&self, cap: &crate::ast::types::Capability) -> super::types::RefCap {
        use super::types::RefCap;
        use crate::ast::types::Capability;
        match cap {
            Capability::Iso => RefCap::Iso,
            Capability::Val => RefCap::Val,
            Capability::Ref => RefCap::Ref,
            Capability::Box => RefCap::Box,
            Capability::Trn => RefCap::Trn,
            Capability::Tag => RefCap::Tag,
        }
    }

    // ------------------------------------------------------------------
    // Comptime checking
    // ------------------------------------------------------------------

    /// Check that an expression is known at compile time.
    ///
    /// Returns an error if the expression cannot be evaluated at compile time.
    #[allow(clippy::result_large_err)]
    fn require_comptime(&self, expr: &Expr) -> Result<(), TypeError> {
        match &expr.node {
            // Literals are always comptime-known
            ExprKind::IntLit(_) |
            ExprKind::FloatLit(_) |
            ExprKind::StringLit(_) |
            ExprKind::BoolLit(_) => Ok(()),

            // Type expressions are comptime-known
            // Note: ExprKind::Type would go here when added to AST

            // Identifiers are comptime-known if they were bound as comptime
            ExprKind::Ident(name) => {
                if self.env.is_comptime_known(name) {
                    Ok(())
                } else {
                    Err(TypeError::new(
                        format!("expected compile-time value, but `{}` is a runtime value", name),
                        expr.span,
                    ).with_note(format!("`{}` must be marked as `comptime` to be used here", name)))
                }
            }

            // Tuple expressions are comptime-known if all elements are
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.require_comptime(elem)?;
                }
                Ok(())
            }

            // List literals are comptime-known if all elements are
            ExprKind::ListLit(elems) => {
                for elem in elems {
                    self.require_comptime(elem)?;
                }
                Ok(())
            }

            // Type constructors (e.g., `Some`, `Ok`) are comptime-known
            // if their arguments are comptime-known
            ExprKind::Call { func, args } => {
                // Check if function name is a type constructor
                if let ExprKind::Ident(_fn_name) = &func.node {
                    // For now, assume type constructors are comptime if their args are
                    for arg in args {
                        self.require_comptime(arg)?;
                    }
                    Ok(())
                } else {
                    Err(TypeError::new(
                        "expected compile-time value, but this expression cannot be evaluated at compile time".to_string(),
                        expr.span,
                    ))
                }
            }

            // Parenthesized expressions
            ExprKind::Paren(inner) => self.require_comptime(inner),

            // Field access on comptime values
            ExprKind::FieldAccess { object, field: _ } => self.require_comptime(object),

            // Everything else is not comptime-known by default
            _ => Err(TypeError::new(
                "expected compile-time value, but this expression cannot be evaluated at compile time".to_string(),
                expr.span,
            )),
        }
    }

    /// Try to evaluate an expression to a compile-time value.
    ///
    /// This is used to extract comptime values for comptime parameter passing.
    fn try_eval_comptime(&mut self, expr: &Expr) -> Result<ComptimeValue, ()> {
        use crate::comptime::evaluator::ComptimeError;

        match &expr.node {
            // Literals convert directly
            ExprKind::IntLit(n) => Ok(ComptimeValue::Int(*n)),
            ExprKind::FloatLit(n) => Ok(ComptimeValue::Float(*n)),
            ExprKind::BoolLit(b) => Ok(ComptimeValue::Bool(*b)),
            ExprKind::StringLit(s) => Ok(ComptimeValue::String(s.clone())),
            ExprKind::UnitLit => Ok(ComptimeValue::Unit),

            // Type literals - check for built-in type names
            ExprKind::Ident(name) => {
                match name.as_str() {
                    "Int" => Ok(ComptimeValue::Type(Ty::Int)),
                    "Float" => Ok(ComptimeValue::Type(Ty::Float)),
                    "Bool" => Ok(ComptimeValue::Type(Ty::Bool)),
                    "String" => Ok(ComptimeValue::Type(Ty::String)),
                    "Unit" | "()" => Ok(ComptimeValue::Type(Ty::Unit)),
                    _ => {
                        // Check if this is a comptime-known variable
                        if let Some(binding) = self.env.lookup_binding(name) {
                            if binding.comptime {
                                // For now, we can't retrieve the actual value from the environment
                                // In the future, we might store comptime values in the environment
                                return Err(());
                            }
                        }
                        Err(())
                    }
                }
            }

            // Try to evaluate using the comptime evaluator for complex expressions
            _ => {
                // Use the evaluator for more complex expressions
                match self.comptime_evaluator.eval_expr(expr) {
                    Ok(value) => Ok(value),
                    Err(ComptimeError::UnknownVariable { name }) => {
                        // Variable not in evaluator env - might be a type name
                        match name.as_str() {
                            "Int" => Ok(ComptimeValue::Type(Ty::Int)),
                            "Float" => Ok(ComptimeValue::Type(Ty::Float)),
                            "Bool" => Ok(ComptimeValue::Type(Ty::Bool)),
                            "String" => Ok(ComptimeValue::Type(Ty::String)),
                            _ => Err(()),
                        }
                    }
                    Err(_) => Err(()),
                }
            }
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
        BinOp::Pipe => "|>",
    }
}
