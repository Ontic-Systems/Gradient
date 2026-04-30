//! Integration gate for #261: ExternFn declarations inside `mod` blocks
//! must be registered by the typechecker's `ModBlock` first-pass.
//!
//! Companion to #259's `TypeEnv::new()` registration: where #259 wires
//! known kernel surfaces in unconditionally, this gate verifies that
//! arbitrary `.gr` modules can declare their own externs locally and
//! the typechecker resolves calls to them within the same `mod` block.
//!
//! Pre-#261 symptom: AST parses cleanly (`ItemKind::ExternFn` lands
//! inside the `ModBlock`), but the typechecker reports
//! `undefined variable my_extern` at every call site because the
//! `ModBlock` first-pass only handles `TypeDecl`/`EnumDecl`/`FnDef`.

use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;

fn typecheck_errors(src: &str) -> Vec<String> {
    let mut lex = Lexer::new(src, 0);
    let tokens = lex.tokenize();
    let (module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);
    let type_errors = typechecker::check_module(&module, 0);
    type_errors
        .into_iter()
        .filter(|e| !e.is_warning)
        .map(|e| e.message)
        .collect()
}

/// Returns true if the error list mentions any `undefined variable` /
/// `unknown function` style failure for `name`. Used so the test gates
/// only on the actual ModBlock-resolution bug, not on unrelated
/// effect-system errors which can vary based on extern defaults.
fn unresolved(errors: &[String], name: &str) -> bool {
    errors.iter().any(|e| {
        e.contains(&format!("undefined variable `{}`", name))
            || e.contains(&format!("unknown function `{}`", name))
    })
}

/// Minimal repro from `references/gradient-modblock-externfn-resolution.md`.
/// Pre-#261 this fails with `undefined variable my_extern`; post-#261 it
/// must resolve (effect-related errors do not count — they reflect a
/// separate concern around extern default-effects).
#[test]
fn extern_in_mod_block_resolves_within_module() {
    let src = "\
mod scratch:

    fn my_extern(x: Int) -> Int

    fn caller(x: Int) -> Int:
        ret my_extern(x)
";
    let errors = typecheck_errors(src);
    assert!(
        !unresolved(&errors, "my_extern"),
        "expected `my_extern` to resolve after #261, got: {:#?}",
        errors
    );
}

/// Multiple externs of different return types should all resolve.
#[test]
fn multiple_externs_in_mod_block_resolve() {
    let src = "\
mod multi:

    fn ext_int(x: Int) -> Int
    fn ext_str(s: String) -> String
    fn ext_bool(b: Int) -> Int

    fn caller(x: Int) -> Int:
        let a = ext_int(x)
        let s = ext_str(\"hi\")
        let b = ext_bool(1)
        ret a + b
";
    let errors = typecheck_errors(src);
    for name in ["ext_int", "ext_str", "ext_bool"] {
        assert!(
            !unresolved(&errors, name),
            "expected `{}` to resolve, got: {:#?}",
            name,
            errors
        );
    }
}

/// Sanity: top-level externs (which were always registered) still
/// resolve after the ModBlock first-pass change. Effect errors are
/// expected (extern defaults to all effects); we only assert resolution.
#[test]
fn top_level_extern_still_resolves() {
    let src = "\
fn top_ext(x: Int) -> Int

fn caller(x: Int) -> Int:
    ret top_ext(x)
";
    let errors = typecheck_errors(src);
    assert!(
        !unresolved(&errors, "top_ext"),
        "top-level extern should resolve, got: {:#?}",
        errors
    );
}

/// A `mod` block with both a `FnDef` and an `ExternFn`: both kinds of
/// declaration must coexist. Pre-#261 the `FnDef` resolved but the
/// `ExternFn` did not.
#[test]
fn mod_block_mixes_fn_def_and_extern_fn() {
    let src = "\
mod mixed:

    fn helper_extern(x: Int) -> Int

    fn helper(x: Int) -> Int:
        ret x + 1

    fn caller(x: Int) -> Int:
        let a = helper(x)
        let b = helper_extern(x)
        ret a + b
";
    let errors = typecheck_errors(src);
    assert!(
        !unresolved(&errors, "helper"),
        "regular fn should still resolve: {:#?}",
        errors
    );
    assert!(
        !unresolved(&errors, "helper_extern"),
        "extern in mod block should resolve after #261: {:#?}",
        errors
    );
}
