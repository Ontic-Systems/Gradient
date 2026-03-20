//! Gradient IR — the intermediate representation for the Gradient compiler.
//!
//! # Architecture
//!
//! The IR sits between the frontend (parser/typechecker) and the backend
//! (Cranelift codegen). It is a target-independent, SSA-based representation
//! that captures the semantics of a Gradient program in a form that is
//! straightforward to lower to machine code.
//!
//! ## Module structure
//!
//! - [`types`] — Core type definitions: `Value`, `Type`, `FuncRef`, `BlockRef`, etc.
//! - [`instruction`] — The `Instruction` enum with all IR operations.
//!
//! ## Key types
//!
//! - [`Module`] — A compilation unit containing functions and data.
//! - [`Function`] — A function definition with a signature and body.
//! - [`BasicBlock`] — An SSA basic block containing a sequence of instructions.
//! - [`Instruction`] — A single IR operation (see [`instruction::Instruction`]).
//!
//! # Current status
//!
//! The IR types are defined but not yet populated by the frontend. The PoC
//! bypasses the IR entirely, generating Cranelift IR directly. The next step
//! is to build an IR builder that the frontend can call, and then implement
//! the IR -> Cranelift IR translation in the codegen layer.

pub mod instruction;
pub mod types;

// Re-export commonly used types for convenience.
pub use instruction::Instruction;
pub use types::{BlockRef, CmpOp, FuncRef, Literal, Type, Value};

/// A compilation unit — the top-level container for a Gradient program.
///
/// A module contains:
/// - A set of function definitions (both user-defined and external declarations)
/// - Global data (string constants, static variables, etc.)
///
/// # Future work
/// - Track external function declarations separately from definitions
/// - Support global variable initializers
/// - Carry module-level metadata (source file, target triple, etc.)
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// The name of this module (typically derived from the source file).
    pub name: String,

    /// Functions defined in this module.
    pub functions: Vec<Function>,
    // Future:
    // pub externals: Vec<ExternalDecl>,
    // pub globals: Vec<GlobalData>,
}

/// A function definition in the Gradient IR.
///
/// Contains the function's signature (name, parameter types, return type)
/// and its body as a list of basic blocks forming the control flow graph.
///
/// # SSA invariants
/// - Every `Value` is defined exactly once (by exactly one instruction).
/// - The first block in `blocks` is the entry block.
/// - All predecessors of a block must dominate it (except for loop headers).
///
/// # Future work
/// - Store a proper `Signature` type with parameter names and attributes
/// - Track the dominator tree for optimization passes
/// - Support function attributes (inline, no_return, etc.)
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The function's linkage name (e.g., "main", "gradient_add").
    pub name: String,

    /// Parameter types, in order.
    pub params: Vec<Type>,

    /// Return type (`Type::Void` for functions that return nothing).
    pub return_type: Type,

    /// The function body as a list of basic blocks.
    /// The first block is always the entry point.
    pub blocks: Vec<BasicBlock>,
}

/// An SSA basic block — a straight-line sequence of instructions with
/// a single entry point and a single exit (the terminator instruction).
///
/// Basic blocks are the nodes in the function's control flow graph.
/// Edges between blocks are determined by branch/jump instructions.
///
/// # Invariants
/// - A block must end with exactly one terminator instruction
///   (`Ret`, `Branch`, or `Jump`).
/// - No terminator instruction may appear in the middle of a block.
///
/// # Future work
/// - Track predecessor blocks for efficient CFG traversal
/// - Support block parameters (Cranelift style) as an alternative to phi nodes
#[derive(Debug, Clone, PartialEq)]
pub struct BasicBlock {
    /// A unique label for this block within its function.
    pub label: BlockRef,

    /// The instructions in this block, in execution order.
    /// The last instruction must be a terminator.
    pub instructions: Vec<Instruction>,
}
