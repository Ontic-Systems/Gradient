//! Stdlib pilot integration test for the `@verified` contract path
//! over the successor/predecessor slice (sibling to the existing pilot tests).
//!
//! Loads `compiler/stdlib/core_successor_ops.gr` — the eighteenth stdlib
//! module shipped under `@verified` — and runs every `@verified fn`
//! declared there through the [`ContractDischarger`]. Asserts every
//! contract obligation comes back `Discharged`, end-to-end:
//!
//!   parser → AST → checker → VC encoder → Z3 → `Discharged`
//!
//! Skips cleanly when Z3 is unavailable. The dedicated `verified` CI
//! lane installs Z3 and pins `GRADIENT_Z3_REQUIRED=1`, making the
//! solver mandatory there.
//!
//! See ADR 0003 (`docs/adr/0003-tiered-contracts.md`) for design.

use gradient_compiler::ast::item::{FnDef, ItemKind};
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker::vc::{ContractDischarger, DischargeOutcome};
use std::path::{Path, PathBuf};

fn pilot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("stdlib/core_successor_ops.gr")
}

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
        "[skip] Z3 not available; install z3 or set GRADIENT_Z3_BIN to run the successor-ops stdlib pilot"
    );
    None
}

fn load_verified_fns() -> Vec<FnDef> {
    let src = std::fs::read_to_string(pilot_path()).expect("read successor-ops stdlib pilot");
    let mut lexer = Lexer::new(&src, 0);
    let tokens = lexer.tokenize();
    let (module, errs) = parser::parse(tokens, 0);
    assert!(
        errs.is_empty(),
        "successor-ops stdlib pilot must parse cleanly: {errs:?}"
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
        "successor-ops stdlib pilot must declare at least one @verified fn"
    );
    out
}

#[test]
fn stdlib_successor_ops_pilot_every_verified_fn_discharges() {
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
                    "successor-ops stdlib pilot `{}` obligation #{:?} did not discharge: {other:?}",
                    f.name, q.contract_index
                ),
            }
        }
    }

    // Sanity floor: 12 fns × at least 1 obligation = 12 (one fn carries
    // a dual postcondition so the actual count is 13). Drop below = regression.
    assert!(
        total_obligations >= 12,
        "expected at least 12 discharged obligations across the successor-ops pilot, got {total_obligations}"
    );
}

#[test]
fn stdlib_successor_ops_pilot_named_landmark_fns_present() {
    let names: Vec<String> = load_verified_fns().into_iter().map(|f| f.name).collect();
    for required in [
        "succ",
        "pred",
        "succ_then_pred",
        "pred_then_succ",
        "succ_succ",
        "pred_pred",
        "succ_diff_self",
        "self_diff_pred",
        "succ_strictly_greater",
        "pred_strictly_less",
        "succ_pred_difference",
        "succ_dual_witness",
    ] {
        assert!(
            names.iter().any(|n| n == required),
            "successor-ops stdlib pilot must declare `{required}`; found {names:?}"
        );
    }
}
