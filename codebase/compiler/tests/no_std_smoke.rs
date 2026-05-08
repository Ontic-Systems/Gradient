//! `no_std` smoke test (issue #347).
//!
//! Per ADR 0005 § "Implementation order" #347, this is the regression
//! gate that **compiles a known-pure module against the `core`-only
//! surface and asserts zero `!{Heap}` (and zero `!{IO}` / `!{FS}` /
//! `!{Net}` / `!{Time}` / `!{Mut}`) appears in the inferred effect
//! closure**.
//!
//! Today (post-#345 scaffold from #528) the gate is software-only:
//! we lex + parse + type-check each fixture, walk every function's
//! effect summary (`declared` ∪ `inferred`), and assert
//! [`StdlibTier::classify_effects`] returns [`StdlibTier::Core`] for
//! the union. The CI cross-compile target-triple matrix referenced in
//! issue #347's body (`x86_64-unknown-none`, `arm-none-eabi`,
//! `riscv32imac-unknown-none-elf`) is parked behind E5 (modular
//! runtime split) and E6 (cross-compile backend split) per the
//! issue's "Blocked by" line. When E5/E6 land, this test grows a
//! parallel matrix; the tier-classification half landed here is the
//! foundation.
//!
//! Adding a new fixture: drop a file under
//! `tests/no_std_corpus/<name>.gr`. The test discovers every `.gr`
//! under that directory automatically, so any new fixture is
//! immediately enforced.
//!
//! Authoring rule for fixtures: every function must end up classified
//! at [`StdlibTier::Core`]. If you want to demonstrate an `Alloc`-tier
//! pattern, add it under a sibling `tests/alloc_only_corpus/` (no such
//! corpus exists yet — file a follow-on issue if you need one).
//!
//! See also:
//! - `docs/adr/0005-stdlib-split.md` — locked decision.
//! - `docs/stdlib-migration.md` — migration guide.
//! - `codebase/compiler/src/typechecker/stdlib_tier.rs` — classifier impl.
//! - `codebase/compiler/tests/stdlib_tier_classification.rs` —
//!   per-builtin tier pinning (sibling regression target).

use std::fs;
use std::path::{Path, PathBuf};

use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;
use gradient_compiler::typechecker::stdlib_tier::{classify_effects, StdlibTier};

/// Locate the no_std fixture corpus directory relative to the test
/// binary (Cargo sets `CARGO_MANIFEST_DIR` to the crate root).
fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/no_std_corpus")
}

/// Discover every `.gr` fixture under [`corpus_dir`].
fn discover_fixtures() -> Vec<PathBuf> {
    let dir = corpus_dir();
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("read_dir {}: {err}", dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("gr"))
        .collect();
    out.sort();
    out
}

/// Lex + parse + type-check `src` and return the per-function effect
/// summary. Asserts the front-end is clean — any parse or type error
/// in a no_std fixture is a fixture authoring bug.
fn analyze(src: &str, label: &str) -> Vec<gradient_compiler::typechecker::effects::EffectInfo> {
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(
        parse_errors.is_empty(),
        "[{label}] parse errors: {parse_errors:?}",
    );

    let (type_errors, summary) = typechecker::check_module_with_effects(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        real_errors.is_empty(),
        "[{label}] type errors: {real_errors:?}",
    );

    summary.functions
}

/// Union the `declared` and `inferred` effect rows for a function;
/// this is the conservative effect closure the tier classifier should
/// see.
fn function_effect_closure(
    info: &gradient_compiler::typechecker::effects::EffectInfo,
) -> Vec<String> {
    let mut effects: Vec<String> = info.declared.clone();
    for eff in &info.inferred {
        if !effects.iter().any(|e| e == eff) {
            effects.push(eff.clone());
        }
    }
    effects
}

/// Run the no_std smoke against a single fixture.
fn assert_fixture_is_core(path: &Path) {
    let label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("<unknown>");
    let src =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));

    let infos = analyze(&src, label);
    assert!(
        !infos.is_empty(),
        "[{label}] expected at least one function in fixture",
    );

    for info in infos {
        let closure = function_effect_closure(&info);
        let tier = classify_effects(&closure);
        assert_eq!(
            tier,
            StdlibTier::Core,
            "[{label}] fn `{}` classified as {tier:?} via effect closure {closure:?}; \
             no_std fixtures must stay at Core (no Heap/IO/FS/Net/Time/Mut). \
             If this is intentional, move the function to an alloc/std fixture corpus \
             and update the test routing.",
            info.function,
        );
    }
}

// ── Per-fixture targeted tests (named for diagnostic clarity) ─────────

#[test]
fn arithmetic_fixture_is_core() {
    let path = corpus_dir().join("arithmetic.gr");
    assert!(path.exists(), "expected fixture at {}", path.display());
    assert_fixture_is_core(&path);
}

#[test]
fn control_flow_fixture_is_core() {
    let path = corpus_dir().join("control_flow.gr");
    assert!(path.exists(), "expected fixture at {}", path.display());
    assert_fixture_is_core(&path);
}

#[test]
fn data_structures_no_alloc_fixture_is_core() {
    let path = corpus_dir().join("data_structures_no_alloc.gr");
    assert!(path.exists(), "expected fixture at {}", path.display());
    assert_fixture_is_core(&path);
}

// ── Auto-discovering regression target ────────────────────────────────

/// Walks every `.gr` under `tests/no_std_corpus/`, lexes + parses +
/// type-checks each, and asserts every function's effect closure
/// classifies at [`StdlibTier::Core`]. New fixtures are picked up
/// automatically — no test wiring required.
///
/// This is the regression gate: if a future stdlib change drags
/// `!{Heap}` (or any Std-tier effect) into a transitively-reachable
/// builtin used by these fixtures, this test fires.
#[test]
fn every_no_std_fixture_classifies_as_core() {
    let fixtures = discover_fixtures();
    assert!(
        !fixtures.is_empty(),
        "no_std corpus is empty — see {}",
        corpus_dir().display(),
    );
    // Pin the minimum coverage so a future cleanup PR can't silently
    // empty the corpus and leave a green-but-vacuous gate. Per ADR 0005
    // #347 acceptance, the fixtures must cover arithmetic + control
    // flow + basic data structures.
    let names: Vec<String> = fixtures
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(
        names.iter().any(|n| n.contains("arithmetic")),
        "expected an arithmetic.* fixture, got {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("control_flow")),
        "expected a control_flow.* fixture, got {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("data_structures")),
        "expected a data_structures.* fixture, got {names:?}",
    );

    for path in fixtures {
        assert_fixture_is_core(&path);
    }
}

// ── Counterexample sanity: a heap-allocating call IS classified above Core ──

/// Negative control: a synthetic source that calls a `!{Heap}` builtin
/// must NOT classify at [`StdlibTier::Core`]. This pins the gate's
/// teeth — without this test, an accidentally-permissive classifier
/// (e.g. one that always returns `Core`) could pass every fixture
/// vacuously.
#[test]
fn alloc_call_does_not_classify_as_core() {
    // `string_to_int` is `!{Heap}` post-wave-2 (#524).
    let src = "fn parse_one() -> !{Heap} Option[Int]:\n    string_to_int(\"42\")\n";
    let infos = analyze(src, "negative_control.alloc_call");
    let target = infos
        .iter()
        .find(|i| i.function == "parse_one")
        .expect("parse_one not found in summary");
    let closure = function_effect_closure(target);
    let tier = classify_effects(&closure);
    assert_ne!(
        tier,
        StdlibTier::Core,
        "negative-control fn calling string_to_int classified at {tier:?}; \
         expected ≥ Alloc. closure={closure:?}",
    );
    assert!(
        tier >= StdlibTier::Alloc,
        "expected tier ≥ Alloc for Heap-bearing fn, got {tier:?}",
    );
}
