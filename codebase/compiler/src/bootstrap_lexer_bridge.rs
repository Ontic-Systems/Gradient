//! Bridge that mirrors `compiler/lexer.gr::tokenize` over the runtime-backed
//! [`BootstrapCollectionStore`].
//!
//! Issue #220: until the self-hosted runtime can execute `lexer.gr` directly,
//! we model the same tokenize algorithm in Rust so it can drive — and be
//! verified against — the host-side bootstrap collection store. This proves
//! that the rewritten `tokenize` will emit a non-empty, runtime-backed
//! `TokenList` and gives downstream parser/IR work a stable substrate to
//! consume real token streams from self-hosted code.
//!
//! The bridge intentionally mirrors the *current* `compiler/lexer.gr` scanner
//! semantics — single-character whitespace skipping, no INDENT/DEDENT, no
//! float literal parsing — rather than the richer Rust lexer. Closing the
//! whitespace/indent gap is tracked under follow-up self-hosting issues.
//!
//! This module is `#[cfg(any(test, feature = "bootstrap-bridge"))]`-friendly
//! but kept always-compiled to avoid feature-flag fragmentation while the
//! self-hosting initiative is active. It has no runtime cost when unused.

use crate::bootstrap_collections::{
    BootstrapCollectionKind, BootstrapCollectionStore, BootstrapHandle,
};
use crate::lexer::token::{Position, Span, Token, TokenKind};

/// A handle plus its backing store, returned by [`tokenize_via_bootstrap_store`].
pub struct BootstrapTokenList {
    pub store: BootstrapCollectionStore<Token>,
    pub handle: BootstrapHandle<Token>,
}

impl BootstrapTokenList {
    pub fn len(&self) -> usize {
        self.store.len(self.handle).expect("bootstrap len")
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> &Token {
        self.store.get(self.handle, index).expect("bootstrap get")
    }

    pub fn iter(&self) -> impl Iterator<Item = &Token> + '_ {
        let len = self.len();
        (0..len).map(move |i| self.get(i))
    }

    pub fn kinds(&self) -> Vec<TokenKind> {
        self.iter().map(|t| t.kind.clone()).collect()
    }
}

#[derive(Debug, Clone)]
struct LexerState<'src> {
    source: &'src [u8],
    file_id: u32,
    pos: usize,
    line: u32,
    col: u32,
}

impl<'src> LexerState<'src> {
    fn new(source: &'src str, file_id: u32) -> Self {
        Self {
            source: source.as_bytes(),
            file_id,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn current_char(&self) -> i32 {
        if self.pos >= self.source.len() {
            -1
        } else {
            self.source[self.pos] as i32
        }
    }

    fn peek_char(&self, offset: usize) -> i32 {
        let idx = self.pos + offset;
        if idx >= self.source.len() {
            -1
        } else {
            self.source[idx] as i32
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.source.len()
    }

    fn current_position(&self) -> Position {
        Position {
            line: self.line,
            col: self.col,
            offset: self.pos as u32,
        }
    }

    fn advance(&mut self) {
        let ch = self.current_char();
        self.pos += 1;
        self.col += 1;
        if ch == 10 {
            self.line += 1;
            self.col = 1;
        }
    }

    fn substring(&self, start: usize, end: usize) -> String {
        std::str::from_utf8(&self.source[start..end])
            .expect("ascii bootstrap corpus")
            .to_string()
    }
}

fn is_digit(ch: i32) -> bool {
    (48..=57).contains(&ch)
}

fn is_ident_start(ch: i32) -> bool {
    ch == 95 || (65..=90).contains(&ch) || (97..=122).contains(&ch)
}

fn is_ident_continue(ch: i32) -> bool {
    is_ident_start(ch) || is_digit(ch)
}

fn is_whitespace(ch: i32) -> bool {
    ch == 32 || ch == 9 || ch == 13
}

fn lookup_keyword(name: &str) -> TokenKind {
    match name {
        "fn" => TokenKind::Fn,
        "let" => TokenKind::Let,
        "mut" => TokenKind::Mut,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "for" => TokenKind::For,
        "in" => TokenKind::In,
        "while" => TokenKind::While,
        "ret" => TokenKind::Ret,
        "type" => TokenKind::Type,
        "mod" => TokenKind::Mod,
        "use" => TokenKind::Use,
        "impl" => TokenKind::Impl,
        "match" => TokenKind::Match,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "and" => TokenKind::And,
        "or" => TokenKind::Or,
        "not" => TokenKind::Not,
        // The self-hosted token.gr has TokenKind::Extern and TokenKind::Semi
        // variants that the Rust reference TokenKind does not surface (the
        // Rust lexer treats `extern` as a regular Ident in this bootstrap
        // subset and rejects `;` outright). The bridge mirrors that
        // observable behaviour rather than introducing new variants.
        "pub" => TokenKind::Pub,
        _ => TokenKind::Ident(name.to_string()),
    }
}

fn skip_whitespace(lex: &mut LexerState<'_>) {
    while is_whitespace(lex.current_char()) {
        lex.advance();
    }
}

fn read_identifier(lex: &mut LexerState<'_>) -> TokenKind {
    let start = lex.pos;
    while is_ident_continue(lex.current_char()) {
        lex.advance();
    }
    let end = lex.pos;
    let name = lex.substring(start, end);
    lookup_keyword(&name)
}

fn read_number(lex: &mut LexerState<'_>) -> TokenKind {
    while is_digit(lex.current_char()) {
        lex.advance();
    }
    if lex.current_char() == 46 && is_digit(lex.peek_char(1)) {
        lex.advance();
        while is_digit(lex.current_char()) {
            lex.advance();
        }
    }
    // Mirror lexer.gr: numeric value parsing is deferred (#220 scope keeps the
    // current self-hosted contract — IntLit(0) — until float/int literal
    // primitives are added in a follow-up issue).
    TokenKind::IntLit(0)
}

fn read_string(lex: &mut LexerState<'_>) -> TokenKind {
    lex.advance(); // opening quote
    let value_start = lex.pos;
    while !lex.is_eof() {
        let ch = lex.current_char();
        if ch == 34 {
            break;
        } else if ch == 92 {
            lex.advance();
            if !lex.is_eof() {
                lex.advance();
            }
        } else {
            lex.advance();
        }
    }
    let value_end = lex.pos;
    let value = lex.substring(value_start, value_end);
    if lex.current_char() == 34 {
        lex.advance();
    }
    TokenKind::StringLit(value)
}

fn next_token(lex: &mut LexerState<'_>) -> Token {
    loop {
        skip_whitespace(lex);
        let start_pos = lex.current_position();
        let ch = lex.current_char();

        if lex.is_eof() {
            let span = Span {
                file_id: lex.file_id,
                start: start_pos,
                end: start_pos,
            };
            return Token::new(TokenKind::Eof, span);
        }

        if ch == 47 {
            let next = lex.peek_char(1);
            if next == 47 {
                lex.advance();
                lex.advance();
                while !lex.is_eof() && lex.current_char() != 10 {
                    lex.advance();
                }
                continue;
            }
        }

        if is_ident_start(ch) {
            let kind = read_identifier(lex);
            let end_pos = lex.current_position();
            let span = Span {
                file_id: lex.file_id,
                start: start_pos,
                end: end_pos,
            };
            return Token::new(kind, span);
        }

        if is_digit(ch) {
            let kind = read_number(lex);
            let end_pos = lex.current_position();
            let span = Span {
                file_id: lex.file_id,
                start: start_pos,
                end: end_pos,
            };
            return Token::new(kind, span);
        }

        if ch == 34 {
            let kind = read_string(lex);
            let end_pos = lex.current_position();
            let span = Span {
                file_id: lex.file_id,
                start: start_pos,
                end: end_pos,
            };
            return Token::new(kind, span);
        }

        // Single-character / two-character operators and punctuation.
        lex.advance();
        let mut end_pos = lex.current_position();
        let mut span = Span {
            file_id: lex.file_id,
            start: start_pos,
            end: end_pos,
        };

        let kind = match ch {
            43 => TokenKind::Plus,
            45 => {
                if lex.current_char() == 62 {
                    lex.advance();
                    end_pos = lex.current_position();
                    span = Span {
                        file_id: lex.file_id,
                        start: start_pos,
                        end: end_pos,
                    };
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            42 => TokenKind::Star,
            47 => TokenKind::Slash,
            37 => TokenKind::Percent,
            40 => TokenKind::LParen,
            41 => TokenKind::RParen,
            123 => TokenKind::LBrace,
            125 => TokenKind::RBrace,
            91 => TokenKind::LBracket,
            93 => TokenKind::RBracket,
            58 => TokenKind::Colon,
            44 => TokenKind::Comma,
            59 => TokenKind::Error("Unexpected character: ;".to_string()),
            46 => TokenKind::Dot,
            61 => {
                if lex.current_char() == 61 {
                    lex.advance();
                    end_pos = lex.current_position();
                    span = Span {
                        file_id: lex.file_id,
                        start: start_pos,
                        end: end_pos,
                    };
                    TokenKind::Eq
                } else {
                    TokenKind::Assign
                }
            }
            33 => {
                if lex.current_char() == 61 {
                    lex.advance();
                    end_pos = lex.current_position();
                    span = Span {
                        file_id: lex.file_id,
                        start: start_pos,
                        end: end_pos,
                    };
                    TokenKind::Ne
                } else {
                    TokenKind::Error("Unexpected character: !".to_string())
                }
            }
            60 => {
                if lex.current_char() == 61 {
                    lex.advance();
                    end_pos = lex.current_position();
                    span = Span {
                        file_id: lex.file_id,
                        start: start_pos,
                        end: end_pos,
                    };
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            62 => {
                if lex.current_char() == 61 {
                    lex.advance();
                    end_pos = lex.current_position();
                    span = Span {
                        file_id: lex.file_id,
                        start: start_pos,
                        end: end_pos,
                    };
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            _ => TokenKind::Error("Unexpected character".to_string()),
        };

        return Token::new(kind, span);
    }
}

/// Mirror of `compiler/lexer.gr::tokenize` driven through the bootstrap
/// collection store. Allocates a non-zero `TokenList` handle, appends every
/// token produced by the scanner, and finally appends a trailing `Eof`.
///
/// This is the executable counterpart to the rewritten `tokenize` body in
/// `compiler/lexer.gr` and the basis for #220's parity check.
pub fn tokenize_via_bootstrap_store(source: &str, file_id: u32) -> BootstrapTokenList {
    let mut store = BootstrapCollectionStore::<Token>::new();
    let handle = store.alloc(BootstrapCollectionKind::TokenList);

    let mut lex = LexerState::new(source, file_id);
    while !lex.is_eof() {
        let tok = next_token(&mut lex);
        // Mirror lexer.gr: stop accumulating non-EOF tokens once next_token
        // synthesizes an Eof (it does so when start_pos hits EOF before any
        // real character is consumed, which only happens here when source is
        // empty — guarded by the outer is_eof() loop, but kept for safety).
        if matches!(tok.kind, TokenKind::Eof) {
            break;
        }
        store
            .append(handle, tok)
            .expect("bootstrap append (scanner token)");
    }

    let eof_pos = lex.current_position();
    let eof_span = Span {
        file_id: lex.file_id,
        start: eof_pos,
        end: eof_pos,
    };
    store
        .append(handle, Token::new(TokenKind::Eof, eof_span))
        .expect("bootstrap append (eof)");

    BootstrapTokenList { store, handle }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize_via_bootstrap_store(src, 0).kinds()
    }

    #[test]
    fn empty_source_emits_only_eof() {
        let list = tokenize_via_bootstrap_store("", 0);
        assert_eq!(list.len(), 1);
        assert!(matches!(list.get(0).kind, TokenKind::Eof));
    }

    #[test]
    fn simple_expression_accumulates_real_tokens() {
        let src = "x + 1";
        let ks = kinds(src);
        assert_eq!(ks.len(), 4, "x, +, IntLit, Eof");
        assert!(matches!(ks[0], TokenKind::Ident(ref n) if n == "x"));
        assert!(matches!(ks[1], TokenKind::Plus));
        assert!(matches!(ks[2], TokenKind::IntLit(_)));
        assert!(matches!(ks[3], TokenKind::Eof));
    }

    #[test]
    fn keywords_and_operators() {
        let src = "ret x == y";
        let ks = kinds(src);
        assert!(matches!(ks[0], TokenKind::Ret));
        assert!(matches!(ks[1], TokenKind::Ident(ref n) if n == "x"));
        assert!(matches!(ks[2], TokenKind::Eq));
        assert!(matches!(ks[3], TokenKind::Ident(ref n) if n == "y"));
        assert!(matches!(ks.last(), Some(TokenKind::Eof)));
    }

    #[test]
    fn arrow_token_is_two_chars() {
        let ks = kinds("-> x");
        assert!(matches!(ks[0], TokenKind::Arrow));
        assert!(matches!(ks[1], TokenKind::Ident(ref n) if n == "x"));
    }

    #[test]
    fn handle_is_non_zero_token_list() {
        let list = tokenize_via_bootstrap_store("a", 0);
        assert_ne!(list.handle.raw(), 0);
        assert_eq!(list.handle.kind(), BootstrapCollectionKind::TokenList);
    }

    #[test]
    fn line_comments_are_skipped() {
        // Self-hosted lexer.gr does not skip newlines, so the corpus here
        // stays single-line; the comment runs to end-of-string.
        let ks = kinds("x // trailing comment");
        assert!(matches!(ks[0], TokenKind::Ident(ref n) if n == "x"));
        assert!(matches!(ks[1], TokenKind::Eof));
    }

    #[test]
    fn append_order_is_preserved() {
        let list = tokenize_via_bootstrap_store("a b c", 0);
        let names: Vec<String> = list
            .iter()
            .filter_map(|t| match &t.kind {
                TokenKind::Ident(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }
}
