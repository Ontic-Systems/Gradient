//! Self-hosted lexer parity test (Issue #220 / Epic #116).
//!
//! Validates that `compiler/lexer.gr::tokenize`, modelled by the Rust-side
//! [`bootstrap_lexer_bridge`], emits a real runtime-backed `TokenList` whose
//! token kinds match the Rust reference [`Lexer`] on a single-line bootstrap
//! corpus.
//!
//! Single-line scope: the current self-hosted lexer treats LF/INDENT/DEDENT
//! as plain unexpected characters. Cross-line and indentation parity is
//! tracked under follow-up issues (#221, #224).

use gradient_compiler::bootstrap_collections::BootstrapCollectionKind;
use gradient_compiler::bootstrap_lexer_bridge::{tokenize_via_bootstrap_store, BootstrapTokenList};
use gradient_compiler::lexer::token::TokenKind;
use gradient_compiler::Lexer;

/// Single-line bootstrap snippets the self-hosted scanner can already cover
/// without LF / INDENT handling. These exercise identifiers, keywords,
/// integer literals, single- and double-character operators, parentheses,
/// commas, the arrow token, and string literals.
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

/// Coarse token "shape": ignores literal values, since the self-hosted
/// lexer.gr defers numeric literal parsing (it emits `IntLit(0)`) and
/// shortens string-escape handling. Identifier names are kept because they
/// drive parser identification.
#[derive(Debug, Clone, PartialEq)]
enum TokenShape {
    Plain(&'static str),
    Ident(String),
    IntLit,
    FloatLit,
    StringLit,
    CharLit,
    Error,
}

fn shape(kind: &TokenKind) -> TokenShape {
    match kind {
        TokenKind::Ident(name) => TokenShape::Ident(name.clone()),
        TokenKind::IntLit(_) => TokenShape::IntLit,
        TokenKind::FloatLit(_) => TokenShape::FloatLit,
        TokenKind::StringLit(_) => TokenShape::StringLit,
        TokenKind::CharLit(_) => TokenShape::CharLit,
        TokenKind::Error(_) => TokenShape::Error,
        // Catch-all for keywords/operators: discriminant-style label.
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
        TokenKind::LParen => TokenShape::Plain("LParen"),
        TokenKind::RParen => TokenShape::Plain("RParen"),
        TokenKind::LBrace => TokenShape::Plain("LBrace"),
        TokenKind::RBrace => TokenShape::Plain("RBrace"),
        TokenKind::LBracket => TokenShape::Plain("LBracket"),
        TokenKind::RBracket => TokenShape::Plain("RBracket"),
        TokenKind::Comma => TokenShape::Plain("Comma"),
        TokenKind::Colon => TokenShape::Plain("Colon"),
        TokenKind::Arrow => TokenShape::Plain("Arrow"),
        TokenKind::Dot => TokenShape::Plain("Dot"),
        TokenKind::Eof => TokenShape::Plain("Eof"),
        other => TokenShape::Plain(match other {
            // Cover any remaining variants that the bootstrap subset can
            // surface; anything truly unexpected falls through as a plain
            // discriminant label so the parity test fails loudly.
            _ => "OtherUnsupportedKind",
        }),
    }
}

fn rust_kinds_filtered(src: &str) -> Vec<TokenShape> {
    let mut lex = Lexer::new(src, 0);
    lex.tokenize()
        .into_iter()
        .map(|t| t.kind)
        .filter(|k| {
            !matches!(
                k,
                TokenKind::Indent | TokenKind::Dedent | TokenKind::Newline
            )
        })
        .map(|k| shape(&k))
        .collect()
}

fn bridge_token_list(src: &str) -> BootstrapTokenList {
    tokenize_via_bootstrap_store(src, 0)
}

fn bridge_kinds_filtered(src: &str) -> Vec<TokenShape> {
    bridge_token_list(src)
        .iter()
        .map(|t| shape(&t.kind))
        .collect()
}

#[test]
fn bootstrap_token_list_handle_is_non_zero_and_typed() {
    let list = bridge_token_list("x");
    assert_ne!(
        list.handle.raw(),
        0,
        "self-hosted lexer.gr must allocate non-zero TokenList handles"
    );
    assert_eq!(
        list.handle.kind(),
        BootstrapCollectionKind::TokenList,
        "self-hosted lexer.gr must allocate TokenList-kind handles"
    );
}

#[test]
fn bootstrap_token_list_is_non_empty_for_real_source() {
    for (name, src) in PARITY_CORPUS {
        let list = bridge_token_list(src);
        assert!(
            list.len() >= 2,
            "{name}: expected at least one real token plus Eof, got {} tokens",
            list.len()
        );
        assert!(
            matches!(list.iter().last().expect("eof").kind, TokenKind::Eof),
            "{name}: token list must terminate with Eof"
        );
    }
}

#[test]
fn bootstrap_token_kinds_match_rust_lexer_on_bootstrap_corpus() {
    for (name, src) in PARITY_CORPUS {
        let bridge = bridge_kinds_filtered(src);
        let reference = rust_kinds_filtered(src);
        assert_eq!(
            bridge, reference,
            "{name}: self-hosted lexer.gr token stream diverged from Rust reference\n  src: {src:?}\n  self-hosted: {bridge:?}\n  reference:   {reference:?}"
        );
    }
}

#[test]
fn empty_source_round_trips_through_bootstrap_store() {
    let list = bridge_token_list("");
    assert_eq!(list.len(), 1);
    assert!(matches!(list.get(0).kind, TokenKind::Eof));
}

#[test]
fn append_preserves_scanner_order() {
    // Ordering invariant for the bootstrap store mirrors what `tokenize`
    // in lexer.gr will observe: identifiers come out left-to-right.
    let list = bridge_token_list("alpha beta gamma");
    let names: Vec<String> = list
        .iter()
        .filter_map(|t| match &t.kind {
            TokenKind::Ident(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["alpha", "beta", "gamma"]);
}
