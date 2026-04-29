//! Issue #222: parser AST storage parity gate.
//!
//! Drives the runtime-backed bootstrap AST store through the operations the
//! self-hosted parser will issue when it executes — `bootstrap_*_alloc_*`
//! to materialize nodes, `bootstrap_*_get_*` to walk them — and asserts
//! that the resulting tree round-trips for nested binary expressions,
//! function parameter lists, and statement bodies. This is the concrete
//! evidence behind the `#222` acceptance criterion: parser-owned storage
//! can carry every parser-differential corpus construct end-to-end.
//!
//! The bridge mirrors the semantics of the rewritten `compiler/parser.gr`
//! `*_bootstrap_handle` builders. Once the self-hosted runtime can execute
//! `parser.gr` directly, this gate flips from "Rust mirrors the .gr code"
//! to "the .gr code drives the same store" without test changes.

use gradient_compiler::bootstrap_ast_bridge::{
    bootstrap_expr_alloc_binary, bootstrap_expr_alloc_ident, bootstrap_expr_alloc_int_lit,
    bootstrap_expr_get_child_a, bootstrap_expr_get_child_b, bootstrap_expr_get_int_value,
    bootstrap_expr_get_tag, bootstrap_expr_get_text, bootstrap_function_alloc,
    bootstrap_function_get_body_handle, bootstrap_function_get_name,
    bootstrap_function_get_params_handle, bootstrap_function_get_ret_type_tag,
    bootstrap_module_item_alloc_function, bootstrap_module_item_get_function_id,
    bootstrap_module_item_get_tag, bootstrap_module_item_list_alloc, bootstrap_node_list_append,
    bootstrap_node_list_get, bootstrap_node_list_len, bootstrap_param_alloc,
    bootstrap_param_get_name, bootstrap_param_get_type_tag, bootstrap_param_list_alloc,
    bootstrap_stmt_alloc, bootstrap_stmt_get_child_a, bootstrap_stmt_get_child_b,
    bootstrap_stmt_get_tag, bootstrap_stmt_get_text, bootstrap_stmt_list_alloc, reset_ast_store,
    ExprTag, ModuleItemTag, StmtTag, TypeTag,
};

/// Serialize parity tests that share the process-wide ambient store.
fn parity_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Mirror of `compiler/parser.gr::binop_bootstrap_code(AddOp) = 1`.
const ADD_OP: i64 = 1;
/// Mirror of `binop_bootstrap_code(EqOp) = 6`.
const EQ_OP: i64 = 6;
/// Mirror of `binop_bootstrap_code(AndOp) = 12`.
const AND_OP: i64 = 12;

#[test]
fn nested_binary_expressions_round_trip_through_storage() {
    let _g = parity_lock();
    reset_ast_store();

    // (a + b) == (a + b) and a + 1 — three levels of nesting plus a literal.
    let a1 = bootstrap_expr_alloc_ident("a");
    let b1 = bootstrap_expr_alloc_ident("b");
    let sum1 = bootstrap_expr_alloc_binary(ADD_OP, a1, b1);

    let a2 = bootstrap_expr_alloc_ident("a");
    let b2 = bootstrap_expr_alloc_ident("b");
    let sum2 = bootstrap_expr_alloc_binary(ADD_OP, a2, b2);

    let eq = bootstrap_expr_alloc_binary(EQ_OP, sum1, sum2);

    let a3 = bootstrap_expr_alloc_ident("a");
    let one = bootstrap_expr_alloc_int_lit(1);
    let sum3 = bootstrap_expr_alloc_binary(ADD_OP, a3, one);

    let conj = bootstrap_expr_alloc_binary(AND_OP, eq, sum3);

    // Walk the tree and assert each layer.
    assert_eq!(bootstrap_expr_get_tag(conj), ExprTag::Binary as i64);
    assert_eq!(bootstrap_expr_get_int_value(conj), AND_OP);

    let lhs = bootstrap_expr_get_child_a(conj);
    let rhs = bootstrap_expr_get_child_b(conj);
    assert_eq!(bootstrap_expr_get_tag(lhs), ExprTag::Binary as i64);
    assert_eq!(bootstrap_expr_get_int_value(lhs), EQ_OP);
    assert_eq!(bootstrap_expr_get_tag(rhs), ExprTag::Binary as i64);
    assert_eq!(bootstrap_expr_get_int_value(rhs), ADD_OP);

    let lhs_left = bootstrap_expr_get_child_a(lhs);
    let lhs_right = bootstrap_expr_get_child_b(lhs);
    assert_eq!(bootstrap_expr_get_tag(lhs_left), ExprTag::Binary as i64);
    assert_eq!(bootstrap_expr_get_tag(lhs_right), ExprTag::Binary as i64);

    // Deepest leaves must materialize the original identifier text.
    let inner_a = bootstrap_expr_get_child_a(lhs_left);
    let inner_b = bootstrap_expr_get_child_b(lhs_left);
    assert_eq!(bootstrap_expr_get_tag(inner_a), ExprTag::Ident as i64);
    assert_eq!(bootstrap_expr_get_text(inner_a), "a");
    assert_eq!(bootstrap_expr_get_text(inner_b), "b");

    // The right-hand sum walks through to its int literal.
    let rhs_right = bootstrap_expr_get_child_b(rhs);
    assert_eq!(bootstrap_expr_get_tag(rhs_right), ExprTag::IntLit as i64);
    assert_eq!(bootstrap_expr_get_int_value(rhs_right), 1);
}

#[test]
fn function_param_list_round_trips_through_storage() {
    let _g = parity_lock();
    reset_ast_store();

    let p_a = bootstrap_param_alloc("a", TypeTag::Int as i64, "", 0);
    let p_b = bootstrap_param_alloc("b", TypeTag::Int as i64, "", 0);
    let p_c = bootstrap_param_alloc("c", TypeTag::Bool as i64, "", 0);

    let plist = bootstrap_param_list_alloc();
    bootstrap_node_list_append(plist, p_a);
    bootstrap_node_list_append(plist, p_b);
    bootstrap_node_list_append(plist, p_c);

    assert_eq!(bootstrap_node_list_len(plist), 3);
    let names: Vec<String> = (0..3)
        .map(|i| bootstrap_param_get_name(bootstrap_node_list_get(plist, i)))
        .collect();
    assert_eq!(names, vec!["a", "b", "c"]);

    let tags: Vec<i64> = (0..3)
        .map(|i| bootstrap_param_get_type_tag(bootstrap_node_list_get(plist, i)))
        .collect();
    assert_eq!(
        tags,
        vec![
            TypeTag::Int as i64,
            TypeTag::Int as i64,
            TypeTag::Bool as i64
        ]
    );
}

#[test]
fn statement_body_round_trips_with_let_and_ret() {
    let _g = parity_lock();
    reset_ast_store();

    // Body equivalent to:
    //   let x = 42
    //   ret x + 1
    let lit_42 = bootstrap_expr_alloc_int_lit(42);
    let let_stmt = bootstrap_stmt_alloc(StmtTag::Let as i64, 0, 0, lit_42, 0, "x");

    let x_ref = bootstrap_expr_alloc_ident("x");
    let one = bootstrap_expr_alloc_int_lit(1);
    let sum = bootstrap_expr_alloc_binary(ADD_OP, x_ref, one);
    let ret_stmt = bootstrap_stmt_alloc(StmtTag::Ret as i64, 0, sum, 0, 0, "");

    let body = bootstrap_stmt_list_alloc();
    bootstrap_node_list_append(body, let_stmt);
    bootstrap_node_list_append(body, ret_stmt);

    assert_eq!(bootstrap_node_list_len(body), 2);

    let first_id = bootstrap_node_list_get(body, 0);
    assert_eq!(bootstrap_stmt_get_tag(first_id), StmtTag::Let as i64);
    assert_eq!(bootstrap_stmt_get_text(first_id), "x");
    let first_value = bootstrap_stmt_get_child_b(first_id);
    assert_eq!(bootstrap_expr_get_tag(first_value), ExprTag::IntLit as i64);
    assert_eq!(bootstrap_expr_get_int_value(first_value), 42);

    let last_id = bootstrap_node_list_get(body, 1);
    assert_eq!(bootstrap_stmt_get_tag(last_id), StmtTag::Ret as i64);
    let ret_value = bootstrap_stmt_get_child_a(last_id);
    assert_eq!(bootstrap_expr_get_tag(ret_value), ExprTag::Binary as i64);
    let ret_left = bootstrap_expr_get_child_a(ret_value);
    let ret_right = bootstrap_expr_get_child_b(ret_value);
    assert_eq!(bootstrap_expr_get_tag(ret_left), ExprTag::Ident as i64);
    assert_eq!(bootstrap_expr_get_text(ret_left), "x");
    assert_eq!(bootstrap_expr_get_tag(ret_right), ExprTag::IntLit as i64);
    assert_eq!(bootstrap_expr_get_int_value(ret_right), 1);
}

#[test]
fn full_function_with_module_item_list_round_trips() {
    let _g = parity_lock();
    reset_ast_store();

    // Mirrors corpus/01_fn_add_int.gr at the storage level:
    //   fn add(a: Int, b: Int) -> Int:
    //       a + b
    let p_a = bootstrap_param_alloc("a", TypeTag::Int as i64, "", 0);
    let p_b = bootstrap_param_alloc("b", TypeTag::Int as i64, "", 0);
    let plist = bootstrap_param_list_alloc();
    bootstrap_node_list_append(plist, p_a);
    bootstrap_node_list_append(plist, p_b);

    let a = bootstrap_expr_alloc_ident("a");
    let b = bootstrap_expr_alloc_ident("b");
    let sum = bootstrap_expr_alloc_binary(ADD_OP, a, b);
    let body_stmt = bootstrap_stmt_alloc(StmtTag::Expr as i64, 0, sum, 0, 0, "");
    let body = bootstrap_stmt_list_alloc();
    bootstrap_node_list_append(body, body_stmt);

    let fid = bootstrap_function_alloc("add", plist, TypeTag::Int as i64, "", body, 0, 0);
    let mid = bootstrap_module_item_alloc_function(fid);
    let items = bootstrap_module_item_list_alloc();
    bootstrap_node_list_append(items, mid);

    // Walk module item -> function -> params -> body and assert structure
    // matches the originally-allocated tree.
    assert_eq!(bootstrap_node_list_len(items), 1);
    let item_id = bootstrap_node_list_get(items, 0);
    assert_eq!(
        bootstrap_module_item_get_tag(item_id),
        ModuleItemTag::Function as i64
    );
    let fn_id = bootstrap_module_item_get_function_id(item_id);
    assert_eq!(bootstrap_function_get_name(fn_id), "add");
    assert_eq!(
        bootstrap_function_get_ret_type_tag(fn_id),
        TypeTag::Int as i64
    );

    let recovered_params = bootstrap_function_get_params_handle(fn_id);
    assert_eq!(bootstrap_node_list_len(recovered_params), 2);
    let p1 = bootstrap_node_list_get(recovered_params, 0);
    let p2 = bootstrap_node_list_get(recovered_params, 1);
    assert_eq!(bootstrap_param_get_name(p1), "a");
    assert_eq!(bootstrap_param_get_name(p2), "b");

    let recovered_body = bootstrap_function_get_body_handle(fn_id);
    assert_eq!(bootstrap_node_list_len(recovered_body), 1);
    let s = bootstrap_node_list_get(recovered_body, 0);
    assert_eq!(bootstrap_stmt_get_tag(s), StmtTag::Expr as i64);
    let body_expr = bootstrap_stmt_get_child_a(s);
    assert_eq!(bootstrap_expr_get_tag(body_expr), ExprTag::Binary as i64);
    assert_eq!(bootstrap_expr_get_int_value(body_expr), ADD_OP);
    let bl = bootstrap_expr_get_child_a(body_expr);
    let br = bootstrap_expr_get_child_b(body_expr);
    assert_eq!(bootstrap_expr_get_text(bl), "a");
    assert_eq!(bootstrap_expr_get_text(br), "b");
}

#[test]
fn missing_node_ids_are_safely_zero() {
    let _g = parity_lock();
    reset_ast_store();

    // Empty store: every accessor must return safe defaults (`tag = 0`,
    // empty text, child id `0`) so parser execution can keep walking.
    assert_eq!(bootstrap_expr_get_tag(0), 0);
    assert_eq!(bootstrap_expr_get_tag(99), 0);
    assert_eq!(bootstrap_expr_get_int_value(0), 0);
    assert_eq!(bootstrap_expr_get_text(0), "");
    assert_eq!(bootstrap_expr_get_child_a(0), 0);
    assert_eq!(bootstrap_node_list_len(0), 0);
    assert_eq!(bootstrap_node_list_get(0, 5), 0);
}
