//! Enhanced error recovery strategies for the Gradient parser.
//!
//! This module provides sophisticated error recovery strategies that go beyond
//! basic token skipping. It implements:
//!
//! 1. **Context-aware synchronization** - Skip to tokens appropriate for the
//!    current parsing context (statement, expression, top-level item)
//! 2. **Common error pattern detection** - Recognize and provide helpful
//!    messages for frequent mistakes (missing punctuation, wrong keywords)
//! 3. **Recovery suggestions** - Generate actionable fix suggestions for errors
//! 4. **Delimiter matching recovery** - Special handling for mismatched brackets,
//!    parens, and indentation
//!
//! # Recovery Strategies
//!
//! - **Statement-level recovery**: When a statement fails to parse, skip to
//!   the next statement starter or block end
//! - **Expression-level recovery**: Insert placeholder expressions to allow
//!   parsing to continue within an expression context
//! - **Delimiter recovery**: When delimiters mismatch, try to find the
//!   correct closing delimiter or skip to a synchronization point
//!
//! # Usage
//!
//! ```ignore
//! use gradient_compiler::parser::recovery::RecoveryStrategy;
//!
//! // In the parser, when an error occurs:
//! if let Err(err) = self.expect(TokenKind::Colon) {
//!     // Use context-aware recovery
//!     self.recover_with(RecoveryStrategy::Statement);
//! }
//! ```

pub use crate::parser::parser::Parser;
use crate::lexer::token::TokenKind;

/// The context in which error recovery is occurring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecoveryContext {
    /// Top-level module item parsing.
    TopLevel,
    /// Statement within a block.
    Statement,
    /// Expression parsing.
    Expression,
    /// Type expression parsing.
    Type,
    /// Pattern matching context.
    Pattern,
    /// Inside delimiters (parentheses, brackets, etc.).
    Delimiter,
}

/// Recovery strategy determining how to recover from parse errors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecoveryStrategy {
    /// Skip until we find a token that can start a statement in the current context.
    Statement,
    /// Skip until we find a token that can start an expression.
    Expression,
    /// Skip until we find a token that can start a type expression.
    Type,
    /// Skip until we find a delimiter token (closing bracket/paren).
    Delimiter,
    /// Use the default strategy based on current context.
    Default,
    /// Skip a single token only.
    Minimal,
}

/// Common error patterns with suggestions for fixes.
#[derive(Debug, Clone, PartialEq)]
pub struct CommonError {
    /// Description of the detected error.
    pub description: String,
    /// Suggested fix description.
    pub suggestion: String,
    /// The token that was expected instead.
    pub expected_token: Option<TokenKind>,
}

/// Token sets for different synchronization strategies.
pub mod sync_sets {
    use super::TokenKind;

    /// Tokens that can start a top-level item.
    pub const TOP_LEVEL_STARTERS: &[TokenKind] = &[
        TokenKind::Fn,
        TokenKind::Let,
        TokenKind::Type,
        TokenKind::Actor,
        TokenKind::Trait,
        TokenKind::Impl,
        TokenKind::Mod,
        TokenKind::Use,
        TokenKind::At, // Annotations
    ];

    /// Tokens that can start a statement.
    pub const STATEMENT_STARTERS: &[TokenKind] = &[
        TokenKind::Let,
        TokenKind::If,
        TokenKind::For,
        TokenKind::While,
        TokenKind::Match,
        TokenKind::Ret,
        TokenKind::True,
        TokenKind::False,
        TokenKind::Ident(String::new()), // Placeholder for any identifier
        TokenKind::IntLit(0),
        TokenKind::FloatLit(0.0),
        TokenKind::StringLit(String::new()),
        TokenKind::LParen,
        TokenKind::LBracket,
        TokenKind::LBrace,
    ];

    /// Tokens that can start an expression.
    pub const EXPRESSION_STARTERS: &[TokenKind] = &[
        TokenKind::If,
        TokenKind::For,
        TokenKind::While,
        TokenKind::Match,
        TokenKind::True,
        TokenKind::False,
        TokenKind::Ident(String::new()),
        TokenKind::IntLit(0),
        TokenKind::FloatLit(0.0),
        TokenKind::StringLit(String::new()),
        TokenKind::LParen,
        TokenKind::LBracket,
        TokenKind::LBrace,
        TokenKind::Pipe, // Closure
        TokenKind::Not,
        TokenKind::Minus,
    ];

    /// Tokens that represent strong synchronization points.
    pub const STRONG_SYNC: &[TokenKind] = &[TokenKind::Newline, TokenKind::Dedent, TokenKind::Eof];

    /// Tokens that can follow a statement.
    pub const STATEMENT_FOLLOW: &[TokenKind] = &[
        TokenKind::Newline,
        TokenKind::Dedent,
        TokenKind::Eof,
        TokenKind::Else,
    ];
}

/// Recognizable error patterns for intelligent recovery.
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorPattern {
    /// Missing semicolon or newline between statements.
    MissingSemicolon,
    /// Extra token that doesn't belong.
    ExtraToken,
    /// Wrong delimiter used (e.g., ] instead of )).
    WrongDelimiter {
        expected: TokenKind,
        actual: TokenKind,
    },
}

/// Compare token kinds by discriminant only (ignores payload data).
pub fn discriminant_eq(a: &TokenKind, b: &TokenKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::span::{Position, Span};
    use crate::lexer::token::Token;
    use crate::parser::ParseError;

    fn make_token(kind: TokenKind) -> Token {
        Token::new(
            kind,
            Span::new(0, Position::new(1, 1, 0), Position::new(1, 2, 1)),
        )
    }

    #[test]
    fn test_discriminant_eq() {
        let a = TokenKind::Ident("foo".into());
        let b = TokenKind::Ident("bar".into());
        let c = TokenKind::IntLit(42);

        assert!(discriminant_eq(&a, &b));
        assert!(!discriminant_eq(&a, &c));
    }

    #[test]
    fn test_sync_sets_contain_expected_tokens() {
        assert!(sync_sets::TOP_LEVEL_STARTERS.contains(&TokenKind::Fn));
        assert!(sync_sets::TOP_LEVEL_STARTERS.contains(&TokenKind::Let));
        assert!(sync_sets::STATEMENT_STARTERS.contains(&TokenKind::If));
        assert!(sync_sets::STATEMENT_STARTERS.contains(&TokenKind::Ret));
        assert!(sync_sets::EXPRESSION_STARTERS.contains(&TokenKind::Match));
    }

    #[test]
    fn test_error_patterns() {
        let err1 = ErrorPattern::MissingSemicolon;
        assert_eq!(err1, ErrorPattern::MissingSemicolon);

        let err2 = ErrorPattern::ExtraToken;
        assert_eq!(err2, ErrorPattern::ExtraToken);

        let err3 = ErrorPattern::WrongDelimiter {
            expected: TokenKind::RParen,
            actual: TokenKind::RBrace,
        };
        match err3 {
            ErrorPattern::WrongDelimiter { expected, actual } => {
                assert!(matches!(expected, TokenKind::RParen));
                assert!(matches!(actual, TokenKind::RBrace));
            }
            _ => panic!("Expected WrongDelimiter"),
        }
    }

    #[test]
    fn test_common_error_creation() {
        let error = CommonError {
            description: "Missing closing brace".into(),
            suggestion: "Add a '}' at the end".into(),
            expected_token: Some(TokenKind::RBrace),
        };

        assert_eq!(error.description, "Missing closing brace");
        assert_eq!(error.suggestion, "Add a '}' at the end");
        assert!(error.expected_token.is_some());
    }

    #[test]
    fn test_recovery_context_variants() {
        assert_eq!(RecoveryContext::TopLevel, RecoveryContext::TopLevel);
        assert_eq!(RecoveryContext::Statement, RecoveryContext::Statement);
        assert_eq!(RecoveryContext::Expression, RecoveryContext::Expression);
        assert_eq!(RecoveryContext::Type, RecoveryContext::Type);
        assert_eq!(RecoveryContext::Pattern, RecoveryContext::Pattern);
        assert_eq!(RecoveryContext::Delimiter, RecoveryContext::Delimiter);
    }

    #[test]
    fn test_recovery_strategy_variants() {
        assert_eq!(RecoveryStrategy::Statement, RecoveryStrategy::Statement);
        assert_eq!(RecoveryStrategy::Expression, RecoveryStrategy::Expression);
        assert_eq!(RecoveryStrategy::Type, RecoveryStrategy::Type);
        assert_eq!(RecoveryStrategy::Delimiter, RecoveryStrategy::Delimiter);
        assert_eq!(RecoveryStrategy::Default, RecoveryStrategy::Default);
        assert_eq!(RecoveryStrategy::Minimal, RecoveryStrategy::Minimal);
    }

    #[test]
    fn test_sync_sets_dont_contain_invalid_tokens() {
        // These should NOT be in the statement starters
        assert!(!sync_sets::STATEMENT_STARTERS.contains(&TokenKind::RParen));
        assert!(!sync_sets::STATEMENT_STARTERS.contains(&TokenKind::RBrace));
        assert!(!sync_sets::STATEMENT_STARTERS.contains(&TokenKind::Comma));

        // These should NOT be in top-level starters
        assert!(!sync_sets::TOP_LEVEL_STARTERS.contains(&TokenKind::If));
        assert!(!sync_sets::TOP_LEVEL_STARTERS.contains(&TokenKind::Ret));
    }

    #[test]
    fn test_strong_sync_tokens() {
        assert_eq!(sync_sets::STRONG_SYNC.len(), 3);
        assert!(sync_sets::STRONG_SYNC.contains(&TokenKind::Newline));
        assert!(sync_sets::STRONG_SYNC.contains(&TokenKind::Dedent));
        assert!(sync_sets::STRONG_SYNC.contains(&TokenKind::Eof));
    }

    #[test]
    fn test_statement_follow_tokens() {
        assert!(sync_sets::STATEMENT_FOLLOW.contains(&TokenKind::Newline));
        assert!(sync_sets::STATEMENT_FOLLOW.contains(&TokenKind::Dedent));
        assert!(sync_sets::STATEMENT_FOLLOW.contains(&TokenKind::Eof));
        assert!(sync_sets::STATEMENT_FOLLOW.contains(&TokenKind::Else));
    }
}
