#![no_main]
//! Fuzz target — feeds arbitrary text through lexer → parser.
//!
//! Closes adversarial finding F3 partial (sub-issue #357).
//!
//! Acceptance: must not panic on any input. The parser is expected
//! to return a (Module, Vec<ParseError>) tuple even on malformed
//! input; errors are diagnostics, not panics. Crashes ⇒ automatic
//! issue per #357.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let mut lexer = gradient_compiler::lexer::Lexer::new(text, 0);
    let tokens = lexer.tokenize();
    let (_module, _errs) = gradient_compiler::parser::parse(tokens, 0);
    // We don't assert anything about the AST shape or the error
    // count — those are correctness properties handled by the
    // typechecker tests / parser-differential corpus. The fuzz
    // contract is: lex+parse must terminate without panicking.
});
