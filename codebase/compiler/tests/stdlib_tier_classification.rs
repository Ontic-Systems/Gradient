//! Integration tests for stdlib tier classification (ADR 0005, issue #345).
//!
//! These tests pin **representative builtins** registered in
//! `typechecker::env::TypeEnv::new()` to their expected `core` / `alloc` /
//! `std` tier. They are the scaffold's canary: if a future PR changes a
//! builtin's effect row, the tier shifts here first, surfacing the change
//! before it reaches the `no_std` test matrix (#347) or the
//! `import std in no_std` rejection (#348).
//!
//! Pattern: classify-the-effect-row, not the implementation. The ADR 0005
//! rule (smallest-tier-covering-the-closure) is the single source of truth,
//! so the tests assert tier identity rather than effect-row identity. That
//! way an internal effect-row change like wave 6 of #346 can land without
//! editing this file unless it moves the builtin across a tier boundary.
//!
//! See:
//! - `docs/adr/0005-stdlib-split.md` — locked decision.
//! - `docs/stdlib-migration.md` — migration guide (per #345 acceptance).
//! - `codebase/compiler/src/typechecker/stdlib_tier.rs` — classifier impl.

use gradient_compiler::typechecker::env::TypeEnv;
use gradient_compiler::typechecker::stdlib_tier::StdlibTier;

// ── Core tier (zero Heap, zero IO/FS/Net/Time/Mut) ─────────────────────

#[test]
fn pure_arithmetic_is_core() {
    let env = TypeEnv::new();
    // Decomposers / pure helpers stay at Core.
    assert_eq!(env.lookup_fn_tier("abs"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("min"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("max"), Some(StdlibTier::Core));
}

#[test]
fn pure_math_is_core() {
    let env = TypeEnv::new();
    assert_eq!(env.lookup_fn_tier("sin"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("cos"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("pow"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("pi"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("e"), Some(StdlibTier::Core));
}

#[test]
fn option_decomposers_are_core() {
    let env = TypeEnv::new();
    // Non-allocating Option/Result accessors per the audit boundary
    // recorded in handoff §5 post-#527.
    assert_eq!(env.lookup_fn_tier("option_is_some"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("is_ok"), Some(StdlibTier::Core));
}

#[test]
fn pure_accessors_are_core() {
    let env = TypeEnv::new();
    // Per #527 post-audit: pure accessors stay at Core.
    assert_eq!(env.lookup_fn_tier("hashmap_len"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("set_size"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("iter_has_next"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("iter_count"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("string_compare"), Some(StdlibTier::Core));
    assert_eq!(env.lookup_fn_tier("datetime_year"), Some(StdlibTier::Core));
}

// ── Alloc tier (Heap, no IO/FS/Net/Time/Mut) ───────────────────────────

#[test]
fn heap_allocators_are_alloc() {
    let env = TypeEnv::new();
    // Builtins annotated `!{Heap}` across waves 1-5 of #346.
    assert_eq!(env.lookup_fn_tier("string_to_int"), Some(StdlibTier::Alloc));
    assert_eq!(
        env.lookup_fn_tier("string_to_float"),
        Some(StdlibTier::Alloc)
    );
    assert_eq!(env.lookup_fn_tier("string_find"), Some(StdlibTier::Alloc));
    assert_eq!(env.lookup_fn_tier("range_iter"), Some(StdlibTier::Alloc));
}

#[test]
fn to_string_convenience_is_alloc() {
    let env = TypeEnv::new();
    // #526 wave 4 closed the convenience-alias gap; UFCS resolves
    // `Int.to_string()` to the convenience builtin which is Heap.
    assert_eq!(env.lookup_fn_tier("int_to_string"), Some(StdlibTier::Alloc));
}

// ── Spot-checks: classifier is a TOTAL function for known builtins ─────

#[test]
fn lookup_fn_tier_returns_none_for_unknown_names() {
    let env = TypeEnv::new();
    assert_eq!(env.lookup_fn_tier("__not_a_real_builtin__"), None);
    assert_eq!(env.lookup_fn_tier(""), None);
}

#[test]
fn every_registered_builtin_has_a_classifiable_tier() {
    // Smoke test: classification never panics across the full env. We
    // sample a representative subset of the public builtin surface and
    // assert each yields some tier.
    let env = TypeEnv::new();
    let probes = [
        "abs",
        "min",
        "max",
        "sin",
        "cos",
        "pow",
        "pi",
        "e",
        "option_is_some",
        "is_ok",
        "hashmap_len",
        "set_size",
        "iter_has_next",
        "iter_count",
        "string_compare",
        "datetime_year",
        "string_to_int",
        "string_to_float",
        "string_find",
        "range_iter",
        "int_to_string",
    ];
    for name in probes {
        let tier = env.lookup_fn_tier(name);
        assert!(
            tier.is_some(),
            "expected builtin {name} to be registered with a classifiable tier",
        );
    }
}

// ── Tier-distribution invariant ───────────────────────────────────────

/// At the scaffold stage (post-#345, pre-#347), the registered builtin
/// surface partitions into all three tiers. This sanity-check makes the
/// scaffold visible: if a future refactor accidentally collapses every
/// builtin into a single tier (e.g. by overriding effects in a global
/// pass), this test fires early.
#[test]
fn registered_builtins_span_all_three_tiers() {
    let env = TypeEnv::new();
    let core = env.lookup_fn_tier("abs");
    let alloc = env.lookup_fn_tier("string_to_int");
    // Std-tier examples are sparse in the current builtin surface — the
    // existing IO/FS/Net builtins haven't been individually pinned in
    // this test; we only assert that AT LEAST ONE of the canonical Std
    // probes lands at Std. If none does, that's a real signal that the
    // E7 #346 audit missed a category.
    let std_probes = ["print", "file_read", "file_write", "time_now"];
    let any_std = std_probes
        .iter()
        .any(|n| env.lookup_fn_tier(n) == Some(StdlibTier::Std));

    assert_eq!(core, Some(StdlibTier::Core));
    assert_eq!(alloc, Some(StdlibTier::Alloc));
    assert!(
        any_std,
        "expected at least one of {std_probes:?} to classify as Std",
    );
}
