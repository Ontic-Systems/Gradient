//! The Gradient compiler library.
//!
//! This crate is designed as a **library first, binary second**. The primary
//! interface for AI agents and tools is the [`query`] module, which provides
//! structured, JSON-serializable access to all compiler information.
//!
//! # For agents: the query API
//!
//! ```rust
//! use gradient_compiler::query::Session;
//!
//! let session = Session::from_source("fn add(a: Int, b: Int) -> Int:\n    a + b\n");
//!
//! // Structured diagnostics
//! let result = session.check();
//! assert!(result.is_ok());
//!
//! // Module contract (compact API summary)
//! let contract = session.module_contract();
//! println!("{}", contract.to_json());
//!
//! // Symbol table
//! let symbols = session.symbols();
//! ```
//!
//! # Internal modules
//!
//! - [`lexer`] — Tokenisation with indentation tracking
//! - [`parser`] — Recursive-descent parser with error recovery
//! - [`typechecker`] — Type inference, checking, and effect validation
//! - [`ir`] — SSA-based intermediate representation
//! - [`codegen`] — Cranelift-based native code generation
//! - [`query`] — **Structured query API** (the primary agent interface)
//! - [`comptime`] — Compile-time expression evaluator

pub mod agent;
pub mod ast;
pub mod backend;
pub mod codegen;
/// Compile-time expression evaluation.
pub mod comptime;
/// Context budget API for AI agent resource management.
pub mod context_budget;
pub mod fmt;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod query;
pub mod repl;
pub mod resolve;
/// SMT-based contract verification (requires `smt-verify` feature).
#[cfg(feature = "smt-verify")]
pub mod smt;
pub mod typechecker;

// Re-export commonly used types and functions for convenience
pub use lexer::{token::Token, Lexer};
pub use parser::parse;
pub use typechecker::check_module;

/// Compile a Gradient source file through the full pipeline.
/// Returns the compiled object bytes or an error message.
pub fn compile(source: &str, file_id: u32) -> Result<Vec<u8>, String> {
    // Lex
    let mut lexer = Lexer::new(source, file_id);
    let tokens = lexer.tokenize();

    // Parse
    let (ast_module, parse_errors) = parser::parse(tokens, file_id);
    if !parse_errors.is_empty() {
        return Err(format!("parse errors: {:?}", parse_errors));
    }

    // Type check
    let type_errors = typechecker::check_module(&ast_module, file_id);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    if !real_errors.is_empty() {
        return Err(format!("type errors: {:?}", real_errors));
    }

    // Generate IR
    let (ir_module, ir_errors) = ir::builder::IrBuilder::build_module(&ast_module);
    if !ir_errors.is_empty() {
        return Err(format!("IR build errors: {:?}", ir_errors));
    }

    // Codegen
    let mut codegen =
        codegen::CraneliftCodegen::new().map_err(|e| format!("codegen init error: {}", e))?;
    codegen
        .compile_module(&ir_module)
        .map_err(|e| format!("codegen error: {}", e))?;

    codegen.emit_bytes()
}

/// Parse a Gradient source file and return the AST and any parse errors.
pub fn parse_source(source: &str, file_id: u32) -> (ast::module::Module, Vec<parser::ParseError>) {
    let mut lexer = Lexer::new(source, file_id);
    let tokens = lexer.tokenize();
    parser::parse(tokens, file_id)
}

/// Type-check a parsed module and return any type errors.
pub fn typecheck_module(module: &ast::module::Module, file_id: u32) -> Vec<typechecker::TypeError> {
    typechecker::check_module(module, file_id)
}

/// Generate IR from a parsed and type-checked module.
pub fn generate_ir(module: &ast::module::Module, _file_id: u32) -> Result<ir::Module, String> {
    let (ir_module, ir_errors) = ir::builder::IrBuilder::build_module(module);
    if !ir_errors.is_empty() {
        return Err(format!("IR build errors: {:?}", ir_errors));
    }
    Ok(ir_module)
}
