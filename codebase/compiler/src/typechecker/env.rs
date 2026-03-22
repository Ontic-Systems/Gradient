//! Type environment (scope management) for the Gradient type checker.
//!
//! The [`TypeEnv`] maintains a stack of lexical scopes for variable bindings
//! and a separate registry for top-level function signatures. It also tracks
//! the current function's return type and the effects available in the current
//! context, enabling `ret` type checking and effect validation.

use std::collections::{HashMap, HashSet};

use super::types::Ty;

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
    /// Imported module namespaces: maps module name -> function registry.
    /// Used for qualified calls like `math.add(3, 4)`.
    imported_modules: HashMap<String, HashMap<String, FnSig>>,
    /// Registered actor types, keyed by actor name.
    actors: HashMap<String, ActorInfo>,
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
    // Imported modules
    // ------------------------------------------------------------------

    /// Register an imported module's function signatures.
    ///
    /// After this call, qualified references like `module_name.fn_name` can be
    /// resolved via [`lookup_qualified_fn`].
    pub fn import_module(&mut self, module_name: String, functions: HashMap<String, FnSig>) {
        self.imported_modules.insert(module_name, functions);
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
    // Builtins
    // ------------------------------------------------------------------

    /// Preload the environment with Gradient's built-in types.
    fn preload_types(&mut self) {
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
                params: vec![("value".into(), Ty::TypeVar("T".into()))],
                ret: result_enum_ty.clone(),
                effects: vec![],
            },
        );

        // Register Err as a non-generic constructor: Err(error) -> Result.
        self.define_fn(
            "Err".into(),
            FnSig {
                type_params: vec![],
                params: vec![("error".into(), Ty::TypeVar("E".into()))],
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
                params: vec![("value".into(), Ty::TypeVar("T".into()))],
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
    }

    /// Preload the environment with Gradient's built-in functions.
    fn preload_builtins(&mut self) {
        // print(String) -> !{IO} ()
        self.define_fn(
            "print".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::String)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // println(String) -> !{IO} ()
        self.define_fn(
            "println".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::String)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // range(Int) -> List[Int]  (simplified for v0.1: returns Unit, works in for loops)
        self.define_fn(
            "range".into(),
            FnSig {
                type_params: vec![],
                params: vec![("n".into(), Ty::Int)],
                ret: Ty::Unit, // simplified: for-loop handles iterable check specially
                effects: vec![],
            },
        );

        // to_string(Int) -> String  (convenience builtin)
        self.define_fn(
            "to_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Int)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // print_int(Int) -> !{IO} ()
        self.define_fn(
            "print_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Int)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // print_float(Float) -> !{IO} ()
        self.define_fn(
            "print_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Float)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // print_bool(Bool) -> !{IO} ()
        self.define_fn(
            "print_bool".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Bool)],
                ret: Ty::Unit,
                effects: vec!["IO".into()],
            },
        );

        // int_to_string(Int) -> String
        self.define_fn(
            "int_to_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("value".into(), Ty::Int)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // abs(Int) -> Int
        self.define_fn(
            "abs".into(),
            FnSig {
                type_params: vec![],
                params: vec![("n".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // min(Int, Int) -> Int
        self.define_fn(
            "min".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // max(Int, Int) -> Int
        self.define_fn(
            "max".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // mod_int(Int, Int) -> Int
        self.define_fn(
            "mod_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
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
                params: vec![("s".into(), Ty::String)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // string_contains(String, String) -> Bool
        self.define_fn(
            "string_contains".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String), ("substr".into(), Ty::String)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // string_starts_with(String, String) -> Bool
        self.define_fn(
            "string_starts_with".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String), ("prefix".into(), Ty::String)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // string_ends_with(String, String) -> Bool
        self.define_fn(
            "string_ends_with".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String), ("suffix".into(), Ty::String)],
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
                    ("s".into(), Ty::String),
                    ("start".into(), Ty::Int),
                    ("end".into(), Ty::Int),
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
                params: vec![("s".into(), Ty::String)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_to_upper(String) -> String
        self.define_fn(
            "string_to_upper".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_to_lower(String) -> String
        self.define_fn(
            "string_to_lower".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String)],
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
                    ("s".into(), Ty::String),
                    ("old".into(), Ty::String),
                    ("new_str".into(), Ty::String),
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
                params: vec![("s".into(), Ty::String), ("substr".into(), Ty::String)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // string_char_at(String, Int) -> String
        self.define_fn(
            "string_char_at".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String), ("index".into(), Ty::Int)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_split(String, String) -> String (first token for v0.1)
        self.define_fn(
            "string_split".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String), ("delimiter".into(), Ty::String)],
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
                params: vec![("f".into(), Ty::Float)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // int_to_float(Int) -> Float
        self.define_fn(
            "int_to_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![("n".into(), Ty::Int)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // pow(Int, Int) -> Int
        self.define_fn(
            "pow".into(),
            FnSig {
                type_params: vec![],
                params: vec![("base".into(), Ty::Int), ("exp".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // float_abs(Float) -> Float
        self.define_fn(
            "float_abs".into(),
            FnSig {
                type_params: vec![],
                params: vec![("f".into(), Ty::Float)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // float_sqrt(Float) -> Float
        self.define_fn(
            "float_sqrt".into(),
            FnSig {
                type_params: vec![],
                params: vec![("f".into(), Ty::Float)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // float_to_string(Float) -> String
        self.define_fn(
            "float_to_string".into(),
            FnSig {
                type_params: vec![],
                params: vec![("f".into(), Ty::Float)],
                ret: Ty::String,
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

        // is_ok(Result) -> Bool
        self.define_fn(
            "is_ok".into(),
            FnSig {
                type_params: vec![],
                params: vec![("result".into(), result_ty.clone())],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // is_err(Result) -> Bool
        self.define_fn(
            "is_err".into(),
            FnSig {
                type_params: vec![],
                params: vec![("result".into(), result_ty)],
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
        let mut seen = HashMap::new();
        // Walk outermost to innermost; later entries overwrite earlier ones.
        for scope in &self.scopes {
            for (name, ty) in scope {
                seen.insert(name.clone(), ty.clone());
            }
        }
        seen.into_iter()
            .map(|(name, ty)| {
                let mutable = self.mutable_vars.contains(&name);
                (name, ty, mutable)
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
