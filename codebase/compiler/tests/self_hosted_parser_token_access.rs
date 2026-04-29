//! Self-hosted parser token-access parity (Issue #221 / Epic #116).
//!
//! Validates that `compiler/parser.gr::current_token`, `peek_token`, and
//! `parser_advance` — modelled by [`bootstrap_parser_bridge`] — read from a
//! real runtime-backed [`BootstrapTokenList`] and produce the same kind
//! sequence as the Rust reference [`Lexer`] on the bootstrap corpus.
//!
//! Single-line scope: the current self-hosted lexer treats LF/INDENT/DEDENT
//! as plain unexpected characters. Cross-line and indentation parity is
//! tracked under follow-up issues (#224).

use gradient_compiler::bootstrap_parser_bridge::{
    bootstrap_token_list_get_end_offset, bootstrap_token_list_get_file_id,
    bootstrap_token_list_get_int_value, bootstrap_token_list_get_kind,
    bootstrap_token_list_get_start_offset, bootstrap_token_list_get_text, BootstrapParser,
    EOF_KIND_TAG,
};
use gradient_compiler::lexer::token::TokenKind;
use gradient_compiler::Lexer;

const PARITY_CORPUS: &[(&str, &str)] = &[
    ("ident_plus_int", "x + 1"),
    ("ret_eq", "ret x == y"),
    ("call_two_args", "add(x, y)"),
    ("nested_call", "f(g(x), h(y))"),
    ("comparison_chain", "a <= b and c >= d"),
    ("arrow_signature_fragment", "fn add(x: Int, y: Int) -> Int"),
    ("not_and_or", "not a and b or c"),
    ("string_literal", "let s = \"hi\""),
    ("dot_access", "p.x"),
    ("brackets", "[a, b, c]"),
    ("braces", "{x, y}"),
];

#[derive(Debug, Clone, PartialEq)]
enum TokenShape {
    Plain(&'static str),
    Ident,
    IntLit,
    FloatLit,
    StringLit,
    Error,
}

/// Payload parity: #223 widens token access beyond discriminants so the
/// parser direct path can recover names and literal values.
fn shape(kind: &TokenKind) -> TokenShape {
    match kind {
        TokenKind::Ident(_) => TokenShape::Ident,
        TokenKind::IntLit(_) => TokenShape::IntLit,
        TokenKind::FloatLit(_) => TokenShape::FloatLit,
        TokenKind::StringLit(_) => TokenShape::StringLit,
        TokenKind::CharLit(_) => TokenShape::StringLit,
        TokenKind::Error(_) => TokenShape::Error,
        TokenKind::Fn => TokenShape::Plain("Fn"),
        TokenKind::Let => TokenShape::Plain("Let"),
        TokenKind::Mut => TokenShape::Plain("Mut"),
        TokenKind::If => TokenShape::Plain("If"),
        TokenKind::Else => TokenShape::Plain("Else"),
        TokenKind::For => TokenShape::Plain("For"),
        TokenKind::In => TokenShape::Plain("In"),
        TokenKind::While => TokenShape::Plain("While"),
        TokenKind::Ret => TokenShape::Plain("Ret"),
        TokenKind::Type => TokenShape::Plain("Type"),
        TokenKind::Mod => TokenShape::Plain("Mod"),
        TokenKind::Use => TokenShape::Plain("Use"),
        TokenKind::Pub => TokenShape::Plain("Pub"),
        TokenKind::Impl => TokenShape::Plain("Impl"),
        TokenKind::Match => TokenShape::Plain("Match"),
        TokenKind::True => TokenShape::Plain("True"),
        TokenKind::False => TokenShape::Plain("False"),
        TokenKind::And => TokenShape::Plain("And"),
        TokenKind::Or => TokenShape::Plain("Or"),
        TokenKind::Not => TokenShape::Plain("Not"),
        TokenKind::Plus => TokenShape::Plain("Plus"),
        TokenKind::Minus => TokenShape::Plain("Minus"),
        TokenKind::Star => TokenShape::Plain("Star"),
        TokenKind::Slash => TokenShape::Plain("Slash"),
        TokenKind::Percent => TokenShape::Plain("Percent"),
        TokenKind::Eq => TokenShape::Plain("Eq"),
        TokenKind::Ne => TokenShape::Plain("Ne"),
        TokenKind::Lt => TokenShape::Plain("Lt"),
        TokenKind::Gt => TokenShape::Plain("Gt"),
        TokenKind::Le => TokenShape::Plain("Le"),
        TokenKind::Ge => TokenShape::Plain("Ge"),
        TokenKind::Assign => TokenShape::Plain("Assign"),
        TokenKind::Arrow => TokenShape::Plain("Arrow"),
        TokenKind::LParen => TokenShape::Plain("LParen"),
        TokenKind::RParen => TokenShape::Plain("RParen"),
        TokenKind::LBrace => TokenShape::Plain("LBrace"),
        TokenKind::RBrace => TokenShape::Plain("RBrace"),
        TokenKind::LBracket => TokenShape::Plain("LBracket"),
        TokenKind::RBracket => TokenShape::Plain("RBracket"),
        TokenKind::Colon => TokenShape::Plain("Colon"),
        TokenKind::Comma => TokenShape::Plain("Comma"),
        TokenKind::Dot => TokenShape::Plain("Dot"),
        TokenKind::Eof => TokenShape::Plain("Eof"),
        _ => TokenShape::Plain("Other"),
    }
}

fn rust_shapes(src: &str) -> Vec<TokenShape> {
    let mut lex = Lexer::new(src, 0);
    let mut shapes = Vec::new();
    for tok in lex.tokenize() {
        // Skip Newline / Indent / Dedent: the self-hosted lexer doesn't emit
        // them on this single-line corpus, so they would not appear in the
        // bootstrap stream either.
        match tok.kind {
            TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent => continue,
            _ => {}
        }
        shapes.push(shape(&tok.kind));
    }
    // Ensure the stream is terminated by Eof regardless of trailing newlines.
    if !matches!(shapes.last(), Some(TokenShape::Plain("Eof"))) {
        shapes.push(TokenShape::Plain("Eof"));
    }
    shapes
}

#[test]
fn current_token_kind_matches_rust_lexer_on_bootstrap_corpus() {
    for (label, src) in PARITY_CORPUS {
        let p = BootstrapParser::from_source(src, 0);
        let bootstrap_shapes: Vec<TokenShape> = p.drain_kinds().iter().map(shape).collect();
        let reference_shapes = rust_shapes(src);
        assert_eq!(
            bootstrap_shapes, reference_shapes,
            "self-hosted parser token-access drifted from Rust lexer on `{label}`: src={src:?}"
        );
    }
}

#[test]
fn current_token_advances_through_real_stream() {
    let p = BootstrapParser::from_source("ret x == y", 0);

    // current_token at pos=0 is `ret`; advancing must surface Ident, then Eq, etc.
    let cur = p.current_token();
    assert!(matches!(cur.kind, TokenKind::Ret));

    let after_ret = p.advance();
    assert!(matches!(
        after_ret.current_token().kind,
        TokenKind::Ident(_)
    ));

    let after_x = after_ret.advance();
    assert!(matches!(after_x.current_token().kind, TokenKind::Eq));

    // peek_token must respect offset rather than aliasing current_token.
    let peeked = after_ret.peek_token(1);
    assert!(matches!(peeked.kind, TokenKind::Eq));
}

#[test]
fn out_of_bounds_token_access_yields_eof_safely() {
    let p = BootstrapParser::from_source("x", 7);

    // Walk the entire stream + one beyond the end. drain_kinds itself stops at
    // the first Eof emitted by the runtime list; peek_token past that point
    // must keep returning Eof rather than panicking or aliasing earlier tokens.
    let beyond = p.peek_token(50);
    assert!(matches!(beyond.kind, TokenKind::Eof));
    assert_eq!(beyond.span.file_id, 7);

    // Negative indices in the FFI map to the Eof tag without ever hitting
    // the underlying Vec — guards against signed/unsigned mistakes in the
    // self-hosted callsites.
    let tag = bootstrap_token_list_get_kind(&p.store, p.handle, -3);
    assert_eq!(tag, EOF_KIND_TAG);
}

#[test]
fn span_offsets_round_trip_for_real_tokens() {
    let p = BootstrapParser::from_source("x + 1", 0);
    let len = p.store.len(p.handle).expect("bootstrap len");
    assert!(len >= 4, "expected at least [Ident, Plus, IntLit, Eof]");

    for index in 0..(len - 1) as i64 {
        let start_extern = bootstrap_token_list_get_start_offset(&p.store, p.handle, index);
        let end_extern = bootstrap_token_list_get_end_offset(&p.store, p.handle, index);
        let file_id_extern = bootstrap_token_list_get_file_id(&p.store, p.handle, index);

        let stored = p.store.get(p.handle, index as usize).expect("get");
        assert_eq!(start_extern as u32, stored.span.start.offset);
        assert_eq!(end_extern as u32, stored.span.end.offset);
        assert_eq!(file_id_extern as u32, stored.span.file_id);
    }
}

#[test]
fn payload_accessors_round_trip_names_and_literals() {
    let p = BootstrapParser::from_source("let answer = 42", 0);
    assert_eq!(
        bootstrap_token_list_get_text(&p.store, p.handle, 1),
        "answer"
    );
    assert_eq!(
        bootstrap_token_list_get_int_value(&p.store, p.handle, 3),
        42
    );
    assert_eq!(p.peek_token(1).kind, TokenKind::Ident("answer".into()));
    assert_eq!(p.peek_token(3).kind, TokenKind::IntLit(42));

    let s = BootstrapParser::from_source("let msg = \"ok\"", 0);
    assert_eq!(bootstrap_token_list_get_text(&s.store, s.handle, 3), "ok");
    assert_eq!(s.peek_token(3).kind, TokenKind::StringLit("ok".into()));
}

#[test]
fn parser_advance_preserves_token_list_handle_identity() {
    let p = BootstrapParser::from_source("a + b", 0);
    let q = p.advance();
    let r = q.advance();
    // All three parser snapshots must point at the same runtime-backed list,
    // mirroring `parser.gr::parser_advance`'s `tokens: p.tokens` propagation.
    assert_eq!(p.handle.raw(), q.handle.raw());
    assert_eq!(q.handle.raw(), r.handle.raw());
    assert_eq!(p.pos, 0);
    assert_eq!(q.pos, 1);
    assert_eq!(r.pos, 2);
}
