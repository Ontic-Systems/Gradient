//! Lexer module for the Gradient programming language.
//!
//! This module provides the first stage of the compilation pipeline:
//! tokenization of `.gr` source code into a stream of [`Token`]s.
//!
//! # Features
//!
//! - Hand-written lexer (no generated code)
//! - Python-style INDENT / DEDENT injection
//! - Full span tracking on every token
//! - Error recovery via [`TokenKind::Error`] tokens (no panics)
//!
//! # Usage
//!
//! ```ignore
//! use gradient_compiler::lexer::{Lexer, Token, TokenKind};
//!
//! let source = "fn main():\n    ret 0\n";
//! let mut lexer = Lexer::new(source, 0);
//! let tokens = lexer.tokenize();
//! for tok in &tokens {
//!     println!("{}", tok);
//! }
//! ```

#[allow(clippy::module_inception)]
mod lexer;
pub mod token;

#[cfg(test)]
mod tests;

pub use lexer::Lexer;
pub use token::{keyword_from_str, Position, Span, Token, TokenKind};
