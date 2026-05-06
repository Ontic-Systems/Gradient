//! Stdlib pilot integration test for the `@verified` contract path
//! over the pair/triple-symmetric slice (sibling to `vc_stdlib_pilot.rs`,
//! `vc_stdlib_pilot_bool.rs`, `vc_stdlib_pilot_compare.rs`,
//! `vc_stdlib_pilot_int_ops.rs`, `vc_stdlib_pilot_arith_ops.rs`, and
//! `vc_stdlib_pilot_order_ops.rs`).
//!
//! Loads `compiler/stdlib/core_pair_ops.gr` — the seventh stdlib module
//! shipped under `@verified` — and runs every `@verified fn` declared
//! there through the [`ContractDischarger`]. Asserts every contract
//! obligation comes back `Discharged`, end-to-end:
//!
//!   parser → AST → checker → VC encoder → Z3 → `Discharged`
//!
//! Like the sibling pilot tests, this test skips cleanly when Z3 is
//! unavailable so CI lanes that don't install Z3 stay green. The
//! dedicated `verified` CI lane installs Z3 and pins
//! `GRADIENT_Z3_REQUIRED=1`, which makes a missing solver a hard
//! failure — so the discharge step is genuinely exercised on every
//! green build.
//!
//! Adding a new `@verified fn` to `compiler/stdlib/core_pair_ops.gr`
//! automatically extends this test's coverage; nothing else needs
//! editing.
//!
//! See ADR 0003 (`docs/adr/0003-tiered-contracts.md`) for the
//! end-to-end design rationale and `docs/agent-integration.md` for
//! the user-facing pilot demonstration.

use gradient_compiler::ast::item::{FnDef, ItemKind};
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker::vc::{ContractDischarger, DischargeOutcome};
use std::path::{Path, PathBuf};

/// Path to the pair/triple-symmetric stdlib pilot module.
/// Resolved relative to the `gradient-compiler` crate's
/// `CARGO_MANIFEST_DIR` so the test runs regardless of where the
/// workspace was invoked from.
fn pilot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("stdlib/core_pair_ops.gr")
}

/// Skip the test cleanly when Z3 isn't on PATH and the user hasn't
/// pinned `GRADIENT_Z3_REQUIRED=1`. Mirrors the pattern in
/// `tests/vc_z3_integration.rs` and the sibling stdlib pilot tests.
fn z3_or_skip() -> Option<ContractDischarger> {
    let d = ContractDischarger::default();
    if d.solver_available() {
        return Some(d);
    }
    if std::env::var_os("GRADIENT_Z3_REQUIRED").is_some() {
        panic!(
            "GRADIENT_Z3_REQUIRED is set but no Z3 binary was found on PATH or via GRADIENT_Z3_BIN"
        );
    }
    eprintln!(
        "[skip] Z3 not available; install z3 or set GRADIENT_Z3_BIN to run the pair-ops stdlib pilot"
    );
    None
}

/// Parse the pilot file and return every `@verified` function found
/// at the top level. The compiler's parser tolerates leading
/// comments/whitespace, so no preprocessing is needed.
fn load_verified_fns() -> Vec<FnDef> {
    let src = std::fs::read_to_string(pilot_path()).expect("read pair-ops stdlib pilot");
    let mut lexer = Lexer::new(&src, 0);
    let tokens = lexer.tokenize();
    let (module, errs) = parser::parse(tokens, 0);
    assert!(
        errs.is_empty(),
        "pair-ops stdlib pilot must parse cleanly: {errs:?}"
    );
    let mut out = Vec::new();
    for item in &module.items {
        if let ItemKind::FnDef(f) = &item.node {
            if f.is_verified {
                out.push(f.clone());
            }
        }
    }
    assert!(
        !out.is_empty(),
        "pair-ops stdlib pilot must declare at least one @verified fn"
    );
    out
}

// ── End-to-end pilot ────────────────────────────────────────────────────

#[test]
fn stdlib_pair_ops_pilot_every_verified_fn_discharges() {
    let Some(d) = z3_or_skip() else { return };
    let fns = load_verified_fns();

    let mut total_obligations = 0usize;
    for f in &fns {
        let report = d
            .discharge_function(f)
            .unwrap_or_else(|e| panic!("discharge `{}`: {e:?}", f.name));
        assert!(
            !report.outcomes.is_empty(),
            "`{}` declares @verified but produced no obligations; \
             every pilot fn must carry at least one @ensures",
            f.name
        );
        for q in &report.outcomes {
            match &q.outcome {
                DischargeOutcome::Discharged => total_obligations += 1,
                other => panic!(
                    "pair-ops stdlib pilot `{}` obligation #{:?} did not discharge: {other:?}",
                    f.name, q.contract_index
                ),
            }
        }
    }

    // Sanity floor: the file declares 10 fns with 17 obligations total
    // at the time of writing. If a future edit drops below this, treat
    // it as a regression of the pilot's coverage rather than a passing
    // test.
    assert!(
        total_obligations >= 17,
        "expected at least 17 discharged obligations across the pair-ops pilot, got {total_obligations}"
    );
}

#[test]
fn stdlib_pair_ops_pilot_named_landmark_fns_present() {
    // Pin the public surface of the pilot so a refactor that silently
    // renames or drops these functions trips this test before it
    // trips downstream documentation drift.
    let names: Vec<String> = load_verified_fns().into_iter().map(|f| f.name).collect();
    for required in [
        "min3_int",
        "mid3_sorted",
        "abs_diff",
        "pair_eq",
        "pair_neq",
        "triple_all_eq",
        "triple_any_neq",
        "max4_int",
        "add_pair_comm_witness",
        "ordered_diff",
    ] {
        assert!(
            names.iter().any(|n| n == required),
            "pair-ops stdlib pilot must declare `{required}`; found {names:?}"
        );
    }
}
