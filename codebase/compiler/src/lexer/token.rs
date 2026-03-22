//! Token types for the Gradient lexer.
//!
//! Every token produced by the lexer carries a [`Span`] recording the precise
//! source location where it appeared. The [`TokenKind`] enum enumerates every
//! lexical element of the Gradient language, including keywords, operators,
//! literals, indentation markers, and error tokens for recovery.

use std::fmt;

pub use crate::ast::span::{Position, Span};

// ---------------------------------------------------------------------------
// Token
// ---------------------------------------------------------------------------

/// A single lexical token together with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    /// Convenience constructor.
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

// ---------------------------------------------------------------------------
// TokenKind
// ---------------------------------------------------------------------------

/// The kind of a lexical token.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    Fn,
    Let,
    Mut,
    If,
    Else,
    For,
    In,
    While,
    Ret,
    Type,
    Mod,
    Use,
    Impl,
    Match,
    True,
    False,
    And,
    Or,
    Not,
    Actor,
    State,
    On,
    Spawn,
    Send,
    Ask,

    // Literals
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),

    // Identifiers
    Ident(String),

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Assign,

    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Arrow,
    Dot,
    At,
    Bang,
    Question,
    Pipe,

    // Indentation
    Indent,
    Dedent,
    Newline,

    // Documentation
    /// A `///` doc comment. The content is the text after `/// ` (leading
    /// space stripped), without the trailing newline.
    DocComment(String),

    // Special
    Eof,

    // Error recovery
    Error(String),
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Keywords
            TokenKind::Fn => write!(f, "fn"),
            TokenKind::Let => write!(f, "let"),
            TokenKind::Mut => write!(f, "mut"),
            TokenKind::If => write!(f, "if"),
            TokenKind::Else => write!(f, "else"),
            TokenKind::For => write!(f, "for"),
            TokenKind::In => write!(f, "in"),
            TokenKind::While => write!(f, "while"),
            TokenKind::Ret => write!(f, "ret"),
            TokenKind::Type => write!(f, "type"),
            TokenKind::Mod => write!(f, "mod"),
            TokenKind::Use => write!(f, "use"),
            TokenKind::Impl => write!(f, "impl"),
            TokenKind::Match => write!(f, "match"),
            TokenKind::True => write!(f, "true"),
            TokenKind::False => write!(f, "false"),
            TokenKind::And => write!(f, "and"),
            TokenKind::Or => write!(f, "or"),
            TokenKind::Not => write!(f, "not"),
            TokenKind::Actor => write!(f, "actor"),
            TokenKind::State => write!(f, "state"),
            TokenKind::On => write!(f, "on"),
            TokenKind::Spawn => write!(f, "spawn"),
            TokenKind::Send => write!(f, "send"),
            TokenKind::Ask => write!(f, "ask"),

            // Literals
            TokenKind::IntLit(n) => write!(f, "{}", n),
            TokenKind::FloatLit(n) => write!(f, "{}", n),
            TokenKind::StringLit(s) => write!(f, "\"{}\"", s),

            // Identifiers
            TokenKind::Ident(name) => write!(f, "{}", name),

            // Operators
            TokenKind::Plus => write!(f, "+"),
            TokenKind::Minus => write!(f, "-"),
            TokenKind::Star => write!(f, "*"),
            TokenKind::Slash => write!(f, "/"),
            TokenKind::Percent => write!(f, "%"),
            TokenKind::Eq => write!(f, "=="),
            TokenKind::Ne => write!(f, "!="),
            TokenKind::Lt => write!(f, "<"),
            TokenKind::Gt => write!(f, ">"),
            TokenKind::Le => write!(f, "<="),
            TokenKind::Ge => write!(f, ">="),
            TokenKind::Assign => write!(f, "="),

            // Punctuation
            TokenKind::LParen => write!(f, "("),
            TokenKind::RParen => write!(f, ")"),
            TokenKind::LBrace => write!(f, "{{"),
            TokenKind::RBrace => write!(f, "}}"),
            TokenKind::LBracket => write!(f, "["),
            TokenKind::RBracket => write!(f, "]"),
            TokenKind::Comma => write!(f, ","),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Arrow => write!(f, "->"),
            TokenKind::Dot => write!(f, "."),
            TokenKind::At => write!(f, "@"),
            TokenKind::Bang => write!(f, "!"),
            TokenKind::Question => write!(f, "?"),
            TokenKind::Pipe => write!(f, "|"),

            // Indentation
            TokenKind::Indent => write!(f, "INDENT"),
            TokenKind::Dedent => write!(f, "DEDENT"),
            TokenKind::Newline => write!(f, "NEWLINE"),

            // Documentation
            TokenKind::DocComment(text) => write!(f, "/// {}", text),

            // Special
            TokenKind::Eof => write!(f, "EOF"),

            // Error
            TokenKind::Error(msg) => write!(f, "error: {}", msg),
        }
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} @ {}:{}-{}:{}",
            self.kind,
            self.span.start.line,
            self.span.start.col,
            self.span.end.line,
            self.span.end.col,
        )
    }
}

/// Look up a keyword from an identifier string. Returns `None` if the
/// string is not a keyword.
pub fn keyword_from_str(s: &str) -> Option<TokenKind> {
    match s {
        "fn" => Some(TokenKind::Fn),
        "let" => Some(TokenKind::Let),
        "mut" => Some(TokenKind::Mut),
        "if" => Some(TokenKind::If),
        "else" => Some(TokenKind::Else),
        "for" => Some(TokenKind::For),
        "in" => Some(TokenKind::In),
        "while" => Some(TokenKind::While),
        "ret" => Some(TokenKind::Ret),
        "type" => Some(TokenKind::Type),
        "mod" => Some(TokenKind::Mod),
        "use" => Some(TokenKind::Use),
        "impl" => Some(TokenKind::Impl),
        "match" => Some(TokenKind::Match),
        "true" => Some(TokenKind::True),
        "false" => Some(TokenKind::False),
        "and" => Some(TokenKind::And),
        "or" => Some(TokenKind::Or),
        "not" => Some(TokenKind::Not),
        "actor" => Some(TokenKind::Actor),
        "state" => Some(TokenKind::State),
        "on" => Some(TokenKind::On),
        "spawn" => Some(TokenKind::Spawn),
        "send" => Some(TokenKind::Send),
        "ask" => Some(TokenKind::Ask),
        _ => None,
    }
}
