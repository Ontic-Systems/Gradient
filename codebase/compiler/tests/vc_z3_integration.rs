//! Z3 subprocess integration tests for the `@verified` contract
//! discharger (sub-issue #329, ADR 0003 step 3).
//!
//! These tests run only when a Z3 binary is reachable. Resolution
//! order matches `ContractDischarger::resolve_z3_path`:
//!   1. `GRADIENT_Z3_BIN` env var pointing at an executable file.
//!   2. `z3` on `PATH`.
//! When neither is available the test body returns immediately with
//! a single `eprintln!` so CI without Z3 stays green. Set
//! `GRADIENT_Z3_REQUIRED=1` to fail loudly instead — used by the
//! GitHub Actions matrix that does install Z3.
//!
//! Coverage targets the acceptance criteria on issue #329:
//! - Z3 invoked via subprocess.
//! - Counterexample diagnostic includes input values + the
//!   contract that failed.
//! - Timeout handling (default 5 s, configurable).
//! - Tested on representative shapes: a sound function (`clamp_nonneg`),
//!   a deliberately-wrong function (`bad_clamp_nonneg`), a precondition
//!   contradiction probe, and the simple "result equals param + 1"
//!   ensures.

use gradient_compiler::ast::item::{ContractKind, FnDef, ItemKind};
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker::vc::{
    ContractDischarger, DischargeError, DischargeOutcome, DischargerConfig, ModelBinding, VcEncoder,
};

fn parse_first_fn(src: &str) -> FnDef {
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();
    let (module, errs) = parser::parse(tokens, 0);
    assert!(errs.is_empty(), "parse errors: {errs:?}");
    match &module.items[0].node {
        ItemKind::FnDef(f) => f.clone(),
        other => panic!("expected FnDef, got {other:?}"),
    }
}

/// Skip the test cleanly when Z3 isn't on PATH and the user hasn't
/// pinned `GRADIENT_Z3_REQUIRED=1`. The skip emits a single
/// `eprintln!` so it shows up in `cargo test -- --nocapture` runs but
/// doesn't muddy the green/red signal.
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
    eprintln!("[skip] Z3 not available; install z3 or set GRADIENT_Z3_BIN to run discharger tests");
    None
}

// ── Sound function: every obligation discharged ────────────────────────

#[test]
fn discharges_clamp_nonneg() {
    let Some(d) = z3_or_skip() else { return };
    let src = "\
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn clamp_nonneg(n: Int) -> Int:
    if n >= 0:
        n
    else:
        0
";
    let f = parse_first_fn(src);
    let report = d.discharge_function(&f).expect("discharge");
    assert_eq!(report.fn_name, "clamp_nonneg");
    assert_eq!(report.outcomes.len(), 1, "one @ensures => one query");
    let outcome = &report.outcomes[0].outcome;
    match outcome {
        DischargeOutcome::Discharged => {}
        other => panic!("expected Discharged, got {other:?}"),
    }
}

#[test]
fn discharges_max_two() {
    let Some(d) = z3_or_skip() else { return };
    let src = "\
@verified
@requires(true)
@ensures(result >= a)
@ensures(result >= b)
fn max_two(a: Int, b: Int) -> Int:
    if a >= b:
        a
    else:
        b
";
    let f = parse_first_fn(src);
    let report = d.discharge_function(&f).expect("discharge");
    assert_eq!(report.outcomes.len(), 2);
    for q in &report.outcomes {
        assert!(
            matches!(q.outcome, DischargeOutcome::Discharged),
            "expected every @ensures discharged, got {q:?}"
        );
    }
}

// ── Faulty function: counterexample with bindings ──────────────────────

#[test]
fn counterexample_for_buggy_clamp() {
    // Deliberately wrong: returns -1 when n < 0 but @ensures requires
    // result >= 0. Z3 must produce a model with n < 0.
    let Some(d) = z3_or_skip() else { return };
    let src = "\
@verified
@requires(true)
@ensures(result >= 0)
fn bad_clamp(n: Int) -> Int:
    if n >= 0:
        n
    else:
        -1
";
    let f = parse_first_fn(src);
    let report = d.discharge_function(&f).expect("discharge");
    assert_eq!(report.outcomes.len(), 1);
    let outcome = &report.outcomes[0].outcome;
    match outcome {
        DischargeOutcome::Counterexample { bindings } => {
            // We expect at least one binding for n. The exact value
            // is solver-dependent (any n < 0 satisfies), so we only
            // assert that the binding exists and surfaces in source
            // syntax.
            let n_binding: Option<&ModelBinding> = bindings.iter().find(|b| b.name == "n");
            assert!(
                n_binding.is_some(),
                "expected a binding for `n`, got bindings: {bindings:?}"
            );
            let val = &n_binding.unwrap().value;
            // The binding renders as a Gradient-syntax decimal
            // (signed, no SMT-LIB unary-minus form).
            let _: i64 = val
                .parse()
                .unwrap_or_else(|_| panic!("expected numeric n, got `{val}`"));
        }
        other => panic!("expected Counterexample, got {other:?}"),
    }
    // Ensure the counterexample's metadata pinpoints the @ensures.
    assert_eq!(report.outcomes[0].kind, Some(ContractKind::Ensures));
    assert_eq!(report.outcomes[0].contract_index, Some(1));
}

// ── Precondition-only function: satisfiability probe ───────────────────

#[test]
fn precondition_only_emits_satisfiability_probe() {
    let Some(d) = z3_or_skip() else { return };
    let src = "\
@verified
@requires(n >= 0)
fn nonneg(n: Int) -> Int:
    n
";
    let f = parse_first_fn(src);
    let report = d.discharge_function(&f).expect("discharge");
    assert_eq!(report.outcomes.len(), 1);
    // Satisfiability probe: `unsat` would mean the precondition is
    // contradictory; for `n >= 0` it is satisfiable, so Z3 returns
    // `sat` with a witness model.
    match &report.outcomes[0].outcome {
        DischargeOutcome::Counterexample { .. } => {
            // The check-sat returned `sat` (witness, not violation).
            // The discharger reports it as Counterexample because at
            // the wire level it's the same shape; the checker's
            // `surface_discharge_report` distinguishes by
            // `kind == Requires` + `contract_index == None` (the
            // synthetic-probe sentinel).
            assert_eq!(report.outcomes[0].kind, Some(ContractKind::Requires));
            assert!(report.outcomes[0].contract_index.is_none());
        }
        other => panic!("expected sat (witness) for satisfiable precondition, got {other:?}"),
    }
}

// ── Contradictory precondition probe ───────────────────────────────────

#[test]
fn contradictory_precondition_returns_unsat() {
    let Some(d) = z3_or_skip() else { return };
    let src = "\
@verified
@requires(n >= 0)
@requires(n < 0)
fn impossible(n: Int) -> Int:
    n
";
    let f = parse_first_fn(src);
    let report = d.discharge_function(&f).expect("discharge");
    assert_eq!(report.outcomes.len(), 1);
    // No @ensures so we get the satisfiability probe; the
    // conjoined preconditions are contradictory => unsat =>
    // Discharged at the wire level. (The checker side would
    // surface this as a "contradictory precondition" diagnostic in
    // a follow-on issue.)
    assert!(matches!(
        report.outcomes[0].outcome,
        DischargeOutcome::Discharged
    ));
}

// ── Timeout configuration plumbing ─────────────────────────────────────

#[test]
fn discharger_respects_short_timeout_config() {
    let Some(_) = z3_or_skip() else { return };
    // We can't reliably trigger an actual timeout on a tractable
    // query, but we can verify the config plumbs through and a
    // 100 ms timeout still returns a usable outcome (Z3 finishes
    // tiny linear-arith queries in <10 ms).
    let cfg = DischargerConfig {
        timeout: std::time::Duration::from_millis(500),
        z3_path: None,
    };
    let d = ContractDischarger::new(cfg);
    let src = "\
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn clamp(n: Int) -> Int:
    n
";
    let f = parse_first_fn(src);
    let report = d.discharge_function(&f).expect("discharge");
    // unsat (discharged) — the contract holds for n >= 0.
    assert!(matches!(
        report.outcomes[0].outcome,
        DischargeOutcome::Discharged
    ));
}

// ── Encoder error path: discharge_function surfaces EncodeError ────────

#[test]
fn discharger_returns_encode_error_for_unsupported_param() {
    // No Z3 needed: encoder fails before the discharger spawns Z3.
    // (Skip-friendly: still skip if Z3 missing and required, since
    // the test exercises the discharger entry point.)
    let Some(d) = z3_or_skip() else { return };
    let src = "\
@verified
@requires(true)
@ensures(true)
fn opaque(s: String) -> String:
    s
";
    let f = parse_first_fn(src);
    match d.discharge_function(&f) {
        Err(DischargeError::Encode(_)) => {}
        other => panic!("expected DischargeError::Encode, got {other:?}"),
    }
}

// ── Encoded surface stability: discharger consumes EncodedFunction ─────

#[test]
fn discharge_encoded_decouples_from_encoder() {
    let Some(d) = z3_or_skip() else { return };
    // Verify the discharge_encoded path so other parts of the
    // compiler can encode once and discharge separately (e.g. when
    // running the same VC through multiple solvers in a future
    // sub-issue).
    let src = "\
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn id(n: Int) -> Int:
    n
";
    let f = parse_first_fn(src);
    let encoded = VcEncoder::encode_function(&f).expect("encode");
    let report = d.discharge_encoded(&encoded).expect("discharge_encoded");
    assert!(matches!(
        report.outcomes[0].outcome,
        DischargeOutcome::Discharged
    ));
}
