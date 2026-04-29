//! Issue #225: checker AST lookup and environment storage parity gate.
//!
//! Drives the runtime-backed bootstrap_checker_env store through the
//! operations the self-hosted checker (`compiler/checker.gr`) issues
//! when it executes — `bootstrap_checker_env_alloc`,
//! `bootstrap_checker_env_insert_var`, `bootstrap_checker_env_insert_fn`,
//! `bootstrap_checker_env_lookup_*` plus accessor reads — and asserts
//! that the resulting frames let lookups succeed for inserted bindings,
//! fail (return 0) for missing names, walk the parent chain, and respect
//! shadowing.
//!
//! This is the concrete evidence behind the `#225` acceptance criteria:
//! variable lookup succeeds for function params and let-bound locals,
//! undefined variables produce a not-found result, and the env walks the
//! parent chain like the .gr code expects. Once the self-hosted runtime
//! can execute `checker.gr` directly, this gate flips from "Rust mirrors
//! the .gr code" to "the .gr code drives the same store" without test
//! changes.

use gradient_compiler::bootstrap_checker_env::{
    bootstrap_checker_env_alloc, bootstrap_checker_env_get_parent,
    bootstrap_checker_env_get_scope_level, bootstrap_checker_env_insert_fn,
    bootstrap_checker_env_insert_var, bootstrap_checker_env_lookup_fn,
    bootstrap_checker_env_lookup_var, bootstrap_checker_fn_get_is_extern,
    bootstrap_checker_fn_get_name, bootstrap_checker_fn_get_params_handle,
    bootstrap_checker_fn_get_ret_type_tag, bootstrap_checker_var_get_is_mut,
    bootstrap_checker_var_get_name, bootstrap_checker_var_get_scope_level,
    bootstrap_checker_var_get_type_name, bootstrap_checker_var_get_type_tag,
    reset_checker_env_store,
};

/// Mirror of `parser.gr::type_tag_int()` and `checker.gr::type_from_tag_name`.
const TY_INT: i64 = 1;
const TY_FLOAT: i64 = 2;
const TY_BOOL: i64 = 3;
const TY_STRING: i64 = 4;

/// Serialize parity tests that share the process-wide ambient store.
fn parity_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Acceptance: variable lookup succeeds for a let-bound local and
/// fails (returns 0) for undefined names. Mirrors what
/// `checker.gr::check_let_stmt` + `check_ident` will issue at runtime.
#[test]
fn let_bound_local_resolves_and_unknown_name_returns_not_found() {
    let _g = parity_lock();
    reset_checker_env_store();

    // Top-level frame for a function body.
    let root = bootstrap_checker_env_alloc(0, 0);
    // `let x: Int = 42` — record `x` in the env at scope level 1.
    let env_with_x = bootstrap_checker_env_insert_var(root, "x", TY_INT, "", 0, 1);

    // Lookup hits.
    let x_id = bootstrap_checker_env_lookup_var(env_with_x, "x");
    assert!(x_id > 0, "lookup_var(x) must resolve after insert_var");
    assert_eq!(bootstrap_checker_var_get_name(x_id), "x");
    assert_eq!(bootstrap_checker_var_get_type_tag(x_id), TY_INT);
    assert_eq!(bootstrap_checker_var_get_is_mut(x_id), 0);
    assert_eq!(bootstrap_checker_var_get_scope_level(x_id), 1);

    // Unknown variable produces a 0 record id, which checker.gr's
    // `lookup_var` translates to a not-found `VarInfo` whose name is
    // empty (allowing `check_ident` to emit "undefined variable").
    let missing = bootstrap_checker_env_lookup_var(env_with_x, "y");
    assert_eq!(missing, 0);
    assert_eq!(bootstrap_checker_var_get_name(missing), "");
}

/// Acceptance: function parameters resolve in the body scope. Shape
/// mirrors `check_fn` inserting each `Param` into a fresh frame before
/// walking the body.
#[test]
fn function_parameter_bindings_resolve_in_body_scope() {
    let _g = parity_lock();
    reset_checker_env_store();

    // Outer frame: just the function definition. Inner frame: the
    // function body, where each `Param` becomes a var binding.
    let module_env = bootstrap_checker_env_alloc(0, 0);
    let body_with_a = bootstrap_checker_env_insert_var(module_env, "a", TY_INT, "", 0, 1);
    let body_with_a_b = bootstrap_checker_env_insert_var(body_with_a, "b", TY_INT, "", 0, 1);

    // Both params resolve from the body env.
    let a_id = bootstrap_checker_env_lookup_var(body_with_a_b, "a");
    let b_id = bootstrap_checker_env_lookup_var(body_with_a_b, "b");
    assert!(a_id > 0 && b_id > 0);
    assert_eq!(bootstrap_checker_var_get_name(a_id), "a");
    assert_eq!(bootstrap_checker_var_get_name(b_id), "b");
    assert_eq!(bootstrap_checker_var_get_type_tag(a_id), TY_INT);
    assert_eq!(bootstrap_checker_var_get_type_tag(b_id), TY_INT);
}

/// Acceptance: an inner shadowing binding wins over an outer one and
/// the outer frame still resolves the original. This is the
/// `enter_scope` / `exit_scope` invariant that `check_block` and
/// `check_for_stmt` rely on.
#[test]
fn shadowing_at_inner_scope_does_not_leak() {
    let _g = parity_lock();
    reset_checker_env_store();

    let outer = bootstrap_checker_env_alloc(0, 0);
    let outer_with_x = bootstrap_checker_env_insert_var(outer, "x", TY_INT, "", 0, 0);

    // Enter a new scope — analogous to checker.gr's `enter_scope`,
    // which chains scope_level + 1 onto the current env.
    let inner = bootstrap_checker_env_alloc(outer_with_x, 1);
    let inner_with_x = bootstrap_checker_env_insert_var(inner, "x", TY_BOOL, "", 0, 1);

    let inner_x = bootstrap_checker_env_lookup_var(inner_with_x, "x");
    assert_eq!(bootstrap_checker_var_get_type_tag(inner_x), TY_BOOL);

    let outer_x = bootstrap_checker_env_lookup_var(outer_with_x, "x");
    assert_eq!(bootstrap_checker_var_get_type_tag(outer_x), TY_INT);
}

/// Acceptance: function names resolve through the env separately from
/// vars, which is the contract for `check_call` looking up a callee.
/// Inserted fns carry their full signature payload so the .gr code can
/// reconstruct `FnInfo` without losing data.
#[test]
fn fn_lookup_returns_full_signature_payload() {
    let _g = parity_lock();
    reset_checker_env_store();

    let root = bootstrap_checker_env_alloc(0, 0);
    // `fn add(a: Int, b: Int) -> Int` — params_handle is a stand-in
    // for the runtime bootstrap_param_list_alloc handle; ret type is Int.
    let env_with_add = bootstrap_checker_env_insert_fn(root, "add", 42, TY_INT, "", 0, 0);

    let fn_id = bootstrap_checker_env_lookup_fn(env_with_add, "add");
    assert!(fn_id > 0);
    assert_eq!(bootstrap_checker_fn_get_name(fn_id), "add");
    assert_eq!(bootstrap_checker_fn_get_params_handle(fn_id), 42);
    assert_eq!(bootstrap_checker_fn_get_ret_type_tag(fn_id), TY_INT);
    assert_eq!(bootstrap_checker_fn_get_is_extern(fn_id), 0);

    // Variable lookup with the same name returns 0 — fns and vars live
    // in separate slots in each frame.
    assert_eq!(bootstrap_checker_env_lookup_var(env_with_add, "add"), 0);
}

/// Acceptance: parent / scope_level walk works for chained frames.
/// `exit_scope` in checker.gr reads the parent id back through this
/// accessor, so the parity gate keeps that contract.
#[test]
fn parent_chain_round_trips_through_accessors() {
    let _g = parity_lock();
    reset_checker_env_store();

    let root = bootstrap_checker_env_alloc(0, 0);
    let mid = bootstrap_checker_env_alloc(root, 1);
    let leaf = bootstrap_checker_env_insert_var(mid, "k", TY_FLOAT, "", 1, 2);

    // The leaf's parent must be the inserted-on frame's predecessor —
    // that's the env we passed in (`mid`), not `root`. Per the bridge
    // contract, `insert_var` allocates a new frame parented at the
    // caller's env id.
    assert_eq!(bootstrap_checker_env_get_parent(leaf), mid);
    assert_eq!(bootstrap_checker_env_get_scope_level(leaf), 2);
    assert_eq!(bootstrap_checker_env_get_parent(mid), root);
    assert_eq!(bootstrap_checker_env_get_scope_level(mid), 1);
}

/// Acceptance: lookups walk through arbitrarily long chains. Mirrors
/// the multi-stmt `let a = ...; let b = ...; let c = ...; ret a + b + c`
/// shape `check_stmt_list` will produce.
#[test]
fn nested_let_statements_resolve_through_long_chain() {
    let _g = parity_lock();
    reset_checker_env_store();

    let mut env = bootstrap_checker_env_alloc(0, 0);
    let names = ["a", "b", "c", "d", "e"];
    for name in names.iter() {
        env = bootstrap_checker_env_insert_var(env, name, TY_INT, "", 0, 1);
    }
    for name in names.iter() {
        let id = bootstrap_checker_env_lookup_var(env, name);
        assert!(id > 0, "must resolve {name} after long chain insert");
        assert_eq!(bootstrap_checker_var_get_name(id), *name);
        assert_eq!(bootstrap_checker_var_get_type_tag(id), TY_INT);
    }
    // Lookup at the head still finds the deepest binding for "a" (no
    // shadowing happened, so the original binding wins).
    let a_id = bootstrap_checker_env_lookup_var(env, "a");
    assert_eq!(bootstrap_checker_var_get_name(a_id), "a");
}

/// Defensive guard: unknown ids and 0 sentinel return safe defaults so
/// the .gr dispatch never panics on malformed input. This is what lets
/// `check_ident` produce a clean "undefined variable" error rather than
/// crashing when the runtime hands it a stale id.
#[test]
fn safe_defaults_on_zero_and_unknown_ids() {
    let _g = parity_lock();
    reset_checker_env_store();

    // Sentinel root frame: lookups must short-circuit.
    assert_eq!(bootstrap_checker_env_lookup_var(0, "x"), 0);
    assert_eq!(bootstrap_checker_env_lookup_fn(0, "f"), 0);

    // Bogus large id paths.
    assert_eq!(bootstrap_checker_env_get_parent(99999), 0);
    assert_eq!(bootstrap_checker_env_get_scope_level(99999), 0);
    assert_eq!(bootstrap_checker_var_get_name(99999), "");
    assert_eq!(bootstrap_checker_var_get_type_name(99999), "");
    assert_eq!(bootstrap_checker_fn_get_name(99999), "");
}

/// Acceptance: `let` initializers can be String / Float / Bool valued.
/// The store carries the raw tag through so checker.gr's
/// `type_from_tag_name` can rebuild the original `Type`.
#[test]
fn let_bound_locals_carry_arbitrary_primitive_types() {
    let _g = parity_lock();
    reset_checker_env_store();

    let mut env = bootstrap_checker_env_alloc(0, 0);
    let bindings = [
        ("name", TY_STRING),
        ("ratio", TY_FLOAT),
        ("flag", TY_BOOL),
        ("count", TY_INT),
    ];
    for (name, tag) in bindings.iter() {
        env = bootstrap_checker_env_insert_var(env, name, *tag, "", 0, 1);
    }
    for (name, tag) in bindings.iter() {
        let id = bootstrap_checker_env_lookup_var(env, name);
        assert!(id > 0);
        assert_eq!(bootstrap_checker_var_get_type_tag(id), *tag);
    }
}
