//! IR instruction definitions for the Gradient compiler.
//!
//! Each instruction operates on SSA [`Value`]s and produces zero or one result
//! value. Instructions are grouped into basic blocks, which form the control
//! flow graph of a function.
//!
//! # Design notes
//!
//! The instruction set is intentionally minimal — just enough to express the
//! semantics of Gradient programs. Higher-level constructs (loops, match
//! expressions, closures) are lowered into these primitives by the IR builder.
//!
//! During codegen, each `Instruction` variant maps to one or more Cranelift IR
//! instructions. The mapping is straightforward for most variants; the `Phi`
//! node is the exception, as Cranelift uses block parameters instead of phi
//! nodes (the codegen layer handles this translation).

use super::types::{BlockRef, CmpOp, FuncRef, Literal, Type, Value};

/// An IR instruction in the Gradient intermediate representation.
///
/// Instructions are the atomic units of computation. Each variant documents
/// its operands and the value (if any) it produces.
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// Load a compile-time constant into a value.
    ///
    /// `Const(result, literal)` — materializes `literal` as `result`.
    /// For string literals, this produces a pointer to the string data
    /// in the object file's read-only data section.
    Const(Value, Literal),

    /// Call a function with the given arguments.
    ///
    /// `Call(result, func, args)` — calls `func` with `args`, binding the
    /// return value to `result`. For void functions, `result` is unused.
    ///
    /// In the codegen layer, this becomes:
    ///   1. A Cranelift `call` instruction for direct calls
    ///   2. Arguments are passed according to the platform calling convention
    Call(Value, FuncRef, Vec<Value>),

    /// Return from the current function.
    ///
    /// `Ret(Some(value))` — return `value` to the caller.
    /// `Ret(None)` — return void.
    Ret(Option<Value>),

    /// Integer addition.
    ///
    /// `Add(result, lhs, rhs)` — `result = lhs + rhs`.
    /// Maps to Cranelift's `iadd` instruction.
    Add(Value, Value, Value),

    /// Integer subtraction.
    ///
    /// `Sub(result, lhs, rhs)` — `result = lhs - rhs`.
    /// Maps to Cranelift's `isub` instruction.
    Sub(Value, Value, Value),

    /// Integer multiplication.
    ///
    /// `Mul(result, lhs, rhs)` — `result = lhs * rhs`.
    /// Maps to Cranelift's `imul` instruction.
    Mul(Value, Value, Value),

    /// Integer division (signed).
    ///
    /// `Div(result, lhs, rhs)` — `result = lhs / rhs`.
    /// Maps to Cranelift's `sdiv` instruction.
    Div(Value, Value, Value),

    /// Comparison.
    ///
    /// `Cmp(result, op, lhs, rhs)` — `result = lhs op rhs` (boolean).
    /// Maps to Cranelift's `icmp` instruction with the appropriate `IntCC`.
    Cmp(Value, CmpOp, Value, Value),

    /// Conditional branch.
    ///
    /// `Branch(cond, then_block, else_block)` — if `cond` is true, jump to
    /// `then_block`; otherwise jump to `else_block`.
    /// Maps to Cranelift's `brif` instruction.
    Branch(Value, BlockRef, BlockRef),

    /// Unconditional jump.
    ///
    /// `Jump(target)` — transfer control to `target`.
    /// Maps to Cranelift's `jump` instruction.
    Jump(BlockRef),

    /// SSA phi node.
    ///
    /// `Phi(result, [(block, value), ...])` — merges values from predecessor
    /// blocks. `result` takes the value corresponding to whichever predecessor
    /// block was executed.
    ///
    /// **Note:** Cranelift does not use phi nodes — it uses block parameters
    /// instead. The codegen layer must translate phi nodes into block parameters
    /// during lowering. This is a well-understood transformation:
    ///   1. For each phi, add a block parameter to the target block
    ///   2. For each predecessor, pass the appropriate value as a branch argument
    Phi(Value, Vec<(BlockRef, Value)>),

    /// Stack allocation.
    ///
    /// `Alloca(result, ty)` — allocates space for a value of type `ty` on the
    /// stack and returns a pointer to it in `result`.
    /// Maps to Cranelift's `stack_slot` mechanism (StackSlotData).
    Alloca(Value, Type),

    /// Memory load.
    ///
    /// `Load(result, addr)` — loads a value from memory at `addr` into `result`.
    /// Maps to Cranelift's `load` instruction.
    Load(Value, Value),

    /// Memory store.
    ///
    /// `Store(value, addr)` — stores `value` into memory at `addr`.
    /// Maps to Cranelift's `store` instruction.
    Store(Value, Value),
}
