#![no_main]
//! Fuzz target — feeds arbitrary text through lexer → parser → checker.
//!
//! Closes adversarial finding F3 partial (sub-issue #358).
//!
//! Acceptance: must not panic on any input. The checker is expected to
//! return `Vec<TypeError>` (possibly empty, possibly error-laden) but
//! must terminate without panicking. Crashes ⇒ automatic issue per
//! #358.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let mut lexer = gradient_compiler::lexer::Lexer::new(text, 0);
    let tokens = lexer.tokenize();
    let (module, _parse_errs) = gradient_compiler::parser::parse(tokens, 0);
    // Only attempt typecheck if parser produced something; an
    // unrecoverable parse may yield an empty module which is still
    // valid input to the checker.
    let _type_errs = gradient_compiler::typechecker::check_module(&module, 0);
});
