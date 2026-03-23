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
    /// Maps enum variant names to their integer tag values.
    /// Populated during enum declaration processing.
    enum_variant_tags: HashMap<String, i64>,
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
    /// Counter for generating unique closure function names.
    closure_counter: u32,
    /// Closure functions generated during expression building.
    /// These are accumulated and appended to the module's function list.
    closure_functions: Vec<Function>,
    /// Maps a tuple base value (address of first element) to the addresses
    /// of all its elements. Used by TupleField access to load the right slot.
    tuple_element_addrs: HashMap<Value, Vec<Value>>,
    /// Set of SSA values known to be list-typed (Ptr to list data).
    /// Used to track which values are lists for list builtin operations.
    list_values: HashSet<Value>,
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
            enum_variant_tags: HashMap::new(),
            closure_counter: 0,
            closure_functions: Vec::new(),
            tuple_element_addrs: HashMap::new(),
            list_values: HashSet::new(),
        }
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
        let mut builder = IrBuilder::new();

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
                ast::ItemKind::LetTupleDestructure { names, value, .. } => {
                    let tuple_val = builder.build_expr(value);
                    if let Some(addrs) = builder.tuple_element_addrs.get(&tuple_val).cloned() {
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
                ast::ItemKind::EnumDecl { variants, .. } => {
                    // Register variant tags for codegen.
                    for (i, variant) in variants.iter().enumerate() {
                        builder.enum_variant_tags.insert(variant.name.clone(), i as i64);
                    }
                }
                ast::ItemKind::CapDecl { .. } => {
                    // Capability declarations are compile-time only.
                }
                ast::ItemKind::ActorDecl { .. } => {
                    // Actor declarations are not yet lowered to IR.
                }
                ast::ItemKind::TraitDecl { .. } => {
                    // Trait declarations are compile-time only (no runtime representation).
                }
                ast::ItemKind::ImplBlock { target_type, methods, .. } => {
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
            }
        }

        // Build IR function stubs for imported module functions (empty blocks,
        // like extern functions), so the codegen knows their signatures.
        let defined_fn_names: HashSet<String> = functions.iter().map(|f| f.name.clone()).collect();
        for (_mod_name, imported_ast) in imported_modules {
            for item in &imported_ast.items {
                match &item.node {
                    ast::ItemKind::FnDef(fn_def) => {
                        if !defined_fn_names.contains(&fn_def.name) {
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
                    }
                    ast::ItemKind::ExternFn(decl) => {
                        if !defined_fn_names.contains(&decl.name) {
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
        self.register_func("__gradient_contract_fail");
        self.function_return_types.insert("__gradient_contract_fail".to_string(), Type::Void);

        // ── String operations ────────────────────────────────────────────
        self.register_func("string_length");
        self.function_return_types.insert("string_length".to_string(), Type::I64);
        self.register_func("string_contains");
        self.function_return_types.insert("string_contains".to_string(), Type::Bool);
        self.register_func("string_starts_with");
        self.function_return_types.insert("string_starts_with".to_string(), Type::Bool);
        self.register_func("string_ends_with");
        self.function_return_types.insert("string_ends_with".to_string(), Type::Bool);
        self.register_func("string_substring");
        self.function_return_types.insert("string_substring".to_string(), Type::Ptr);
        self.register_func("string_trim");
        self.function_return_types.insert("string_trim".to_string(), Type::Ptr);
        self.register_func("string_to_upper");
        self.function_return_types.insert("string_to_upper".to_string(), Type::Ptr);
        self.register_func("string_to_lower");
        self.function_return_types.insert("string_to_lower".to_string(), Type::Ptr);
        self.register_func("string_replace");
        self.function_return_types.insert("string_replace".to_string(), Type::Ptr);
        self.register_func("string_index_of");
        self.function_return_types.insert("string_index_of".to_string(), Type::I64);
        self.register_func("string_char_at");
        self.function_return_types.insert("string_char_at".to_string(), Type::Ptr);
        self.register_func("string_split");
        self.function_return_types.insert("string_split".to_string(), Type::Ptr);

        // ── Numeric operations ───────────────────────────────────────────
        self.register_func("float_to_int");
        self.function_return_types.insert("float_to_int".to_string(), Type::I64);
        self.register_func("int_to_float");
        self.function_return_types.insert("int_to_float".to_string(), Type::F64);
        self.register_func("pow");
        self.function_return_types.insert("pow".to_string(), Type::I64);
        self.register_func("float_abs");
        self.function_return_types.insert("float_abs".to_string(), Type::F64);
        self.register_func("float_sqrt");
        self.function_return_types.insert("float_sqrt".to_string(), Type::F64);
        self.register_func("float_to_string");
        self.function_return_types.insert("float_to_string".to_string(), Type::Ptr);
        self.register_func("bool_to_string");
        self.function_return_types.insert("bool_to_string".to_string(), Type::Ptr);

        // ── List operations ─────────────────────────────────────────────
        self.register_func("list_length");
        self.function_return_types.insert("list_length".to_string(), Type::I64);
        self.register_func("list_get");
        self.function_return_types.insert("list_get".to_string(), Type::I64);
        self.register_func("list_push");
        self.function_return_types.insert("list_push".to_string(), Type::Ptr);
        self.register_func("list_concat");
        self.function_return_types.insert("list_concat".to_string(), Type::Ptr);
        self.register_func("list_is_empty");
        self.function_return_types.insert("list_is_empty".to_string(), Type::Bool);
        self.register_func("list_head");
        self.function_return_types.insert("list_head".to_string(), Type::I64);
        self.register_func("list_tail");
        self.function_return_types.insert("list_tail".to_string(), Type::Ptr);
        self.register_func("list_contains");
        self.function_return_types.insert("list_contains".to_string(), Type::Bool);

        // ── Higher-order list operations ───────────────────────────────
        self.register_func("list_map");
        self.function_return_types.insert("list_map".to_string(), Type::Ptr);
        self.register_func("list_filter");
        self.function_return_types.insert("list_filter".to_string(), Type::Ptr);
        self.register_func("list_foreach");
        self.function_return_types.insert("list_foreach".to_string(), Type::Void);
        self.register_func("list_fold");
        self.function_return_types.insert("list_fold".to_string(), Type::I64);
        self.register_func("list_any");
        self.function_return_types.insert("list_any".to_string(), Type::Bool);
        self.register_func("list_all");
        self.function_return_types.insert("list_all".to_string(), Type::Bool);
        self.register_func("list_find");
        self.function_return_types.insert("list_find".to_string(), Type::I64);
        self.register_func("list_sort");
        self.function_return_types.insert("list_sort".to_string(), Type::Ptr);
        self.register_func("list_reverse");
        self.function_return_types.insert("list_reverse".to_string(), Type::Ptr);

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
                ast::ItemKind::EnumDecl { variants, .. } => {
                    // Pre-register enum variant tags so they're available
                    // during function building.
                    for (i, variant) in variants.iter().enumerate() {
                        self.enum_variant_tags.insert(variant.name.clone(), i as i64);
                    }
                }
                ast::ItemKind::ImplBlock { target_type, methods, .. } => {
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

        // Use the IR value type to determine the source type for trait methods.
        let ir_type = self.value_types.get(&obj_val).cloned().unwrap_or(Type::I64);
        let type_name = match ir_type {
            Type::I64 | Type::I32 => "Int",
            Type::F64 => "Float",
            Type::Bool => "Bool",
            Type::Ptr => {
                // Ptr could be String or List. If we haven't matched above,
                // try both prefixes.
                let string_candidate = format!("string_{}", method);
                if self.function_refs.contains_key(&string_candidate) {
                    return string_candidate;
                }
                let list_candidate = format!("list_{}", method);
                if self.function_refs.contains_key(&list_candidate) {
                    return list_candidate;
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

        // Fallback: try all registered type prefixes.
        for tn in &["Int", "Float", "String", "Bool", "Unit", "List"] {
            let candidate = format!("{}::{}", tn, method);
            if self.function_refs.contains_key(&candidate) {
                return candidate;
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

        // Emit @requires precondition checks at function entry.
        for contract in &fn_def.contracts {
            if contract.kind == ast::ContractKind::Requires {
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
            .filter(|c| c.kind == ast::ContractKind::Ensures)
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
                ast::StmtKind::Let { name, value, mutable, .. } => {
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
                    if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
                        for (i, name) in names.iter().enumerate() {
                            if i < addrs.len() {
                                let elem_addr = addrs[i];
                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
                    } else {
                        for name in names {
                            let v = self.fresh_value(Type::I64);
                            self.emit(Instruction::Const(v, Literal::Int(0)));
                            self.define_var(name, v);
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
            ast::StmtKind::Let { name, value, mutable, .. } => {
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
                if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
                    for (i, name) in names.iter().enumerate() {
                        if i < addrs.len() {
                            let elem_addr = addrs[i];
                            let result = self.fresh_value(Type::I64);
                            self.emit(Instruction::Load(result, elem_addr));
                            self.define_var(name, result);
                        }
                    }
                } else {
                    self.errors.push(
                        "tuple destructuring on a non-tuple value in IR builder".to_string(),
                    );
                    for name in names {
                        let v = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(v, Literal::Int(0)));
                        self.define_var(name, v);
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

    // ── Contract checking ────────────────────────────────────────────

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
        self.emit(Instruction::Const(msg_val, Literal::Str(message.to_string())));
        self.string_values.insert(msg_val);

        let func_ref = self.function_refs.get("__gradient_contract_fail").copied()
            .expect("__gradient_contract_fail should be pre-registered");
        let call_result = self.fresh_value(Type::Void);
        self.emit(Instruction::Call(call_result, func_ref, vec![msg_val]));
        // After the contract failure call, we abort (but emit a Ret for well-formedness).
        self.emit(Instruction::Ret(None));
        self.seal_block();

        // ok_block: continue normal execution.
        self.current_block_label = ok_block;
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
            ast::ExprKind::StringInterp { parts } => {
                self.build_string_interp(parts)
            }
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
                    if self.lookup_var(name).is_none() && !self.mutable_vars.contains(name.as_str()) {
                        let v = self.fresh_value(Type::I64);
                        self.emit(Instruction::Const(v, Literal::Int(tag)));
                        return v;
                    }
                }
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
            ast::ExprKind::Match { scrutinee, arms } => {
                self.build_match(scrutinee, arms)
            }
            ast::ExprKind::Paren(inner) => {
                // Parentheses are purely syntactic — pass through.
                self.build_expr(inner)
            }
            ast::ExprKind::Closure { params, return_type, body } => {
                self.build_closure(params, return_type.as_ref(), body)
            }
            ast::ExprKind::ListLit(elements) => {
                // Lists are represented as heap-allocated: [length: i64, capacity: i64, data...]
                // We emit a call to a synthetic "list_literal_N" function that the codegen
                // layer will handle inline.
                let n = elements.len();
                let elem_vals: Vec<Value> = elements.iter().map(|e| self.build_expr(e)).collect();
                let func_name = format!("list_literal_{}", n);
                self.register_func(&func_name);
                self.function_return_types.insert(func_name.clone(), Type::Ptr);
                let func_ref = self.function_refs.get(&func_name).copied()
                    .expect("list_literal_N should be registered");
                let result = self.fresh_value(Type::Ptr);
                self.emit(Instruction::Call(result, func_ref, elem_vals));
                self.list_values.insert(result);
                result
            }
            ast::ExprKind::Tuple(elems) => {
                let mut elem_addrs = Vec::new();
                for elem_expr in elems.iter() {
                    let elem_val = self.build_expr(elem_expr);
                    let addr = self.fresh_value(Type::Ptr);
                    self.emit(Instruction::Alloca(addr, Type::I64));
                    self.emit(Instruction::Store(elem_val, addr));
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
            ast::ExprKind::TupleField { tuple, index } => {
                let tuple_val = self.build_expr(tuple);
                if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
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
            ast::ExprKind::Spawn { .. }
            | ast::ExprKind::Send { .. }
            | ast::ExprKind::Ask { .. } => {
                // Actor expressions are not yet lowered to IR.
                let v = self.fresh_value(Type::Void);
                self.emit(Instruction::Const(v, Literal::Int(0)));
                v
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
                        if matches!(name.as_str(),
                            "int_to_string" | "string_concat"
                            | "string_substring" | "string_trim"
                            | "string_to_upper" | "string_to_lower"
                            | "string_replace" | "string_char_at"
                            | "string_split" | "float_to_string"
                            | "bool_to_string"
                        ) {
                            self.string_values.insert(result);
                        }
                        // Track list-returning builtins.
                        if matches!(name.as_str(),
                            "list_push" | "list_concat" | "list_tail"
                            | "list_map" | "list_filter" | "list_sort" | "list_reverse"
                        ) || name.starts_with("list_literal_") {
                            self.list_values.insert(result);
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
                        let ret_ty = self.function_return_types
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
                        let ret_ty = self.function_return_types
                            .get(&resolved_name)
                            .cloned()
                            .unwrap_or(Type::I64);
                        let result = self.fresh_value(ret_ty);
                        self.emit(Instruction::Call(result, func_ref, full_args));
                        // Track string-returning builtins.
                        if matches!(resolved_name.as_str(),
                            "string_substring" | "string_trim"
                            | "string_to_upper" | "string_to_lower"
                            | "string_replace" | "string_char_at"
                            | "string_split"
                        ) {
                            self.string_values.insert(result);
                        }
                        // Track list-returning builtins.
                        if matches!(resolved_name.as_str(),
                            "list_push" | "list_concat" | "list_tail"
                        ) {
                            self.list_values.insert(result);
                        }
                        result
                    }
                    None => {
                        self.errors.push(format!(
                            "call to undefined method: '{}'",
                            field
                        ));
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
                                let func_ref = self.function_refs.get("int_to_string").copied().unwrap();
                                let result = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Call(result, func_ref, vec![val]));
                                self.string_values.insert(result);
                                string_vals.push(result);
                            }
                            Type::F64 => {
                                // Call float_to_string.
                                let func_ref = self.function_refs.get("float_to_string").copied().unwrap();
                                let result = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Call(result, func_ref, vec![val]));
                                self.string_values.insert(result);
                                string_vals.push(result);
                            }
                            Type::Bool => {
                                // Call bool_to_string.
                                let func_ref = self.function_refs.get("bool_to_string").copied().unwrap();
                                let result = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Call(result, func_ref, vec![val]));
                                self.string_values.insert(result);
                                string_vals.push(result);
                            }
                            _ => {
                                self.errors.push(format!(
                                    "cannot convert type {:?} to string in interpolation",
                                    val_ty
                                ));
                                let v = self.fresh_value(Type::Ptr);
                                self.emit(Instruction::Const(v, Literal::Str("<error>".to_string())));
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
        let mut acc = string_vals[0];
        let concat_ref = self.function_refs.get("string_concat").copied().unwrap();
        for val in &string_vals[1..] {
            let result = self.fresh_value(Type::Ptr);
            self.emit(Instruction::Call(result, concat_ref, vec![acc, *val]));
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
                ast::StmtKind::LetTupleDestructure { names, value, .. } => {
                    let tuple_val = self.build_expr(value);
                    if let Some(addrs) = self.tuple_element_addrs.get(&tuple_val).cloned() {
                        for (i, name) in names.iter().enumerate() {
                            if i < addrs.len() {
                                let elem_addr = addrs[i];
                                let result = self.fresh_value(Type::I64);
                                self.emit(Instruction::Load(result, elem_addr));
                                self.define_var(name, result);
                            }
                        }
                    } else {
                        for name in names {
                            let v = self.fresh_value(Type::I64);
                            self.emit(Instruction::Const(v, Literal::Int(0)));
                            self.define_var(name, v);
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
    fn build_match(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
    ) -> Value {
        let scrutinee_val = self.build_expr(scrutinee);
        let scrutinee_ty = self.value_types.get(&scrutinee_val).cloned().unwrap_or(Type::I64);

        let merge_block = self.fresh_block();
        let mut phi_entries: Vec<(BlockRef, Value)> = Vec::new();

        // Separate wildcard arm from pattern arms.
        let mut wildcard_arm: Option<&ast::MatchArm> = None;
        let mut pattern_arms: Vec<&ast::MatchArm> = Vec::new();
        for arm in arms {
            match &arm.pattern {
                ast::Pattern::Wildcard => {
                    wildcard_arm = Some(arm);
                }
                _ => {
                    pattern_arms.push(arm);
                }
            }
        }

        for arm in pattern_arms.iter() {
            // Build the pattern comparison value.
            let pattern_val = match &arm.pattern {
                ast::Pattern::IntLit(n) => {
                    let v = self.fresh_value(Type::I64);
                    self.emit(Instruction::Const(v, Literal::Int(*n)));
                    v
                }
                ast::Pattern::BoolLit(b) => {
                    let v = self.fresh_value(Type::Bool);
                    self.emit(Instruction::Const(v, Literal::Bool(*b)));
                    v
                }
                ast::Pattern::Variant { variant, .. } => {
                    let tag = self.enum_variant_tags.get(variant.as_str()).copied().unwrap_or(0);
                    let v = self.fresh_value(Type::I64);
                    self.emit(Instruction::Const(v, Literal::Int(tag)));
                    v
                }
                ast::Pattern::Wildcard => unreachable!("wildcard handled separately"),
                ast::Pattern::Tuple(_) => {
                    // Tuple patterns in match are not supported; produce a dummy value.
                    let v = self.fresh_value(Type::I64);
                    self.emit(Instruction::Const(v, Literal::Int(0)));
                    v
                }
            };

            // Compare scrutinee to pattern value.
            let cmp_val = self.fresh_value(Type::Bool);
            self.emit(Instruction::Cmp(cmp_val, CmpOp::Eq, scrutinee_val, pattern_val));

            let arm_block = self.fresh_block();
            // Always use a fresh block for the "else" branch, even for the
            // last arm. The merge block needs to be a separate target so
            // that block parameters (from phi nodes) are handled correctly.
            let next_block = self.fresh_block();

            self.emit(Instruction::Branch(cmp_val, arm_block, next_block));
            self.seal_block();

            // Arm block: build the arm body.
            self.current_block_label = arm_block;
            let arm_val = self.build_block_expr(&arm.body);
            let arm_exit_block = self.current_block_label;
            if !self.current_block_has_terminator() {
                self.emit(Instruction::Jump(merge_block));
            }
            phi_entries.push((arm_exit_block, arm_val));
            self.seal_block();

            // Move to the next check block.
            self.current_block_label = next_block;
        }

        // Wildcard / default arm.
        if let Some(wc_arm) = wildcard_arm {
            let wc_val = self.build_block_expr(&wc_arm.body);
            let wc_exit_block = self.current_block_label;
            if !self.current_block_has_terminator() {
                self.emit(Instruction::Jump(merge_block));
            }
            phi_entries.push((wc_exit_block, wc_val));
            self.seal_block();
        } else if self.current_block_label != merge_block {
            // No wildcard arm and we're in a fallthrough block.
            // Emit a unit value and jump to merge.
            let unit_val = self.fresh_value(scrutinee_ty.clone());
            self.emit(Instruction::Const(unit_val, Literal::Int(0)));
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
        self.function_return_types.insert(closure_name.clone(), ret_type);

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
        self.value_types = saved_value_types;

        // In the parent function, return the closure as a function pointer
        // (represented as an i64 constant with a symbolic reference to the
        // closure function). For now, we emit a const 0 placeholder -- the
        // codegen layer will resolve the function address at link time.
        let func_ref = self.function_refs.get(&closure_name).copied()
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
            ast::TypeExpr::Named(name) => self.resolve_named_type(name),
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
}

#[cfg(test)]
mod tests;
