#![no_main]
//! Fuzz target — feeds arbitrary bytes to the lexer.
//!
//! Closes adversarial finding F3 partial (sub-issue #357).
//!
//! Acceptance: must not panic on any byte sequence. Diagnostics are
//! allowed; the lexer exposes them via the returned token stream
//! and/or a side error channel. Crashes ⇒ automatic issue per #357.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The lexer takes &str, so we accept only valid UTF-8 here.
    // Invalid UTF-8 is filtered upstream — the lexer is never asked
    // to handle it. (A future fuzz target can specifically stress
    // the UTF-8 boundary if useful.)
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let mut lexer = gradient_compiler::lexer::Lexer::new(text, 0);
    let _tokens = lexer.tokenize();
    // We don't care about the token contents — only that lexing
    // terminates without panicking. The token stream may include
    // error tokens; that's the compiler's diagnostic channel, not
    // a fuzz failure.
});
