//! Golden-file tests for the verification-condition (VC) generator
//! (sub-issue #328, ADR 0003).
//!
//! Each test parses a small `@verified` Gradient function, runs it
//! through `VcEncoder::encode_function`, and compares the produced
//! SMT-LIB queries against a hand-written golden file under
//! `tests/vc_goldens/`. The golden files are deliberately
//! human-readable: they document what `gradient-compiler` is
//! contractually obligated to emit for each shape, and form the
//! input the #329 Z3 driver will consume.
//!
//! To regenerate goldens after an intentional encoder change, set
//! `UPDATE_GOLDENS=1` in the environment and re-run the tests.

use gradient_compiler::ast::item::{FnDef, ItemKind};
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker::vc::VcEncoder;
use std::path::{Path, PathBuf};

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

fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vc_goldens")
}

/// Compare `actual` against the golden file. When `UPDATE_GOLDENS=1`,
/// rewrite the golden instead of failing.
fn assert_matches_golden(name: &str, actual: &str) {
    let path = goldens_dir().join(format!("{name}.smt2"));
    if std::env::var("UPDATE_GOLDENS").is_ok() {
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden file `{}` ({e}); run with UPDATE_GOLDENS=1 to create it",
            path.display()
        )
    });
    if expected != actual {
        // Print a unified-diff-ish view to make CI failures readable.
        eprintln!("=== EXPECTED ({}) ===\n{}", path.display(), expected);
        eprintln!("=== ACTUAL ===\n{actual}");
        panic!(
            "golden mismatch for `{name}`; rerun with UPDATE_GOLDENS=1 if intentional"
        );
    }
}

/// Render an encoded function as the canonical multi-query block
/// stored on disk. Each query is separated by a `; ---` divider for
/// human readability.
fn render_for_golden(encoded: &gradient_compiler::typechecker::vc::EncodedFunction) -> String {
    let mut out = String::new();
    for (i, q) in encoded.queries.iter().enumerate() {
        if i > 0 {
            out.push_str("; ---\n");
        }
        out.push_str(&q.smtlib);
    }
    out
}

// ── 5 representative functions ──────────────────────────────────────────

#[test]
fn golden_clamp_nonneg() {
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
    let encoded = VcEncoder::encode_function(&f).expect("encode");
    assert_matches_golden("clamp_nonneg", &render_for_golden(&encoded));
}

#[test]
fn golden_max_two() {
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
    let encoded = VcEncoder::encode_function(&f).expect("encode");
    assert_matches_golden("max_two", &render_for_golden(&encoded));
}

#[test]
fn golden_abs_value() {
    let src = "\
@verified
@requires(true)
@ensures(result >= 0)
fn abs_value(n: Int) -> Int:
    if n < 0:
        -n
    else:
        n
";
    let f = parse_first_fn(src);
    let encoded = VcEncoder::encode_function(&f).expect("encode");
    assert_matches_golden("abs_value", &render_for_golden(&encoded));
}

#[test]
fn golden_double_with_bound() {
    let src = "\
@verified
@requires(n >= 0)
@requires(n <= 100)
@ensures(result >= 0)
@ensures(result <= 200)
fn double_bounded(n: Int) -> Int:
    n + n
";
    let f = parse_first_fn(src);
    let encoded = VcEncoder::encode_function(&f).expect("encode");
    assert_matches_golden("double_bounded", &render_for_golden(&encoded));
}

#[test]
fn golden_xor_bool() {
    let src = "\
@verified
@requires(true)
@ensures(result == (a or b))
@ensures(not (result and (a and b)))
fn xor_bool(a: Bool, b: Bool) -> Bool:
    if a:
        not b
    else:
        b
";
    let f = parse_first_fn(src);
    let encoded = VcEncoder::encode_function(&f).expect("encode");
    assert_matches_golden("xor_bool", &render_for_golden(&encoded));
}

// ── Stability gates ─────────────────────────────────────────────────────

#[test]
fn encoder_is_deterministic() {
    // Encoding twice must produce byte-identical output. Z3's input
    // is a string, so non-determinism here would surface as flaky
    // counterexamples in #329.
    let src = "\
@verified
@requires(n >= 0)
@ensures(result >= 0)
fn id_nonneg(n: Int) -> Int:
    n
";
    let f = parse_first_fn(src);
    let a = VcEncoder::encode_function(&f).expect("encode");
    let b = VcEncoder::encode_function(&f).expect("encode");
    assert_eq!(a, b);
}

#[test]
fn encoder_query_count_matches_ensures_count() {
    // 0 ensures (only requires) → 1 satisfiability probe.
    // N ensures → N obligation queries.
    for n_ensures in 0..=3 {
        let mut src = String::from("@verified\n@requires(true)\n");
        for _ in 0..n_ensures {
            src.push_str("@ensures(result >= 0)\n");
        }
        src.push_str("fn f(n: Int) -> Int:\n    n\n");
        let f = parse_first_fn(&src);
        let encoded = VcEncoder::encode_function(&f).expect("encode");
        let expected_queries = if n_ensures == 0 { 1 } else { n_ensures };
        assert_eq!(
            encoded.queries.len(),
            expected_queries,
            "mismatch for n_ensures={n_ensures}"
        );
    }
}
