//! IR-level type definitions for the Gradient compiler.
//!
//! These types represent the intermediate representation that sits between
//! the frontend AST and the backend Cranelift IR. The IR is designed to be:
//!   - SSA-based (every value is assigned exactly once)
//!   - Target-independent (no machine-specific details)
//!   - Simple enough to lower directly to Cranelift IR
//!
//! # Current status
//! These are placeholder definitions for the PoC. They will be fleshed out as
//! the frontend matures and begins producing real IR.

/// A unique identifier for an SSA value within a function.
///
/// Values are produced by instructions and consumed by other instructions.
/// In the future, this will likely become a newtype index into a value table
/// for efficient storage and lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Value(pub u32);

/// A reference to a function (local or external).
///
/// Used in `Call` instructions to identify the callee. Will eventually
/// map to entries in the module's function table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncRef(pub u32);

/// A reference to a basic block within a function.
///
/// Used in branch/jump instructions to identify control flow targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockRef(pub u32);

/// IR-level types for the Gradient language.
///
/// These map to the type system exposed by the language, but at a lower level
/// suitable for code generation. Cranelift has its own type system, and the
/// codegen layer will translate between the two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// 32-bit signed integer
    I32,
    /// 64-bit signed integer
    I64,
    /// Pointer type (size depends on target platform)
    Ptr,
    /// Void / unit type (no value)
    Void,
    /// Boolean (represented as i8 in codegen)
    Bool,
    /// 64-bit floating point
    F64,
    // Future additions:
    // Str — string slice (ptr + len)
    // Array(Box<Type>, usize) — fixed-size array
    // Struct(Vec<(String, Type)>) — named struct
    // Func(Vec<Type>, Box<Type>) — function type
}

/// Comparison operations used in `Cmp` instructions.
///
/// These map directly to Cranelift's `IntCC` and `FloatCC` comparison codes
/// during code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// Equal
    Eq,
    /// Not equal
    Ne,
    /// Less than (signed)
    Lt,
    /// Less than or equal (signed)
    Le,
    /// Greater than (signed)
    Gt,
    /// Greater than or equal (signed)
    Ge,
}

/// A literal constant value that can appear in the IR.
///
/// Used by the `Const` instruction to materialize compile-time known values.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// Integer constant (64-bit to accommodate both i32 and i64)
    Int(i64),
    /// Floating-point constant
    Float(f64),
    /// Boolean constant
    Bool(bool),
    /// String constant (the actual bytes; will be placed in a data section)
    Str(String),
}
