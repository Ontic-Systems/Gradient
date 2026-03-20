//! Recursive descent parser for the Gradient programming language.
//!
//! This module consumes a flat stream of tokens (produced by the lexer) and
//! produces a typed AST rooted at [`Module`](crate::ast::module::Module).
//! The parser implements error recovery: even when syntax errors are present,
//! it returns a partial AST together with a list of diagnostics so that
//! downstream tools (editors, linters) can still operate on the valid
//! portions of the source.
//!
//! # Usage
//!
//! ```ignore
//! use gradient_compiler::parser::{Parser, parse};
//! use gradient_compiler::lexer::token::Token;
//!
//! let tokens: Vec<Token> = /* ... */;
//! let (module, errors) = parse(tokens, /*file_id=*/ 0);
//! if errors.is_empty() {
//!     // proceed with type checking
//! } else {
//!     for e in &errors {
//!         eprintln!("{}", e);
//!     }
//! }
//! ```

pub mod error;
#[allow(clippy::module_inception)]
pub mod parser;

#[cfg(test)]
mod tests;

pub use error::ParseError;
pub use parser::Parser;

use crate::ast::module::Module;
use crate::lexer::token::Token;

/// Convenience entry point: parse a token stream into a module AST.
///
/// This is a thin wrapper around [`Parser::parse`].
pub fn parse(tokens: Vec<Token>, file_id: u32) -> (Module, Vec<ParseError>) {
    Parser::parse(tokens, file_id)
}
