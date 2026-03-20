//! Token and span types for the Gradient lexer.
//!
//! Every token produced by the lexer carries a [`Span`] recording the precise
//! source location where it appeared. The [`TokenKind`] enum enumerates every
//! lexical element of the Gradient language, including keywords, operators,
//! literals, indentation markers, and error tokens for recovery.

use std::fmt;

// ---------------------------------------------------------------------------
// Span types (local definitions — will be unified with ast::span later)
// ---------------------------------------------------------------------------

/// A position within a single source file.
///
/// Lines and columns are 1-based; `offset` is 0-based byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number.
    pub col: u32,
    /// 0-based byte offset from the start of the file.
    pub offset: u32,
}

impl Position {
    /// Create a new source position.
    pub fn new(line: u32, col: u32, offset: u32) -> Self {
        Self { line, col, offset }
    }
}

/// A contiguous region of source text within a single file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Index into the compiler's file table.
    pub file_id: u32,
    /// The position of the first byte covered by this span.
    pub start: Position,
    /// The position one past the last byte covered by this span.
    pub end: Position,
}

impl Span {
    /// Create a new span from start to end in the given file.
    pub fn new(file_id: u32, start: Position, end: Position) -> Self {
        Self { file_id, start, end }
    }

    /// Create a zero-width span at a single position.
    pub fn point(file_id: u32, pos: Position) -> Self {
        Self {
            file_id,
            start: pos,
            end: pos,
        }
    }
}

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
    If,
    Else,
    For,
    In,
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
    Comma,
    Colon,
    Arrow,
    Dot,
    At,
    Bang,
    Question,

    // Indentation
    Indent,
    Dedent,
    Newline,

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
            TokenKind::If => write!(f, "if"),
            TokenKind::Else => write!(f, "else"),
            TokenKind::For => write!(f, "for"),
            TokenKind::In => write!(f, "in"),
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
            TokenKind::Comma => write!(f, ","),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Arrow => write!(f, "->"),
            TokenKind::Dot => write!(f, "."),
            TokenKind::At => write!(f, "@"),
            TokenKind::Bang => write!(f, "!"),
            TokenKind::Question => write!(f, "?"),

            // Indentation
            TokenKind::Indent => write!(f, "INDENT"),
            TokenKind::Dedent => write!(f, "DEDENT"),
            TokenKind::Newline => write!(f, "NEWLINE"),

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
        "if" => Some(TokenKind::If),
        "else" => Some(TokenKind::Else),
        "for" => Some(TokenKind::For),
        "in" => Some(TokenKind::In),
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
        _ => None,
    }
}
