#![no_main]
//! Fuzz target — feeds arbitrary text through full pipeline:
//! lexer → parser → checker → IR builder.
//!
//! partially tracks an adversarial-review item (sub-issue #358).
//!
//! Acceptance: must not panic on any input. The IR builder is
//! expected to return `(Module, Vec<String>)` (warnings) but must
//! terminate without panicking. Crashes ⇒ automatic issue per #358.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let mut lexer = gradient_compiler::lexer::Lexer::new(text, 0);
    let tokens = lexer.tokenize();
    let (module, _parse_errs) = gradient_compiler::parser::parse(tokens, 0);
    // Run typechecker first — IR builder generally assumes a
    // type-checked module, but the contract for the fuzz harness is
    // simply "no panics", so a malformed input that surfaces as
    // type errors should still be safe to attempt to lower.
    let _type_errs = gradient_compiler::typechecker::check_module(&module, 0);
    let (_ir_module, _warnings) = gradient_compiler::ir::IrBuilder::build_module(&module);
});
