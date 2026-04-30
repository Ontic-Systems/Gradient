//! Type environment (scope management) for the Gradient type checker.
//!
//! The [`TypeEnv`] maintains a stack of lexical scopes for variable bindings
//! and a separate registry for top-level function signatures. It also tracks
//! the current function's return type and the effects available in the current
//! context, enabling `ret` type checking and effect validation.

use std::collections::{HashMap, HashSet};

use super::types::Ty;
use crate::ast::span::Span;

/// A variable binding with type and comptime information.
#[derive(Debug, Clone)]
pub struct Binding {
    /// The type of the binding.
    pub ty: Ty,
    /// Whether this binding is known at compile time.
    pub comptime: bool,
}

/// Information about a registered actor type, used by the type checker
/// to validate `spawn`, `send`, and `ask` expressions.
#[derive(Debug, Clone)]
pub struct ActorInfo {
    /// The actor's name.
    pub name: String,
    /// State fields: `(field_name, field_type)`.
    pub state_fields: Vec<(String, Ty)>,
    /// Message handlers: `(message_name, return_type)`.
    /// Return type is `Ty::Unit` for fire-and-forget messages.
    pub handlers: Vec<(String, Ty)>,
}

/// The signature of a function, as recorded in the type environment.
#[derive(Debug, Clone)]
pub struct FnSig {
    /// Type parameters for generic functions (e.g. `["T", "U"]`).
    /// Empty for non-generic functions.
    pub type_params: Vec<String>,
    /// The parameters: each is a `(name, type, comptime)` triple.
    /// `comptime` is true if the parameter must be known at compile time.
    pub params: Vec<(String, Ty, bool)>,
    /// The return type.
    pub ret: Ty,
    /// The effects declared on this function.
    pub effects: Vec<String>,
}

/// Information about a registered trait type.
#[derive(Debug, Clone)]
pub struct TraitInfo {
    /// The trait name.
    pub name: String,
    /// Method signatures declared in this trait.
    pub methods: Vec<TraitMethodSig>,
}

/// A trait method signature (excluding `self`).
#[derive(Debug, Clone)]
pub struct TraitMethodSig {
    /// The method name.
    pub name: String,
    /// Parameters excluding `self`: `(param_name, param_type, comptime)`.
    pub params: Vec<(String, Ty, bool)>,
    /// The return type.
    pub ret: Ty,
    /// Declared effects.
    pub effects: Vec<String>,
}

/// Information about a trait implementation.
#[derive(Debug, Clone)]
pub struct ImplInfo {
    /// The trait being implemented.
    pub trait_name: String,
    /// The type implementing the trait.
    pub target_type: String,
}

/// The type environment used during type checking.
///
/// It maintains:
/// - A stack of lexical scopes for variable lookups.
/// - A flat registry of function signatures for call resolution.
/// - Context about the current function being checked (return type, effects).
pub struct TypeEnv {
    /// Stack of variable scopes. The last element is the innermost scope.
    scopes: Vec<HashMap<String, Binding>>,
    /// Top-level function signatures, keyed by function name.
    functions: HashMap<String, FnSig>,
    /// Type aliases registered via `type Name = ...` declarations.
    type_aliases: HashMap<String, Ty>,
    /// Enum types registered via `type Name = Variant | ...` declarations.
    /// Maps enum type name -> Ty::Enum.
    enums: HashMap<String, Ty>,
    /// Maps variant name -> (enum_type_name, variant_index).
    /// Used to resolve bare variant names in expressions and patterns.
    variant_to_enum: HashMap<String, (String, usize)>,
    /// The expected return type for the function currently being checked.
    /// `None` when not inside a function body.
    current_fn_return: Option<Ty>,
    /// The effects available in the current function context.
    current_effects: Vec<String>,
    /// Set of variable names that have been declared as mutable (`let mut`).
    mutable_vars: HashSet<String>,
    /// Imported module namespaces: maps module name -> function registry.
    /// Used for qualified calls like `math.add(3, 4)`.
    imported_modules: HashMap<String, HashMap<String, FnSig>>,
    /// Registered actor types, keyed by actor name.
    actors: HashMap<String, ActorInfo>,
    /// Registered trait types, keyed by trait name.
    traits: HashMap<String, TraitInfo>,
    /// Registered trait implementations.
    impls: Vec<ImplInfo>,
    /// Formal type parameter names for generic enum types.
    /// e.g. "Option" -> ["T"], "Result" -> ["T", "E"]
    enum_type_params: HashMap<String, Vec<String>>,
    /// Linear type state tracking: which linear variables have been consumed.
    /// Maps variable name -> (consumed: bool, span where consumed if applicable).
    linear_states: HashMap<String, (bool, Option<Span>)>,
    /// Stack of linear states for each scope (for nested scopes).
    linear_scopes: Vec<HashMap<String, (bool, Option<Span>)>>,
}

impl Default for TypeEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeEnv {
    /// Create a new type environment with a global scope pre-populated with
    /// builtin function signatures.
    pub fn new() -> Self {
        let mut env = Self {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            type_aliases: HashMap::new(),
            enums: HashMap::new(),
            variant_to_enum: HashMap::new(),
            current_fn_return: None,
            current_effects: Vec::new(),
            mutable_vars: HashSet::new(),
            imported_modules: HashMap::new(),
            actors: HashMap::new(),
            traits: HashMap::new(),
            impls: Vec::new(),
            enum_type_params: HashMap::new(),
            linear_states: HashMap::new(),
            linear_scopes: Vec::new(),
        };
        env.preload_types();
        env.preload_builtins();
        env
    }

    /// Push a new (empty) lexical scope onto the scope stack.
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pop the innermost lexical scope from the scope stack.
    ///
    /// # Panics
    ///
    /// Panics if called when only the global scope remains.
    pub fn pop_scope(&mut self) {
        debug_assert!(self.scopes.len() > 1, "cannot pop the global scope");
        self.scopes.pop();
    }

    /// Define a variable in the current (innermost) scope.
    pub fn define(&mut self, name: String, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(
                name,
                Binding {
                    ty,
                    comptime: false,
                },
            );
        }
    }

    /// Define a comptime variable in the current (innermost) scope.
    pub fn define_comptime(&mut self, name: String, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, Binding { ty, comptime: true });
        }
    }

    /// Define a mutable variable in the current (innermost) scope.
    pub fn define_mutable(&mut self, name: String, ty: Ty) {
        self.mutable_vars.insert(name.clone());
        self.define(name, ty);
    }

    /// Check whether a variable is mutable.
    pub fn is_mutable(&self, name: &str) -> bool {
        self.mutable_vars.contains(name)
    }

    /// Look up a variable by name, searching from the innermost scope outward.
    ///
    /// Returns `None` if the name is not bound in any enclosing scope.
    pub fn lookup(&self, name: &str) -> Option<&Ty> {
        self.lookup_binding(name).map(|b| &b.ty)
    }

    /// Look up a binding by name, searching from the innermost scope outward.
    ///
    /// Returns `None` if the name is not bound in any enclosing scope.
    pub fn lookup_binding(&self, name: &str) -> Option<&Binding> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.get(name) {
                return Some(binding);
            }
        }
        None
    }

    /// Check whether a variable is known at compile time.
    pub fn is_comptime_known(&self, name: &str) -> bool {
        self.lookup_binding(name)
            .map(|b| b.comptime)
            .unwrap_or(false)
    }

    /// Register a top-level function signature.
    pub fn define_fn(&mut self, name: String, sig: FnSig) {
        self.functions.insert(name, sig);
    }

    /// Look up a function signature by name.
    pub fn lookup_fn(&self, name: &str) -> Option<&FnSig> {
        self.functions.get(name)
    }

    /// Register a type alias (e.g. `type Count = Int`).
    pub fn define_type_alias(&mut self, name: String, ty: Ty) {
        self.type_aliases.insert(name, ty);
    }

    /// Look up a type alias by name.
    pub fn lookup_type_alias(&self, name: &str) -> Option<&Ty> {
        self.type_aliases.get(name)
    }

    /// Set the expected return type for the function currently being checked.
    pub fn set_current_fn_return(&mut self, ty: Ty) {
        self.current_fn_return = Some(ty);
    }

    /// Clear the current function return type (when leaving a function body).
    pub fn clear_current_fn_return(&mut self) {
        self.current_fn_return = None;
    }

    /// Get the expected return type for the current function, if any.
    pub fn current_fn_return(&self) -> Option<&Ty> {
        self.current_fn_return.as_ref()
    }

    /// Set the effects available in the current function context.
    pub fn set_current_effects(&mut self, effects: Vec<String>) {
        self.current_effects = effects;
    }

    /// Clear the current effects (when leaving a function body).
    pub fn clear_current_effects(&mut self) {
        self.current_effects.clear();
    }

    /// Get the effects available in the current function context.
    pub fn current_effects(&self) -> &[String] {
        &self.current_effects
    }

    // ------------------------------------------------------------------
    // Linear type tracking
    // ------------------------------------------------------------------

    /// Register a linear variable when it's defined.
    /// Linear variables start in an unconsumed (available) state.
    pub fn define_linear(&mut self, name: String, span: Span) {
        self.linear_states.insert(name, (false, Some(span)));
    }

    /// Mark a linear variable as consumed.
    /// Returns true if the variable was available and is now consumed.
    /// Returns false if the variable was already consumed (double-use error).
    pub fn consume_linear(&mut self, name: &str, use_span: Span) -> bool {
        if let Some((consumed, _)) = self.linear_states.get(name) {
            if *consumed {
                // Already consumed - double use
                return false;
            }
            // Mark as consumed
            self.linear_states
                .insert(name.to_string(), (true, Some(use_span)));
            return true;
        }
        // Not a linear variable - no tracking needed
        true
    }

    /// Check if a linear variable has been consumed.
    pub fn is_linear_consumed(&self, name: &str) -> bool {
        self.linear_states
            .get(name)
            .map(|(c, _)| *c)
            .unwrap_or(false)
    }

    /// Check if a variable is a tracked linear variable.
    pub fn is_linear_var(&self, name: &str) -> bool {
        self.linear_states.contains_key(name)
    }

    /// Get all unconsumed linear variables at the current point.
    pub fn unconsumed_linears(&self) -> Vec<(String, Span)> {
        self.linear_states
            .iter()
            .filter(|(_, (consumed, _))| !*consumed)
            .filter_map(|(name, (_, span))| span.map(|s| (name.clone(), s)))
            .collect()
    }

    /// Clear all linear tracking (called when entering a new function).
    pub fn clear_linear_tracking(&mut self) {
        self.linear_states.clear();
        self.linear_scopes.clear();
    }

    /// Push a new linear scope (for nested blocks).
    pub fn push_linear_scope(&mut self) {
        self.linear_scopes.push(HashMap::new());
    }

    /// Pop a linear scope.
    pub fn pop_linear_scope(&mut self) {
        self.linear_scopes.pop();
    }

    // ------------------------------------------------------------------
    // Enum types
    // ------------------------------------------------------------------

    /// Register an enum type and its variant-to-enum mappings.
    pub fn define_enum(&mut self, name: String, ty: Ty) {
        if let Ty::Enum { variants, .. } = &ty {
            for (i, (vname, _)) in variants.iter().enumerate() {
                self.variant_to_enum
                    .insert(vname.clone(), (name.clone(), i));
            }
        }
        self.enums.insert(name, ty);
    }

    /// Look up an enum type by name.
    pub fn lookup_enum(&self, name: &str) -> Option<&Ty> {
        self.enums.get(name)
    }

    /// Look up which enum a variant belongs to.
    /// Returns `(enum_name, variant_index)`.
    pub fn lookup_variant(&self, variant_name: &str) -> Option<&(String, usize)> {
        self.variant_to_enum.get(variant_name)
    }

    /// Register formal type parameter names for a generic enum.
    pub fn define_enum_type_params(&mut self, name: String, type_params: Vec<String>) {
        self.enum_type_params.insert(name, type_params);
    }

    /// Look up the formal type parameter names for a generic enum.
    pub fn lookup_enum_type_params(&self, name: &str) -> Option<&Vec<String>> {
        self.enum_type_params.get(name)
    }

    // ------------------------------------------------------------------
    // Imported modules
    // ------------------------------------------------------------------

    /// Register an imported module's function signatures.
    ///
    /// After this call, qualified references like `module_name.fn_name` can be
    /// resolved via [`lookup_qualified_fn`].
    pub fn import_module(&mut self, module_name: String, functions: HashMap<String, FnSig>) {
        self.imported_modules.insert(module_name, functions);
    }

    /// Register an imported module with full type information (functions and types).
    ///
    /// This makes both functions and type definitions available for cross-module
    /// type checking. Type names are imported into the global namespace (without
    /// module prefix) to match self-hosted file expectations.
    pub fn import_module_full(
        &mut self,
        _module_name: String,
        info: super::checker::ImportedModuleInfo,
    ) {
        // Register functions for qualified access.
        self.imported_modules
            .insert(_module_name.clone(), info.functions);

        // Import type aliases into global namespace.
        for (name, ty) in info.type_aliases {
            self.type_aliases.insert(name, ty);
        }

        // Import enum types into global namespace.
        for (name, ty) in info.enums {
            self.enums.insert(name.clone(), ty.clone());
            // Also register enum type params if available.
            if let Some(params) = info.enum_type_params.get(&name) {
                self.enum_type_params.insert(name, params.clone());
            }
        }

        // Import variant mappings for pattern matching and construction.
        for (variant, (enum_name, idx)) in info.variant_mappings {
            self.variant_to_enum.insert(variant, (enum_name, idx));
        }
    }

    /// Check if a name refers to an imported module.
    pub fn is_imported_module(&self, name: &str) -> bool {
        self.imported_modules.contains_key(name)
    }

    /// Look up a function in an imported module by qualified name.
    ///
    /// For example, `lookup_qualified_fn("math", "add")` resolves `math.add`.
    pub fn lookup_qualified_fn(&self, module_name: &str, fn_name: &str) -> Option<&FnSig> {
        self.imported_modules
            .get(module_name)
            .and_then(|fns| fns.get(fn_name))
    }

    // ------------------------------------------------------------------
    // Actor types
    // ------------------------------------------------------------------

    /// Register an actor type.
    pub fn define_actor(&mut self, name: String, info: ActorInfo) {
        self.actors.insert(name, info);
    }

    /// Look up an actor type by name.
    pub fn lookup_actor(&self, name: &str) -> Option<&ActorInfo> {
        self.actors.get(name)
    }

    /// Return all registered actor types.
    pub fn all_actors(&self) -> &HashMap<String, ActorInfo> {
        &self.actors
    }

    // ------------------------------------------------------------------
    // Traits
    // ------------------------------------------------------------------

    /// Register a trait type.
    pub fn define_trait(&mut self, name: String, info: TraitInfo) {
        self.traits.insert(name, info);
    }

    /// Look up a trait type by name.
    pub fn lookup_trait(&self, name: &str) -> Option<&TraitInfo> {
        self.traits.get(name)
    }

    /// Register a trait implementation.
    pub fn register_impl(&mut self, info: ImplInfo) {
        self.impls.push(info);
    }

    /// Check whether a type has an implementation for a given trait.
    pub fn has_impl(&self, trait_name: &str, target_type: &str) -> bool {
        self.impls
            .iter()
            .any(|i| i.trait_name == trait_name && i.target_type == target_type)
    }

    /// Return all registered traits.
    pub fn all_traits(&self) -> &HashMap<String, TraitInfo> {
        &self.traits
    }

    /// Return all registered impls.
    pub fn all_impls(&self) -> &[ImplInfo] {
        &self.impls
    }

    // ------------------------------------------------------------------
    // Builtins
    // ------------------------------------------------------------------

    /// Preload the environment with Gradient's built-in types.
    fn preload_types(&mut self) {
        // Register formal type parameters for built-in generic enums.
        self.define_enum_type_params("Option".into(), vec!["T".into()]);
        self.define_enum_type_params("Result".into(), vec!["T".into(), "E".into()]);

        // Register Result[T, E] = Ok(T) | Err(E) as a built-in generic enum.
        self.define_enum(
            "Result".into(),
            Ty::Enum {
                name: "Result".into(),
                variants: vec![
                    ("Ok".into(), Some(Ty::TypeVar("T".into()))),
                    ("Err".into(), Some(Ty::TypeVar("E".into()))),
                ],
            },
        );

        let result_enum_ty = Ty::Enum {
            name: "Result".into(),
            variants: vec![
                ("Ok".into(), Some(Ty::TypeVar("T".into()))),
                ("Err".into(), Some(Ty::TypeVar("E".into()))),
            ],
        };

        // Register Ok as a non-generic constructor: Ok(value) -> Result.
        // The TypeVar param type acts as a wildcard accepting any argument.
        self.define_fn(
            "Ok".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::TypeVar("T".into()), false)],
                ret: result_enum_ty.clone(),
                effects: vec![],
            },
        );

        // Register Err as a non-generic constructor: Err(error) -> Result.
        self.define_fn(
            "Err".into(),
            FnSig {
                type_params: vec![],
                params: vec![("error".into(), Ty::TypeVar("E".into()), false)],
                ret: result_enum_ty,
                effects: vec![],
            },
        );

        // Register Option[T] = Some(T) | None as a built-in generic enum.
        self.define_enum(
            "Option".into(),
            Ty::Enum {
                name: "Option".into(),
                variants: vec![
                    ("Some".into(), Some(Ty::TypeVar("T".into()))),
                    ("None".into(), None),
                ],
            },
        );

        // Register JsonValue as a built-in enum type.
        // We model it nominally for now; runtime representation exists in C.
        // Variant-level static modeling can be expanded later.
        self.define_enum(
            "JsonValue".into(),
            Ty::Enum {
                name: "JsonValue".into(),
                variants: vec![],
            },
        );

        let option_enum_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![
                ("Some".into(), Some(Ty::TypeVar("T".into()))),
                ("None".into(), None),
            ],
        };

        // Register Some as a non-generic constructor: Some(value) -> Option.
        // The TypeVar param type acts as a wildcard accepting any argument.
        self.define_fn(
            "Some".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::TypeVar("T".into()), false)],
                ret: option_enum_ty,
                effects: vec![],
            },
        );

        // Register None as a variable of type Option.
        self.define(
            "None".into(),
            Ty::Enum {
                name: "Option".into(),
                variants: vec![
                    ("Some".into(), Some(Ty::TypeVar("T".into()))),
                    ("None".into(), None),
                ],
            },
        );

        // ── Option helper functions ───────────────────────────────────────

        // option_is_some(opt: Option[T]) -> Bool
        let option_ty_t = Ty::Enum {
            name: "Option".into(),
            variants: vec![
                ("Some".into(), Some(Ty::TypeVar("T".into()))),
                ("None".into(), None),
            ],
        };
        self.define_fn(
            "option_is_some".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![("opt".into(), option_ty_t.clone(), false)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // option_is_none(opt: Option[T]) -> Bool
        self.define_fn(
            "option_is_none".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![("opt".into(), option_ty_t.clone(), false)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // option_unwrap(opt: Option[T]) -> T
        // Panics on None - use with caution
        self.define_fn(
            "option_unwrap".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![("opt".into(), option_ty_t.clone(), false)],
                ret: Ty::TypeVar("T".into()),
                effects: vec![],
            },
        );

        // option_unwrap_or(opt: Option[T], default: T) -> T
        // Returns default on None
        self.define_fn(
            "option_unwrap_or".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    ("opt".into(), option_ty_t, false),
                    ("default".into(), Ty::TypeVar("T".into()), false),
                ],
                ret: Ty::TypeVar("T".into()),
                effects: vec![],
            },
        );
    }

    /// Preload the environment with Gradient's built-in functions.
    fn preload_builtins(&mut self) {
        // print(String) -> !{IO} ()
        self.define_fn(
            "print".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::String, false)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // println(String) -> !{IO} ()
        self.define_fn(
            "println".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::String, false)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // range(Int) -> List[Int]  (simplified for v0.1: returns Unit, works in for loops)
        self.define_fn(
            "range".into(),
            FnSig {
                type_params: vec![],
                params: vec![("n".into(), Ty::Int, false)],
                ret: Ty::Unit, // simplified: for-loop handles iterable check specially
                effects: vec![],
            },
        );

        // to_string(Int) -> String  (convenience builtin)
        self.define_fn(
            "to_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // print_int(Int) -> !{IO} ()
        self.define_fn(
            "print_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Int, false)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // print_float(Float) -> !{IO} ()
        self.define_fn(
            "print_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Float, false)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // print_bool(Bool) -> !{IO} ()
        self.define_fn(
            "print_bool".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Bool, false)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // int_to_string(Int) -> String
        self.define_fn(
            "int_to_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // abs(Int) -> Int
        self.define_fn(
            "abs".into(),
            FnSig {
                type_params: vec![],
                params: vec![("n".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // min(Int, Int) -> Int
        self.define_fn(
            "min".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Int, false), ("b".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // max(Int, Int) -> Int
        self.define_fn(
            "max".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Int, false), ("b".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // mod_int(Int, Int) -> Int
        self.define_fn(
            "mod_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Int, false), ("b".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // ── String operations ────────────────────────────────────────────

        // string_length(String) -> Int
        self.define_fn(
            "string_length".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // string_contains(String, String) -> Bool
        self.define_fn(
            "string_contains".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("substr".into(), Ty::String, false),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // string_starts_with(String, String) -> Bool
        self.define_fn(
            "string_starts_with".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("prefix".into(), Ty::String, false),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // string_ends_with(String, String) -> Bool
        self.define_fn(
            "string_ends_with".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("suffix".into(), Ty::String, false),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // string_substring(String, Int, Int) -> String
        self.define_fn(
            "string_substring".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("start".into(), Ty::Int, false),
                    ("end".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_trim(String) -> String
        self.define_fn(
            "string_trim".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_to_upper(String) -> String
        self.define_fn(
            "string_to_upper".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_to_lower(String) -> String
        self.define_fn(
            "string_to_lower".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_replace(String, String, String) -> String
        self.define_fn(
            "string_replace".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("old".into(), Ty::String, false),
                    ("new_str".into(), Ty::String, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_index_of(String, String) -> Int
        self.define_fn(
            "string_index_of".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("substr".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // string_char_at(String, Int) -> String (returns single-char string)
        self.define_fn(
            "string_char_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_char_code_at(String, Int) -> Int (returns byte value, -1 if out of bounds)
        // This is the primitive needed for self-hosted lexer
        self.define_fn(
            "string_char_code_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // string_append(String, String) -> String
        // Concatenates two strings - needed for self-hosted error messages
        self.define_fn(
            "string_append".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("a".into(), Ty::String, false),
                    ("b".into(), Ty::String, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_split(String, String) -> List[String]
        self.define_fn(
            "string_split".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("delimiter".into(), Ty::String, false),
                ],
                ret: Ty::List(Box::new(Ty::String)),
                effects: vec![],
            },
        );

        // ── Bootstrap collection externs (#220) ──────────────────────────
        // Self-hosted lexer/parser code allocates and appends to runtime-
        // backed token / AST / diagnostic lists via these primitives. Until
        // the runtime can pass record values across the FFI, callers
        // decompose tokens/nodes into primitive components (kind tags +
        // span offsets).

        // bootstrap_token_list_alloc() -> Int
        self.define_fn(
            "bootstrap_token_list_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // bootstrap_token_list_append(handle, kind_tag, file_id, start_offset, end_offset) -> Int
        self.define_fn(
            "bootstrap_token_list_append".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("kind_tag".into(), Ty::Int, false),
                    ("file_id".into(), Ty::Int, false),
                    ("start_offset".into(), Ty::Int, false),
                    ("end_offset".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // bootstrap_token_list_len(handle) -> Int
        self.define_fn(
            "bootstrap_token_list_len".into(),
            FnSig {
                type_params: vec![],
                params: vec![("handle".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // bootstrap_token_list_get_kind(handle, index) -> Int
        // Returns the token's kind tag (matching `lexer.gr::token_kind_tag`),
        // or 1 (= Eof) when `index` is out of bounds. This is the inverse
        // direction of `_append`: parser-side code reads kind tags by index
        // and reconstructs a TokenKind. Out-of-bounds-as-Eof keeps parser
        // execution safe past end-of-stream (#221).
        self.define_fn(
            "bootstrap_token_list_get_kind".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // bootstrap_token_list_get_int_value(handle, index) -> Int
        // Returns the IntLit payload, or 0 for non-int/OOB tokens.
        self.define_fn(
            "bootstrap_token_list_get_int_value".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // bootstrap_token_list_get_text(handle, index) -> String
        // Returns Ident/StringLit/Error payload text, or empty for other/OOB tokens.
        self.define_fn(
            "bootstrap_token_list_get_text".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // bootstrap_token_list_get_file_id(handle, index) -> Int
        // Returns the token's span file_id, or 0 when `index` is out of
        // bounds.
        self.define_fn(
            "bootstrap_token_list_get_file_id".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // bootstrap_token_list_get_start_offset(handle, index) -> Int
        // Returns the token's span start offset, or 0 when `index` is out of
        // bounds.
        self.define_fn(
            "bootstrap_token_list_get_start_offset".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // bootstrap_token_list_get_end_offset(handle, index) -> Int
        // Returns the token's span end offset, or 0 when `index` is out of
        // bounds.
        self.define_fn(
            "bootstrap_token_list_get_end_offset".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // ── Bootstrap AST node externs (#222) ─────────────────────────────
        // Self-hosted parser allocates and reads back AST nodes through a
        // runtime-backed store. Each kind has its own id space (expr ids
        // distinct from stmt ids etc.); generic node-id lists back the
        // parser's `*List` wrappers. Out-of-range / unknown ids return
        // safe zero/empty defaults so parser execution can keep walking.

        // Expression alloc primitives.
        self.define_fn(
            "bootstrap_expr_alloc_int_lit".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_bool_lit".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_string_lit".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_ident".into(),
            FnSig {
                type_params: vec![],
                params: vec![("name".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_binary".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("op_tag".into(), Ty::Int, false),
                    ("left".into(), Ty::Int, false),
                    ("right".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_unary".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("op_tag".into(), Ty::Int, false),
                    ("operand".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_call".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("callee".into(), Ty::Int, false),
                    ("args_handle".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_if".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("cond".into(), Ty::Int, false),
                    ("then_branch".into(), Ty::Int, false),
                    ("else_branch".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_block".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("stmts_handle".into(), Ty::Int, false),
                    ("final_expr".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_expr_alloc_error".into(),
            FnSig {
                type_params: vec![],
                params: vec![("message".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // Expression read primitives.
        for (name, ret) in [
            ("bootstrap_expr_get_tag", Ty::Int),
            ("bootstrap_expr_get_int_value", Ty::Int),
            ("bootstrap_expr_get_text", Ty::String),
            ("bootstrap_expr_get_child_a", Ty::Int),
            ("bootstrap_expr_get_child_b", Ty::Int),
            ("bootstrap_expr_get_child_c", Ty::Int),
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("id".into(), Ty::Int, false)],
                    ret,
                    effects: vec![],
                },
            );
        }

        // Statement alloc + read.
        self.define_fn(
            "bootstrap_stmt_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("node_tag".into(), Ty::Int, false),
                    ("int_value".into(), Ty::Int, false),
                    ("child_a".into(), Ty::Int, false),
                    ("child_b".into(), Ty::Int, false),
                    ("child_c".into(), Ty::Int, false),
                    ("text".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        for (name, ret) in [
            ("bootstrap_stmt_get_tag", Ty::Int),
            ("bootstrap_stmt_get_int_value", Ty::Int),
            ("bootstrap_stmt_get_text", Ty::String),
            ("bootstrap_stmt_get_child_a", Ty::Int),
            ("bootstrap_stmt_get_child_b", Ty::Int),
            ("bootstrap_stmt_get_child_c", Ty::Int),
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("id".into(), Ty::Int, false)],
                    ret,
                    effects: vec![],
                },
            );
        }

        // Param alloc + read.
        self.define_fn(
            "bootstrap_param_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("name".into(), Ty::String, false),
                    ("type_tag".into(), Ty::Int, false),
                    ("type_name".into(), Ty::String, false),
                    ("default_id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        for (name, ret) in [
            ("bootstrap_param_get_name", Ty::String),
            ("bootstrap_param_get_type_tag", Ty::Int),
            ("bootstrap_param_get_type_name", Ty::String),
            ("bootstrap_param_get_default", Ty::Int),
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("id".into(), Ty::Int, false)],
                    ret,
                    effects: vec![],
                },
            );
        }

        // Function alloc + read.
        self.define_fn(
            "bootstrap_function_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("name".into(), Ty::String, false),
                    ("params_handle".into(), Ty::Int, false),
                    ("ret_type_tag".into(), Ty::Int, false),
                    ("ret_type_name".into(), Ty::String, false),
                    ("body_handle".into(), Ty::Int, false),
                    ("is_pub".into(), Ty::Int, false),
                    ("is_extern".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        for (name, ret) in [
            ("bootstrap_function_get_name", Ty::String),
            ("bootstrap_function_get_params_handle", Ty::Int),
            ("bootstrap_function_get_ret_type_tag", Ty::Int),
            ("bootstrap_function_get_ret_type_name", Ty::String),
            ("bootstrap_function_get_body_handle", Ty::Int),
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("id".into(), Ty::Int, false)],
                    ret,
                    effects: vec![],
                },
            );
        }

        // Module item alloc + read.
        self.define_fn(
            "bootstrap_module_item_alloc_function".into(),
            FnSig {
                type_params: vec![],
                params: vec![("function_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        for (name, ret) in [
            ("bootstrap_module_item_get_tag", Ty::Int),
            ("bootstrap_module_item_get_function_id", Ty::Int),
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("id".into(), Ty::Int, false)],
                    ret,
                    effects: vec![],
                },
            );
        }

        // Generic node-id lists.
        for name in [
            "bootstrap_expr_list_alloc",
            "bootstrap_stmt_list_alloc",
            "bootstrap_param_list_alloc",
            "bootstrap_module_item_list_alloc",
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![],
                    ret: Ty::Int,
                    effects: vec![],
                },
            );
        }
        self.define_fn(
            "bootstrap_node_list_append".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_node_list_len".into(),
            FnSig {
                type_params: vec![],
                params: vec![("handle".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_node_list_get".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // ── Bootstrap checker env externs (#225) ──────────────────────────
        // Self-hosted checker maintains lexical/function environments
        // through a runtime-backed store. Each `insert_*` allocates a new
        // immutable frame whose parent points at the caller's env id.
        // Lookups walk the parent chain; missing names return 0 so
        // `check_ident` / `check_call` can produce error types.

        // Env frame ops.
        self.define_fn(
            "bootstrap_checker_env_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("parent".into(), Ty::Int, false),
                    ("scope_level".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_checker_env_insert_var".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("env_id".into(), Ty::Int, false),
                    ("name".into(), Ty::String, false),
                    ("type_tag".into(), Ty::Int, false),
                    ("type_name".into(), Ty::String, false),
                    ("is_mut".into(), Ty::Int, false),
                    ("scope_level".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_checker_env_insert_fn".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("env_id".into(), Ty::Int, false),
                    ("name".into(), Ty::String, false),
                    ("params_handle".into(), Ty::Int, false),
                    ("ret_type_tag".into(), Ty::Int, false),
                    ("ret_type_name".into(), Ty::String, false),
                    ("effects_handle".into(), Ty::Int, false),
                    ("is_extern".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_checker_env_lookup_var".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("env_id".into(), Ty::Int, false),
                    ("name".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_checker_env_lookup_fn".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("env_id".into(), Ty::Int, false),
                    ("name".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        for (name, ret) in [
            ("bootstrap_checker_env_get_parent", Ty::Int),
            ("bootstrap_checker_env_get_scope_level", Ty::Int),
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("env_id".into(), Ty::Int, false)],
                    ret,
                    effects: vec![],
                },
            );
        }

        // Var record accessors.
        for (name, ret) in [
            ("bootstrap_checker_var_get_name", Ty::String),
            ("bootstrap_checker_var_get_type_tag", Ty::Int),
            ("bootstrap_checker_var_get_type_name", Ty::String),
            ("bootstrap_checker_var_get_is_mut", Ty::Int),
            ("bootstrap_checker_var_get_scope_level", Ty::Int),
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("var_id".into(), Ty::Int, false)],
                    ret,
                    effects: vec![],
                },
            );
        }

        // Fn record accessors.
        for (name, ret) in [
            ("bootstrap_checker_fn_get_name", Ty::String),
            ("bootstrap_checker_fn_get_params_handle", Ty::Int),
            ("bootstrap_checker_fn_get_ret_type_tag", Ty::Int),
            ("bootstrap_checker_fn_get_ret_type_name", Ty::String),
            ("bootstrap_checker_fn_get_effects_handle", Ty::Int),
            ("bootstrap_checker_fn_get_is_extern", Ty::Int),
        ] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("fn_id".into(), Ty::Int, false)],
                    ret,
                    effects: vec![],
                },
            );
        }

        // ── Bootstrap IR / pipeline / driver / query / LSP externs (#259) ─
        // The runtime-backed kernels under `codebase/compiler/src/bootstrap_*.rs`
        // expose integer-handle FFIs that the .gr-side compiler delegates to.
        // Registering them here makes the names typecheckable when called from
        // .gr modules (see #259); without this, `compiler/query.gr`,
        // `compiler/lsp.gr`, `compiler/compiler.gr`, `compiler/main.gr`, and
        // `compiler/codegen.gr` cannot move from stub bodies to delegating calls.

        // ir bridge (bootstrap_ir_bridge.rs, 65 externs)
        self.define_fn(
            "bootstrap_ir_type_alloc_primitive".into(),
            FnSig {
                type_params: vec![],
                params: vec![("tag_arg".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_type_alloc_ptr".into(),
            FnSig {
                type_params: vec![],
                params: vec![("pointee".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_type_alloc_named".into(),
            FnSig {
                type_params: vec![],
                params: vec![("name".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_type_get_tag".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_type_get_child".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_type_get_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_const_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("ty".into(), Ty::Int, false),
                    ("value_arg".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_const_bool".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value_arg".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_const_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value_arg".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_const_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("ty".into(), Ty::Int, false),
                    ("value_arg".into(), Ty::Float, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_register".into(),
            FnSig {
                type_params: vec![],
                params: vec![("ty".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_param".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("index".into(), Ty::Int, false),
                    ("ty".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_global".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("name".into(), Ty::String, false),
                    ("ty".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_undef".into(),
            FnSig {
                type_params: vec![],
                params: vec![("ty".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_alloc_error".into(),
            FnSig {
                type_params: vec![],
                params: vec![("message".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_get_tag".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_get_type".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_get_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_get_bool".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_get_slot".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_get_text".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("tag_arg".into(), Ty::Int, false),
                    ("ty".into(), Ty::Int, false),
                    ("left".into(), Ty::Int, false),
                    ("right".into(), Ty::Int, false),
                    ("cond_or_value".into(), Ty::Int, false),
                    ("then_target".into(), Ty::Int, false),
                    ("else_target".into(), Ty::Int, false),
                    ("int_extra".into(), Ty::Int, false),
                    ("result_arg".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_tag".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_type".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_left".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_right".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_cond".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_then_target".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_else_target".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_int_extra".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_instr_get_result".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_block_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![("name".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_block_append_instr".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("block_id".into(), Ty::Int, false),
                    ("instr_id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_block_get_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_block_get_instrs".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_block_get_instr_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_block_get_instr_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_param_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("name".into(), Ty::String, false),
                    ("ty".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_param_get_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_param_get_type".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("name".into(), Ty::String, false),
                    ("ret_ty".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_append_param".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("fn_id".into(), Ty::Int, false),
                    ("param_id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_append_block".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("fn_id".into(), Ty::Int, false),
                    ("block_id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_ret_type".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_params".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_blocks".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_param_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_param_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_block_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_block_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_function_get_entry_block".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_module_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![("name".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_module_append_function".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("mod_id".into(), Ty::Int, false),
                    ("fn_id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_module_set_entry".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("mod_id".into(), Ty::Int, false),
                    ("fn_id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_module_get_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_module_get_functions".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_module_get_entry_fn".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_module_get_function_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_module_get_function_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_value_list_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_int_list_alloc".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_list_append".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_list_len".into(),
            FnSig {
                type_params: vec![],
                params: vec![("handle".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_ir_list_get".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("handle".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // ir emit (bootstrap_ir_emit.rs, 1 externs)
        self.define_fn(
            "bootstrap_ir_emit_text".into(),
            FnSig {
                type_params: vec![],
                params: vec![("mod_id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // pipeline (bootstrap_pipeline.rs, 7 externs)
        self.define_fn(
            "bootstrap_pipeline_lex".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("source".into(), Ty::String, false),
                    ("file_id".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_pipeline_token_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_pipeline_parse".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_pipeline_parse_error_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_pipeline_check".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_pipeline_lower".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("mod_name".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_pipeline_emit".into(),
            FnSig {
                type_params: vec![],
                params: vec![("ir_module_id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // driver (bootstrap_driver.rs, 8 externs)
        self.define_fn(
            "bootstrap_driver_run_source".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("source".into(), Ty::String, false),
                    ("output_path".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_driver_run_file".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("input_path".into(), Ty::String, false),
                    ("output_path".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_driver_get_exit_code".into(),
            FnSig {
                type_params: vec![],
                params: vec![("run_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_driver_get_diagnostic_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("run_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_driver_get_diagnostic_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("run_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_driver_get_captured_output".into(),
            FnSig {
                type_params: vec![],
                params: vec![("run_id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_driver_get_written_path".into(),
            FnSig {
                type_params: vec![],
                params: vec![("run_id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_driver_get_module_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![("run_id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // query (bootstrap_query.rs, 32 externs)
        self.define_fn(
            "bootstrap_query_new_session".into(),
            FnSig {
                type_params: vec![],
                params: vec![("source".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_session_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_session_source".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_parse_error_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_type_error_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_is_type_checked".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_check_ok".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_error_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_diagnostic_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_diagnostic_phase".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_diagnostic_severity".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_diagnostic_message".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_diagnostic_line".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_diagnostic_col".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("session_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_kind".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_type".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_is_pure".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_is_extern".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_is_export".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_is_test".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_line".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_col".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_param_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_param_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("sym_index".into(), Ty::Int, false),
                    ("param_index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_param_type".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("sym_index".into(), Ty::Int, false),
                    ("param_index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_effect_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_effect_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("sym_index".into(), Ty::Int, false),
                    ("effect_index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_find_symbol".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("name".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_symbol_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("line".into(), Ty::Int, false),
                    ("col".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_query_type_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("session_id".into(), Ty::Int, false),
                    ("line".into(), Ty::Int, false),
                    ("col".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // lsp (bootstrap_lsp.rs, 33 externs)
        self.define_fn(
            "bootstrap_lsp_new_server".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_initialize".into(),
            FnSig {
                type_params: vec![],
                params: vec![("server_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_is_initialized".into(),
            FnSig {
                type_params: vec![],
                params: vec![("server_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_did_open".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("language_id".into(), Ty::String, false),
                    ("version".into(), Ty::Int, false),
                    ("text".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_did_change".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("version".into(), Ty::Int, false),
                    ("new_text".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_did_close".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_did_save".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("text".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("server_id".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_text".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_version".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_session".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_diagnostic_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_diagnostic_severity".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_diagnostic_message".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_diagnostic_line".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_diagnostic_character".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_symbol_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_symbol_name".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_symbol_kind".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_symbol_line".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_document_symbol_character".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_hover".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("line0".into(), Ty::Int, false),
                    ("char0".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_completion_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_completion_label".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_completion_kind".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_completion_detail".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("server_id".into(), Ty::Int, false),
                    ("uri".into(), Ty::String, false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_is_keyword".into(),
            FnSig {
                type_params: vec![],
                params: vec![("word".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_is_builtin".into(),
            FnSig {
                type_params: vec![],
                params: vec![("word".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_keyword_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_keyword_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![("index".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_builtin_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_builtin_name_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![("index".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "bootstrap_lsp_builtin_signature_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![("index".into(), Ty::Int, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // ── Numeric operations ───────────────────────────────────────────

        // float_to_int(Float) -> Int
        self.define_fn(
            "float_to_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("f".into(), Ty::Float, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // int_to_float(Int) -> Float
        self.define_fn(
            "int_to_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![("n".into(), Ty::Int, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // pow(Int, Int) -> Int
        self.define_fn(
            "pow".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("base".into(), Ty::Int, false),
                    ("exp".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // float_abs(Float) -> Float
        self.define_fn(
            "float_abs".into(),
            FnSig {
                type_params: vec![],
                params: vec![("f".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // float_sqrt(Float) -> Float
        self.define_fn(
            "float_sqrt".into(),
            FnSig {
                type_params: vec![],
                params: vec![("f".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // float_to_string(Float) -> String
        self.define_fn(
            "float_to_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("f".into(), Ty::Float, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // ── Phase PP: Math builtins (Trigonometric, Logarithmic, Rounding) ──

        // Trigonometric functions (Float -> Float)
        // sin(x: Float) -> Float
        self.define_fn(
            "sin".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // cos(x: Float) -> Float
        self.define_fn(
            "cos".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // tan(x: Float) -> Float
        self.define_fn(
            "tan".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // asin(x: Float) -> Float
        self.define_fn(
            "asin".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // acos(x: Float) -> Float
        self.define_fn(
            "acos".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // atan(x: Float) -> Float
        self.define_fn(
            "atan".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // atan2(y: Float, x: Float) -> Float
        self.define_fn(
            "atan2".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("y".into(), Ty::Float, false),
                    ("x".into(), Ty::Float, false),
                ],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // Logarithmic and exponential functions (Float -> Float)
        // log(x: Float) -> Float (natural logarithm)
        self.define_fn(
            "log".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // log10(x: Float) -> Float
        self.define_fn(
            "log10".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // log2(x: Float) -> Float
        self.define_fn(
            "log2".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // exp(x: Float) -> Float
        self.define_fn(
            "exp".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // exp2(x: Float) -> Float
        self.define_fn(
            "exp2".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // Rounding functions (Float -> Float)
        // ceil(x: Float) -> Float
        self.define_fn(
            "ceil".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // floor(x: Float) -> Float
        self.define_fn(
            "floor".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // round(x: Float) -> Float
        self.define_fn(
            "round".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // trunc(x: Float) -> Float
        self.define_fn(
            "trunc".into(),
            FnSig {
                type_params: vec![],
                params: vec![("x".into(), Ty::Float, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // Math constants (unit functions returning Float)
        // pi() -> Float
        self.define_fn(
            "pi".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // e() -> Float
        self.define_fn(
            "e".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // Additional math utilities
        // gcd(a: Int, b: Int) -> Int
        self.define_fn(
            "gcd".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Int, false), ("b".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // float_mod(a: Float, b: Float) -> Float
        self.define_fn(
            "float_mod".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("a".into(), Ty::Float, false),
                    ("b".into(), Ty::Float, false),
                ],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // clamp[T](value: T, min: T, max: T) -> T
        self.define_fn(
            "clamp".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    ("value".into(), Ty::TypeVar("T".into()), false),
                    ("min".into(), Ty::TypeVar("T".into()), false),
                    ("max".into(), Ty::TypeVar("T".into()), false),
                ],
                ret: Ty::TypeVar("T".into()),
                effects: vec![],
            },
        );

        // ── Result convenience functions ────────────────────────────────

        let result_ty = Ty::Enum {
            name: "Result".into(),
            variants: vec![
                ("Ok".into(), Some(Ty::TypeVar("T".into()))),
                ("Err".into(), Some(Ty::TypeVar("E".into()))),
            ],
        };

        // is_ok[T, E](Result[T, E]) -> Bool
        self.define_fn(
            "is_ok".into(),
            FnSig {
                type_params: vec!["T".into(), "E".into()],
                params: vec![("result".into(), result_ty.clone(), false)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // is_err[T, E](Result[T, E]) -> Bool
        self.define_fn(
            "is_err".into(),
            FnSig {
                type_params: vec!["T".into(), "E".into()],
                params: vec![("result".into(), result_ty, false)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // bool_to_string(Bool) -> String
        self.define_fn(
            "bool_to_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("b".into(), Ty::Bool, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // ── Standard I/O (Phase MM) ──────────────────────────────────────

        // read_line() -> !{IO} String
        self.define_fn(
            "read_line".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::String,
                effects: vec!["IO".into()],
            },
        );

        // parse_int(String) -> Int
        self.define_fn(
            "parse_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // parse_float(String) -> Float
        self.define_fn(
            "parse_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // exit(Int) -> !{IO} ()
        self.define_fn(
            "exit".into(),
            FnSig {
                type_params: vec![],
                params: vec![("code".into(), Ty::Int, false)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // args() -> !{IO} List[String]
        self.define_fn(
            "args".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::List(Box::new(Ty::String)),
                effects: vec!["IO".into()],
            },
        );

        // ── File I/O (FS effect) — Phase NN ─────────────────────────────

        // file_read(path: String) -> !{FS} String
        self.define_fn(
            "file_read".into(),
            FnSig {
                type_params: vec![],
                params: vec![("path".into(), Ty::String, false)],
                ret: Ty::String,
                effects: vec!["FS".into()],
            },
        );

        // file_write(path: String, content: String) -> !{FS} Bool
        self.define_fn(
            "file_write".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("path".into(), Ty::String, false),
                    ("content".into(), Ty::String, false),
                ],
                ret: Ty::Bool,
                effects: vec!["FS".into()],
            },
        );

        // file_exists(path: String) -> !{FS} Bool
        self.define_fn(
            "file_exists".into(),
            FnSig {
                type_params: vec![],
                params: vec![("path".into(), Ty::String, false)],
                ret: Ty::Bool,
                effects: vec!["FS".into()],
            },
        );

        // file_append(path: String, content: String) -> !{FS} Bool
        self.define_fn(
            "file_append".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("path".into(), Ty::String, false),
                    ("content".into(), Ty::String, false),
                ],
                ret: Ty::Bool,
                effects: vec!["FS".into()],
            },
        );

        // file_delete(path: String) -> !{FS} Bool
        self.define_fn(
            "file_delete".into(),
            FnSig {
                type_params: vec![],
                params: vec![("path".into(), Ty::String, false)],
                ret: Ty::Bool,
                effects: vec!["FS".into()],
            },
        );

        // ── Map operations (Phase OO) ────────────────────────────────────

        // map_new() -> Map[String, String]
        // Note: map_new is generic over the value type; the type checker uses
        // TypeVar("V") as a wildcard. The actual type is inferred from context.
        self.define_fn(
            "map_new".into(),
            FnSig {
                type_params: vec!["V".into()],
                params: vec![],
                ret: Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                effects: vec![],
            },
        );

        // map_set(m: Map[String, V], key: String, value: V) -> Map[String, V]
        self.define_fn(
            "map_set".into(),
            FnSig {
                type_params: vec!["V".into()],
                params: vec![
                    (
                        "m".into(),
                        Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                        false,
                    ),
                    ("key".into(), Ty::String, false),
                    ("value".into(), Ty::TypeVar("V".into()), false),
                ],
                ret: Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                effects: vec![],
            },
        );

        // map_get(m: Map[String, V], key: String) -> Option[V]
        let option_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![
                ("Some".into(), Some(Ty::TypeVar("V".into()))),
                ("None".into(), None),
            ],
        };
        self.define_fn(
            "map_get".into(),
            FnSig {
                type_params: vec!["V".into()],
                params: vec![
                    (
                        "m".into(),
                        Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                        false,
                    ),
                    ("key".into(), Ty::String, false),
                ],
                ret: option_ty.clone(),
                effects: vec![],
            },
        );

        // map_contains(m: Map[String, V], key: String) -> Bool
        self.define_fn(
            "map_contains".into(),
            FnSig {
                type_params: vec!["V".into()],
                params: vec![
                    (
                        "m".into(),
                        Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                        false,
                    ),
                    ("key".into(), Ty::String, false),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // map_remove(m: Map[String, V], key: String) -> Map[String, V]
        self.define_fn(
            "map_remove".into(),
            FnSig {
                type_params: vec!["V".into()],
                params: vec![
                    (
                        "m".into(),
                        Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                        false,
                    ),
                    ("key".into(), Ty::String, false),
                ],
                ret: Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                effects: vec![],
            },
        );

        // map_size(m: Map[String, V]) -> Int
        self.define_fn(
            "map_size".into(),
            FnSig {
                type_params: vec!["V".into()],
                params: vec![(
                    "m".into(),
                    Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                    false,
                )],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // map_keys(m: Map[String, V]) -> List[String]
        self.define_fn(
            "map_keys".into(),
            FnSig {
                type_params: vec!["V".into()],
                params: vec![(
                    "m".into(),
                    Ty::Map(Box::new(Ty::String), Box::new(Ty::TypeVar("V".into()))),
                    false,
                )],
                ret: Ty::List(Box::new(Ty::String)),
                effects: vec![],
            },
        );

        // ── HashMap operations (Self-Hosting Phase 1.1) ───────────────────
        // HashMap with generic keys requires Hash + Eq traits

        // hashmap_new[K, V]() -> HashMap[K, V]
        self.define_fn(
            "hashmap_new".into(),
            FnSig {
                type_params: vec!["K".into(), "V".into()],
                params: vec![],
                ret: Ty::HashMap(
                    Box::new(Ty::TypeVar("K".into())),
                    Box::new(Ty::TypeVar("V".into())),
                ),
                effects: vec![],
            },
        );

        // hashmap_insert(m: HashMap[K, V], key: K, value: V) -> Option[V]
        self.define_fn(
            "hashmap_insert".into(),
            FnSig {
                type_params: vec!["K".into(), "V".into()],
                params: vec![
                    (
                        "m".into(),
                        Ty::HashMap(
                            Box::new(Ty::TypeVar("K".into())),
                            Box::new(Ty::TypeVar("V".into())),
                        ),
                        false,
                    ),
                    ("key".into(), Ty::TypeVar("K".into()), false),
                    ("value".into(), Ty::TypeVar("V".into()), false),
                ],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![
                        ("Some".into(), Some(Ty::TypeVar("V".into()))),
                        ("None".into(), None),
                    ],
                },
                effects: vec![],
            },
        );

        // hashmap_get(m: HashMap[K, V], key: K) -> Option[V]
        self.define_fn(
            "hashmap_get".into(),
            FnSig {
                type_params: vec!["K".into(), "V".into()],
                params: vec![
                    (
                        "m".into(),
                        Ty::HashMap(
                            Box::new(Ty::TypeVar("K".into())),
                            Box::new(Ty::TypeVar("V".into())),
                        ),
                        false,
                    ),
                    ("key".into(), Ty::TypeVar("K".into()), false),
                ],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![
                        ("Some".into(), Some(Ty::TypeVar("V".into()))),
                        ("None".into(), None),
                    ],
                },
                effects: vec![],
            },
        );

        // hashmap_remove(m: HashMap[K, V], key: K) -> Option[V]
        self.define_fn(
            "hashmap_remove".into(),
            FnSig {
                type_params: vec!["K".into(), "V".into()],
                params: vec![
                    (
                        "m".into(),
                        Ty::HashMap(
                            Box::new(Ty::TypeVar("K".into())),
                            Box::new(Ty::TypeVar("V".into())),
                        ),
                        false,
                    ),
                    ("key".into(), Ty::TypeVar("K".into()), false),
                ],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![
                        ("Some".into(), Some(Ty::TypeVar("V".into()))),
                        ("None".into(), None),
                    ],
                },
                effects: vec![],
            },
        );

        // hashmap_contains(m: HashMap[K, V], key: K) -> Bool
        self.define_fn(
            "hashmap_contains".into(),
            FnSig {
                type_params: vec!["K".into(), "V".into()],
                params: vec![
                    (
                        "m".into(),
                        Ty::HashMap(
                            Box::new(Ty::TypeVar("K".into())),
                            Box::new(Ty::TypeVar("V".into())),
                        ),
                        false,
                    ),
                    ("key".into(), Ty::TypeVar("K".into()), false),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // hashmap_len(m: HashMap[K, V]) -> Int
        self.define_fn(
            "hashmap_len".into(),
            FnSig {
                type_params: vec!["K".into(), "V".into()],
                params: vec![(
                    "m".into(),
                    Ty::HashMap(
                        Box::new(Ty::TypeVar("K".into())),
                        Box::new(Ty::TypeVar("V".into())),
                    ),
                    false,
                )],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // hashmap_clear(m: HashMap[K, V]) -> Unit
        self.define_fn(
            "hashmap_clear".into(),
            FnSig {
                type_params: vec!["K".into(), "V".into()],
                params: vec![(
                    "m".into(),
                    Ty::HashMap(
                        Box::new(Ty::TypeVar("K".into())),
                        Box::new(Ty::TypeVar("V".into())),
                    ),
                    false,
                )],
                ret: Ty::Unit,
                effects: vec![],
            },
        );

        // ── Iterator Protocol (Self-Hosting Phase 1.2) ─────────────────────
        // Core iterator types and functions for lazy iteration over collections

        // list_iter[T](list: List[T]) -> Iterator[T]
        self.define_fn(
            "list_iter".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "list".into(),
                    Ty::List(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: Ty::Iterator(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // range_iter(start: Int, end: Int) -> Iterator[Int]
        self.define_fn(
            "range_iter".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("start".into(), Ty::Int, false),
                    ("end".into(), Ty::Int, false),
                ],
                ret: Ty::Iterator(Box::new(Ty::Int)),
                effects: vec![],
            },
        );

        // iter_next[T](iter: Iterator[T]) -> Option[T]
        self.define_fn(
            "iter_next".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "iter".into(),
                    Ty::Iterator(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![
                        ("Some".into(), Some(Ty::TypeVar("T".into()))),
                        ("None".into(), None),
                    ],
                },
                effects: vec![],
            },
        );

        // iter_has_next[T](iter: Iterator[T]) -> Bool
        self.define_fn(
            "iter_has_next".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "iter".into(),
                    Ty::Iterator(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // iter_count[T](iter: Iterator[T]) -> Int
        self.define_fn(
            "iter_count".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "iter".into(),
                    Ty::Iterator(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // ── StringBuilder (Self-Hosting Phase 1.3) ──────────────────────────
        // Efficient string construction with O(1) amortized append

        // stringbuilder_new() -> StringBuilder
        self.define_fn(
            "stringbuilder_new".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::StringBuilder,
                effects: vec![],
            },
        );

        // stringbuilder_new_with_capacity(capacity: Int) -> StringBuilder
        self.define_fn(
            "stringbuilder_with_capacity".into(),
            FnSig {
                type_params: vec![],
                params: vec![("capacity".into(), Ty::Int, false)],
                ret: Ty::StringBuilder,
                effects: vec![],
            },
        );

        // stringbuilder_append(builder: StringBuilder, s: String) -> StringBuilder
        self.define_fn(
            "stringbuilder_append".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("builder".into(), Ty::StringBuilder, false),
                    ("s".into(), Ty::String, false),
                ],
                ret: Ty::StringBuilder,
                effects: vec![],
            },
        );

        // stringbuilder_append_char(builder: StringBuilder, c: Int) -> StringBuilder
        self.define_fn(
            "stringbuilder_append_char".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("builder".into(), Ty::StringBuilder, false),
                    ("c".into(), Ty::Int, false),
                ],
                ret: Ty::StringBuilder,
                effects: vec![],
            },
        );

        // stringbuilder_append_int(builder: StringBuilder, n: Int) -> StringBuilder
        self.define_fn(
            "stringbuilder_append_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("builder".into(), Ty::StringBuilder, false),
                    ("n".into(), Ty::Int, false),
                ],
                ret: Ty::StringBuilder,
                effects: vec![],
            },
        );

        // stringbuilder_length(builder: StringBuilder) -> Int
        self.define_fn(
            "stringbuilder_length".into(),
            FnSig {
                type_params: vec![],
                params: vec![("builder".into(), Ty::StringBuilder, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // stringbuilder_capacity(builder: StringBuilder) -> Int
        self.define_fn(
            "stringbuilder_capacity".into(),
            FnSig {
                type_params: vec![],
                params: vec![("builder".into(), Ty::StringBuilder, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // stringbuilder_to_string(builder: StringBuilder) -> String
        self.define_fn(
            "stringbuilder_to_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("builder".into(), Ty::StringBuilder, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // stringbuilder_clear(builder: StringBuilder) -> StringBuilder
        self.define_fn(
            "stringbuilder_clear".into(),
            FnSig {
                type_params: vec![],
                params: vec![("builder".into(), Ty::StringBuilder, false)],
                ret: Ty::StringBuilder,
                effects: vec![],
            },
        );

        // ── File System Operations (Self-Hosting Phase 1.4) ─────────────────
        // Directory listing and file metadata for module discovery

        // file_list_directory(path: String) -> !{FS} List[String]
        self.define_fn(
            "file_list_directory".into(),
            FnSig {
                type_params: vec![],
                params: vec![("path".into(), Ty::String, false)],
                ret: Ty::List(Box::new(Ty::String)),
                effects: vec!["FS".into()],
            },
        );

        // file_is_directory(path: String) -> !{FS} Bool
        self.define_fn(
            "file_is_directory".into(),
            FnSig {
                type_params: vec![],
                params: vec![("path".into(), Ty::String, false)],
                ret: Ty::Bool,
                effects: vec!["FS".into()],
            },
        );

        // file_size(path: String) -> !{FS} Option[Int]
        self.define_fn(
            "file_size".into(),
            FnSig {
                type_params: vec![],
                params: vec![("path".into(), Ty::String, false)],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![("Some".into(), Some(Ty::Int)), ("None".into(), None)],
                },
                effects: vec!["FS".into()],
            },
        );

        // ── Set operations (Phase PP) ────────────────────────────────────

        // set_new[T]() -> Set[T]
        self.define_fn(
            "set_new".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![],
                ret: Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // set_add(s: Set[T], elem: T) -> Set[T]
        self.define_fn(
            "set_add".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    (
                        "s".into(),
                        Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                        false,
                    ),
                    ("elem".into(), Ty::TypeVar("T".into()), false),
                ],
                ret: Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // set_remove(s: Set[T], elem: T) -> Set[T]
        self.define_fn(
            "set_remove".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    (
                        "s".into(),
                        Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                        false,
                    ),
                    ("elem".into(), Ty::TypeVar("T".into()), false),
                ],
                ret: Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // set_contains(s: Set[T], elem: T) -> Bool
        self.define_fn(
            "set_contains".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    (
                        "s".into(),
                        Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                        false,
                    ),
                    ("elem".into(), Ty::TypeVar("T".into()), false),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // set_size(s: Set[T]) -> Int
        self.define_fn(
            "set_size".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "s".into(),
                    Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // set_union(a: Set[T], b: Set[T]) -> Set[T]
        self.define_fn(
            "set_union".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    (
                        "a".into(),
                        Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                        false,
                    ),
                    (
                        "b".into(),
                        Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                        false,
                    ),
                ],
                ret: Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // set_intersection(a: Set[T], b: Set[T]) -> Set[T]
        self.define_fn(
            "set_intersection".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    (
                        "a".into(),
                        Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                        false,
                    ),
                    (
                        "b".into(),
                        Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                        false,
                    ),
                ],
                ret: Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // set_to_list(s: Set[T]) -> List[T]
        self.define_fn(
            "set_to_list".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "s".into(),
                    Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: Ty::List(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // ── Phase PP: Queue Builtins ─────────────────────────────────────

        // queue_new[T]() -> Queue[T]
        self.define_fn(
            "queue_new".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![],
                ret: Ty::Queue(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // queue_enqueue[T](q: Queue[T], item: T) -> Queue[T]
        self.define_fn(
            "queue_enqueue".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    (
                        "q".into(),
                        Ty::Queue(Box::new(Ty::TypeVar("T".into()))),
                        false,
                    ),
                    ("item".into(), Ty::TypeVar("T".into()), false),
                ],
                ret: Ty::Queue(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // queue_dequeue[T](q: Queue[T]) -> Option[(T, Queue[T])]
        let dequeue_ret_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![
                (
                    "Some".into(),
                    Some(Ty::Tuple(vec![
                        Ty::TypeVar("T".into()),
                        Ty::Queue(Box::new(Ty::TypeVar("T".into()))),
                    ])),
                ),
                ("None".into(), None),
            ],
        };
        self.define_fn(
            "queue_dequeue".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "q".into(),
                    Ty::Queue(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: dequeue_ret_ty,
                effects: vec![],
            },
        );

        // queue_peek[T](q: Queue[T]) -> Option[T]
        let queue_peek_ret_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![
                ("Some".into(), Some(Ty::TypeVar("T".into()))),
                ("None".into(), None),
            ],
        };
        self.define_fn(
            "queue_peek".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "q".into(),
                    Ty::Queue(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: queue_peek_ret_ty,
                effects: vec![],
            },
        );

        // queue_size[T](q: Queue[T]) -> Int
        self.define_fn(
            "queue_size".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "q".into(),
                    Ty::Queue(Box::new(Ty::TypeVar("T".into()))),
                    false,
                )],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // ── Phase PP: String Utilities ────────────────────────────────────

        let option_string_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![("Some".into(), Some(Ty::String)), ("None".into(), None)],
        };

        // string_join(strings: List[String], separator: String) -> String
        self.define_fn(
            "string_join".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("strings".into(), Ty::List(Box::new(Ty::String)), false),
                    ("separator".into(), Ty::String, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_repeat(s: String, n: Int) -> String
        self.define_fn(
            "string_repeat".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("n".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_pad_left(s: String, n: Int, pad: String) -> String
        self.define_fn(
            "string_pad_left".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("n".into(), Ty::Int, false),
                    ("pad".into(), Ty::String, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_pad_right(s: String, n: Int, pad: String) -> String
        self.define_fn(
            "string_pad_right".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("n".into(), Ty::Int, false),
                    ("pad".into(), Ty::String, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_strip(s: String) -> String
        self.define_fn(
            "string_strip".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_strip_prefix(s: String, prefix: String) -> Option[String]
        self.define_fn(
            "string_strip_prefix".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("prefix".into(), Ty::String, false),
                ],
                ret: option_string_ty.clone(),
                effects: vec![],
            },
        );

        // string_strip_suffix(s: String, suffix: String) -> Option[String]
        self.define_fn(
            "string_strip_suffix".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("suffix".into(), Ty::String, false),
                ],
                ret: option_string_ty.clone(),
                effects: vec![],
            },
        );

        // string_to_int(s: String) -> Option[Int]
        let option_int_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![("Some".into(), Some(Ty::Int)), ("None".into(), None)],
        };
        self.define_fn(
            "string_to_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: option_int_ty.clone(),
                effects: vec![],
            },
        );

        // string_to_float(s: String) -> Option[Float]
        let option_float_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![("Some".into(), Some(Ty::Float)), ("None".into(), None)],
        };
        self.define_fn(
            "string_to_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: option_float_ty,
                effects: vec![],
            },
        );

        // ── HTTP Client Builtins (Net effect) — Phase RR ─────────────────

        let result_string_string = Ty::Enum {
            name: "Result".into(),
            variants: vec![
                ("Ok".into(), Some(Ty::String)),
                ("Err".into(), Some(Ty::String)),
            ],
        };

        // http_get(url: String) -> !{Net} Result[String, String]
        self.define_fn(
            "http_get".into(),
            FnSig {
                type_params: vec![],
                params: vec![("url".into(), Ty::String, false)],
                ret: result_string_string.clone(),
                effects: vec!["Net".into()],
            },
        );

        // http_post(url: String, body: String) -> !{Net} Result[String, String]
        self.define_fn(
            "http_post".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("url".into(), Ty::String, false),
                    ("body".into(), Ty::String, false),
                ],
                ret: result_string_string.clone(),
                effects: vec!["Net".into()],
            },
        );

        // http_post_json(url: String, json: String) -> !{Net} Result[String, String]
        self.define_fn(
            "http_post_json".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("url".into(), Ty::String, false),
                    ("json".into(), Ty::String, false),
                ],
                ret: result_string_string,
                effects: vec!["Net".into()],
            },
        );

        // ── JSON builtins ───────────────────────────────────────────────
        let json_value = Ty::Enum {
            name: "JsonValue".into(),
            variants: vec![],
        };
        let result_json_string = Ty::Enum {
            name: "Result".into(),
            variants: vec![
                ("Ok".into(), Some(json_value.clone())),
                ("Err".into(), Some(Ty::String)),
            ],
        };
        let option_json = Ty::Enum {
            name: "Option".into(),
            variants: vec![
                ("Some".into(), Some(json_value.clone())),
                ("None".into(), None),
            ],
        };

        self.define_fn(
            "json_parse".into(),
            FnSig {
                type_params: vec![],
                params: vec![("input".into(), Ty::String, false)],
                ret: result_json_string,
                effects: vec![],
            },
        );
        self.define_fn(
            "json_stringify".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value.clone(), false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "json_type".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value.clone(), false)],
                ret: Ty::String,
                effects: vec![],
            },
        );
        self.define_fn(
            "json_get".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("value".into(), json_value.clone(), false),
                    ("key".into(), Ty::String, false),
                ],
                ret: option_json,
                effects: vec![],
            },
        );
        self.define_fn(
            "json_is_null".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value.clone(), false)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );
        self.define_fn(
            "json_has".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("value".into(), json_value.clone(), false),
                    ("key".into(), Ty::String, false),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );
        self.define_fn(
            "json_keys".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value.clone(), false)],
                ret: Ty::List(Box::new(Ty::String)),
                effects: vec![],
            },
        );
        self.define_fn(
            "json_len".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value.clone(), false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
        self.define_fn(
            "json_array_get".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("value".into(), json_value.clone(), false),
                    ("index".into(), Ty::Int, false),
                ],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![
                        (
                            "Some".into(),
                            Some(Ty::Enum {
                                name: "JsonValue".into(),
                                variants: vec![],
                            }),
                        ),
                        ("None".into(), None),
                    ],
                },
                effects: vec![],
            },
        );
        // Typed JSON extractors
        self.define_fn(
            "json_as_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value.clone(), false)],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![("Some".into(), Some(Ty::String)), ("None".into(), None)],
                },
                effects: vec![],
            },
        );
        self.define_fn(
            "json_as_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value.clone(), false)],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![("Some".into(), Some(Ty::Int)), ("None".into(), None)],
                },
                effects: vec![],
            },
        );
        self.define_fn(
            "json_as_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value.clone(), false)],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![("Some".into(), Some(Ty::Float)), ("None".into(), None)],
                },
                effects: vec![],
            },
        );
        self.define_fn(
            "json_as_bool".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), json_value, false)],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![("Some".into(), Some(Ty::Bool)), ("None".into(), None)],
                },
                effects: vec![],
            },
        );

        // ── Phase PP: Random Number Generation ────────────────────────────

        // random() -> Float
        self.define_fn(
            "random".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // random_int(min: Int, max: Int) -> Int
        self.define_fn(
            "random_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("min".into(), Ty::Int, false),
                    ("max".into(), Ty::Int, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // random_float() -> Float
        self.define_fn(
            "random_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // seed_random(seed: Int) -> ()
        self.define_fn(
            "seed_random".into(),
            FnSig {
                type_params: vec![],
                params: vec![("seed".into(), Ty::Int, false)],
                ret: Ty::Unit,
                effects: vec![],
            },
        );

        // ── Phase PP: String Utilities Batch 2 ─────────────────────────────

        // string_format(fmt: String, args: List[String]) -> String
        self.define_fn(
            "string_format".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("fmt".into(), Ty::String, false),
                    ("args".into(), Ty::List(Box::new(Ty::String)), false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_is_empty(s: String) -> Bool
        self.define_fn(
            "string_is_empty".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // string_reverse(s: String) -> String
        self.define_fn(
            "string_reverse".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String, false)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_compare(a: String, b: String) -> Int
        // Returns negative if a < b, 0 if equal, positive if a > b
        self.define_fn(
            "string_compare".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("a".into(), Ty::String, false),
                    ("b".into(), Ty::String, false),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // string_find(s: String, substr: String) -> Option[Int]
        // Returns Some(index) if found, None if not found
        self.define_fn(
            "string_find".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("substr".into(), Ty::String, false),
                ],
                ret: Ty::Enum {
                    name: "Option".into(),
                    variants: vec![("Some".into(), Some(Ty::Int)), ("None".into(), None)],
                },
                effects: vec![],
            },
        );

        // string_slice(s: String, start: Int, end: Int) -> String
        // Extracts substring from start (inclusive) to end (exclusive)
        self.define_fn(
            "string_slice".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String, false),
                    ("start".into(), Ty::Int, false),
                    ("end".into(), Ty::Int, false),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // ── Phase PP: Date/Time Builtins ───────────────────────────────────

        // now() -> Int (Unix timestamp in seconds, !{Time})
        self.define_fn(
            "now".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec!["Time".into()],
            },
        );

        // now_ms() -> Int (Unix timestamp in milliseconds, !{Time})
        self.define_fn(
            "now_ms".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec!["Time".into()],
            },
        );

        // sleep(ms: Int) -> () (sleep for milliseconds, !{Time})
        self.define_fn(
            "sleep".into(),
            FnSig {
                type_params: vec![],
                params: vec![("ms".into(), Ty::Int, false)],
                ret: Ty::Unit,
                effects: vec!["Time".into()],
            },
        );

        // time_string() -> String (RFC3339 format, !{Time})
        self.define_fn(
            "time_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::String,
                effects: vec!["Time".into()],
            },
        );

        // date_string() -> String (YYYY-MM-DD, !{Time})
        self.define_fn(
            "date_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::String,
                effects: vec!["Time".into()],
            },
        );

        // datetime_year(ts: Int) -> Int (extract year from timestamp - pure)
        self.define_fn(
            "datetime_year".into(),
            FnSig {
                type_params: vec![],
                params: vec![("ts".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // datetime_month(ts: Int) -> Int (extract month 1-12 from timestamp - pure)
        self.define_fn(
            "datetime_month".into(),
            FnSig {
                type_params: vec![],
                params: vec![("ts".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // datetime_day(ts: Int) -> Int (extract day 1-31 from timestamp - pure)
        self.define_fn(
            "datetime_day".into(),
            FnSig {
                type_params: vec![],
                params: vec![("ts".into(), Ty::Int, false)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // ── Phase PP: Environment/Process Builtins ────────────────────────

        // get_env(name: String) -> Option[String] (!{IO})
        let option_string_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![("Some".into(), Some(Ty::String)), ("None".into(), None)],
        };
        self.define_fn(
            "get_env".into(),
            FnSig {
                type_params: vec![],
                params: vec![("name".into(), Ty::String, false)],
                ret: option_string_ty.clone(),
                effects: vec!["IO".into()],
            },
        );

        // set_env(name: String, value: String) -> () (!{IO})
        self.define_fn(
            "set_env".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("name".into(), Ty::String, false),
                    ("value".into(), Ty::String, false),
                ],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // current_dir() -> String (!{IO})
        self.define_fn(
            "current_dir".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::String,
                effects: vec!["IO".into()],
            },
        );

        // change_dir(path: String) -> () (!{IO})
        self.define_fn(
            "change_dir".into(),
            FnSig {
                type_params: vec![],
                params: vec![("path".into(), Ty::String, false)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // process_id() -> Int (pure - no effects)
        self.define_fn(
            "process_id".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // NOTE: system() builtin removed for security (RCE risk via shell injection)
        // Use @extern declaration if shell execution is absolutely required.

        // M-5: spawn(program: String, args: List[String]) -> Int (!{IO})
        // Executes a program directly without invoking a shell (safer than system()).
        // Returns the process exit code, or -1 on error.
        self.define_fn(
            "spawn".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("program".into(), Ty::String, false),
                    ("args".into(), Ty::List(Box::new(Ty::String)), false),
                ],
                ret: Ty::Int,
                effects: vec!["IO".into()],
            },
        );

        // sleep_seconds(s: Int) -> () (!{Time})
        self.define_fn(
            "sleep_seconds".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::Int, false)],
                ret: Ty::Unit,
                effects: vec!["Time".into()],
            },
        );

        // ── Generational References (Tier 2 Memory Model) ────────────────

        // Generic Option[T] return type for genref_get
        let option_t_ty = Ty::Enum {
            name: "Option".into(),
            variants: vec![
                ("Some".into(), Some(Ty::TypeVar("T".into()))),
                ("None".into(), None),
            ],
        };

        // genref_alloc[T](size: Int) -> GenRef[T]
        // Allocates memory with generation tracking
        self.define_fn(
            "genref_alloc".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![("size".into(), Ty::Int, false)],
                ret: Ty::GenRef {
                    inner: Box::new(Ty::TypeVar("T".into())),
                    cap: super::types::RefCap::Ref,
                },
                effects: vec![],
            },
        );

        // genref_new[T](ptr: GenRef[T]) -> GenRef[T]
        // Creates a new GenRef capturing current generation
        self.define_fn(
            "genref_new".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "ptr".into(),
                    Ty::GenRef {
                        inner: Box::new(Ty::TypeVar("T".into())),
                        cap: super::types::RefCap::Ref,
                    },
                    false,
                )],
                ret: Ty::GenRef {
                    inner: Box::new(Ty::TypeVar("T".into())),
                    cap: super::types::RefCap::Ref,
                },
                effects: vec![],
            },
        );

        // genref_get[T](ref: GenRef[T]) -> Option[T]
        // Validates generation and returns Some(value) or None
        self.define_fn(
            "genref_get".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![(
                    "ref".into(),
                    Ty::GenRef {
                        inner: Box::new(Ty::TypeVar("T".into())),
                        cap: super::types::RefCap::Ref,
                    },
                    false,
                )],
                ret: option_t_ty,
                effects: vec![],
            },
        );

        // genref_set[T](ref: GenRef[T], value: T) -> Bool
        // Updates allocation, increments generation, invalidates old refs
        self.define_fn(
            "genref_set".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    (
                        "ref".into(),
                        Ty::GenRef {
                            inner: Box::new(Ty::TypeVar("T".into())),
                            cap: super::types::RefCap::Ref,
                        },
                        false,
                    ),
                    ("value".into(), Ty::TypeVar("T".into()), false),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );
    }

    // ------------------------------------------------------------------
    // Accessors for completion context
    // ------------------------------------------------------------------

    /// Return all bindings visible from all scopes, from outermost to innermost.
    /// If a name appears in multiple scopes, the innermost one wins.
    pub fn all_bindings(&self) -> Vec<(String, Ty, bool)> {
        let mut seen: HashMap<String, Binding> = HashMap::new();
        // Walk outermost to innermost; later entries overwrite earlier ones.
        for scope in &self.scopes {
            for (name, binding) in scope {
                seen.insert(name.clone(), binding.clone());
            }
        }
        seen.into_iter()
            .map(|(name, binding)| {
                let mutable = self.mutable_vars.contains(&name);
                (name, binding.ty, mutable)
            })
            .collect()
    }

    /// Return all registered function signatures.
    pub fn all_functions(&self) -> &HashMap<String, FnSig> {
        &self.functions
    }

    /// Return all registered enum types.
    pub fn all_enums(&self) -> &HashMap<String, Ty> {
        &self.enums
    }
}
