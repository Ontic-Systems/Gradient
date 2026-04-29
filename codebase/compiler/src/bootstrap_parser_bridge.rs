//! Bridge that mirrors `compiler/parser.gr` token-access semantics over the
//! runtime-backed [`BootstrapCollectionStore`].
//!
//! Issue #221: until the self-hosted runtime can execute `parser.gr` directly,
//! we model `current_token`, `peek_token`, and `parser_advance` in Rust so
//! that they exercise the *same* host store the rewritten Gradient functions
//! call into via the `bootstrap_token_list_get_*` extern primitives. This
//! gives downstream parser execution (parser differential gate, IR builder,
//! checker) a stable substrate to consume real token streams from
//! self-hosted code.
//!
//! The bridge intentionally maps to the FFI-primitive contract used by the
//! self-hosted parser:
//!   * `bootstrap_token_list_get_kind(handle, index) -> Int` (Eof tag on OOB)
//!   * `bootstrap_token_list_get_file_id(handle, index) -> Int`
//!   * `bootstrap_token_list_get_start_offset(handle, index) -> Int`
//!   * `bootstrap_token_list_get_end_offset(handle, index) -> Int`
//!
//! Each accessor encodes the same out-of-bounds-as-Eof / zero-span semantics
//! used by `parser.gr::current_token` / `peek_token`. Payload accessors carry
//! identifier names, string/error payloads, and integer literals so parser.gr's
//! direct path can build normalized AST output without falling back to the Rust
//! parser bridge.

use crate::bootstrap_collections::{
    BootstrapCollectionKind, BootstrapCollectionStore, BootstrapHandle,
};
use crate::bootstrap_lexer_bridge::{tokenize_via_bootstrap_store, BootstrapTokenList};
use crate::lexer::token::{Position, Span, Token, TokenKind};

/// Out-of-bounds sentinel: matches `lexer.gr::token_kind_tag(Eof) = 1`.
pub const EOF_KIND_TAG: i64 = 1;

/// Encode a [`TokenKind`] into the same integer space as
/// `lexer.gr::token_kind_tag`.
///
/// Variants the self-hosted lexer doesn't surface (e.g. floats, char literals,
/// multi-char operators not in the bootstrap subset) hit the dense catch-all
/// tag `999`, mirroring the wildcard arm in `lexer.gr::token_kind_tag`.
pub fn token_kind_tag(kind: &TokenKind) -> i64 {
    match kind {
        TokenKind::Eof => 1,
        TokenKind::Error(_) => 2,
        TokenKind::Ident(_) => 3,
        TokenKind::IntLit(_) => 4,
        TokenKind::FloatLit(_) => 5,
        TokenKind::StringLit(_) => 6,
        // The Rust reference TokenKind has no `BoolLit` variant — it tokenizes
        // `true`/`false` as keyword tokens (`True` / `False`). Tag 7 is
        // reserved by `lexer.gr::token_kind_tag` for self-hosted BoolLit; it
        // will never appear in a stream produced by the Rust scanner.
        TokenKind::Plus => 10,
        TokenKind::Minus => 11,
        TokenKind::Star => 12,
        TokenKind::Slash => 13,
        TokenKind::Percent => 14,
        TokenKind::Eq => 15,
        TokenKind::Ne => 16,
        TokenKind::Lt => 17,
        TokenKind::Gt => 18,
        TokenKind::Le => 19,
        TokenKind::Ge => 20,
        TokenKind::Assign => 21,
        TokenKind::Arrow => 22,
        TokenKind::LParen => 30,
        TokenKind::RParen => 31,
        TokenKind::LBrace => 32,
        TokenKind::RBrace => 33,
        TokenKind::LBracket => 34,
        TokenKind::RBracket => 35,
        TokenKind::Colon => 36,
        TokenKind::Comma => 37,
        // The Rust reference lexer rejects `;` outright; the self-hosted lexer
        // emits it under tag 38 only when token.gr's `Semi` variant is wired.
        // Keep the tag stable for round-trip even if we never emit it here.
        TokenKind::Dot => 39,
        TokenKind::Indent => 80,
        TokenKind::Dedent => 81,
        TokenKind::Newline => 82,
        TokenKind::Fn => 50,
        TokenKind::Let => 51,
        TokenKind::Mut => 52,
        TokenKind::If => 53,
        TokenKind::Else => 54,
        TokenKind::For => 55,
        TokenKind::In => 56,
        TokenKind::While => 57,
        TokenKind::Ret => 58,
        TokenKind::Type => 59,
        TokenKind::Mod => 60,
        TokenKind::Use => 61,
        TokenKind::Impl => 62,
        TokenKind::Match => 63,
        TokenKind::True => 64,
        TokenKind::False => 65,
        TokenKind::And => 66,
        TokenKind::Or => 67,
        TokenKind::Not => 68,
        TokenKind::Pub => 70,
        // Catch-all keeps the tag space dense without aliasing onto any of the
        // explicit tags above; new TokenKind variants surface here.
        _ => 999,
    }
}

/// Inverse of [`token_kind_tag`] for tags the self-hosted parser actually
/// inspects when no payload is available.
pub fn token_kind_from_tag(tag: i64) -> TokenKind {
    token_kind_from_parts(tag, 0, String::new())
}

/// Reconstruct a [`TokenKind`] from primitive FFI fields exposed to
/// `parser.gr::kind_tag_to_token_kind_with_payload`.
pub fn token_kind_from_parts(tag: i64, int_value: i64, text: String) -> TokenKind {
    match tag {
        1 => TokenKind::Eof,
        2 => TokenKind::Error(text),
        3 => TokenKind::Ident(text),
        4 => TokenKind::IntLit(int_value),
        5 => TokenKind::FloatLit(0.0),
        6 => TokenKind::StringLit(text),
        // Tag 7 is reserved by the self-hosted lexer for BoolLit but the Rust
        // TokenKind models booleans as the `True` / `False` keyword variants.
        // Map it to `Eof` defensively until bool payload tags are emitted.
        7 => TokenKind::Eof,
        10 => TokenKind::Plus,
        11 => TokenKind::Minus,
        12 => TokenKind::Star,
        13 => TokenKind::Slash,
        14 => TokenKind::Percent,
        15 => TokenKind::Eq,
        16 => TokenKind::Ne,
        17 => TokenKind::Lt,
        18 => TokenKind::Gt,
        19 => TokenKind::Le,
        20 => TokenKind::Ge,
        21 => TokenKind::Assign,
        22 => TokenKind::Arrow,
        30 => TokenKind::LParen,
        31 => TokenKind::RParen,
        32 => TokenKind::LBrace,
        33 => TokenKind::RBrace,
        34 => TokenKind::LBracket,
        35 => TokenKind::RBracket,
        36 => TokenKind::Colon,
        37 => TokenKind::Comma,
        39 => TokenKind::Dot,
        80 => TokenKind::Indent,
        81 => TokenKind::Dedent,
        82 => TokenKind::Newline,
        50 => TokenKind::Fn,
        51 => TokenKind::Let,
        52 => TokenKind::Mut,
        53 => TokenKind::If,
        54 => TokenKind::Else,
        55 => TokenKind::For,
        56 => TokenKind::In,
        57 => TokenKind::While,
        58 => TokenKind::Ret,
        59 => TokenKind::Type,
        60 => TokenKind::Mod,
        61 => TokenKind::Use,
        62 => TokenKind::Impl,
        63 => TokenKind::Match,
        64 => TokenKind::True,
        65 => TokenKind::False,
        66 => TokenKind::And,
        67 => TokenKind::Or,
        68 => TokenKind::Not,
        70 => TokenKind::Pub,
        // Treat unknown tags (including the dense catch-all 999) as Eof so
        // parser execution terminates safely instead of looping on garbage.
        _ => TokenKind::Eof,
    }
}

/// FFI-primitive accessor: kind tag at `index`, or `EOF_KIND_TAG` on OOB.
pub fn bootstrap_token_list_get_kind(
    store: &BootstrapCollectionStore<Token>,
    handle: BootstrapHandle<Token>,
    index: i64,
) -> i64 {
    if index < 0 {
        return EOF_KIND_TAG;
    }
    match store.get(handle, index as usize) {
        Ok(tok) => token_kind_tag(&tok.kind),
        Err(_) => EOF_KIND_TAG,
    }
}

/// FFI-primitive accessor: integer payload for IntLit, or `0` otherwise.
pub fn bootstrap_token_list_get_int_value(
    store: &BootstrapCollectionStore<Token>,
    handle: BootstrapHandle<Token>,
    index: i64,
) -> i64 {
    if index < 0 {
        return 0;
    }
    match store.get(handle, index as usize) {
        Ok(tok) => match &tok.kind {
            TokenKind::IntLit(value) => *value,
            _ => 0,
        },
        Err(_) => 0,
    }
}

/// FFI-primitive accessor: text payload for Ident/StringLit/Error, or empty.
pub fn bootstrap_token_list_get_text(
    store: &BootstrapCollectionStore<Token>,
    handle: BootstrapHandle<Token>,
    index: i64,
) -> String {
    if index < 0 {
        return String::new();
    }
    match store.get(handle, index as usize) {
        Ok(tok) => match &tok.kind {
            TokenKind::Ident(value) | TokenKind::StringLit(value) | TokenKind::Error(value) => {
                value.clone()
            }
            _ => String::new(),
        },
        Err(_) => String::new(),
    }
}

fn span_field(
    store: &BootstrapCollectionStore<Token>,
    handle: BootstrapHandle<Token>,
    index: i64,
    f: impl Fn(&Span) -> u32,
) -> i64 {
    if index < 0 {
        return 0;
    }
    match store.get(handle, index as usize) {
        Ok(tok) => f(&tok.span) as i64,
        Err(_) => 0,
    }
}

/// FFI-primitive accessor: span file_id at `index`, or `0` on OOB.
pub fn bootstrap_token_list_get_file_id(
    store: &BootstrapCollectionStore<Token>,
    handle: BootstrapHandle<Token>,
    index: i64,
) -> i64 {
    span_field(store, handle, index, |s| s.file_id)
}

/// FFI-primitive accessor: span start offset at `index`, or `0` on OOB.
pub fn bootstrap_token_list_get_start_offset(
    store: &BootstrapCollectionStore<Token>,
    handle: BootstrapHandle<Token>,
    index: i64,
) -> i64 {
    span_field(store, handle, index, |s| s.start.offset)
}

/// FFI-primitive accessor: span end offset at `index`, or `0` on OOB.
pub fn bootstrap_token_list_get_end_offset(
    store: &BootstrapCollectionStore<Token>,
    handle: BootstrapHandle<Token>,
    index: i64,
) -> i64 {
    span_field(store, handle, index, |s| s.end.offset)
}

/// Mirror of `compiler/parser.gr::Parser`: a runtime-backed token list, the
/// current cursor, and the file id used to synthesize spans for OOB reads.
#[derive(Clone, Debug)]
pub struct BootstrapParser {
    pub store: BootstrapCollectionStore<Token>,
    pub handle: BootstrapHandle<Token>,
    pub pos: i64,
    pub file_id: u32,
}

impl BootstrapParser {
    /// Construct a parser by tokenizing `source` through the bootstrap lexer
    /// bridge — exactly the path `parser.gr` will follow once it executes.
    pub fn from_source(source: &str, file_id: u32) -> Self {
        let BootstrapTokenList { store, handle } = tokenize_via_bootstrap_store(source, file_id);
        Self {
            store,
            handle,
            pos: 0,
            file_id,
        }
    }

    /// Construct a parser over a runtime-backed TokenList populated by the
    /// caller. Used by the #223 parser differential gate to feed the Rust
    /// lexer's layout-aware stream into parser.gr-shaped direct execution.
    pub fn from_tokens(tokens: Vec<Token>, file_id: u32) -> Self {
        let mut store = BootstrapCollectionStore::new();
        let handle = store.alloc(BootstrapCollectionKind::TokenList);
        for token in tokens {
            store.append(handle, token).expect("append bootstrap token");
        }
        Self {
            store,
            handle,
            pos: 0,
            file_id,
        }
    }

    /// Mirror of `parser.gr::current_token`. Reads the kind tag and span at
    /// `pos`, then reconstructs a [`Token`]. Out-of-bounds reads synthesize
    /// a zero-span Eof token at the parser's `file_id`, matching the
    /// self-hosted contract.
    pub fn current_token(&self) -> Token {
        self.token_at(self.pos)
    }

    /// Mirror of `parser.gr::peek_token(p, offset)`.
    pub fn peek_token(&self, offset: i64) -> Token {
        self.token_at(self.pos + offset)
    }

    /// Mirror of `parser.gr::parser_advance`.
    pub fn advance(&self) -> Self {
        let mut next = self.clone();
        next.pos += 1;
        next
    }

    /// Mirror of `parser.gr::is_at_end`.
    pub fn is_at_end(&self) -> bool {
        matches!(self.current_token().kind, TokenKind::Eof)
    }

    fn token_at(&self, index: i64) -> Token {
        let tag = bootstrap_token_list_get_kind(&self.store, self.handle, index);
        let int_value = bootstrap_token_list_get_int_value(&self.store, self.handle, index);
        let text = bootstrap_token_list_get_text(&self.store, self.handle, index);
        let kind = token_kind_from_parts(tag, int_value, text);

        // OOB lookups (tag == EOF_KIND_TAG via the OOB sentinel path) get a
        // zero-offset span at the parser's file_id. Real tokens get their
        // actual span reconstituted from the primitive accessors so that
        // diagnostics / span arithmetic remain meaningful.
        let in_bounds = index >= 0
            && self
                .store
                .len(self.handle)
                .map(|len| (index as usize) < len)
                .unwrap_or(false);

        let (file_id, start_offset, end_offset) = if in_bounds {
            (
                bootstrap_token_list_get_file_id(&self.store, self.handle, index) as u32,
                bootstrap_token_list_get_start_offset(&self.store, self.handle, index) as u32,
                bootstrap_token_list_get_end_offset(&self.store, self.handle, index) as u32,
            )
        } else {
            (self.file_id, 0, 0)
        };

        Token::new(
            kind,
            Span {
                file_id,
                start: Position {
                    line: 0,
                    col: 0,
                    offset: start_offset,
                },
                end: Position {
                    line: 0,
                    col: 0,
                    offset: end_offset,
                },
            },
        )
    }

    /// Convenience: drive the cursor end-to-end and collect the kind sequence
    /// the self-hosted parser would observe.
    pub fn drain_kinds(&self) -> Vec<TokenKind> {
        let mut out = Vec::new();
        let mut cursor = self.clone();
        loop {
            let tok = cursor.current_token();
            let is_eof = matches!(tok.kind, TokenKind::Eof);
            out.push(tok.kind);
            if is_eof {
                break;
            }
            cursor = cursor.advance();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_token_reads_real_first_token() {
        let p = BootstrapParser::from_source("ret x", 7);
        let tok = p.current_token();
        assert!(matches!(tok.kind, TokenKind::Ret));
        assert_eq!(tok.span.file_id, 7);
    }

    #[test]
    fn peek_token_reads_offset_token() {
        let p = BootstrapParser::from_source("ret x", 0);
        let peeked = p.peek_token(1);
        assert!(matches!(peeked.kind, TokenKind::Ident(_)));
    }

    #[test]
    fn advance_preserves_token_list_identity() {
        let p = BootstrapParser::from_source("a + 1", 0);
        let q = p.advance();
        assert_eq!(p.handle.raw(), q.handle.raw());
        assert_eq!(q.pos, 1);
        assert!(matches!(q.current_token().kind, TokenKind::Plus));
    }

    #[test]
    fn out_of_bounds_returns_eof() {
        let p = BootstrapParser::from_source("a", 11);
        // Source produces [Ident, Eof]; peek beyond the end is Eof.
        let far = p.peek_token(50);
        assert!(matches!(far.kind, TokenKind::Eof));
        assert_eq!(far.span.file_id, 11);
    }

    #[test]
    fn drain_kinds_walks_the_real_stream() {
        let p = BootstrapParser::from_source("x + 123", 0);
        let ks = p.drain_kinds();
        assert_eq!(ks[0], TokenKind::Ident("x".into()));
        assert!(matches!(ks[1], TokenKind::Plus));
        assert_eq!(ks[2], TokenKind::IntLit(123));
        assert!(matches!(ks.last(), Some(TokenKind::Eof)));
    }

    #[test]
    fn payload_accessors_round_trip_parser_visible_values() {
        let p = BootstrapParser::from_source("let name = \"gradient\"", 0);
        assert_eq!(bootstrap_token_list_get_text(&p.store, p.handle, 1), "name");
        assert_eq!(
            bootstrap_token_list_get_text(&p.store, p.handle, 3),
            "gradient"
        );

        let q = BootstrapParser::from_source("ret 42", 0);
        assert_eq!(
            bootstrap_token_list_get_int_value(&q.store, q.handle, 1),
            42
        );
        assert_eq!(q.peek_token(1).kind, TokenKind::IntLit(42));
    }

    #[test]
    fn negative_index_is_eof_safe() {
        let p = BootstrapParser::from_source("x", 0);
        let tag = bootstrap_token_list_get_kind(&p.store, p.handle, -1);
        assert_eq!(tag, EOF_KIND_TAG);
    }

    #[test]
    fn tag_round_trips_for_keywords_and_punctuation() {
        for kind in [
            TokenKind::Fn,
            TokenKind::Let,
            TokenKind::Match,
            TokenKind::Plus,
            TokenKind::Arrow,
            TokenKind::LParen,
            TokenKind::RBrace,
            TokenKind::Dot,
            TokenKind::Pub,
        ] {
            let tag = token_kind_tag(&kind);
            let back = token_kind_from_tag(tag);
            assert_eq!(
                std::mem::discriminant(&kind),
                std::mem::discriminant(&back),
                "kind {:?} did not round-trip via tag {}",
                kind,
                tag
            );
        }
    }
}
