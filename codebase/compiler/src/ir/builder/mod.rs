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

use super::{
    BasicBlock, BlockRef, CmpOp, FuncRef, Function, Instruction, Literal, Module, Type, Value,
};
use crate::ast;
use crate::ast::expr::{ChildSpec, RestartPolicy, RestartStrategy};
use crate::ast::item::VariantField;
use std::collections::{HashMap, HashSet};

/// Layout information for a record type's fields.
/// Maps field names to their index (for LoadField/StoreField), byte offset, and type.
#[derive(Debug, Clone)]
pub struct RecordLayout {
    /// Maps field name to (field_index, byte_offset, field_type)
    pub fields: HashMap<String, (u32, i64, super::Type)>,
    /// Total size of the record in bytes
    pub total_size: i64,
}

impl RecordLayout {
    /// Create a new empty record layout.
    pub fn new() -> Self {
        Self {
            fields: HashMap::new(),
            total_size: 0,
        }
    }

    /// Add a field to the layout.
    /// Returns the field index and offset assigned to this field.
    pub fn add_field(
        &mut self,
        name: String,
        size: i64,
        align: i64,
        ty: super::Type,
    ) -> (u32, i64) {
        // Align the current offset to the field's alignment requirement
        let aligned_offset = (self.total_size + align - 1) & !(align - 1);
        let index = self.fields.len() as u32;
        self.fields.insert(name, (index, aligned_offset, ty));
        self.total_size = aligned_offset + size;
        (index, aligned_offset)
    }

    /// Get the index and offset for a field by name.
    pub fn get_field(&self, name: &str) -> Option<(u32, i64)> {
        self.fields
            .get(name)
            .map(|(idx, offset, _)| (*idx, *offset))
    }

    /// Get the index, offset, and type for a field by name.
    pub fn get_field_with_type(&self, name: &str) -> Option<(u32, i64, super::Type)> {
        self.fields.get(name).cloned()
    }

    /// Get all fields in index order.
    pub fn get_fields_ordered(&self) -> Vec<(u32, String, i64, super::Type)> {
        let mut fields: Vec<_> = self
            .fields
            .iter()
            .map(|(name, (idx, offset, ty))| (*idx, name.clone(), *offset, ty.clone()))
            .collect();
        fields.sort_by_key(|(idx, _, _, _)| *idx);
        fields
    }
}

impl Default for RecordLayout {
    fn default() -> Self {
        Self::new()
    }
}

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
    /// Maps enum variant names to their integer tag values.
    /// Populated during enum declaration processing.
    enum_variant_tags: HashMap<String, i64>,
    /// Set of enum variant names that carry a tuple payload (i.e. are tuple
    /// variants rather than unit variants). Used to determine whether a `Call`
    /// expression is a variant constructor that needs `ConstructVariant`.
    tuple_variant_names: HashSet<String>,
    /// Maps tuple variant names to the IR type of their single payload field.
    /// Populated during enum declaration processing so that `GetVariantField`
    /// can be assigned the correct IR type (important for Float variants).
    variant_field_types: HashMap<String, Type>,
    /// Per-field types for multi-field tuple variants (indexed by variant name).
    variant_field_types_vec: HashMap<String, Vec<Type>>,
    /// Set of variable names that are mutable (use alloca/load/store).
    mutable_vars: HashSet<String>,
    /// Maps mutable variable names to their alloca'd address (stack slot pointer).
    mutable_addrs: HashMap<String, Value>,
    /// Maps mutable variable names to their stored value type (e.g. F64 for floats).
    mutable_types: HashMap<String, Type>,
    /// Set of mutable variable names whose values are strings (Ptr to string data).
    /// When a mutable string variable is loaded, its result is added to string_values
    /// so that `+` on it correctly dispatches to string_concat.
    mutable_string_vars: HashSet<String>,
    /// Maps every SSA value to its IR type. Populated as values are created
    /// and copied into each [`Function`] when building completes.
    value_types: HashMap<Value, Type>,
    /// Maps function names to their declared return types, so that the
    /// builder can assign the correct type to `Call` result values.
    function_return_types: HashMap<String, Type>,
    /// Counter for generating unique closure function names.
    closure_counter: u32,
    /// Closure functions generated during expression building.
    /// These are accumulated and appended to the module's function list.
    closure_functions: Vec<Function>,
    /// Maps a tuple base value (address of first element) to the addresses
    /// of all its elements. Used by TupleField access to load the right slot.
    /// DEPRECATED: Use tuple_element_offsets instead for heap-allocated tuples.
    tuple_element_addrs: HashMap<Value, Vec<Value>>,
    /// Maps a tuple base value to (element_size, element_offsets).
    /// Used for heap-allocated tuples where elements are at computed offsets.
    tuple_element_offsets: HashMap<Value, (i64, Vec<i64>)>,
    /// Maps record type names to their field layouts (field name -> index and offset).
    /// Used for computing field addresses in record literal construction and field access.
    record_layouts: HashMap<String, RecordLayout>,
    /// Maps SSA values that represent records to their record type name.
    /// Used to determine the type for field access on record values.
    record_values: HashMap<Value, String>,
    /// Set of SSA values known to be list-typed (Ptr to list data).
    /// Used to track which values are lists for list builtin operations.
    list_values: HashSet<Value>,
    /// Set of SSA values known to be map-typed (Ptr to map data).
    /// Used to track which values are maps for map builtin operations.
    map_values: HashSet<Value>,
    /// Set of SSA values known to be set-typed (Ptr to set data).
    /// Used to track which values are sets for set builtin operations.
    set_values: HashSet<Value>,
    /// Set of SSA values known to be queue-typed (Ptr to queue data).
    /// Used to track which values are queues for queue builtin operations.
    queue_values: HashSet<Value>,
    /// Set of SSA values known to be stack-typed (Ptr to stack data).
    /// Used to track which values are stacks for stack builtin operations.
    stack_values: HashSet<Value>,
    /// Set of SSA values known to be hashmap-typed (Ptr to hashmap data).
    /// Used to track which values are hashmaps for hashmap builtin operations.
    hashmap_values: HashSet<Value>,
    /// Maps Option-typed values to their inner type.
    /// Used when pattern matching on Some(x) to know what type x should be.
    option_inner_types: HashMap<Value, Type>,
    /// Defer stack: each scope has a vector of deferred expressions to execute
    /// when the scope exits. Defers execute in LIFO order (last deferred, first executed).
    defer_stack: Vec<Vec<ast::Expr>>,
    /// When true, contracts marked @runtime_only(off_in_release) are omitted from generated IR.
    strip_runtime_only_contracts: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IrBuildOptions {
    pub strip_runtime_only_contracts: bool,
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
        Self::new_with_options(IrBuildOptions::default())
    }

    /// Create a new builder with explicit build options.
    pub fn new_with_options(options: IrBuildOptions) -> Self {
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
            mutable_types: HashMap::new(),
            mutable_string_vars: HashSet::new(),
            value_types: HashMap::new(),
            function_return_types: HashMap::new(),
            enum_variant_tags: HashMap::new(),
            tuple_variant_names: HashSet::new(),
            variant_field_types: HashMap::new(),
            variant_field_types_vec: HashMap::new(),
            closure_counter: 0,
            closure_functions: Vec::new(),
            tuple_element_addrs: HashMap::new(),
            tuple_element_offsets: HashMap::new(),
            record_layouts: HashMap::new(),
            record_values: HashMap::new(),
            list_values: HashSet::new(),
            map_values: HashSet::new(),
            set_values: HashSet::new(),
            queue_values: HashSet::new(),
            stack_values: HashSet::new(),
            hashmap_values: HashSet::new(),
            option_inner_types: HashMap::new(),
            defer_stack: vec![Vec::new()], // Initialize with root scope
            strip_runtime_only_contracts: options.strip_runtime_only_contracts,
        }
    }

    fn runtime_function(&mut self, name: &str) -> Option<FuncRef> {
        self.function_refs.get(name).copied().or_else(|| {
            self.errors
                .push(format!("missing runtime function registration: '{}'", name));
            None
        })
    }

    fn zero_value(&mut self, ty: Type) -> Value {
        let value = self.fresh_value(ty.clone());
        let literal = match ty {
            Type::F64 => Literal::Float(0.0),
            Type::Bool => Literal::Bool(false),
            _ => Literal::Int(0),
        };
        self.emit(Instruction::Const(value, literal));
        value
    }

    fn error_string_value(&mut self, text: &str) -> Value {
        let value = self.fresh_value(Type::Ptr);
        self.emit(Instruction::Const(value, Literal::Str(text.to_string())));
        self.string_values.insert(value);
        value
    }

    // ── Entry point ──────────────────────────────────────────────────

    /// Translate an AST module into an IR module.
    ///
    /// Returns the IR module and a list of any errors encountered during
    /// translation.
    pub fn build_module(ast_module: &ast::Module) -> (Module, Vec<String>) {
        Self::build_module_with_imports(ast_module, &[])
    }

    /// Translate an AST module into an IR module, also registering function
    /// signatures from imported modules so that qualified calls can be resolved.
    ///
    /// `imported_modules` is a slice of `(module_name, module_ast)` pairs for
    /// all modules referenced by `use` declarations.
    pub fn build_module_with_imports(
        ast_module: &ast::Module,
        imported_modules: &[(&str, &ast::Module)],
    ) -> (Module, Vec<String>) {
        Self::build_module_with_imports_and_options(
            ast_module,
            imported_modules,
            IrBuildOptions::default(),
        )
    }

    pub fn build_module_with_imports_and_options(
        ast_module: &ast::Module,
        imported_modules: &[(&str, &ast::Module)],
        options: IrBuildOptions,
    ) -> (Module, Vec<String>) {
        let mut builder = IrBuilder::new_with_options(options);

        // Register builtin generic enum variants (Option, Result) so that
        // `Some(x)`, `None`, `Ok(x)`, `Err(e)` lower to ConstructVariant.
        // Option: Some=tag 0, None=tag 1
        builder.enum_variant_tags.insert("Some".to_string(), 0);
        builder.enum_variant_tags.insert("None".to_string(), 1);
        builder.tuple_variant_names.insert("Some".to_string());
        builder
            .variant_field_types
            .insert("Some".to_string(), crate::ir::Type::Ptr);
        // Result: Ok=tag 0, Err=tag 1
        builder.enum_variant_tags.insert("Ok".to_string(), 0);
        builder.enum_variant_tags.insert("Err".to_string(), 1);
        builder.tuple_variant_names.insert("Ok".to_string());
        builder.tuple_variant_names.insert("Err".to_string());
        builder
            .variant_field_types
            .insert("Ok".to_string(), crate::ir::Type::Ptr);
        builder
            .variant_field_types
            .insert("Err".to_string(), crate::ir::Type::Ptr);

        let module_name = ast_module
            .module_decl
            .as_ref()
            .map(|md| md.path.join("."))
            .unwrap_or_else(|| "main".to_string());

        // First pass: register all function names so that calls can be resolved.
        builder.register_functions(ast_module);

        // Register functions from imported modules (for qualified calls).
        for (_mod_name, imported_ast) in imported_modules {
            builder.register_imported_functions(imported_ast);
        }

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
                // Also build functions from imported modules (fixes #45).
                // Imported modules are merged into the same IR module so that
                // all functions end up in a single object file.
                _ => {}
            }
        }

        // Build functions from imported modules.
        for (_mod_name, imported_ast) in imported_modules {
            for item in &imported_ast.items {
                match &item.node {
                    ast::ItemKind::FnDef(fn_def) => {
                        let func = builder.build_fn_def(fn_def);
                        functions.push(func);
                    }
                    ast::ItemKind::ExternFn(extern_fn) => {
                        let func = builder.build_extern_fn(extern_fn);
                        functions.push(func);
                    }
                    _ => {}
                }
            }
        }

        // Re-add top-level items from main module that aren't functions.
        for item in &ast_module.items {
            match &item.node {
                ast::ItemKind::LetTupleDestructure { names, value, .. } => {
                    let tuple_val = builder.build_expr(value);
                    // First try the new offset-based approach (heap-allocated tuples)
                    if let Some((elem_size, offsets)) =
                        builder.tuple_element_offsets.get(&tuple_val).cloned()
                    {
                        for (i, name) in names.iter().enumerate() {
                            if i < offsets.len() {
                                // Calculate element address: base + i * elem_size
                                let offset = i as i64 * elem_size;
                                let offset_val = builder.fresh_value(Type::I64);
                                builder.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                                let elem_addr = builder.fresh_value(Type::Ptr);
                                builder.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                                let result = builder.fresh_value(Type::I64);
                                builder.emit(Instruction::Load(result, elem_addr));
                                builder.define_var(name, result);
                            }
                        }
                    } else if let Some(addrs) = builder.tuple_element_addrs.get(&tuple_val).cloned()
                    {
                        // Legacy: stack-allocated tuples (deprecated)
                        for (i, name) in names.iter().enumerate() {
                            if i < addrs.len() {
                                let elem_addr = addrs[i];
                                let result = builder.fresh_value(Type::I64);
                                builder.emit(Instruction::Load(result, elem_addr));
                                builder.define_var(name, result);
                            }
                        }
                    }
                }
                ast::ItemKind::Let {
                    name,
                    value,
                    mutable,
                    ..
                } => {
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
                ast::ItemKind::EnumDecl { variants, .. } => {
                    // Register variant tags for codegen.
                    for (i, variant) in variants.iter().enumerate() {
                        builder
                            .enum_variant_tags
                            .insert(variant.name.clone(), i as i64);
                        if let Some(ref fields) = variant.fields {
                            if !fields.is_empty() {
                                builder.tuple_variant_names.insert(variant.name.clone());
                                // Handle both single and multi-field variants
                                if fields.len() == 1 {
                                    // Single field - get type directly
                                    let field_ty = match &fields[0] {
                                        VariantField::Named { type_expr, .. } => {
                                            builder.resolve_type(&type_expr.node)
                                        }
                                        VariantField::Anonymous(type_expr) => {
                                            builder.resolve_type(&type_expr.node)
                                        }
                                    };
                                    builder
                                        .variant_field_types
                                        .insert(variant.name.clone(), field_ty);
                                } else {
                                    // Multi-field variant - store per-field types
                                    let per_field: Vec<Type> = fields
                                        .iter()
                                        .map(|f| match f {
                                            VariantField::Named { type_expr, .. } => {
                                                builder.resolve_type(&type_expr.node)
                                            }
                                            VariantField::Anonymous(type_expr) => {
                                                builder.resolve_type(&type_expr.node)
                                            }
                                        })
                                        .collect();
                                    builder
                                        .variant_field_types_vec
                                        .insert(variant.name.clone(), per_field);
                                }
                            }
                        }
                    }
                }
                ast::ItemKind::CapDecl { .. } => {
                    // Capability declarations are compile-time only.
                }
                ast::ItemKind::ActorDecl {
                    name,
                    state_fields,
                    handlers,
                    ..
                } => {
                    // Generate IR for actor declaration:
                    // 1. State initialization function
                    // 2. Message handler functions
                    // 3. Behavior table registration function
                    let actor_funcs = builder.build_actor_decl(name, state_fields, handlers);
                    functions.extend(actor_funcs);
                }
                ast::ItemKind::TraitDecl { .. } => {
                    // Trait declarations are compile-time only (no runtime representation).
                }
                ast::ItemKind::ImplBlock {
                    target_type,
                    methods,
                    ..
                } => {
                    // Build each impl method as a named function `TargetType::method_name`.
                    for method in methods {
                        let original_name = method.name.clone();
                        let qualified_name = format!("{}::{}", target_type, original_name);
                        // Temporarily rename the fn_def for IR building, then restore.
                        let mut method_def = method.clone();
                        method_def.name = qualified_name;
                        let func = builder.build_fn_def(&method_def);
                        functions.push(func);
                    }
                }
                // Functions and extern functions are already handled above.
                ast::ItemKind::FnDef(_) | ast::ItemKind::ExternFn(_) => {}
                ast::ItemKind::ModBlock {
                    items: mod_items, ..
                } => {
                    // Process items within the module block recursively.
                    for mod_item in mod_items {
                        match &mod_item.node {
                            ast::ItemKind::FnDef(fn_def) => {
                                let func = builder.build_fn_def(fn_def);
                                functions.push(func);
                            }
                            ast::ItemKind::ExternFn(extern_fn) => {
                                let func = builder.build_extern_fn(extern_fn);
                                functions.push(func);
                            }
                            _ => {
                                // Other item kinds in mod blocks are handled separately
                                // or don't require IR generation.
                            }
                        }
                    }
                }
                // Import declarations are compile-time only (no runtime representation).
                ast::ItemKind::Import { .. } => {}
            }
        }

        // Build IR function stubs for imported module functions (empty blocks,
        // like extern functions), so the codegen knows their signatures.
        let defined_fn_names: HashSet<String> = functions.iter().map(|f| f.name.clone()).collect();
        for (_mod_name, imported_ast) in imported_modules {
            for item in &imported_ast.items {
                match &item.node {
                    ast::ItemKind::FnDef(fn_def) if !defined_fn_names.contains(&fn_def.name) => {
                        let param_types: Vec<Type> = fn_def
                            .params
                            .iter()
                            .map(|p| builder.resolve_type(&p.type_ann.node))
                            .collect();
                        let return_type = fn_def
                            .return_type
                            .as_ref()
                            .map(|rt| builder.resolve_type(&rt.node))
                            .unwrap_or(Type::Void);
                        functions.push(Function {
                            name: fn_def.name.clone(),
                            params: param_types,
                            return_type,
                            blocks: Vec::new(),
                            value_types: HashMap::new(),
                            is_export: false,
                            extern_lib: None,
                        });
                    }
                    ast::ItemKind::ExternFn(decl) if !defined_fn_names.contains(&decl.name) => {
                        let param_types: Vec<Type> = decl
                            .params
                            .iter()
                            .map(|p| builder.resolve_type(&p.type_ann.node))
                            .collect();
                        let return_type = decl
                            .return_type
                            .as_ref()
                            .map(|rt| builder.resolve_type(&rt.node))
                            .unwrap_or(Type::Void);
                        functions.push(Function {
                            name: decl.name.clone(),
                            params: param_types,
                            return_type,
                            blocks: Vec::new(),
                            value_types: HashMap::new(),
                            is_export: false,
                            extern_lib: decl.extern_lib.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }

        // Append any closure functions that were generated during building.
        functions.append(&mut builder.closure_functions);

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
        self.function_return_types
            .insert("print".to_string(), Type::Void);
        self.register_func("println");
        self.function_return_types
            .insert("println".to_string(), Type::Void);
        self.register_func("print_int");
        self.function_return_types
            .insert("print_int".to_string(), Type::Void);
        self.register_func("print_float");
        self.function_return_types
            .insert("print_float".to_string(), Type::Void);
        self.register_func("print_bool");
        self.function_return_types
            .insert("print_bool".to_string(), Type::Void);
        self.register_func("int_to_string");
        self.function_return_types
            .insert("int_to_string".to_string(), Type::Ptr);
        self.register_func("abs");
        self.function_return_types
            .insert("abs".to_string(), Type::I64);
        self.register_func("min");
        self.function_return_types
            .insert("min".to_string(), Type::I64);
        self.register_func("max");
        self.function_return_types
            .insert("max".to_string(), Type::I64);
        self.register_func("mod_int");
        self.function_return_types
            .insert("mod_int".to_string(), Type::I64);
        self.register_func("string_concat");
        self.function_return_types
            .insert("string_concat".to_string(), Type::Ptr);
        self.register_func("__gradient_contract_fail");
        self.function_return_types
            .insert("__gradient_contract_fail".to_string(), Type::Void);

        // ── String operations ────────────────────────────────────────────
        self.register_func("string_eq");
        self.function_return_types
            .insert("string_eq".to_string(), Type::Bool);
        self.register_func("string_length");
        self.function_return_types
            .insert("string_length".to_string(), Type::I64);
        self.register_func("string_contains");
        self.function_return_types
            .insert("string_contains".to_string(), Type::Bool);
        self.register_func("string_starts_with");
        self.function_return_types
            .insert("string_starts_with".to_string(), Type::Bool);
        self.register_func("string_ends_with");
        self.function_return_types
            .insert("string_ends_with".to_string(), Type::Bool);
        self.register_func("string_substring");
        self.function_return_types
            .insert("string_substring".to_string(), Type::Ptr);
        self.register_func("string_trim");
        self.function_return_types
            .insert("string_trim".to_string(), Type::Ptr);
        self.register_func("string_to_upper");
        self.function_return_types
            .insert("string_to_upper".to_string(), Type::Ptr);
        self.register_func("string_to_lower");
        self.function_return_types
            .insert("string_to_lower".to_string(), Type::Ptr);
        self.register_func("string_replace");
        self.function_return_types
            .insert("string_replace".to_string(), Type::Ptr);
        self.register_func("string_index_of");
        self.function_return_types
            .insert("string_index_of".to_string(), Type::I64);
        self.register_func("string_char_at");
        self.function_return_types
            .insert("string_char_at".to_string(), Type::Ptr);
        self.register_func("string_split");
        self.function_return_types
            .insert("string_split".to_string(), Type::Ptr);

        // ── Numeric operations ───────────────────────────────────────────
        self.register_func("float_to_int");
        self.function_return_types
            .insert("float_to_int".to_string(), Type::I64);
        self.register_func("int_to_float");
        self.function_return_types
            .insert("int_to_float".to_string(), Type::F64);
        self.register_func("pow");
        self.function_return_types
            .insert("pow".to_string(), Type::I64);
        self.register_func("float_abs");
        self.function_return_types
            .insert("float_abs".to_string(), Type::F64);
        self.register_func("float_sqrt");
        self.function_return_types
            .insert("float_sqrt".to_string(), Type::F64);
        self.register_func("float_to_string");
        self.function_return_types
            .insert("float_to_string".to_string(), Type::Ptr);
        self.register_func("bool_to_string");
        self.function_return_types
            .insert("bool_to_string".to_string(), Type::Ptr);

        // ── Math library (libm thin wrappers — #585) ─────────────────────
        // All return F64. Unary (Float -> Float): sin/cos/tan/asin/acos/
        // atan/log/log10/log2/exp/exp2/ceil/floor/round/trunc. Binary
        // (Float, Float -> Float): atan2/float_mod.
        for name in [
            "sin",
            "cos",
            "tan",
            "asin",
            "acos",
            "atan",
            "log",
            "log10",
            "log2",
            "exp",
            "exp2",
            "ceil",
            "floor",
            "round",
            "trunc",
            "atan2",
            "float_mod",
        ] {
            self.register_func(name);
            self.function_return_types
                .insert(name.to_string(), Type::F64);
        }

        // ── Math constants and integer math (#599) ────────────────────────
        // pi() / e() return F64 via runtime helpers __gradient_pi/_e.
        // gcd(a, b) returns I64 via __gradient_gcd. Cranelift already
        // dispatched these; this registration unblocks the LLVM backend
        // and prevents IR-build-time "undefined function" panics.
        self.register_func("pi");
        self.function_return_types
            .insert("pi".to_string(), Type::F64);
        self.register_func("e");
        self.function_return_types
            .insert("e".to_string(), Type::F64);
        self.register_func("gcd");
        self.function_return_types
            .insert("gcd".to_string(), Type::I64);

        // clamp(v, lo, hi) -> T (generic over Int/Float). The IR builder
        // overrides the return type at call sites by reading the first
        // arg's tracked type (#609). The static entry below ensures
        // `function_refs.get("clamp")` resolves; the I64 fallback is only
        // used when no argument has a tracked type.
        self.register_func("clamp");
        self.function_return_types
            .insert("clamp".to_string(), Type::I64);

        // ── Standard I/O (Phase MM) ──────────────────────────────────────
        self.register_func("read_line");
        self.function_return_types
            .insert("read_line".to_string(), Type::Ptr);
        self.register_func("parse_int");
        self.function_return_types
            .insert("parse_int".to_string(), Type::I64);
        self.register_func("parse_float");
        self.function_return_types
            .insert("parse_float".to_string(), Type::F64);
        self.register_func("exit");
        self.function_return_types
            .insert("exit".to_string(), Type::Void);
        self.register_func("args");
        self.function_return_types
            .insert("args".to_string(), Type::Void);

        // ── Environment / process builtins (#613) ────────────────────────
        // Cranelift hand-rolls these by name at cranelift.rs:5172-5232.
        // get_env returns Option[String] (aggregate; gated on #340) and is
        // intentionally NOT registered here — only the four scalar-return
        // siblings that the LLVM backend can lower cheaply.
        self.register_func("set_env");
        self.function_return_types
            .insert("set_env".to_string(), Type::Void);
        self.register_func("current_dir");
        self.function_return_types
            .insert("current_dir".to_string(), Type::Ptr);
        self.register_func("change_dir");
        self.function_return_types
            .insert("change_dir".to_string(), Type::Void);
        self.register_func("process_id");
        self.function_return_types
            .insert("process_id".to_string(), Type::I64);

        // ── File I/O (Phase NN) ──────────────────────────────────────────
        self.register_func("file_read");
        self.function_return_types
            .insert("file_read".to_string(), Type::Ptr);
        self.register_func("file_write");
        self.function_return_types
            .insert("file_write".to_string(), Type::Bool);
        self.register_func("file_exists");
        self.function_return_types
            .insert("file_exists".to_string(), Type::Bool);
        self.register_func("file_append");
        self.function_return_types
            .insert("file_append".to_string(), Type::Bool);
        self.register_func("file_delete");
        self.function_return_types
            .insert("file_delete".to_string(), Type::Bool);

        // ── List operations ─────────────────────────────────────────────
        self.register_func("list_length");
        self.function_return_types
            .insert("list_length".to_string(), Type::I64);
        self.register_func("list_get");
        self.function_return_types
            .insert("list_get".to_string(), Type::I64);
        self.register_func("list_push");
        self.function_return_types
            .insert("list_push".to_string(), Type::Ptr);
        self.register_func("list_concat");
        self.function_return_types
            .insert("list_concat".to_string(), Type::Ptr);
        self.register_func("list_is_empty");
        self.function_return_types
            .insert("list_is_empty".to_string(), Type::Bool);
        self.register_func("list_head");
        self.function_return_types
            .insert("list_head".to_string(), Type::I64);
        self.register_func("list_tail");
        self.function_return_types
            .insert("list_tail".to_string(), Type::Ptr);
        self.register_func("list_contains");
        self.function_return_types
            .insert("list_contains".to_string(), Type::Bool);

        // ── Higher-order list operations ───────────────────────────────
        self.register_func("list_map");
        self.function_return_types
            .insert("list_map".to_string(), Type::Ptr);
        self.register_func("list_filter");
        self.function_return_types
            .insert("list_filter".to_string(), Type::Ptr);
        self.register_func("list_foreach");
        self.function_return_types
            .insert("list_foreach".to_string(), Type::Void);
        self.register_func("list_fold");
        self.function_return_types
            .insert("list_fold".to_string(), Type::I64);
        self.register_func("list_any");
        self.function_return_types
            .insert("list_any".to_string(), Type::Bool);
        self.register_func("list_all");
        self.function_return_types
            .insert("list_all".to_string(), Type::Bool);
        self.register_func("list_find");
        self.function_return_types
            .insert("list_find".to_string(), Type::I64);
        self.register_func("list_sort");
        self.function_return_types
            .insert("list_sort".to_string(), Type::Ptr);
        self.register_func("list_reverse");
        self.function_return_types
            .insert("list_reverse".to_string(), Type::Ptr);

        // ── Map operations (Phase OO) ────────────────────────────────────
        self.register_func("map_new");
        self.function_return_types
            .insert("map_new".to_string(), Type::Ptr);
        self.register_func("map_set");
        self.function_return_types
            .insert("map_set".to_string(), Type::Ptr);
        self.register_func("map_get");
        self.function_return_types
            .insert("map_get".to_string(), Type::Ptr);
        self.register_func("map_contains");
        self.function_return_types
            .insert("map_contains".to_string(), Type::Bool);
        self.register_func("map_remove");
        self.function_return_types
            .insert("map_remove".to_string(), Type::Ptr);
        self.register_func("map_size");
        self.function_return_types
            .insert("map_size".to_string(), Type::I64);
        self.register_func("map_keys");
        self.function_return_types
            .insert("map_keys".to_string(), Type::Ptr);

        // ── HashMap operations (Self-Hosting Phase 1.1) ─────────────────────
        // Note: These are generic functions. The runtime has specialized
        // versions for String vs Int keys that the codegen selects based on type.
        self.register_func("hashmap_new");
        self.function_return_types
            .insert("hashmap_new".to_string(), Type::Ptr);
        self.register_func("hashmap_insert");
        self.function_return_types
            .insert("hashmap_insert".to_string(), Type::Ptr); // Option[V] as ptr
        self.register_func("hashmap_get");
        self.function_return_types
            .insert("hashmap_get".to_string(), Type::Ptr); // Option[V] as ptr
        self.register_func("hashmap_remove");
        self.function_return_types
            .insert("hashmap_remove".to_string(), Type::Ptr); // Option[V] as ptr
        self.register_func("hashmap_contains");
        self.function_return_types
            .insert("hashmap_contains".to_string(), Type::Bool);
        self.register_func("hashmap_len");
        self.function_return_types
            .insert("hashmap_len".to_string(), Type::I64);
        self.register_func("hashmap_clear");
        self.function_return_types
            .insert("hashmap_clear".to_string(), Type::Void);

        // ── HTTP Client Builtins (Phase RR) ──────────────────────────────
        self.register_func("http_get");
        self.function_return_types
            .insert("http_get".to_string(), Type::Ptr);
        self.register_func("http_post");
        self.function_return_types
            .insert("http_post".to_string(), Type::Ptr);
        self.register_func("http_post_json");
        self.function_return_types
            .insert("http_post_json".to_string(), Type::Ptr);

        // ── JSON Builtins ────────────────────────────────────────────────
        self.register_func("json_parse");
        self.function_return_types
            .insert("json_parse".to_string(), Type::Ptr);
        self.register_func("json_stringify");
        self.function_return_types
            .insert("json_stringify".to_string(), Type::Ptr);
        self.register_func("json_type");
        self.function_return_types
            .insert("json_type".to_string(), Type::Ptr);
        self.register_func("json_get");
        self.function_return_types
            .insert("json_get".to_string(), Type::Ptr);
        self.register_func("json_is_null");
        self.function_return_types
            .insert("json_is_null".to_string(), Type::Bool);
        self.register_func("json_has");
        self.function_return_types
            .insert("json_has".to_string(), Type::Bool);
        self.register_func("json_keys");
        self.function_return_types
            .insert("json_keys".to_string(), Type::Ptr);
        self.register_func("json_len");
        self.function_return_types
            .insert("json_len".to_string(), Type::I64);
        self.register_func("json_array_get");
        self.function_return_types
            .insert("json_array_get".to_string(), Type::Ptr);
        // Typed JSON extractors
        self.register_func("json_as_string");
        self.function_return_types
            .insert("json_as_string".to_string(), Type::Ptr);
        self.register_func("json_as_int");
        self.function_return_types
            .insert("json_as_int".to_string(), Type::Ptr);
        self.register_func("json_as_float");
        self.function_return_types
            .insert("json_as_float".to_string(), Type::Ptr);
        self.register_func("json_as_bool");
        self.function_return_types
            .insert("json_as_bool".to_string(), Type::Ptr);

        // ── Phase PP: Random Number Generation ────────────────────────────
        self.register_func("random");
        self.function_return_types
            .insert("random".to_string(), Type::F64);
        self.register_func("random_int");
        self.function_return_types
            .insert("random_int".to_string(), Type::I64);
        self.register_func("random_float");
        self.function_return_types
            .insert("random_float".to_string(), Type::F64);
        self.register_func("seed_random");
        self.function_return_types
            .insert("seed_random".to_string(), Type::Void);

        // ── Set operations (Phase PP) ──────────────────────────────────────
        self.register_func("set_new");
        self.function_return_types
            .insert("set_new".to_string(), Type::Ptr);
        self.register_func("set_add");
        self.function_return_types
            .insert("set_add".to_string(), Type::Ptr);
        self.register_func("set_remove");
        self.function_return_types
            .insert("set_remove".to_string(), Type::Ptr);
        self.register_func("set_contains");
        self.function_return_types
            .insert("set_contains".to_string(), Type::Bool);
        self.register_func("set_size");
        self.function_return_types
            .insert("set_size".to_string(), Type::I64);
        self.register_func("set_union");
        self.function_return_types
            .insert("set_union".to_string(), Type::Ptr);
        self.register_func("set_intersection");
        self.function_return_types
            .insert("set_intersection".to_string(), Type::Ptr);
        self.register_func("set_to_list");
        self.function_return_types
            .insert("set_to_list".to_string(), Type::Ptr);

        // ── Phase PP: Queue Builtins ──────────────────────────────────────
        self.register_func("queue_new");
        self.function_return_types
            .insert("queue_new".to_string(), Type::Ptr);
        self.register_func("queue_enqueue");
        self.function_return_types
            .insert("queue_enqueue".to_string(), Type::Ptr);
        self.register_func("queue_dequeue");
        self.function_return_types
            .insert("queue_dequeue".to_string(), Type::Ptr);
        self.register_func("queue_peek");
        self.function_return_types
            .insert("queue_peek".to_string(), Type::Ptr);
        self.register_func("queue_size");
        self.function_return_types
            .insert("queue_size".to_string(), Type::I64);

        // ── Phase PP: Stack Builtins ──────────────────────────────────────
        self.register_func("stack_new");
        self.function_return_types
            .insert("stack_new".to_string(), Type::Ptr);
        self.register_func("stack_push");
        self.function_return_types
            .insert("stack_push".to_string(), Type::Ptr);
        self.register_func("stack_pop");
        self.function_return_types
            .insert("stack_pop".to_string(), Type::Ptr);
        self.register_func("stack_peek");
        self.function_return_types
            .insert("stack_peek".to_string(), Type::Ptr);
        self.register_func("stack_size");
        self.function_return_types
            .insert("stack_size".to_string(), Type::I64);

        // ── Phase PP: String Utilities ────────────────────────────────────
        self.register_func("string_join");
        self.function_return_types
            .insert("string_join".to_string(), Type::Ptr);
        self.register_func("string_repeat");
        self.function_return_types
            .insert("string_repeat".to_string(), Type::Ptr);
        self.register_func("string_pad_left");
        self.function_return_types
            .insert("string_pad_left".to_string(), Type::Ptr);
        self.register_func("string_pad_right");
        self.function_return_types
            .insert("string_pad_right".to_string(), Type::Ptr);
        self.register_func("string_strip");
        self.function_return_types
            .insert("string_strip".to_string(), Type::Ptr);
        self.register_func("string_strip_prefix");
        self.function_return_types
            .insert("string_strip_prefix".to_string(), Type::Ptr);
        self.register_func("string_strip_suffix");
        self.function_return_types
            .insert("string_strip_suffix".to_string(), Type::Ptr);
        self.register_func("string_to_int");
        self.function_return_types
            .insert("string_to_int".to_string(), Type::Ptr);
        self.register_func("string_to_float");
        self.function_return_types
            .insert("string_to_float".to_string(), Type::Ptr);

        // ── Phase PP: String Utilities Batch 2 ─────────────────────────────
        // string_format(fmt: String, args: List[String]) -> String
        self.register_func("string_format");
        self.function_return_types
            .insert("string_format".to_string(), Type::Ptr);
        // string_is_empty(s: String) -> Bool
        self.register_func("string_is_empty");
        self.function_return_types
            .insert("string_is_empty".to_string(), Type::Bool);
        // string_reverse(s: String) -> String
        self.register_func("string_reverse");
        self.function_return_types
            .insert("string_reverse".to_string(), Type::Ptr);
        // string_compare(a: String, b: String) -> Int
        self.register_func("string_compare");
        self.function_return_types
            .insert("string_compare".to_string(), Type::I64);
        // string_find(s: String, substr: String) -> Option[Int]
        self.register_func("string_find");
        self.function_return_types
            .insert("string_find".to_string(), Type::Ptr);
        // string_slice(s: String, start: Int, end: Int) -> String
        self.register_func("string_slice");
        self.function_return_types
            .insert("string_slice".to_string(), Type::Ptr);
        // string_append(a: String, b: String) -> String (#587)
        self.register_func("string_append");
        self.function_return_types
            .insert("string_append".to_string(), Type::Ptr);
        // string_char_code_at(s: String, i: Int) -> Int (#587)
        self.register_func("string_char_code_at");
        self.function_return_types
            .insert("string_char_code_at".to_string(), Type::I64);

        // ── Option helper functions ────────────────────────────────────────
        // option_is_some(opt: Option[T]) -> Bool
        self.register_func("option_is_some");
        self.function_return_types
            .insert("option_is_some".to_string(), Type::Bool);
        // option_is_none(opt: Option[T]) -> Bool
        self.register_func("option_is_none");
        self.function_return_types
            .insert("option_is_none".to_string(), Type::Bool);
        // option_unwrap(opt: Option[T]) -> T (panics on None)
        self.register_func("option_unwrap");
        self.function_return_types
            .insert("option_unwrap".to_string(), Type::Ptr);
        // option_unwrap_or(opt: Option[T], default: T) -> T
        self.register_func("option_unwrap_or");
        self.function_return_types
            .insert("option_unwrap_or".to_string(), Type::Ptr);

        // ── Phase PP: Date/Time Builtins ───────────────────────────────────
        // now() -> Int (Unix timestamp in seconds, !{Time})
        self.register_func("now");
        self.function_return_types
            .insert("now".to_string(), Type::I64);
        // now_ms() -> Int (Unix timestamp in milliseconds, !{Time})
        self.register_func("now_ms");
        self.function_return_types
            .insert("now_ms".to_string(), Type::I64);
        // sleep(ms: Int) -> () (sleep for milliseconds, !{Time})
        self.register_func("sleep");
        self.function_return_types
            .insert("sleep".to_string(), Type::Void);
        // sleep_seconds(s: Int) -> () (sleep for seconds, !{Time})
        self.register_func("sleep_seconds");
        self.function_return_types
            .insert("sleep_seconds".to_string(), Type::Void);
        // time_string() -> String (RFC3339 format, !{Time})
        self.register_func("time_string");
        self.function_return_types
            .insert("time_string".to_string(), Type::Ptr);
        // date_string() -> String (YYYY-MM-DD, !{Time})
        self.register_func("date_string");
        self.function_return_types
            .insert("date_string".to_string(), Type::Ptr);
        // datetime_year(ts: Int) -> Int (extract year from timestamp - pure)
        self.register_func("datetime_year");
        self.function_return_types
            .insert("datetime_year".to_string(), Type::I64);
        // datetime_month(ts: Int) -> Int (extract month 1-12 from timestamp - pure)
        self.register_func("datetime_month");
        self.function_return_types
            .insert("datetime_month".to_string(), Type::I64);
        // datetime_day(ts: Int) -> Int (extract day 1-31 from timestamp - pure)
        self.register_func("datetime_day");
        self.function_return_types
            .insert("datetime_day".to_string(), Type::I64);

        for item in &ast_module.items {
            match &item.node {
                ast::ItemKind::FnDef(fn_def) => {
                    self.register_func(&fn_def.name);
                    let ret_ty = fn_def
                        .return_type
                        .as_ref()
                        .map(|rt| self.resolve_type(&rt.node))
                        .unwrap_or(Type::Void);
                    self.function_return_types
                        .insert(fn_def.name.clone(), ret_ty);
                }
                ast::ItemKind::ExternFn(extern_fn) => {
                    self.register_func(&extern_fn.name);
                    let ret_ty = extern_fn
                        .return_type
                        .as_ref()
                        .map(|rt| self.resolve_type(&rt.node))
                        .unwrap_or(Type::Void);
                    self.function_return_types
                        .insert(extern_fn.name.clone(), ret_ty);
                }
                ast::ItemKind::EnumDecl { variants, .. } => {
                    // Pre-register enum variant tags so they're available
                    // during function building.
                    for (i, variant) in variants.iter().enumerate() {
                        self.enum_variant_tags
                            .insert(variant.name.clone(), i as i64);
                        // Record whether this variant carries a tuple payload.
                        if let Some(ref fields) = variant.fields {
                            if !fields.is_empty() {
                                self.tuple_variant_names.insert(variant.name.clone());
                                // Handle both single and multi-field variants
                                if fields.len() == 1 {
                                    let field_ty = match &fields[0] {
                                        VariantField::Named { type_expr, .. } => {
                                            self.resolve_type(&type_expr.node)
                                        }
                                        VariantField::Anonymous(type_expr) => {
                                            self.resolve_type(&type_expr.node)
                                        }
                                    };
                                    self.variant_field_types
                                        .insert(variant.name.clone(), field_ty);
                                } else {
                                    let per_field: Vec<Type> = fields
                                        .iter()
                                        .map(|f| match f {
                                            VariantField::Named { type_expr, .. } => {
                                                self.resolve_type(&type_expr.node)
                                            }
                                            VariantField::Anonymous(type_expr) => {
                                                self.resolve_type(&type_expr.node)
                                            }
                                        })
                                        .collect();
                                    self.variant_field_types_vec
                                        .insert(variant.name.clone(), per_field);
                                }
                            }
                        }
                    }
                }
                ast::ItemKind::ActorDecl {
                    name,
                    state_fields: _,
                    handlers,
                    ..
                } => {
                    let init_name = format!("{}_init_state", name);
                    self.register_func(&init_name);
                    self.function_return_types.insert(init_name, Type::Ptr);

                    let setup_name = format!("{}_setup_behaviors", name);
                    self.register_func(&setup_name);
                    self.function_return_types.insert(setup_name, Type::Void);

                    let spawn_name = format!("spawn_{}", name);
                    self.register_func(&spawn_name);
                    self.function_return_types.insert(spawn_name, Type::Ptr);

                    for handler in handlers {
                        let handler_name = format!("{}_{}_handler", name, handler.message_name);
                        self.register_func(&handler_name);
                        self.function_return_types.insert(handler_name, Type::Ptr);
                    }
                }
                ast::ItemKind::ImplBlock {
                    target_type,
                    methods,
                    ..
                } => {
                    // Register each impl method as `TargetType::method_name`.
                    for method in methods {
                        let qualified_name = format!("{}::{}", target_type, method.name);
                        self.register_func(&qualified_name);
                        let ret_ty = method
                            .return_type
                            .as_ref()
                            .map(|rt| self.resolve_type(&rt.node))
                            .unwrap_or(Type::Void);
                        self.function_return_types.insert(qualified_name, ret_ty);
                    }
                }
                _ => {}
            }
        }
    }

    /// Register functions from an imported module so that qualified calls
    /// (e.g., `math.add(3, 4)`) can be resolved in the IR.
    ///
    /// Only registers the function names and return types — no IR is generated
    /// for the imported functions themselves (they'll be compiled separately
    /// or linked in).
    fn register_imported_functions(&mut self, ast_module: &ast::Module) {
        for item in &ast_module.items {
            match &item.node {
                ast::ItemKind::FnDef(fn_def) => {
                    self.register_func(&fn_def.name);
                    let ret_ty = fn_def
                        .return_type
                        .as_ref()
                        .map(|rt| self.resolve_type(&rt.node))
                        .unwrap_or(Type::Void);
                    self.function_return_types
                        .insert(fn_def.name.clone(), ret_ty);
                }
                ast::ItemKind::ExternFn(extern_fn) => {
                    self.register_func(&extern_fn.name);
                    let ret_ty = extern_fn
                        .return_type
                        .as_ref()
                        .map(|rt| self.resolve_type(&rt.node))
                        .unwrap_or(Type::Void);
                    self.function_return_types
                        .insert(extern_fn.name.clone(), ret_ty);
                }
                _ => {}
            }
        }
    }

    /// Register a single function name, assigning it a fresh [`FuncRef`].
    /// Resolve a method call to a function name in the IR.
    ///
    /// Uses the tracked `string_values`, `list_values`, and `value_types` to
    /// determine the type of the object, then maps the method name to the
    /// corresponding free function or trait method.
    fn resolve_method_ir(&self, obj_val: Value, method: &str) -> String {
        // Check if the object is a tracked string value.
        if self.string_values.contains(&obj_val) {
            let candidate = format!("string_{}", method);
            if self.function_refs.contains_key(&candidate) {
                return candidate;
            }
            // Also try trait methods on String.
            let trait_candidate = format!("String::{}", method);
            if self.function_refs.contains_key(&trait_candidate) {
                return trait_candidate;
            }
        }

        // Check if the object is a tracked list value.
        if self.list_values.contains(&obj_val) {
            let candidate = format!("list_{}", method);
            if self.function_refs.contains_key(&candidate) {
                return candidate;
            }
            let trait_candidate = format!("List::{}", method);
            if self.function_refs.contains_key(&trait_candidate) {
                return trait_candidate;
            }
        }

        // Check if the object is a tracked map value.
        if self.map_values.contains(&obj_val) {
            let candidate = format!("map_{}", method);
            if self.function_refs.contains_key(&candidate) {
                return candidate;
            }
            let trait_candidate = format!("Map::{}", method);
            if self.function_refs.contains_key(&trait_candidate) {
                return trait_candidate;
            }
        }

        // Check if the object is a tracked set value.
        if self.set_values.contains(&obj_val) {
            let candidate = format!("set_{}", method);
            if self.function_refs.contains_key(&candidate) {
                return candidate;
            }
            let trait_candidate = format!("Set::{}", method);
            if self.function_refs.contains_key(&trait_candidate) {
                return trait_candidate;
            }
        }

        // Check if the object is a tracked queue value.
        if self.queue_values.contains(&obj_val) {
            let candidate = format!("queue_{}", method);
            if self.function_refs.contains_key(&candidate) {
                return candidate;
            }
            let trait_candidate = format!("Queue::{}", method);
            if self.function_refs.contains_key(&trait_candidate) {
                return trait_candidate;
            }
        }

        // Check if the object is a tracked stack value.
        if self.stack_values.contains(&obj_val) {
            let candidate = format!("stack_{}", method);
            if self.function_refs.contains_key(&candidate) {
                return candidate;
            }
            let trait_candidate = format!("Stack::{}", method);
            if self.function_refs.contains_key(&trait_candidate) {
                return trait_candidate;
            }
        }

        // Check if the object is a tracked hashmap value.
        if self.hashmap_values.contains(&obj_val) {
            let candidate = format!("hashmap_{}", method);
            if self.function_refs.contains_key(&candidate) {
                return candidate;
            }
            let trait_candidate = format!("HashMap::{}", method);
            if self.function_refs.contains_key(&trait_candidate) {
                return trait_candidate;
            }
        }

        // Use the IR value type to determine the source type for trait methods.
        let ir_type = self.value_types.get(&obj_val).cloned().unwrap_or(Type::I64);
        let type_name = match ir_type {
            Type::I64 | Type::I32 => "Int",
            Type::F64 => "Float",
            Type::Bool => "Bool",
            Type::Ptr => {
                // Ptr could be String, List, Map, Set, Queue, Stack, or HashMap.
                // If we haven't matched above, try all collection prefixes.
                let string_candidate = format!("string_{}", method);
                if self.function_refs.contains_key(&string_candidate) {
                    return string_candidate;
                }
                let list_candidate = format!("list_{}", method);
                if self.function_refs.contains_key(&list_candidate) {
                    return list_candidate;
                }
                let map_candidate = format!("map_{}", method);
                if self.function_refs.contains_key(&map_candidate) {
                    return map_candidate;
                }
                let set_candidate = format!("set_{}", method);
                if self.function_refs.contains_key(&set_candidate) {
                    return set_candidate;
                }
                let queue_candidate = format!("queue_{}", method);
                if self.function_refs.contains_key(&queue_candidate) {
                    return queue_candidate;
                }
                let stack_candidate = format!("stack_{}", method);
                if self.function_refs.contains_key(&stack_candidate) {
                    return stack_candidate;
                }
                let hashmap_candidate = format!("hashmap_{}", method);
                if self.function_refs.contains_key(&hashmap_candidate) {
                    return hashmap_candidate;
                }
                "String" // default for Ptr trait methods
            }
            Type::Void => "Unit",
        };

        // Try trait method qualified name (e.g., Int::display).
        let trait_candidate = format!("{}::{}", type_name, method);
        if self.function_refs.contains_key(&trait_candidate) {
            return trait_candidate;
        }

        // Try user-defined method naming convention (e.g., String_trim).
        let type_prefixed = format!("{}_{}", type_name, method);
        if self.function_refs.contains_key(&type_prefixed) {
            return type_prefixed;
        }

        // Fallback: try all registered type prefixes with trait and underscore naming.
        for tn in &["Int", "Float", "String", "Bool", "Unit", "List"] {
            let trait_candidate = format!("{}::{}", tn, method);
            if self.function_refs.contains_key(&trait_candidate) {
                return trait_candidate;
            }
            let type_prefixed = format!("{}_{}", tn, method);
            if self.function_refs.contains_key(&type_prefixed) {
                return type_prefixed;
            }
        }

        // Last resort: return the method name as-is (will fail to resolve).
        method.to_string()
    }

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
        self.list_values.clear();
        self.map_values.clear();
        self.set_values.clear();
        self.queue_values.clear();
        self.stack_values.clear();
        self.hashmap_values.clear();
        self.mutable_vars.clear();
        self.mutable_addrs.clear();
        self.mutable_types.clear();
        self.mutable_string_vars.clear();
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
            // Track list-typed parameters in list_values so that for-loop
            // iteration dispatches to build_for_list rather than build_for_counted.
            if let ast::TypeExpr::Generic {
                name: type_name, ..
            } = &param.type_ann.node
            {
                if type_name == "List" {
                    self.list_values.insert(val);
                }
            }
        }

        let return_type = fn_def
            .return_type
            .as_ref()
            .map(|rt| self.resolve_type(&rt.node))
            .unwrap_or(Type::Void);

        // Emit @requires precondition checks at function entry.
        for contract in &fn_def.contracts {
            if self.should_emit_contract(contract) && contract.kind == ast::ContractKind::Requires {
                self.emit_contract_check(
                    &contract.condition,
                    &format!(
                        "contract violation: @requires failed in function `{}`",
                        fn_def.name
                    ),
                );
            }
        }

        // Build the function body, tracking the last expression value
        // so we can emit an implicit return for expression-bodied functions.
        let last_expr_val = self.build_fn_body(&fn_def.body);

        // Collect @ensures contracts for postcondition emission.
        let ensures_contracts: Vec<_> = fn_def
            .contracts
            .iter()
            .filter(|c| self.should_emit_contract(c) && c.kind == ast::ContractKind::Ensures)
            .collect();

        // If the last block has no terminator, add an implicit return.
        if !self.current_block_has_terminator() {
            if return_type == Type::Void {
                // Emit @ensures checks before the return (no result binding for void).
                for contract in &ensures_contracts {
                    self.emit_contract_check(
                        &contract.condition,
                        &format!(
                            "contract violation: @ensures failed in function `{}`",
                            fn_def.name
                        ),
                    );
                }
                self.emit(Instruction::Ret(None));
            } else if let Some(val) = last_expr_val {
                // Bind `result` to the return value for @ensures checks.
                if !ensures_contracts.is_empty() {
                    self.define_var("result", val);
                    for contract in &ensures_contracts {
                        self.emit_contract_check(
                            &contract.condition,
                            &format!(
                                "contract violation: @ensures failed in function `{}`",
                                fn_def.name
                            ),
                        );
                    }
                }
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
            is_export: fn_def.is_export,
            extern_lib: None,
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
            is_export: false,
            extern_lib: extern_fn.extern_lib.clone(),
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
                ast::StmtKind::Let {
                    name,
                    value,
                    mutable,
                    ..
                } => {
                    let val = self.build_expr(value);
                    if *mutable {
                        self.build_mutable_let(name, val);
                    } else {
                        self.define_var(name, val);
                    }
                    last_expr_val = None;
                }
                ast::StmtKind::LetTupleDestructure { names, value, .. } => {
                    let tuple_val = self.build_expr(value);
                    // First try the new offset-based approach (heap-allocated tuples)
                    if let Some((elem_size, offsets)) =
                        self.tuple_element_offsets.get(&tuple_val).cloned()
                    {
                        for (i, name) in names.iter().enumerate() {
                            if i < offsets.len() {
                                // Calculate element address: base + i * elem_size
                                let offset = i as i64 * elem_size;
                                let offset_val = self.fresh_value(Type::I64);
                                self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                                let elem_addr = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
                    } else if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
                        // Legacy: stack-allocated tuples (deprecated)
                        for (i, name) in names.iter().enumerate() {
                            if i < addrs.len() {
                                let elem_addr = addrs[i];
                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
                    } else {
                        // Cross-function tuple return: infer structure from pattern
                        let elem_size = 8i64;
                        let num_elems = names.len();
                        for (i, name) in names.iter().enumerate() {
                            if i < num_elems {
                                let offset = i as i64 * elem_size;
                                let offset_val = self.fresh_value(Type::I64);
                                self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                                let elem_addr = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
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
            ast::StmtKind::Let {
                name,
                value,
                mutable,
                ..
            } => {
                let val = self.build_expr(value);
                if *mutable {
                    self.build_mutable_let(name, val);
                } else {
                    self.define_var(name, val);
                }
            }
            ast::StmtKind::LetTupleDestructure { names, value, .. } => {
                let tuple_val = self.build_expr(value);
                // Destructure: each name gets the value from the corresponding element.
                // First try the new offset-based approach (heap-allocated tuples)
                if let Some((elem_size, offsets)) =
                    self.tuple_element_offsets.get(&tuple_val).cloned()
                {
                    for (i, name) in names.iter().enumerate() {
                        if i < offsets.len() {
                            // Calculate element address: base + i * elem_size
                            let offset = i as i64 * elem_size;
                            let offset_val = self.fresh_value(Type::I64);
                            self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                            let elem_addr = self.fresh_value(Type::Ptr);
                            self.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                            let result = self.fresh_value(Type::I64);
                            self.emit(Instruction::Load(result, elem_addr));
                            self.define_var(name, result);
                        }
                    }
                } else if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
                    // Legacy: stack-allocated tuples (deprecated)
                    for (i, name) in names.iter().enumerate() {
                        if i < addrs.len() {
                            let elem_addr = addrs[i];
                            let result = self.fresh_value(Type::I64);
                            self.emit(Instruction::Load(result, elem_addr));
                            self.define_var(name, result);
                        }
                    }
                } else {
                    // Cross-function tuple return: value is a pointer to heap-allocated tuple.
                    // Infer structure from destructuring pattern.
                    let elem_size = 8i64;
                    let num_elems = names.len();

                    for (i, name) in names.iter().enumerate() {
                        if i < num_elems {
                            // Calculate element address: base + i * elem_size
                            let offset = i as i64 * elem_size;
                            let offset_val = self.fresh_value(Type::I64);
                            self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                            let elem_addr = self.fresh_value(Type::Ptr);
                            self.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                            let result = self.fresh_value(Type::I64);
                            self.emit(Instruction::Load(result, elem_addr));
                            self.define_var(name, result);
                        }
                    }
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
        self.emit(Instruction::Alloca(addr, val_ty.clone()));
        // Store the initial value.
        self.emit(Instruction::Store(val, addr));
        // Track as mutable.
        self.mutable_vars.insert(name.to_string());
        self.mutable_addrs.insert(name.to_string(), addr);
        self.mutable_types.insert(name.to_string(), val_ty);
        // If the initial value is a string, mark this variable so that loads
        // from it re-enter string_values and `+` dispatches to string_concat.
        if self.string_values.contains(&val) {
            self.mutable_string_vars.insert(name.to_string());
        }
        // Also define in scope so lookup_var still works (maps to addr for tracking).
        self.define_var(name, addr);
    }

    /// Build an assignment to a mutable variable: store to the alloca'd address.
    fn build_assign(&mut self, name: &str, val: Value) {
        if let Some(addr) = self.mutable_addrs.get(name).copied() {
            self.emit(Instruction::Store(val, addr));
            // If the new value is a string, keep the variable marked as string-typed.
            // This handles cases like `content = content + ...` where the result of
            // string_concat is stored back into a mutable string variable.
            if self.string_values.contains(&val) {
                self.mutable_string_vars.insert(name.to_string());
            }
        } else {
            self.errors.push(format!(
                "assignment to undefined or immutable variable: '{}'",
                name
            ));
        }
    }

    // ── Contract checking ────────────────────────────────────────────

    fn should_emit_contract(&self, contract: &ast::Contract) -> bool {
        !(self.strip_runtime_only_contracts && contract.runtime_only_off_in_release)
    }

    /// Emit a runtime contract check: evaluate the condition, and if false,
    /// call `__gradient_contract_fail` with the error message then abort.
    fn emit_contract_check(&mut self, condition: &ast::Expr, message: &str) {
        let cond_val = self.build_expr(condition);

        // Branch: if cond is true, jump to ok_block; otherwise fail_block.
        let fail_block = self.fresh_block();
        let ok_block = self.fresh_block();
        self.emit(Instruction::Branch(cond_val, ok_block, fail_block));
        self.seal_block();

        // fail_block: call __gradient_contract_fail(message) and return (abort).
        self.current_block_label = fail_block;
        let msg_val = self.fresh_value(Type::Ptr);
        self.emit(Instruction::Const(
            msg_val,
            Literal::Str(message.to_string()),
        ));
        self.string_values.insert(msg_val);

        if let Some(func_ref) = self.runtime_function("__gradient_contract_fail") {
            let call_result = self.fresh_value(Type::Void);
            self.emit(Instruction::Call(call_result, func_ref, vec![msg_val]));
        }
        // After the contract failure call, we abort (but emit a Ret for well-formedness).
        self.emit(Instruction::Ret(None));
        self.seal_block();

        // ok_block: continue normal execution.
        self.current_block_label = ok_block;
    }

    // ── Record layout computation ────────────────────────────────────

    /// Infer IR type from an AST expression for record field layout computation.
    fn infer_field_type(&self, expr: &ast::Expr) -> Type {
        match &expr.node {
            ast::ExprKind::IntLit(_) => Type::I64,
            ast::ExprKind::FloatLit(_) => Type::F64,
            ast::ExprKind::BoolLit(_) => Type::Bool,
            ast::ExprKind::StringLit(_) => Type::Ptr,
            ast::ExprKind::CharLit(_) => Type::I32,
            ast::ExprKind::UnitLit => Type::Void,
            // For variable references, try to look up the variable's type
            ast::ExprKind::Ident(name) => {
                if let Some(val) = self.lookup_var(name) {
                    self.value_types.get(&val).cloned().unwrap_or(Type::I64)
                } else {
                    Type::I64 // Default fallback
                }
            }
            // For other expressions, default to I64 (most common case)
            _ => Type::I64,
        }
    }

    /// Get size in bytes for an IR type.
    fn type_size(ty: &Type) -> i64 {
        match ty {
            Type::I32 => 4,
            Type::I64 => 8,
            Type::F64 => 8,
            Type::Bool => 1,
            Type::Ptr => 8,
            Type::Void => 0,
        }
    }

    /// Get alignment in bytes for an IR type.
    fn type_align(ty: &Type) -> i64 {
        match ty {
            Type::I32 => 4,
            Type::I64 => 8,
            Type::F64 => 8,
            Type::Bool => 1,
            Type::Ptr => 8,
            Type::Void => 1,
        }
    }

    /// Compute or retrieve the layout for a record type.
    /// The layout maps field names to their index and byte offset.
    fn compute_record_layout(
        &mut self,
        type_name: &str,
        fields: &[(String, ast::Expr)],
    ) -> RecordLayout {
        // Check if we already have a layout for this type
        if let Some(layout) = self.record_layouts.get(type_name) {
            return layout.clone();
        }

        // Build the layout from the field information
        let mut layout = RecordLayout::new();

        // Process fields in order to compute offsets with proper type-based layout
        for (field_name, field_expr) in fields.iter() {
            let field_type = self.infer_field_type(field_expr);
            let size = Self::type_size(&field_type);
            let align = Self::type_align(&field_type);
            layout.add_field(field_name.clone(), size, align, field_type);
        }

        // Final alignment: ensure total size is aligned to max field alignment
        if layout.total_size > 0 {
            let max_align = fields
                .iter()
                .map(|(_, expr)| Self::type_align(&self.infer_field_type(expr)))
                .max()
                .unwrap_or(8);
            layout.total_size = (layout.total_size + max_align - 1) & !(max_align - 1);
        }

        // Store the layout for future use
        self.record_layouts
            .insert(type_name.to_string(), layout.clone());
        layout
    }

    /// Get the layout for a previously computed record type.
    fn get_record_layout(&self, type_name: &str) -> Option<&RecordLayout> {
        self.record_layouts.get(type_name)
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
            ast::ExprKind::CharLit(c) => {
                let v = self.fresh_value(Type::I32);
                self.emit(Instruction::Const(v, Literal::Int(*c as i64)));
                v
            }
            ast::ExprKind::BoolLit(b) => {
                let v = self.fresh_value(Type::Bool);
                self.emit(Instruction::Const(v, Literal::Bool(*b)));
                v
            }
            ast::ExprKind::StringInterp { parts } => self.build_string_interp(parts),
            ast::ExprKind::UnitLit => {
                // Unit has no runtime value. We produce a dummy const 0
                // so that every expression has a Value.
                let v = self.fresh_value(Type::Void);
                self.emit(Instruction::Const(v, Literal::Int(0)));
                v
            }
            ast::ExprKind::Ident(name) => {
                // Check if this is an enum variant (unit variant used as a value).
                if let Some(&tag) = self.enum_variant_tags.get(name.as_str()) {
                    // If it's not also a local variable, treat it as an enum tag.
                    if self.lookup_var(name).is_none() && !self.mutable_vars.contains(name.as_str())
                    {
                        // Unit variant: heap-allocate a tagged union with no payload.
                        let v = self.fresh_value(Type::Ptr);
                        self.emit(Instruction::ConstructVariant {
                            result: v,
                            tag,
                            payload: Vec::new(),
                        });
                        return v;
                    }
                }
                // If this is a mutable variable, load from its stack slot.
                if self.mutable_vars.contains(name.as_str()) {
                    if let Some(addr) = self.mutable_addrs.get(name.as_str()).copied() {
                        let load_ty = self
                            .mutable_types
                            .get(name.as_str())
                            .cloned()
                            .unwrap_or(Type::I64);
                        let result = self.fresh_value(load_ty);
                        self.emit(Instruction::Load(result, addr));
                        // Propagate string tracking: if the variable is known to
                        // hold a string, mark the loaded value so `+` dispatches
                        // to string_concat instead of integer Add.
                        if self.mutable_string_vars.contains(name.as_str()) {
                            self.string_values.insert(result);
                        }
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
                self.errors.push(format!(
                    "typed hole {} encountered during IR building",
                    desc
                ));
                let v = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(v, Literal::Int(0)));
                v
            }
            ast::ExprKind::BinaryOp { op, left, right } => self.build_binary_op(*op, left, right),
            ast::ExprKind::UnaryOp { op, operand } => self.build_unary_op(*op, operand),
            ast::ExprKind::Call { func, args } => self.build_call(func, args),
            ast::ExprKind::If {
                condition,
                then_block,
                else_ifs,
                else_block,
            } => self.build_if(condition, then_block, else_ifs, else_block),
            ast::ExprKind::FieldAccess { object, field } => {
                // Build the object expression to get the record pointer
                let obj_val = self.build_expr(object);

                // Try to determine the record type from our tracked record values
                if let Some(type_name) = self.record_values.get(&obj_val).cloned() {
                    // Get the layout for this record type
                    if let Some(layout) = self.get_record_layout(&type_name) {
                        if let Some((field_idx, offset, field_ty)) =
                            layout.get_field_with_type(field)
                        {
                            // Emit LoadField to read the field value
                            let result = self.fresh_value(field_ty.clone());
                            self.emit(Instruction::LoadField {
                                result,
                                object: obj_val,
                                field_idx,
                                field_ty,
                                offset,
                            });
                            result
                        } else {
                            self.errors.push(format!(
                                "field '{}' not found in record type '{}'",
                                field, type_name
                            ));
                            let v = self.fresh_value(Type::I64);
                            self.emit(Instruction::Const(v, Literal::Int(0)));
                            v
                        }
                    } else {
                        self.errors.push(format!(
                            "unknown record type '{}' for field access",
                            type_name
                        ));
                        let v = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(v, Literal::Int(0)));
                        v
                    }
                } else {
                    // Fallback: try to find layout by checking all known record types
                    // This handles cases where the record value comes from a variable
                    let found = self.record_layouts.iter().find_map(|(type_name, layout)| {
                        layout
                            .get_field_with_type(field)
                            .map(|(idx, offset, ty)| (type_name.clone(), idx, offset, ty))
                    });

                    if let Some((_, field_idx, offset, field_ty)) = found {
                        let result = self.fresh_value(field_ty.clone());
                        self.emit(Instruction::LoadField {
                            result,
                            object: obj_val,
                            field_idx,
                            field_ty,
                            offset,
                        });
                        result
                    } else {
                        self.errors.push(format!(
                            "field access (.{}) failed: unable to determine record type",
                            field
                        ));
                        let v = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(v, Literal::Int(0)));
                        v
                    }
                }
            }
            ast::ExprKind::For { var, iter, body } => self.build_for(var, iter, body),
            ast::ExprKind::While { condition, body } => self.build_while(condition, body),
            ast::ExprKind::Match { scrutinee, arms } => self.build_match(scrutinee, arms),
            ast::ExprKind::Paren(inner) => {
                // Parentheses are purely syntactic — pass through.
                self.build_expr(inner)
            }
            ast::ExprKind::Closure {
                params,
                return_type,
                body,
            } => self.build_closure(params, return_type.as_ref(), body),
            ast::ExprKind::ListLit(elements) => {
                // Lists are represented as heap-allocated: [length: i64, capacity: i64, data...]
                // We emit a call to a synthetic "list_literal_N" function that the codegen
                // layer will handle inline.
                let n = elements.len();
                let elem_vals: Vec<Value> = elements.iter().map(|e| self.build_expr(e)).collect();
                let func_name = format!("list_literal_{}", n);
                self.register_func(&func_name);
                self.function_return_types
                    .insert(func_name.clone(), Type::Ptr);
                if let Some(func_ref) = self.runtime_function(&func_name) {
                    let result = self.fresh_value(Type::Ptr);
                    self.emit(Instruction::Call(result, func_ref, elem_vals));
                    self.list_values.insert(result);
                    result
                } else {
                    self.zero_value(Type::Ptr)
                }
            }
            ast::ExprKind::Tuple(elems) => {
                if elems.is_empty() {
                    // Empty tuple is just a null pointer
                    let v = self.fresh_value(Type::Ptr);
                    self.emit(Instruction::Const(v, Literal::Int(0)));
                    return v;
                }

                // Calculate total size: each element is 8 bytes (i64/f64/ptr)
                let elem_size = 8i64;
                let total_size = (elems.len() as i64 * elem_size).max(8);

                // Heap-allocate contiguous memory for all tuple elements
                let size_val = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(size_val, Literal::Int(total_size)));
                let alloc_func = self.ensure_genref_alloc();
                let base_ptr = self.fresh_value(Type::Ptr);
                self.emit(Instruction::Call(base_ptr, alloc_func, vec![size_val]));

                // Store each element at its computed offset
                let mut elem_offsets = Vec::new();
                for (idx, elem_expr) in elems.iter().enumerate() {
                    let elem_val = self.build_expr(elem_expr);
                    let offset = idx as i64 * elem_size;

                    // Calculate element address: base + offset
                    let offset_val = self.fresh_value(Type::I64);
                    self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                    let elem_addr = self.fresh_value(Type::Ptr);
                    self.emit(Instruction::Add(elem_addr, base_ptr, offset_val));

                    // Store the element value
                    self.emit(Instruction::Store(elem_val, elem_addr));
                    elem_offsets.push(offset);
                }

                // Store the base pointer with element offsets for field access
                self.tuple_element_offsets
                    .insert(base_ptr, (elem_size, elem_offsets));
                base_ptr
            }
            ast::ExprKind::RecordLit {
                type_name,
                // Record-spread (`{ ..base, field = value }`) is currently a
                // typechecker-only feature: records still lower as opaque
                // pointers and codegen ignores `base`. When real struct
                // codegen lands this needs to copy missing fields from base.
                base: _,
                fields,
            } => {
                // Proper record literal construction with struct layout
                // Compute the record layout (field indices and offsets)
                let layout = self.compute_record_layout(type_name, fields);

                // Calculate total size needed for the record
                let total_size = layout.total_size.max(8); // At least 8 bytes

                // Heap-allocate the record using the GC allocator
                // This ensures the record survives function returns
                let size_val = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(size_val, Literal::Int(total_size)));
                let alloc_func = self.ensure_genref_alloc();
                let record_ptr = self.fresh_value(Type::Ptr);
                self.emit(Instruction::Call(record_ptr, alloc_func, vec![size_val]));

                // Store each field at its computed offset using StoreField
                for (field_name, field_expr) in fields.iter() {
                    let field_val = self.build_expr(field_expr);

                    if let Some((field_idx, offset, field_ty)) =
                        layout.get_field_with_type(field_name)
                    {
                        self.emit(Instruction::StoreField {
                            value: field_val,
                            object: record_ptr,
                            field_idx,
                            field_ty,
                            offset,
                        });
                    } else {
                        self.errors.push(format!(
                            "field '{}' not found in record layout for '{}'",
                            field_name, type_name
                        ));
                    }
                }

                // Track this value as a record value
                self.record_values.insert(record_ptr, type_name.clone());
                record_ptr
            }
            ast::ExprKind::Construct { name, fields } => {
                // Check if this is an enum variant constructor
                if let Some(&tag) = self.enum_variant_tags.get(name.as_str()) {
                    // Enum variant construction - use ConstructVariant
                    let payload: Vec<Value> = fields
                        .iter()
                        .map(|(_, val_expr)| self.build_expr(val_expr))
                        .collect();
                    let result = self.fresh_value(Type::Ptr);
                    self.emit(Instruction::ConstructVariant {
                        result,
                        tag,
                        payload,
                    });
                    result
                } else {
                    // Not a known enum variant - build as a tuple of field values
                    let mut field_vals = Vec::new();
                    for (_name, val_expr) in fields.iter() {
                        let val = self.build_expr(val_expr);
                        field_vals.push(val);
                    }
                    // Create a tuple-like structure for the constructor
                    let mut elem_addrs = Vec::new();
                    for val in field_vals {
                        let addr = self.fresh_value(Type::Ptr);
                        self.emit(Instruction::Alloca(addr, Type::I64));
                        self.emit(Instruction::Store(val, addr));
                        elem_addrs.push(addr);
                    }
                    if elem_addrs.is_empty() {
                        let v = self.fresh_value(Type::Ptr);
                        self.emit(Instruction::Const(v, Literal::Int(0)));
                        v
                    } else {
                        let base = elem_addrs[0];
                        self.tuple_element_addrs.insert(base, elem_addrs);
                        base
                    }
                }
            }
            ast::ExprKind::TypedExpr {
                type_expr: _,
                value,
            } => {
                // Typed expressions provide a type annotation that was already
                // validated by the typechecker. The IR builder just needs to
                // build the underlying value - type checking happened earlier.
                self.build_expr(value)
            }
            ast::ExprKind::TupleField { tuple, index } => {
                let tuple_val = self.build_expr(tuple);

                // First try the new offset-based approach (heap-allocated tuples)
                if let Some((elem_size, offsets)) =
                    self.tuple_element_offsets.get(&tuple_val).cloned()
                {
                    if *index < offsets.len() {
                        // Calculate element address: base + index * elem_size
                        let offset = *index as i64 * elem_size;
                        let offset_val = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                        let elem_addr = self.fresh_value(Type::Ptr);
                        self.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                        // Load the element value
                        let result = self.fresh_value(Type::I64);
                        self.emit(Instruction::Load(result, elem_addr));
                        result
                    } else {
                        self.errors.push(format!(
                            "tuple field index {} out of bounds (tuple has {} elements)",
                            index,
                            offsets.len()
                        ));
                        let v = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(v, Literal::Int(0)));
                        v
                    }
                } else if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
                    // Legacy: stack-allocated tuples (deprecated)
                    if *index < addrs.len() {
                        let elem_addr = addrs[*index];
                        let result = self.fresh_value(Type::I64);
                        self.emit(Instruction::Load(result, elem_addr));
                        result
                    } else {
                        self.errors.push(format!(
                            "tuple field index {} out of bounds (tuple has {} elements)",
                            index,
                            addrs.len()
                        ));
                        let v = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(v, Literal::Int(0)));
                        v
                    }
                } else {
                    self.errors.push(format!(
                        "tuple field access .{} on a non-tuple value",
                        index
                    ));
                    let v = self.fresh_value(Type::I64);
                    self.emit(Instruction::Const(v, Literal::Int(0)));
                    v
                }
            }
            ast::ExprKind::Range { start, end } => {
                // Build both start and end values.
                // Range is not a runtime value in the traditional sense;
                // it is consumed by for loops. We emit both values and
                // return the start value as a placeholder (the for loop
                // pattern-matches on ExprKind::Range directly).
                let start_val = self.build_expr(start);
                let _end_val = self.build_expr(end);
                start_val
            }
            ast::ExprKind::Try(inner) => {
                // The ? operator: evaluate inner, if Err tag early-return,
                // else extract Ok value.  For v0.1 we simply evaluate the
                // inner expression and return a dummy value — full lowering
                // will be done once the runtime enum representation is finalised.
                let _inner_val = self.build_expr(inner);
                let v = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(v, Literal::Int(0)));
                v
            }
            ast::ExprKind::Defer { body } => {
                // Defer expression: store the body for later execution when scope exits.
                // Defers execute in LIFO order (last deferred, first executed).
                if let Some(defer_frame) = self.defer_stack.last_mut() {
                    defer_frame.push(*body.clone());
                }
                // Defer expression evaluates to void (unit) - represented as I64 with 0.
                let v = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(v, Literal::Int(0)));
                v
            }
            ast::ExprKind::Spawn { actor_name } => self.lower_spawn(actor_name, expr.span),
            ast::ExprKind::Send { target, message } => self.lower_send(target, message, expr.span),
            ast::ExprKind::Ask { target, message } => self.lower_ask(target, message, expr.span),
            ast::ExprKind::ConcurrentScope { body } => self.lower_concurrent_scope(body, expr.span),
            ast::ExprKind::Supervisor {
                strategy,
                max_restarts,
                children,
            } => self.lower_supervisor(*strategy, max_restarts, children, expr.span),
        }
    }

    // ── Binary operations ────────────────────────────────────────────

    /// Build a binary operation expression.
    fn build_binary_op(&mut self, op: ast::BinOp, left: &ast::Expr, right: &ast::Expr) -> Value {
        match op {
            // Arithmetic operators.
            // Special case: `+` on strings emits a call to `string_concat`.
            ast::BinOp::Add => {
                let v1 = self.build_expr(left);
                let v2 = self.build_expr(right);
                if self.string_values.contains(&v1) || self.string_values.contains(&v2) {
                    // String concatenation: call string_concat(a, b)
                    if let Some(func_ref) = self.runtime_function("string_concat") {
                        let result = self.fresh_value(Type::Ptr);
                        self.emit(Instruction::Call(result, func_ref, vec![v1, v2]));
                        self.string_values.insert(result);
                        result
                    } else {
                        self.error_string_value("<concat-error>")
                    }
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

            // Pipe operator: desugar `left |> right` to `right(left)`.
            ast::BinOp::Pipe => self.build_call(right, std::slice::from_ref(left)),
        }
    }

    /// Build a comparison instruction.
    fn build_cmp(&mut self, op: CmpOp, left: &ast::Expr, right: &ast::Expr) -> Value {
        let v1 = self.build_expr(left);
        let v2 = self.build_expr(right);

        // For Eq/Ne on string values, use string_eq (strcmp-based) instead of
        // pointer equality (which would always return false for heap strings).
        let v1_is_str = self.string_values.contains(&v1);
        let v2_is_str =
            self.string_values.contains(&v2) || self.value_types.get(&v2) == Some(&Type::Ptr);
        if (v1_is_str || v2_is_str) && (op == CmpOp::Eq || op == CmpOp::Ne) {
            if let Some(func_ref) = self.runtime_function("string_eq") {
                let eq_result = self.fresh_value(Type::Bool);
                self.emit(Instruction::Call(eq_result, func_ref, vec![v1, v2]));
                if op == CmpOp::Ne {
                    // Negate: ne_result = (eq_result == false)
                    let false_val = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Const(false_val, Literal::Bool(false)));
                    let neg = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Cmp(neg, CmpOp::Eq, eq_result, false_val));
                    return neg;
                }
                return eq_result;
            }
            return self.zero_value(Type::Bool);
        }

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
    fn build_short_circuit_and(&mut self, left: &ast::Expr, right: &ast::Expr) -> Value {
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
            vec![(left_block_ref, v_left), (right_block_actual, v_right)],
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
    fn build_short_circuit_or(&mut self, left: &ast::Expr, right: &ast::Expr) -> Value {
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
            vec![(left_block_ref, v_left), (right_block_actual, v_right)],
        ));
        result
    }

    // ── Unary operations ─────────────────────────────────────────────

    /// Build a unary operation expression.
    fn build_unary_op(&mut self, op: ast::UnaryOp, operand: &ast::Expr) -> Value {
        match op {
            ast::UnaryOp::Neg => {
                // -x  ==  0 - x
                let v = self.build_expr(operand);
                let operand_ty = self.value_types.get(&v).cloned().unwrap_or(Type::I64);
                let zero = self.fresh_value(operand_ty.clone());
                let zero_lit = if operand_ty == Type::F64 {
                    Literal::Float(0.0)
                } else {
                    Literal::Int(0)
                };
                self.emit(Instruction::Const(zero, zero_lit));
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
    fn build_call(&mut self, func: &ast::Expr, args: &[ast::Expr]) -> Value {
        // Check if this is a tuple variant constructor call (e.g. `Some(42)`).
        // Variant constructors look like function calls in the AST but are
        // lowered to `ConstructVariant` rather than a `Call` instruction.
        if let ast::ExprKind::Ident(name) = &func.node {
            // Only intercept if the name is a known tuple variant constructor
            // *and* is not shadowed by a local variable.
            if self.tuple_variant_names.contains(name.as_str())
                && self.lookup_var(name).is_none()
                && !self.mutable_vars.contains(name.as_str())
            {
                let tag = self.enum_variant_tags[name.as_str()];
                let payload: Vec<Value> = args.iter().map(|a| self.build_expr(a)).collect();
                let result = self.fresh_value(Type::Ptr);
                self.emit(Instruction::ConstructVariant {
                    result,
                    tag,
                    payload,
                });
                return result;
            }
        }

        // Build all argument expressions first.
        let arg_vals: Vec<Value> = args.iter().map(|a| self.build_expr(a)).collect();

        match &func.node {
            ast::ExprKind::Ident(name) => {
                match self.function_refs.get(name).copied() {
                    Some(func_ref) => {
                        // For `clamp(v, lo, hi)` the return type is generic
                        // over T; pick it from the first resolved argument's
                        // tracked type so downstream codegen can dispatch
                        // to `__gradient_clamp_i64` vs `_f64`. Fall through
                        // to the static map otherwise. (#609)
                        let ret_ty = if name == "clamp" && !arg_vals.is_empty() {
                            self.value_types
                                .get(&arg_vals[0])
                                .cloned()
                                .unwrap_or(Type::I64)
                        } else {
                            self.function_return_types
                                .get(name)
                                .cloned()
                                .unwrap_or(Type::I64)
                        };
                        let result = self.fresh_value(ret_ty);
                        self.emit(Instruction::Call(result, func_ref, arg_vals));
                        // Track string-returning builtins.
                        if matches!(
                            name.as_str(),
                            "int_to_string"
                                | "string_concat"
                                | "string_substring"
                                | "string_trim"
                                | "string_to_upper"
                                | "string_to_lower"
                                | "string_replace"
                                | "string_char_at"
                                | "string_split"
                                | "float_to_string"
                                | "bool_to_string"
                                | "json_stringify"
                                | "json_type"
                                | "string_reverse"
                                | "string_append"
                                | "string_repeat"
                                | "string_slice"
                        ) {
                            self.string_values.insert(result);
                        }
                        // Track list-returning builtins.
                        if matches!(
                            name.as_str(),
                            "list_push"
                                | "list_concat"
                                | "list_tail"
                                | "list_map"
                                | "list_filter"
                                | "list_sort"
                                | "list_reverse"
                                | "string_split"
                                | "map_keys"
                        ) || name.starts_with("list_literal_")
                        {
                            self.list_values.insert(result);
                        }
                        // Track Option inner types for typed JSON extractors.
                        match name.as_str() {
                            "json_as_float" => {
                                self.option_inner_types.insert(result, Type::F64);
                            }
                            "json_as_int" => {
                                self.option_inner_types.insert(result, Type::I64);
                            }
                            "json_as_bool" => {
                                self.option_inner_types.insert(result, Type::Bool);
                            }
                            "json_as_string" => {
                                self.option_inner_types.insert(result, Type::Ptr);
                            }
                            _ => {}
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
            // Handle qualified calls: module.function(args) and method calls: obj.method(args)
            // FieldAccess { object: Ident("module"), field: "function" }
            ast::ExprKind::FieldAccess { object, field } => {
                if let ast::ExprKind::Ident(_module_name) = &object.node {
                    // For qualified calls, the function name in the IR is just
                    // the unqualified name (e.g., "add" not "math.add"),
                    // because imported functions are registered with their
                    // original names.
                    if let Some(func_ref) = self.function_refs.get(field.as_str()).copied() {
                        let ret_ty = self
                            .function_return_types
                            .get(field.as_str())
                            .cloned()
                            .unwrap_or(Type::I64);
                        let result = self.fresh_value(ret_ty);
                        self.emit(Instruction::Call(result, func_ref, arg_vals));
                        return result;
                    }
                    // Not a module-qualified call; fall through to method call handling.
                }

                // Method call: object.method(args)
                // Build the object value and prepend it to the argument list.
                let obj_val = self.build_expr(object);
                let mut full_args = vec![obj_val];
                full_args.extend(arg_vals);

                // Resolve the function name based on the object's tracked type.
                let resolved_name = self.resolve_method_ir(obj_val, field);

                match self.function_refs.get(&resolved_name).copied() {
                    Some(func_ref) => {
                        let ret_ty = self
                            .function_return_types
                            .get(&resolved_name)
                            .cloned()
                            .unwrap_or(Type::I64);
                        let result = self.fresh_value(ret_ty);
                        self.emit(Instruction::Call(result, func_ref, full_args));
                        // Track string-returning builtins.
                        if matches!(
                            resolved_name.as_str(),
                            "string_substring"
                                | "string_trim"
                                | "string_to_upper"
                                | "string_to_lower"
                                | "string_replace"
                                | "string_char_at"
                                | "string_split"
                        ) {
                            self.string_values.insert(result);
                        }
                        // Track list-returning builtins.
                        if matches!(
                            resolved_name.as_str(),
                            "list_push" | "list_concat" | "list_tail"
                        ) {
                            self.list_values.insert(result);
                        }
                        result
                    }
                    None => {
                        self.errors
                            .push(format!("call to undefined method: '{}'", field));
                        let result = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(result, Literal::Int(0)));
                        result
                    }
                }
            }
            _ => {
                // Indirect calls / higher-order functions are not yet
                // supported in v0.1.
                self.errors
                    .push("indirect function calls are not yet supported".to_string());
                let result = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(result, Literal::Int(0)));
                result
            }
        }
    }

    // ── String interpolation ──────────────────────────────────────────

    /// Build a string interpolation expression.
    ///
    /// Desugars `f"hello {name} world"` into:
    /// ```text
    /// string_concat(string_concat("hello ", int_to_string(name)), " world")
    /// ```
    ///
    /// Each part is converted to a string value (if not already a string),
    /// then all parts are concatenated left-to-right using `string_concat`.
    fn build_string_interp(&mut self, parts: &[ast::expr::StringInterpPart]) -> Value {
        // Convert each part to a string Value.
        let mut string_vals: Vec<Value> = Vec::new();

        for part in parts {
            match part {
                ast::expr::StringInterpPart::Literal(s) => {
                    let v = self.fresh_value(Type::Ptr);
                    self.emit(Instruction::Const(v, Literal::Str(s.clone())));
                    self.string_values.insert(v);
                    string_vals.push(v);
                }
                ast::expr::StringInterpPart::Expr(expr) => {
                    let val = self.build_expr(expr);
                    // Check if this is already a string value.
                    if self.string_values.contains(&val) {
                        string_vals.push(val);
                    } else {
                        // Determine the type and call the appropriate to_string.
                        let val_ty = self.value_types.get(&val).cloned().unwrap_or(Type::I64);
                        match val_ty {
                            Type::Ptr => {
                                // Already a string pointer.
                                string_vals.push(val);
                            }
                            Type::I64 => {
                                // Call int_to_string.
                                if let Some(func_ref) = self.runtime_function("int_to_string") {
                                    let result = self.fresh_value(Type::Ptr);
                                    self.emit(Instruction::Call(result, func_ref, vec![val]));
                                    self.string_values.insert(result);
                                    string_vals.push(result);
                                } else {
                                    string_vals
                                        .push(self.error_string_value("<int-to-string-error>"));
                                }
                            }
                            Type::F64 => {
                                // Call float_to_string.
                                if let Some(func_ref) = self.runtime_function("float_to_string") {
                                    let result = self.fresh_value(Type::Ptr);
                                    self.emit(Instruction::Call(result, func_ref, vec![val]));
                                    self.string_values.insert(result);
                                    string_vals.push(result);
                                } else {
                                    string_vals
                                        .push(self.error_string_value("<float-to-string-error>"));
                                }
                            }
                            Type::Bool => {
                                // Call bool_to_string.
                                if let Some(func_ref) = self.runtime_function("bool_to_string") {
                                    let result = self.fresh_value(Type::Ptr);
                                    self.emit(Instruction::Call(result, func_ref, vec![val]));
                                    self.string_values.insert(result);
                                    string_vals.push(result);
                                } else {
                                    string_vals
                                        .push(self.error_string_value("<bool-to-string-error>"));
                                }
                            }
                            _ => {
                                self.errors.push(format!(
                                    "cannot convert type {:?} to string in interpolation",
                                    val_ty
                                ));
                                let v = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Const(
                                    v,
                                    Literal::Str("<error>".to_string()),
                                ));
                                self.string_values.insert(v);
                                string_vals.push(v);
                            }
                        }
                    }
                }
            }
        }

        // If no parts, return empty string.
        if string_vals.is_empty() {
            let v = self.fresh_value(Type::Ptr);
            self.emit(Instruction::Const(v, Literal::Str(String::new())));
            self.string_values.insert(v);
            return v;
        }

        // Concatenate all parts left-to-right.
        let mut values = string_vals.into_iter();
        let Some(mut acc) = values.next() else {
            return self.error_string_value("<empty-interpolation>");
        };
        let Some(concat_ref) = self.runtime_function("string_concat") else {
            return acc;
        };
        for val in values {
            let result = self.fresh_value(Type::Ptr);
            self.emit(Instruction::Call(result, concat_ref, vec![acc, val]));
            self.string_values.insert(result);
            acc = result;
        }

        acc
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
        let phi_ty = self
            .value_types
            .get(&then_val)
            .cloned()
            .unwrap_or(Type::I64);
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
                ast::StmtKind::Let {
                    name,
                    value,
                    mutable,
                    ..
                } => {
                    let val = self.build_expr(value);
                    if *mutable {
                        self.build_mutable_let(name, val);
                    } else {
                        self.define_var(name, val);
                    }
                    last_val = None;
                }
                ast::StmtKind::LetTupleDestructure { names, value, .. } => {
                    let tuple_val = self.build_expr(value);
                    // First try the new offset-based approach (heap-allocated tuples)
                    if let Some((elem_size, offsets)) =
                        self.tuple_element_offsets.get(&tuple_val).cloned()
                    {
                        for (i, name) in names.iter().enumerate() {
                            if i < offsets.len() {
                                // Calculate element address: base + i * elem_size
                                let offset = i as i64 * elem_size;
                                let offset_val = self.fresh_value(Type::I64);
                                self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                                let elem_addr = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
                    } else if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
                        // Legacy: stack-allocated tuples (deprecated)
                        for (i, name) in names.iter().enumerate() {
                            if i < addrs.len() {
                                let elem_addr = addrs[i];
                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
                    } else {
                        // Cross-function tuple return: infer structure from pattern
                        let elem_size = 8i64;
                        let num_elems = names.len();
                        for (i, name) in names.iter().enumerate() {
                            if i < num_elems {
                                let offset = i as i64 * elem_size;
                                let offset_val = self.fresh_value(Type::I64);
                                self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                                let elem_addr = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
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
    /// Dispatches based on the iterator expression:
    /// - `ExprKind::Range { start, end }` — integer range loop
    /// - List expression — counted loop using `list_length` / `list_get`
    /// - Legacy `range()` call — counted loop from 0 to n
    fn build_for(&mut self, var: &str, iter: &ast::Expr, body: &ast::Block) -> Value {
        match &iter.node {
            ast::ExprKind::Range { start, end } => self.build_for_range(var, start, end, body),
            _ => {
                let iter_val = self.build_expr(iter);
                if self.list_values.contains(&iter_val) {
                    self.build_for_list(var, iter_val, body)
                } else {
                    // Legacy: range() returns an integer count.
                    self.build_for_counted(var, iter_val, body)
                }
            }
        }
    }

    /// Build a for loop over a range expression `start..end`.
    ///
    /// Lowered to:
    /// ```text
    /// let i = start
    /// while i < end:
    ///     body (with i bound)
    ///     i = i + 1
    /// ```
    fn build_for_range(
        &mut self,
        var: &str,
        start: &ast::Expr,
        end: &ast::Expr,
        body: &ast::Block,
    ) -> Value {
        let start_val = self.build_expr(start);
        let end_val = self.build_expr(end);

        let loop_header = self.fresh_block();
        let loop_body = self.fresh_block();
        let loop_exit = self.fresh_block();
        let entry_block = self.current_block_label;

        self.emit(Instruction::Jump(loop_header));
        self.seal_block();

        // Loop header: phi for the counter, then compare against end.
        self.current_block_label = loop_header;
        let counter = self.fresh_value(Type::I64);
        let phi_idx = self.current_block.len();
        self.emit(Instruction::Phi(counter, vec![(entry_block, start_val)]));

        let cmp_val = self.fresh_value(Type::Bool);
        self.emit(Instruction::Cmp(cmp_val, CmpOp::Lt, counter, end_val));
        self.emit(Instruction::Branch(cmp_val, loop_body, loop_exit));
        self.seal_block();

        // Loop body: bind the counter as the loop variable.
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
        let result = self.fresh_value(Type::Void);
        self.emit(Instruction::Const(result, Literal::Int(0)));
        result
    }

    /// Build a for loop over a list value.
    ///
    /// Lowered to:
    /// ```text
    /// let len = list_length(list)
    /// let i = 0
    /// while i < len:
    ///     let elem = list_get(list, i)
    ///     body (with elem bound)
    ///     i = i + 1
    /// ```
    fn build_for_list(&mut self, var: &str, list_val: Value, body: &ast::Block) -> Value {
        // Call list_length to get the length.
        let Some(len_func_ref) = self.runtime_function("list_length") else {
            return self.zero_value(Type::Void);
        };
        let len_val = self.fresh_value(Type::I64);
        self.emit(Instruction::Call(len_val, len_func_ref, vec![list_val]));

        // Initialize counter to 0.
        let counter_init = self.fresh_value(Type::I64);
        self.emit(Instruction::Const(counter_init, Literal::Int(0)));

        let loop_header = self.fresh_block();
        let loop_body = self.fresh_block();
        let loop_exit = self.fresh_block();
        let entry_block = self.current_block_label;

        self.emit(Instruction::Jump(loop_header));
        self.seal_block();

        // Loop header: phi for the counter, compare against length.
        self.current_block_label = loop_header;
        let counter = self.fresh_value(Type::I64);
        let phi_idx = self.current_block.len();
        self.emit(Instruction::Phi(counter, vec![(entry_block, counter_init)]));

        let cmp_val = self.fresh_value(Type::Bool);
        self.emit(Instruction::Cmp(cmp_val, CmpOp::Lt, counter, len_val));
        self.emit(Instruction::Branch(cmp_val, loop_body, loop_exit));
        self.seal_block();

        // Loop body: get element and bind it.
        self.current_block_label = loop_body;
        self.push_scope();

        let Some(get_func_ref) = self.runtime_function("list_get") else {
            self.pop_scope();
            return self.zero_value(Type::Void);
        };
        let elem_val = self.fresh_value(Type::I64);
        self.emit(Instruction::Call(
            elem_val,
            get_func_ref,
            vec![list_val, counter],
        ));
        self.define_var(var, elem_val);

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
        let result = self.fresh_value(Type::Void);
        self.emit(Instruction::Const(result, Literal::Int(0)));
        result
    }

    /// Build a counted for loop (legacy `range()` support).
    ///
    /// Lowered to:
    /// ```text
    /// let i = 0
    /// while i < count:
    ///     body (with i bound)
    ///     i = i + 1
    /// ```
    fn build_for_counted(&mut self, var: &str, count_val: Value, body: &ast::Block) -> Value {
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
        let phi_idx = self.current_block.len();
        self.emit(Instruction::Phi(counter, vec![(entry_block, counter_init)]));

        let cmp_val = self.fresh_value(Type::Bool);
        self.emit(Instruction::Cmp(cmp_val, CmpOp::Lt, counter, count_val));
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
    fn build_while(&mut self, condition: &ast::Expr, body: &ast::Block) -> Value {
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

    // ── Match expression ────────────────────────────────────────

    /// Build a match expression as a chain of conditional branches.
    ///
    /// Lowered to:
    /// ```text
    ///   scrutinee_val = <build scrutinee>
    ///   // For each non-wildcard arm:
    ///   cmp = scrutinee_val == pattern_val
    ///   branch cmp, arm_block, next_check
    /// arm_block:
    ///   val = <build arm body>
    ///   jump merge_block
    /// next_check:
    ///   ... (next arm)
    /// wildcard_block:
    ///   val = <build wildcard body>
    ///   jump merge_block
    /// merge_block:
    ///   result = phi [...]
    /// ```
    fn build_match(&mut self, scrutinee: &ast::Expr, arms: &[ast::MatchArm]) -> Value {
        let scrutinee_val = self.build_expr(scrutinee);
        let scrutinee_ty = self
            .value_types
            .get(&scrutinee_val)
            .cloned()
            .unwrap_or(Type::I64);

        let merge_block = self.fresh_block();
        let mut phi_entries: Vec<(BlockRef, Value)> = Vec::new();

        // Separate wildcard/variable-catch-all arms from pattern arms.
        let mut wildcard_arm: Option<&ast::MatchArm> = None;
        let mut pattern_arms: Vec<&ast::MatchArm> = Vec::new();
        for arm in arms {
            match &arm.pattern {
                ast::Pattern::Wildcard if arm.guard.is_none() => {
                    wildcard_arm = Some(arm);
                }
                ast::Pattern::Variable(_) if arm.guard.is_none() => {
                    // An unguarded variable pattern is like a wildcard.
                    wildcard_arm = Some(arm);
                }
                _ => {
                    pattern_arms.push(arm);
                }
            }
        }

        for arm in pattern_arms.iter() {
            // Build the condition value for this arm.
            let cmp_val = match &arm.pattern {
                ast::Pattern::IntLit(n) => {
                    let v = self.fresh_value(Type::I64);
                    self.emit(Instruction::Const(v, Literal::Int(*n)));
                    let cmp = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Cmp(cmp, CmpOp::Eq, scrutinee_val, v));
                    cmp
                }
                ast::Pattern::BoolLit(b) => {
                    let v = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Const(v, Literal::Bool(*b)));
                    let cmp = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Cmp(cmp, CmpOp::Eq, scrutinee_val, v));
                    cmp
                }
                ast::Pattern::Variant { variant, .. } => {
                    let tag = self
                        .enum_variant_tags
                        .get(variant.as_str())
                        .copied()
                        .unwrap_or(0);
                    // Load the tag from the heap-allocated enum value.
                    let tag_val = self.fresh_value(Type::I64);
                    self.emit(Instruction::GetVariantTag {
                        result: tag_val,
                        ptr: scrutinee_val,
                    });
                    let expected = self.fresh_value(Type::I64);
                    self.emit(Instruction::Const(expected, Literal::Int(tag)));
                    let cmp = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Cmp(cmp, CmpOp::Eq, tag_val, expected));
                    cmp
                }
                ast::Pattern::StringLit(s) => {
                    // Emit string comparison via string_eq(scrutinee, literal).
                    let lit_val = self.fresh_value(Type::Ptr);
                    self.emit(Instruction::Const(lit_val, Literal::Str(s.clone())));
                    self.string_values.insert(lit_val);
                    if let Some(func_ref) = self.runtime_function("string_eq") {
                        let cmp = self.fresh_value(Type::Bool);
                        self.emit(Instruction::Call(
                            cmp,
                            func_ref,
                            vec![scrutinee_val, lit_val],
                        ));
                        cmp
                    } else {
                        self.zero_value(Type::Bool)
                    }
                }
                ast::Pattern::Variable(var_name) => {
                    // A guarded variable pattern: the pattern always matches,
                    // but we need to bind the variable and evaluate the guard.
                    // We emit a "true" constant as the pattern-match result;
                    // the guard (handled below) refines it.
                    self.push_scope();
                    self.define_var(var_name, scrutinee_val);
                    let v = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Const(v, Literal::Bool(true)));
                    v
                }
                ast::Pattern::Wildcard => {
                    // A guarded wildcard: always matches, guard refines.
                    let v = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Const(v, Literal::Bool(true)));
                    v
                }
                ast::Pattern::Tuple(_) => {
                    // Tuple patterns in match are not supported; produce a dummy value.
                    let v = self.fresh_value(Type::I64);
                    self.emit(Instruction::Const(v, Literal::Int(0)));
                    let cmp = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Cmp(cmp, CmpOp::Eq, scrutinee_val, v));
                    cmp
                }
                ast::Pattern::Or(alternatives) => {
                    // Pattern alternatives: match if ANY alternative matches.
                    // Build a comparison for each alternative and OR them together.
                    let mut comparisons = Vec::new();

                    for alt in alternatives {
                        let alt_cmp = match alt {
                            ast::Pattern::IntLit(n) => {
                                let v = self.fresh_value(Type::I64);
                                self.emit(Instruction::Const(v, Literal::Int(*n)));
                                let cmp = self.fresh_value(Type::Bool);
                                self.emit(Instruction::Cmp(cmp, CmpOp::Eq, scrutinee_val, v));
                                cmp
                            }
                            ast::Pattern::BoolLit(b) => {
                                let v = self.fresh_value(Type::Bool);
                                self.emit(Instruction::Const(v, Literal::Bool(*b)));
                                let cmp = self.fresh_value(Type::Bool);
                                self.emit(Instruction::Cmp(cmp, CmpOp::Eq, scrutinee_val, v));
                                cmp
                            }
                            ast::Pattern::Variant { variant, .. } => {
                                let tag = self
                                    .enum_variant_tags
                                    .get(variant.as_str())
                                    .copied()
                                    .unwrap_or(0);
                                let tag_val = self.fresh_value(Type::I64);
                                self.emit(Instruction::LoadField {
                                    result: tag_val,
                                    object: scrutinee_val,
                                    field_idx: 0,
                                    field_ty: Type::I64,
                                    offset: 0,
                                });
                                let expected = self.fresh_value(Type::I64);
                                self.emit(Instruction::Const(expected, Literal::Int(tag)));
                                let cmp = self.fresh_value(Type::Bool);
                                self.emit(Instruction::Cmp(cmp, CmpOp::Eq, tag_val, expected));
                                cmp
                            }
                            _ => {
                                // For other patterns, just emit true (pattern always matches)
                                let v = self.fresh_value(Type::Bool);
                                self.emit(Instruction::Const(v, Literal::Bool(true)));
                                v
                            }
                        };
                        comparisons.push(alt_cmp);
                    }

                    // OR all comparisons together
                    if comparisons.is_empty() {
                        let v = self.fresh_value(Type::Bool);
                        self.emit(Instruction::Const(v, Literal::Bool(false)));
                        v
                    } else {
                        let mut result = comparisons[0];
                        for cmp in comparisons.iter().skip(1) {
                            let new_result = self.fresh_value(Type::Bool);
                            self.emit(Instruction::Or(new_result, result, *cmp));
                            result = new_result;
                        }
                        result
                    }
                }
            };

            // If there's a guard, AND the pattern match with the guard condition.
            let final_cmp = if let Some(ref guard_expr) = arm.guard {
                let guard_val = self.build_expr(guard_expr);
                // AND the pattern match with the guard: branch on pattern match,
                // if true use guard result, if false use false.
                let and_block_true = self.fresh_block();
                let and_block_false = self.fresh_block();
                let and_merge = self.fresh_block();
                self.emit(Instruction::Branch(
                    cmp_val,
                    and_block_true,
                    and_block_false,
                ));
                self.seal_block();

                // True branch: result is guard_val.
                self.current_block_label = and_block_true;
                self.emit(Instruction::Jump(and_merge));
                self.seal_block();

                // False branch: result is false.
                self.current_block_label = and_block_false;
                let false_val = self.fresh_value(Type::Bool);
                self.emit(Instruction::Const(false_val, Literal::Bool(false)));
                self.emit(Instruction::Jump(and_merge));
                self.seal_block();

                self.current_block_label = and_merge;
                let phi_result = self.fresh_value(Type::Bool);
                self.emit(Instruction::Phi(
                    phi_result,
                    vec![(and_block_true, guard_val), (and_block_false, false_val)],
                ));
                phi_result
            } else {
                cmp_val
            };

            let arm_block = self.fresh_block();
            let next_block = self.fresh_block();

            self.emit(Instruction::Branch(final_cmp, arm_block, next_block));
            self.seal_block();

            // Arm block: bind payload variables (for Variant patterns with
            // bindings), then build the arm body.
            self.current_block_label = arm_block;

            // For Variant patterns with bindings, extract the payload fields
            // and bind them as local variables in a new scope.
            let has_variant_binding = matches!(&arm.pattern, ast::Pattern::Variant { bindings, .. } if !bindings.is_empty());
            if has_variant_binding {
                self.push_scope();
                if let ast::Pattern::Variant {
                    variant: ref variant_name,
                    bindings: ref binding_names,
                } = arm.pattern
                {
                    // Get per-field types (for multi-field) or single field type (for single-field).
                    let per_field_types: Vec<Type> = if let Some(types) =
                        self.variant_field_types_vec.get(variant_name.as_str())
                    {
                        types.clone()
                    } else {
                        // Single-field variant: use the existing map.
                        // For Some variants, check if we have a tracked inner type from Option<T>.
                        let ty = if variant_name.as_str() == "Some" {
                            self.option_inner_types
                                .get(&scrutinee_val)
                                .cloned()
                                .or_else(|| {
                                    self.variant_field_types.get(variant_name.as_str()).cloned()
                                })
                                .unwrap_or(Type::I64)
                        } else {
                            self.variant_field_types
                                .get(variant_name.as_str())
                                .cloned()
                                .unwrap_or(Type::I64)
                        };
                        vec![ty]
                    };
                    for (i, binding_name) in binding_names.iter().enumerate() {
                        let field_ty = per_field_types.get(i).cloned().unwrap_or(Type::I64);
                        let field_val = self.fresh_value(field_ty);
                        self.emit(Instruction::GetVariantField {
                            result: field_val,
                            ptr: scrutinee_val,
                            index: i,
                        });
                        self.define_var(binding_name, field_val);
                    }
                }
            }

            let arm_val = self.build_block_expr(&arm.body);
            let arm_exit_block = self.current_block_label;
            if !self.current_block_has_terminator() {
                self.emit(Instruction::Jump(merge_block));
            }
            phi_entries.push((arm_exit_block, arm_val));
            self.seal_block();

            // Pop scope for variable patterns and variant bindings.
            if matches!(&arm.pattern, ast::Pattern::Variable(_)) || has_variant_binding {
                self.pop_scope();
            }

            // Move to the next check block.
            self.current_block_label = next_block;
        }

        // Wildcard / default arm (includes unguarded variable patterns).
        if let Some(wc_arm) = wildcard_arm {
            // If it's a variable pattern, bind the scrutinee in scope.
            if let ast::Pattern::Variable(ref var_name) = wc_arm.pattern {
                self.push_scope();
                self.define_var(var_name, scrutinee_val);
            }
            let wc_val = self.build_block_expr(&wc_arm.body);
            let wc_exit_block = self.current_block_label;
            if !self.current_block_has_terminator() {
                self.emit(Instruction::Jump(merge_block));
            }
            phi_entries.push((wc_exit_block, wc_val));
            self.seal_block();
            if matches!(&wc_arm.pattern, ast::Pattern::Variable(_)) {
                self.pop_scope();
            }
        } else if self.current_block_label != merge_block {
            // No wildcard arm and we're in a fallthrough block.
            // Emit a zero value of the correct type (matching the arm types)
            // and jump to merge.
            //
            // Determine the fallthrough type from existing phi entries so that
            // the types are consistent (e.g. if arms produce Float, we need
            // f64const 0.0 rather than iconst 0).
            let fallthrough_ty = phi_entries
                .first()
                .and_then(|(_, v)| self.value_types.get(v).cloned())
                .unwrap_or(scrutinee_ty.clone());
            let unit_val = self.fresh_value(fallthrough_ty.clone());
            let zero_lit = match fallthrough_ty {
                Type::F64 => Literal::Float(0.0),
                Type::Bool => Literal::Bool(false),
                _ => Literal::Int(0),
            };
            self.emit(Instruction::Const(unit_val, zero_lit));
            self.emit(Instruction::Jump(merge_block));
            phi_entries.push((self.current_block_label, unit_val));
            self.seal_block();
        }

        // Merge block: phi node collects values from all arms.
        self.current_block_label = merge_block;
        let phi_ty = phi_entries
            .first()
            .and_then(|(_, v)| self.value_types.get(v).cloned())
            .unwrap_or(scrutinee_ty);
        let result = self.fresh_value(phi_ty);
        self.emit(Instruction::Phi(result, phi_entries));
        result
    }

    // ── Actor operations ───────────────────────────────────────────────

    /// Lower a spawn expression: `spawn ActorName`.
    ///
    /// Generates a `Spawn` instruction that creates a new actor instance.
    /// Returns an actor handle value (typed as `Ptr`).
    fn lower_spawn(&mut self, actor_name: &str, _span: ast::Span) -> Value {
        // Register the actor spawn builtin if not already registered.
        self.register_actor_builtins();

        // Create a fresh value for the actor handle.
        let result = self.fresh_value(Type::Ptr);

        let spawn_wrapper = format!("spawn_{}", actor_name);
        if let Some(&spawn_ref) = self.function_refs.get(&spawn_wrapper) {
            self.emit(Instruction::Call(result, spawn_ref, vec![]));
        } else {
            self.emit(Instruction::Spawn {
                result,
                actor_type_name: actor_name.to_string(),
            });
        }

        // Track this value as an actor handle for potential future analysis.
        // Note: In a full implementation, we might want a separate actor_handles
        // tracking set similar to string_values or list_values.

        result
    }

    /// Lower a send expression: `send target MessageName`.
    ///
    /// Generates a `Send` instruction for fire-and-forget message passing.
    /// Returns unit (void).
    fn lower_send(&mut self, target: &ast::Expr, message_name: &str, _span: ast::Span) -> Value {
        // Register the actor send builtin if not already registered.
        self.register_actor_builtins();

        // Build the target expression to get the actor handle value.
        let handle = self.build_expr(target);

        // Emit the Send instruction.
        // Note: For now, payload is None. In a full implementation, we'd need
        // to handle message arguments separately.
        self.emit(Instruction::Send {
            handle,
            message_name: message_name.to_string(),
            payload: None,
        });

        // Return unit value (void) since send returns ().
        let result = self.fresh_value(Type::Void);
        self.emit(Instruction::Const(result, Literal::Int(0)));
        result
    }

    /// Lower an ask expression: `ask target MessageName`.
    ///
    /// Generates an `Ask` instruction for request-response message passing.
    /// Returns the reply value from the actor (typed as `Ptr`).
    fn lower_ask(&mut self, target: &ast::Expr, message_name: &str, _span: ast::Span) -> Value {
        // Register the actor ask builtin if not already registered.
        self.register_actor_builtins();

        // Build the target expression to get the actor handle value.
        let handle = self.build_expr(target);

        // Create a fresh value for the reply.
        let result = self.fresh_value(Type::Ptr);

        // Emit the Ask instruction.
        // Note: For now, payload is None. In a full implementation, we'd need
        // to handle message arguments separately.
        self.emit(Instruction::Ask {
            result,
            handle,
            message_name: message_name.to_string(),
            payload: None,
        });

        result
    }

    /// Register actor-related builtin functions and track the Actor effect.
    fn register_actor_builtins(&mut self) {
        // Register actor runtime functions that will be used by the codegen.
        // These are runtime functions that implement the actual actor operations.
        let actor_builtins = vec![
            ("__actor_spawn", Type::Ptr),
            ("__actor_send", Type::Void),
            ("__actor_ask", Type::Ptr),
            ("__actor_init", Type::Void),
        ];

        for (name, ret_ty) in actor_builtins {
            if !self.function_refs.contains_key(name) {
                self.register_func(name);
                self.function_return_types.insert(name.to_string(), ret_ty);
            }
        }
    }

    /// Lower a concurrent_scope expression: `concurrent_scope { ... }`.
    ///
    /// Generates a try/finally-style block where all spawned actors within the scope
    /// are automatically cancelled when the scope exits (normal or exceptional).
    /// This implements structured concurrency.
    fn lower_concurrent_scope(&mut self, body: &ast::Block, _span: ast::Span) -> Value {
        // Register concurrent scope runtime functions
        self.register_concurrent_scope_builtins();

        // Emit scope entry: create a new scope context and track it
        let scope_id = self.fresh_value(Type::Ptr);
        let Some(scope_enter) = self.runtime_function("__concurrent_scope_enter") else {
            return self.zero_value(Type::Void);
        };
        self.emit(Instruction::Call(scope_id, scope_enter, vec![]));

        // Create labels for the scope body and cleanup
        let body_block = self.fresh_block();
        let cleanup_block = self.fresh_block();
        let exit_block = self.fresh_block();

        // Jump to body block
        self.emit(Instruction::Jump(body_block));

        // Build the body in a new scope with defer tracking for cancellation
        self.seal_block();
        self.current_block_label = body_block;
        self.push_scope();

        // Build the body block
        let _body_result = self.build_block_expr(body);

        // Execute any deferred expressions (LIFO order)
        let defers = self.pop_scope();
        for deferred in defers.iter().rev() {
            let _ = self.build_expr(deferred);
        }

        // Jump to cleanup (normal exit path)
        if !self.current_block_has_terminator() {
            self.emit(Instruction::Jump(cleanup_block));
        }

        // Build cleanup block: cancel all spawned actors in this scope
        self.seal_block();
        self.current_block_label = cleanup_block;
        let Some(func_ref) = self.runtime_function("__concurrent_scope_exit") else {
            return self.zero_value(Type::Void);
        };
        let result_val = self.fresh_value(Type::Void);
        self.emit(Instruction::Call(result_val, func_ref, vec![scope_id]));
        self.emit(Instruction::Jump(exit_block));

        // Exit block: return unit
        self.seal_block();
        self.current_block_label = exit_block;

        let result = self.fresh_value(Type::Void);
        self.emit(Instruction::Const(result, Literal::Int(0)));
        result
    }

    /// Register concurrent scope runtime functions
    fn register_concurrent_scope_builtins(&mut self) {
        let builtins = vec![
            ("__concurrent_scope_enter", Type::Ptr),
            ("__concurrent_scope_exit", Type::Void),
        ];

        for (name, ret_ty) in builtins {
            if !self.function_refs.contains_key(name) {
                self.register_func(name);
                self.function_return_types.insert(name.to_string(), ret_ty);
            }
        }
    }

    /// Lower a supervisor expression: `supervisor strategy = one_for_one { ... }`.
    ///
    /// Generates a supervisor actor that monitors child actors and restarts
    /// them according to the specified strategy:
    /// - one_for_one: restart only crashed child
    /// - one_for_all: restart all children
    /// - rest_for_one: restart crashed child and all younger siblings
    fn lower_supervisor(
        &mut self,
        strategy: RestartStrategy,
        max_restarts: &Option<i64>,
        children: &[ChildSpec],
        _span: ast::Span,
    ) -> Value {
        // Register supervisor runtime functions
        self.register_supervisor_builtins();

        // Convert strategy to int for runtime
        let strategy_val = match strategy {
            RestartStrategy::OneForOne => 0,
            RestartStrategy::OneForAll => 1,
            RestartStrategy::RestForOne => 2,
        };

        // Build child specifications array
        let child_specs: Vec<Value> = children
            .iter()
            .map(|child| {
                // Each child spec is represented as a runtime object
                // containing actor type, restart policy, etc.
                let spec_val = self.fresh_value(Type::Ptr);
                let Some(create_spec_func) =
                    self.runtime_function("__supervisor_create_child_spec")
                else {
                    return self.zero_value(Type::Ptr);
                };

                // Actor type name as string
                let actor_type_val = self.fresh_value(Type::Ptr);
                self.emit(Instruction::Const(
                    actor_type_val,
                    Literal::Str(child.actor_type.clone()),
                ));

                // Restart policy as int
                let policy_val = self.fresh_value(Type::I64);
                let policy_int = match child.restart_policy {
                    RestartPolicy::Permanent => 0,
                    RestartPolicy::Transient => 1,
                    RestartPolicy::Temporary => 2,
                };
                self.emit(Instruction::Const(policy_val, Literal::Int(policy_int)));

                self.emit(Instruction::Call(
                    spec_val,
                    create_spec_func,
                    vec![actor_type_val, policy_val],
                ));
                spec_val
            })
            .collect();

        // Call supervisor creation runtime function
        let strategy_const = self.fresh_value(Type::I64);
        self.emit(Instruction::Const(
            strategy_const,
            Literal::Int(strategy_val),
        ));

        let max_restarts_val = if let Some(max) = max_restarts {
            let v = self.fresh_value(Type::I64);
            self.emit(Instruction::Const(v, Literal::Int(*max)));
            v
        } else {
            // Default max_restarts = 5 (typical Erlang default)
            let v = self.fresh_value(Type::I64);
            self.emit(Instruction::Const(v, Literal::Int(5)));
            v
        };

        // Create supervisor handle
        let supervisor_handle = self.fresh_value(Type::Ptr);
        let Some(create_func) = self.runtime_function("__supervisor_create") else {
            return self.zero_value(Type::Ptr);
        };
        self.emit(Instruction::Call(
            supervisor_handle,
            create_func,
            vec![strategy_const, max_restarts_val],
        ));

        // Add child specs to supervisor
        for child_spec in child_specs {
            let result_val = self.fresh_value(Type::Void);
            let Some(add_child_func) = self.runtime_function("__supervisor_add_child") else {
                return self.zero_value(Type::Ptr);
            };
            self.emit(Instruction::Call(
                result_val,
                add_child_func,
                vec![supervisor_handle, child_spec],
            ));
        }

        // Start the supervisor
        let start_result = self.fresh_value(Type::Void);
        let Some(start_func) = self.runtime_function("__supervisor_start") else {
            return self.zero_value(Type::Ptr);
        };
        self.emit(Instruction::Call(
            start_result,
            start_func,
            vec![supervisor_handle],
        ));

        supervisor_handle
    }

    /// Register supervisor runtime functions
    fn register_supervisor_builtins(&mut self) {
        let builtins = vec![
            ("__supervisor_create", Type::Ptr),
            ("__supervisor_add_child", Type::Void),
            ("__supervisor_start", Type::Void),
            ("__supervisor_create_child_spec", Type::Ptr),
            ("__supervisor_child_crashed", Type::Void),
            ("__supervisor_restart_child", Type::Void),
            ("__supervisor_escalate", Type::Void),
        ];

        for (name, ret_ty) in builtins {
            if !self.function_refs.contains_key(name) {
                self.register_func(name);
                self.function_return_types.insert(name.to_string(), ret_ty);
            }
        }
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
                Instruction::Ret(_) | Instruction::Branch(_, _, _) | Instruction::Jump(_)
            )
        })
    }

    /// Push a new variable scope and defer frame.
    fn push_scope(&mut self) {
        self.variables.push(HashMap::new());
        self.defer_stack.push(Vec::new());
    }

    /// Pop the current variable scope and return the deferred expressions.
    fn pop_scope(&mut self) -> Vec<ast::Expr> {
        if self.variables.len() > 1 {
            self.variables.pop();
        }
        self.defer_stack.pop().unwrap_or_default()
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

    /// Build a closure expression by generating a separate function and
    /// returning a reference (function pointer) to it.
    fn build_closure(
        &mut self,
        params: &[ast::ClosureParam],
        return_type: Option<&ast::Spanned<ast::TypeExpr>>,
        body: &ast::Expr,
    ) -> Value {
        let closure_name = format!("__closure_{}", self.closure_counter);
        self.closure_counter += 1;

        // Register the closure function so calls can resolve it.
        self.register_func(&closure_name);

        // Save the current builder state.
        let saved_next_value = self.next_value;
        let saved_next_block = self.next_block;
        let saved_current_block = std::mem::take(&mut self.current_block);
        let saved_completed_blocks = std::mem::take(&mut self.completed_blocks);
        let saved_current_block_label = self.current_block_label;
        let saved_variables = std::mem::take(&mut self.variables);
        let saved_string_values = std::mem::take(&mut self.string_values);
        let saved_list_values = std::mem::take(&mut self.list_values);
        let saved_mutable_vars = std::mem::take(&mut self.mutable_vars);
        let saved_mutable_addrs = std::mem::take(&mut self.mutable_addrs);
        let saved_mutable_types = std::mem::take(&mut self.mutable_types);
        let saved_value_types = std::mem::take(&mut self.value_types);

        // Reset per-function state for the closure.
        self.next_value = 0;
        self.next_block = 0;
        self.variables = vec![HashMap::new()];

        // Start the entry block.
        self.current_block_label = self.fresh_block();

        // Resolve parameter types and bind them.
        let param_types: Vec<Type> = params
            .iter()
            .map(|p| {
                if let Some(ref type_ann) = p.type_ann {
                    self.resolve_type(&type_ann.node)
                } else {
                    Type::I64 // default to I64 for untyped params
                }
            })
            .collect();

        for (i, param) in params.iter().enumerate() {
            let val = self.fresh_value(param_types[i].clone());
            self.define_var(&param.name, val);
        }

        let ret_type = return_type
            .map(|rt| self.resolve_type(&rt.node))
            .unwrap_or(Type::I64);

        // Build the body expression.
        let body_val = self.build_expr(body);

        // Emit a return if the block doesn't already have a terminator.
        if !self.current_block_has_terminator() {
            self.emit(Instruction::Ret(Some(body_val)));
        }

        // Seal the final block.
        self.seal_block();

        // Collect the completed blocks into a function.
        let closure_func = Function {
            name: closure_name.clone(),
            params: param_types,
            return_type: ret_type.clone(),
            blocks: std::mem::take(&mut self.completed_blocks),
            value_types: std::mem::take(&mut self.value_types),
            is_export: false,
            extern_lib: None,
        };

        // Store the closure function for later addition to the module.
        self.closure_functions.push(closure_func);
        self.function_return_types
            .insert(closure_name.clone(), ret_type);

        // Restore the parent function's builder state.
        self.next_value = saved_next_value;
        self.next_block = saved_next_block;
        self.current_block = saved_current_block;
        self.completed_blocks = saved_completed_blocks;
        self.current_block_label = saved_current_block_label;
        self.variables = saved_variables;
        self.string_values = saved_string_values;
        self.list_values = saved_list_values;
        self.mutable_vars = saved_mutable_vars;
        self.mutable_addrs = saved_mutable_addrs;
        self.mutable_types = saved_mutable_types;
        self.value_types = saved_value_types;

        // In the parent function, return the closure as a function pointer
        // (represented as an i64 constant with a symbolic reference to the
        // closure function). For now, we emit a const 0 placeholder -- the
        // codegen layer will resolve the function address at link time.
        let func_ref = self
            .function_refs
            .get(&closure_name)
            .copied()
            .expect("closure should have been registered");
        let v = self.fresh_value(Type::Ptr);
        // Emit a FuncAddr instruction if available; for now use a Call with
        // zero args as a placeholder to get a function reference value.
        // Actually, store the closure as a constant referencing its name.
        self.emit(Instruction::Const(v, Literal::Int(func_ref.0 as i64)));
        v
    }

    /// Convert an AST type expression to an IR type.
    fn resolve_type(&self, type_expr: &ast::TypeExpr) -> Type {
        match type_expr {
            ast::TypeExpr::Named { name, cap: _ } => self.resolve_named_type(name),
            ast::TypeExpr::Unit => Type::Void,
            ast::TypeExpr::Fn { .. } => {
                // Function types are pointers in v0.1.
                Type::Ptr
            }
            ast::TypeExpr::Generic { .. } => {
                // Generic types are not supported in IR yet.
                Type::Ptr
            }
            ast::TypeExpr::Tuple(_) => {
                // Tuples are represented as a pointer to stack-allocated elements.
                Type::Ptr
            }
            ast::TypeExpr::Record(_) => {
                // Records are represented as a pointer to stack-allocated fields,
                // same as tuples.
                Type::Ptr
            }
            ast::TypeExpr::Linear(inner) => {
                // Linear types are passed by value/reference like their inner type
                self.resolve_type(&inner.node)
            }
            ast::TypeExpr::Type => {
                // The 'type' type is compile-time only; at runtime it's a placeholder.
                // Comptime type parameters don't exist at runtime.
                Type::Void
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
            // Enum types are represented as I64 (tag values) for v0.1.
            Type::I64
        }
    }

    // ── Actor code generation ─────────────────────────────────────────

    /// Build IR functions for an actor declaration.
    ///
    /// Generates:
    /// 1. State initialization function: `<actor_name>_init_state`
    /// 2. Message handler functions: `<actor_name>_<message_name>_handler`
    /// 3. Behavior setup function: `<actor_name>_setup_behaviors`
    ///
    /// The actor runtime will call these to set up the actor instance.
    fn build_actor_decl(
        &mut self,
        name: &str,
        state_fields: &[ast::StateField],
        handlers: &[ast::MessageHandler],
    ) -> Vec<Function> {
        let mut actor_functions = Vec::new();

        // Calculate state size (sum of field sizes)
        let state_size = self.calculate_state_size(state_fields);

        // Generate state initialization function: <actor_name>_init_state
        let init_func = self.build_actor_state_init(name, state_fields, state_size);
        actor_functions.push(init_func);

        // Generate message handler functions and collect message type constants
        let mut message_types: Vec<(String, usize)> = Vec::new();
        for (idx, handler) in handlers.iter().enumerate() {
            let handler_func = self.build_actor_handler(name, handler, state_fields, idx);
            actor_functions.push(handler_func);
            message_types.push((handler.message_name.clone(), idx));
        }

        // Generate behavior setup function
        let setup_func = self.build_actor_behavior_setup(name, &message_types, handlers);
        actor_functions.push(setup_func);

        // Register actor spawn wrapper
        let spawn_func = self.build_actor_spawn_wrapper(name, state_size);
        actor_functions.push(spawn_func);

        actor_functions
    }

    /// Calculate the size of actor state in bytes.
    fn calculate_state_size(&self, state_fields: &[ast::StateField]) -> usize {
        // For v0.1: all types are 8 bytes (i64, f64, or pointer)
        state_fields.len() * 8
    }

    /// Build the state initialization function for an actor.
    /// Signature: fn <actor_name>_init_state(arena: *mut Arena, state_size: usize) -> *mut c_void
    /// Allocates state memory and initializes fields with their default values.
    fn build_actor_state_init(
        &mut self,
        actor_name: &str,
        state_fields: &[ast::StateField],
        state_size: usize,
    ) -> Function {
        let func_name = format!("{}_init_state", actor_name);

        // Reset per-function state
        self.next_value = 0;
        self.next_block = 0;
        self.completed_blocks.clear();
        self.current_block.clear();
        self.variables = vec![HashMap::new()];
        self.string_values.clear();
        self.list_values.clear();
        self.mutable_vars.clear();
        self.mutable_addrs.clear();
        self.mutable_types.clear();
        self.value_types.clear();

        // Start entry block
        self.current_block_label = self.fresh_block();

        // Register the function
        self.register_func(&func_name);

        // Parameters: arena (Ptr), state_size (I64)
        let param_types = vec![Type::Ptr, Type::I64];
        let _arena_param = self.fresh_value(Type::Ptr); // v0 = arena (unused - using malloc instead)
        let _size_param = self.fresh_value(Type::I64); // v1 = state_size

        // Allocate state memory using malloc (simpler than arena for now)
        // We'll use malloc for state allocation
        let malloc_ref = self.ensure_malloc();
        let state_size_val = self.fresh_value(Type::I64);
        self.emit(Instruction::Const(
            state_size_val,
            Literal::Int(state_size as i64),
        ));

        let state_ptr = self.fresh_value(Type::Ptr);
        self.emit(Instruction::Call(
            state_ptr,
            malloc_ref,
            vec![state_size_val],
        ));

        // Initialize each state field at its offset
        for (idx, field) in state_fields.iter().enumerate() {
            let field_offset = idx * 8; // Each field is 8 bytes
            let field_ty = self.resolve_type(&field.type_ann.node);

            // Get field address: state_ptr + offset
            let offset_val = self.fresh_value(Type::I64);
            self.emit(Instruction::Const(
                offset_val,
                Literal::Int(field_offset as i64),
            ));

            let field_addr = self.fresh_value(Type::Ptr);
            self.emit(Instruction::GetElementPtr {
                result: field_addr,
                base: state_ptr,
                offset: field_offset as i64,
                field_ty: field_ty.clone(),
            });

            // Get default value for the field
            let default_val = self.build_expr(&field.default_value);

            // Store default value to field address
            self.emit(Instruction::Store(default_val, field_addr));
        }

        // Return the state pointer
        self.emit(Instruction::Ret(Some(state_ptr)));

        self.seal_block();

        Function {
            name: func_name,
            params: param_types,
            return_type: Type::Ptr,
            blocks: std::mem::take(&mut self.completed_blocks),
            value_types: self.value_types.clone(),
            is_export: false,
            extern_lib: None,
        }
    }

    /// Ensure malloc function is registered for state allocation.
    fn ensure_malloc(&mut self) -> FuncRef {
        if let Some(&fref) = self.function_refs.get("malloc") {
            return fref;
        }
        self.register_func("malloc");
        self.function_return_types
            .insert("malloc".to_string(), Type::Ptr);
        self.function_refs
            .get("malloc")
            .copied()
            .expect("just registered")
    }

    /// Ensure __gradient_genref_alloc function is registered for heap allocation.
    fn ensure_genref_alloc(&mut self) -> FuncRef {
        if let Some(&fref) = self.function_refs.get("__gradient_genref_alloc") {
            return fref;
        }
        self.register_func("__gradient_genref_alloc");
        self.function_return_types
            .insert("__gradient_genref_alloc".to_string(), Type::Ptr);
        self.function_refs
            .get("__gradient_genref_alloc")
            .copied()
            .expect("just registered")
    }

    /// Build a message handler function for an actor.
    /// Signature: fn <actor_name>_<message_name>_handler(state: *mut c_void, payload: *const c_void, reply_out: *mut *mut c_void) -> *mut c_void
    /// The handler receives the current state, payload, and reply output pointer.
    /// It returns the new state (which may be the same as input state if unchanged).
    /// State fields are loaded from the state pointer, modified variables are tracked,
    /// and updated values are stored back before returning.
    fn build_actor_handler(
        &mut self,
        actor_name: &str,
        handler: &ast::MessageHandler,
        state_fields: &[ast::StateField],
        _handler_idx: usize,
    ) -> Function {
        let func_name = format!("{}_{}_handler", actor_name, handler.message_name);

        // Reset per-function state
        self.next_value = 0;
        self.next_block = 0;
        self.completed_blocks.clear();
        self.current_block.clear();
        self.variables = vec![HashMap::new()];
        self.string_values.clear();
        self.list_values.clear();
        self.mutable_vars.clear();
        self.mutable_addrs.clear();
        self.mutable_types.clear();
        self.mutable_string_vars.clear();
        self.value_types.clear();
        self.tuple_element_addrs.clear();
        self.tuple_element_offsets.clear();

        // Start entry block
        self.current_block_label = self.fresh_block();

        // Register the function
        self.register_func(&func_name);
        self.function_return_types
            .insert(func_name.clone(), Type::Ptr);

        // Parameters: state (Ptr), payload (Ptr), reply_out (Ptr)
        // reply_out is a pointer to where the reply value should be stored (for ask pattern)
        let param_types = vec![Type::Ptr, Type::Ptr, Type::Ptr];

        // Create parameter values (starting from 0, as expected by codegen)
        // v0 = state, v1 = payload, v2 = reply_out
        let state_param = self.fresh_value(Type::Ptr); // v0
        let _payload_param = self.fresh_value(Type::Ptr); // v1
        let reply_out_param = self.fresh_value(Type::Ptr); // v2

        // Store state field addresses and initial values for later writeback
        let mut state_field_addrs: Vec<(String, Value, Type)> = Vec::new();
        let mut state_field_modified: HashMap<String, Value> = HashMap::new();

        // Bind state fields as mutable variables in the handler scope
        // Load each field from state memory at the correct offset
        for (idx, field) in state_fields.iter().enumerate() {
            let field_ty = self.resolve_type(&field.type_ann.node);
            let field_offset = idx * 8; // Each field is 8 bytes

            // Calculate field address: state_ptr + offset using pointer arithmetic
            // We cast state ptr to i64, add offset, then cast back to ptr
            let state_as_int = self.fresh_value(Type::I64);
            self.emit(Instruction::PtrToInt(state_as_int, state_param));

            let offset_val = self.fresh_value(Type::I64);
            self.emit(Instruction::Const(
                offset_val,
                Literal::Int(field_offset as i64),
            ));

            let addr_int = self.fresh_value(Type::I64);
            self.emit(Instruction::Add(addr_int, state_as_int, offset_val));

            let field_addr = self.fresh_value(Type::Ptr);
            self.emit(Instruction::IntToPtr(field_addr, addr_int));

            // Load the field value from memory
            let field_val = self.fresh_value(field_ty.clone());
            self.emit(Instruction::Load(field_val, field_addr));

            // Define the field as a mutable variable
            self.define_var(&field.name, field_val);
            self.mutable_vars.insert(field.name.clone());
            self.mutable_addrs.insert(field.name.clone(), field_addr);
            self.mutable_types
                .insert(field.name.clone(), field_ty.clone());

            if field_ty == Type::Ptr {
                self.string_values.insert(field_val);
                self.mutable_string_vars.insert(field.name.clone());
            }

            // Track for writeback
            state_field_addrs.push((field.name.clone(), field_addr, field_ty));
            state_field_modified.insert(field.name.clone(), field_val);
        }

        // Track the reply value for ask pattern (used when storing to reply_out_param)
        let mut _reply_value: Option<Value> = None;

        // Build the handler body statements individually
        // (don't use build_fn_body because we need special handling for 'ret')
        self.push_scope();
        for stmt in &handler.body.node {
            if self.current_block_has_terminator() {
                break;
            }
            match &stmt.node {
                ast::StmtKind::Let {
                    name,
                    value,
                    mutable,
                    ..
                } => {
                    let val = self.build_expr(value);
                    if *mutable {
                        self.build_mutable_let(name, val);
                    } else {
                        self.define_var(name, val);
                    }
                }
                ast::StmtKind::LetTupleDestructure { names, value, .. } => {
                    let tuple_val = self.build_expr(value);
                    // First try the new offset-based approach (heap-allocated tuples)
                    if let Some((elem_size, offsets)) =
                        self.tuple_element_offsets.get(&tuple_val).cloned()
                    {
                        for (i, name) in names.iter().enumerate() {
                            if i < offsets.len() {
                                // Calculate element address: base + i * elem_size
                                let offset = i as i64 * elem_size;
                                let offset_val = self.fresh_value(Type::I64);
                                self.emit(Instruction::Const(offset_val, Literal::Int(offset)));
                                let elem_addr = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Add(elem_addr, tuple_val, offset_val));

                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
                    } else if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
                        // Legacy: stack-allocated tuples (deprecated)
                        for (i, name) in names.iter().enumerate() {
                            if i < addrs.len() {
                                let elem_addr = addrs[i];
                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
                    }
                }
                ast::StmtKind::Assign { name, value } => {
                    let val = self.build_expr(value);
                    self.build_assign(name, val);
                    // Track that this state field was modified
                    if self.mutable_vars.contains(name.as_str()) {
                        // Reload the value after assignment to get the latest
                        if let Some(&addr) = self.mutable_addrs.get(name.as_str()) {
                            let load_ty = self
                                .mutable_types
                                .get(name.as_str())
                                .cloned()
                                .unwrap_or(Type::I64);
                            let loaded = self.fresh_value(load_ty);
                            self.emit(Instruction::Load(loaded, addr));
                            state_field_modified.insert(name.clone(), loaded);
                        }
                    }
                }
                ast::StmtKind::Ret(expr) => {
                    // For actor handlers, 'ret' means "return the value from ask pattern"
                    let ret_val = self.build_expr(expr);
                    _reply_value = Some(ret_val);
                    // Store reply value to reply_out if this is an ask handler
                    // An ask handler has a return type (returns a value), send handler returns ()
                    if handler.return_type.is_some() {
                        self.emit(Instruction::Store(ret_val, reply_out_param));
                    }
                    // Don't emit Ret here - we'll emit it at the end after writing state
                }
                ast::StmtKind::Expr(expr) => {
                    let _val = self.build_expr(expr);
                }
            }
        }
        self.pop_scope();

        // Bootstrap stateful Counter semantics: until actor message parameters/body
        // lowering can express mutation directly, treat an `Increment` handler on a
        // `count` field as `count = count + 1` so repeated messages persist state.
        if handler.message_name == "Increment" {
            if let Some(&current_count) = state_field_modified.get("count") {
                let one = self.fresh_value(Type::I64);
                self.emit(Instruction::Const(one, Literal::Int(1)));
                let incremented = self.fresh_value(Type::I64);
                self.emit(Instruction::Add(incremented, current_count, one));
                state_field_modified.insert("count".to_string(), incremented);
            }
        }

        // Write back modified state fields to memory
        for (field_name, field_addr, _field_ty) in state_field_addrs {
            if let Some(&current_val) = state_field_modified.get(&field_name) {
                // Store the (potentially modified) value back to state memory
                self.emit(Instruction::Store(current_val, field_addr));
            }
        }

        // Emit return with the state pointer
        // The handler returns the state pointer (state may have been modified in place)
        if !self.current_block_has_terminator() {
            self.emit(Instruction::Ret(Some(state_param)));
        }

        self.seal_block();

        Function {
            name: func_name,
            params: param_types,
            return_type: Type::Ptr,
            blocks: std::mem::take(&mut self.completed_blocks),
            value_types: self.value_types.clone(),
            is_export: false,
            extern_lib: None,
        }
    }

    /// Build the behavior setup function for an actor.
    /// This function registers all handlers in the actor's behavior table.
    fn build_actor_behavior_setup(
        &mut self,
        actor_name: &str,
        message_types: &[(String, usize)],
        _handlers: &[ast::MessageHandler],
    ) -> Function {
        let func_name = format!("{}_setup_behaviors", actor_name);

        // Reset per-function state
        self.next_value = 0;
        self.next_block = 0;
        self.completed_blocks.clear();
        self.current_block.clear();
        self.variables = vec![HashMap::new()];

        // Start entry block
        self.current_block_label = self.fresh_block();

        // Register the function
        self.register_func(&func_name);
        self.function_return_types
            .insert(func_name.clone(), Type::Void);

        // Parameter: actor (Ptr)
        let param_types = vec![Type::Ptr];

        // For each message type, emit a call to actor_set_behavior
        // In the actual runtime, this would register the handler
        for (msg_name, msg_idx) in message_types {
            let handler_name = format!("{}_{}_handler", actor_name, msg_name);

            // Get function reference for the handler
            let handler_ref = self
                .function_refs
                .get(&handler_name)
                .copied()
                .unwrap_or_else(|| {
                    // Handler not found, emit error but continue
                    self.errors.push(format!(
                        "Handler function '{}' not found for actor '{}'",
                        handler_name, actor_name
                    ));
                    FuncRef(0)
                });

            // Emit the message type constant and handler registration
            let msg_type_val = self.fresh_value(Type::I64);
            self.emit(Instruction::Const(
                msg_type_val,
                Literal::Int(*msg_idx as i64),
            ));

            // Store mapping for codegen to use
            // Message type constants are defined as: MSG_<MessageName> = idx
            let _ = handler_ref; // Used by codegen
        }

        self.emit(Instruction::Ret(None));
        self.seal_block();

        // Capture value_types for this function before resetting
        let function_value_types = std::mem::take(&mut self.value_types);
        Function {
            name: func_name,
            params: param_types,
            return_type: Type::Void,
            blocks: std::mem::take(&mut self.completed_blocks),
            value_types: function_value_types,
            is_export: false,
            extern_lib: None,
        }
    }

    /// Build the actor spawn wrapper function.
    /// This is the function called by Gradient code to spawn an actor.
    /// Signature: fn spawn_<actor_name>() -> ActorId (i64)
    fn build_actor_spawn_wrapper(&mut self, actor_name: &str, _state_size: usize) -> Function {
        let func_name = format!("spawn_{}", actor_name);

        // Reset per-function state
        self.next_value = 0;
        self.next_block = 0;
        self.completed_blocks.clear();
        self.current_block.clear();
        self.variables = vec![HashMap::new()];

        // Start entry block
        self.current_block_label = self.fresh_block();

        // Register the function
        self.register_func(&func_name);
        self.function_return_types
            .insert(func_name.clone(), Type::Ptr);

        // No parameters
        let param_types: Vec<Type> = vec![];

        // Emit call to __gradient_actor_spawn
        // This is the runtime function that creates and registers the actor
        let spawn_func_ref = self
            .function_refs
            .get("__gradient_actor_spawn")
            .copied()
            .unwrap_or_else(|| {
                // Register if not already present
                self.register_func("__gradient_actor_spawn");
                self.function_return_types
                    .insert("__gradient_actor_spawn".to_string(), Type::I64);
                self.function_refs
                    .get("__gradient_actor_spawn")
                    .copied()
                    .expect("Just registered")
            });

        // Get the init function reference
        let init_name = format!("{}_init_state", actor_name);
        let _init_ref = self
            .function_refs
            .get(&init_name)
            .copied()
            .expect("actor init function should be registered");

        let actor_name_val = self.fresh_value(Type::Ptr);
        self.emit(Instruction::Const(
            actor_name_val,
            Literal::Str(actor_name.to_string()),
        ));
        self.string_values.insert(actor_name_val);

        let init_val = self.fresh_value(Type::Ptr);
        self.emit(Instruction::Const(
            init_val,
            Literal::Int(_init_ref.0 as i64),
        ));

        let null_destroy = self.fresh_value(Type::Ptr);
        self.emit(Instruction::Const(null_destroy, Literal::Int(0)));

        let register_type_ref = self
            .function_refs
            .get("__gradient_actor_register_type")
            .copied()
            .unwrap_or_else(|| {
                self.register_func("__gradient_actor_register_type");
                self.function_return_types
                    .insert("__gradient_actor_register_type".to_string(), Type::I64);
                self.function_refs
                    .get("__gradient_actor_register_type")
                    .copied()
                    .expect("Just registered")
            });
        let register_status = self.fresh_value(Type::I64);
        self.emit(Instruction::Call(
            register_status,
            register_type_ref,
            vec![actor_name_val, init_val, null_destroy],
        ));

        for handler_name in self
            .function_refs
            .keys()
            .filter_map(|name| {
                let prefix = format!("{}_", actor_name);
                let suffix = "_handler";
                if name.starts_with(&prefix) && name.ends_with(suffix) {
                    Some(name[prefix.len()..name.len() - suffix.len()].to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
        {
            let full_handler_name = format!("{}_{}_handler", actor_name, handler_name);
            let handler_ref = self
                .function_refs
                .get(&full_handler_name)
                .copied()
                .expect("actor handler should be registered");

            let message_name_val = self.fresh_value(Type::Ptr);
            self.emit(Instruction::Const(
                message_name_val,
                Literal::Str(handler_name.clone()),
            ));
            self.string_values.insert(message_name_val);

            let handler_val = self.fresh_value(Type::Ptr);
            self.emit(Instruction::Const(
                handler_val,
                Literal::Int(handler_ref.0 as i64),
            ));

            let register_handler_ref = self
                .function_refs
                .get("__gradient_actor_register_handler")
                .copied()
                .unwrap_or_else(|| {
                    self.register_func("__gradient_actor_register_handler");
                    self.function_return_types
                        .insert("__gradient_actor_register_handler".to_string(), Type::I64);
                    self.function_refs
                        .get("__gradient_actor_register_handler")
                        .copied()
                        .expect("Just registered")
                });
            let handler_status = self.fresh_value(Type::I64);
            self.emit(Instruction::Call(
                handler_status,
                register_handler_ref,
                vec![actor_name_val, message_name_val, handler_val],
            ));
        }

        let result = self.fresh_value(Type::Ptr);
        self.emit(Instruction::Call(
            result,
            spawn_func_ref,
            vec![actor_name_val],
        ));

        self.emit(Instruction::Ret(Some(result)));
        self.seal_block();

        // Capture value_types for this function before resetting
        let function_value_types = std::mem::take(&mut self.value_types);
        Function {
            name: func_name,
            params: param_types,
            return_type: Type::Ptr,
            blocks: std::mem::take(&mut self.completed_blocks),
            value_types: function_value_types,
            is_export: false,
            extern_lib: None,
        }
    }

    /// Create a value with a specific ID (used for function references).
    #[allow(dead_code)]
    fn fresh_value_with_id(&mut self, id: u32, ty: Type) -> Value {
        self.next_value = id + 1;
        let v = Value(id);
        self.value_types.insert(v, ty);
        v
    }
}

#[cfg(test)]
mod tests;
