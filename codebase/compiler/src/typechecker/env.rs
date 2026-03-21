//! Type environment (scope management) for the Gradient type checker.
//!
//! The [`TypeEnv`] maintains a stack of lexical scopes for variable bindings
//! and a separate registry for top-level function signatures. It also tracks
//! the current function's return type and the effects available in the current
//! context, enabling `ret` type checking and effect validation.

use std::collections::{HashMap, HashSet};

use super::types::Ty;

/// The signature of a function, as recorded in the type environment.
#[derive(Debug, Clone)]
pub struct FnSig {
    /// The parameters: each is a `(name, type)` pair.
    pub params: Vec<(String, Ty)>,
    /// The return type.
    pub ret: Ty,
    /// The effects declared on this function.
    pub effects: Vec<String>,
}

/// The type environment used during type checking.
///
/// It maintains:
/// - A stack of lexical scopes for variable lookups.
/// - A flat registry of function signatures for call resolution.
/// - Context about the current function being checked (return type, effects).
pub struct TypeEnv {
    /// Stack of variable scopes. The last element is the innermost scope.
    scopes: Vec<HashMap<String, Ty>>,
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
        };
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
        debug_assert!(
            self.scopes.len() > 1,
            "cannot pop the global scope"
        );
        self.scopes.pop();
    }

    /// Define a variable in the current (innermost) scope.
    pub fn define(&mut self, name: String, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, ty);
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
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
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

    // ------------------------------------------------------------------
    // Builtins
    // ------------------------------------------------------------------

    /// Preload the environment with Gradient's built-in functions.
    fn preload_builtins(&mut self) {
        // print(String) -> !{IO} ()
        self.define_fn(
            "print".into(),
            FnSig {
                params: vec![("value".into(), Ty::String)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // println(String) -> !{IO} ()
        self.define_fn(
            "println".into(),
            FnSig {
                params: vec![("value".into(), Ty::String)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // range(Int) -> List[Int]  (simplified for v0.1: returns Unit, works in for loops)
        self.define_fn(
            "range".into(),
            FnSig {
                params: vec![("n".into(), Ty::Int)],
                ret: Ty::Unit, // simplified: for-loop handles iterable check specially
                effects: vec![],
            },
        );

        // to_string(Int) -> String  (convenience builtin)
        self.define_fn(
            "to_string".into(),
            FnSig {
                params: vec![("value".into(), Ty::Int)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // print_int(Int) -> !{IO} ()
        self.define_fn(
            "print_int".into(),
            FnSig {
                params: vec![("value".into(), Ty::Int)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // print_float(Float) -> !{IO} ()
        self.define_fn(
            "print_float".into(),
            FnSig {
                params: vec![("value".into(), Ty::Float)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // print_bool(Bool) -> !{IO} ()
        self.define_fn(
            "print_bool".into(),
            FnSig {
                params: vec![("value".into(), Ty::Bool)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // int_to_string(Int) -> String
        self.define_fn(
            "int_to_string".into(),
            FnSig {
                params: vec![("value".into(), Ty::Int)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // abs(Int) -> Int
        self.define_fn(
            "abs".into(),
            FnSig {
                params: vec![("n".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // min(Int, Int) -> Int
        self.define_fn(
            "min".into(),
            FnSig {
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // max(Int, Int) -> Int
        self.define_fn(
            "max".into(),
            FnSig {
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // mod_int(Int, Int) -> Int
        self.define_fn(
            "mod_int".into(),
            FnSig {
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );
    }
}
