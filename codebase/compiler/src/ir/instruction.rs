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

    /// Boolean OR operation.
    ///
    /// `Or(result, lhs, rhs)` — `result = lhs || rhs` (boolean).
    /// Maps to Cranelift's `bor` instruction.
    Or(Value, Value, Value),

    /// Load field from object by index.
    ///
    /// `LoadField { result, object, field_idx }` — loads field at index from object.
    /// Used for enum tag extraction in pattern matching.
    LoadField {
        /// The SSA value that receives the loaded value.
        result: Value,
        /// The object pointer to load from.
        object: Value,
        /// The field index to load.
        field_idx: u32,
    },

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

    /// Pointer to integer cast.
    ///
    /// `PtrToInt(result, ptr)` — casts a pointer to an integer (i64).
    /// Used for pointer arithmetic before casting back to pointer.
    PtrToInt(Value, Value),

    /// Integer to pointer cast.
    ///
    /// `IntToPtr(result, int)` — casts an integer (i64) to a pointer.
    /// Used after pointer arithmetic to convert back to a pointer.
    IntToPtr(Value, Value),

    /// Get element pointer - GEP for field access.
    ///
    /// `GetElementPtr { result, base, offset, field_ty }` — computes address
    /// of a field at offset bytes from base pointer.
    GetElementPtr {
        /// The SSA value that receives the computed address.
        result: Value,
        /// The base pointer value.
        base: Value,
        /// The byte offset from base.
        offset: i64,
        /// The type of the field being accessed (for load/store type info).
        field_ty: Type,
    },

    /// Field address computation for actor state fields.
    ///
    /// `FieldAddr { result, base, field_name, field_ty, offset }` — computes
    /// address of a named state field within an actor's state struct.
    FieldAddr {
        /// The SSA value that receives the computed address.
        result: Value,
        /// The base pointer to the actor state.
        base: Value,
        /// The name of the field.
        field_name: String,
        /// The type of the field.
        field_ty: Type,
        /// The byte offset from base.
        offset: i64,
    },

    /// Construct a heap-allocated enum tagged union.
    ///
    /// `ConstructVariant(result, tag, payload)` — allocates
    /// `(1 + payload.len()) * 8` bytes via `malloc`, stores `tag` as an `i64`
    /// at offset 0, stores each payload value at offset `(i+1)*8`, and
    /// returns the pointer in `result`.
    ///
    /// Layout: `[tag: i64, field_0: i64, field_1: i64, ...]`
    ///
    /// Both unit variants (no payload) and tuple variants (one or more payload
    /// fields) use this uniform heap representation.
    ConstructVariant {
        /// The SSA value that receives the heap pointer.
        result: Value,
        /// The variant's 0-based index within its enum declaration.
        tag: i64,
        /// The payload values to store (empty for unit variants).
        payload: Vec<Value>,
    },

    /// Load the tag field from an enum pointer.
    ///
    /// `GetVariantTag(result, ptr)` — loads the `i64` tag from offset 0 of
    /// the heap-allocated enum value pointed to by `ptr`.
    GetVariantTag {
        /// The SSA value that receives the tag (typed as `I64`).
        result: Value,
        /// The enum pointer.
        ptr: Value,
    },

    /// Load a payload field from an enum pointer.
    ///
    /// `GetVariantField(result, ptr, index)` — loads the `i64` value at
    /// offset `(index + 1) * 8` within the heap-allocated enum value.
    GetVariantField {
        /// The SSA value that receives the field value.
        result: Value,
        /// The enum pointer.
        ptr: Value,
        /// The 0-based field index within the variant's payload.
        index: usize,
    },

    // ── Actor operations ───────────────────────────────────────────────
    /// Spawn an actor instance.
    ///
    /// `Spawn(result, actor_type_name)` — creates a new actor of the given type,
    /// returns an opaque ActorHandle pointer in `result`.
    Spawn {
        /// The SSA value that receives the actor handle (typed as `Ptr`).
        result: Value,
        /// The actor type name (e.g., "Counter").
        actor_type_name: String,
    },

    /// Send a fire-and-forget message to an actor.
    ///
    /// `Send(handle, message_name, payload)` — sends a message to the actor
    /// identified by `handle`. The `payload` is an optional pointer to
    /// message arguments (null for messages without payload).
    Send {
        /// The actor handle (typed as `Ptr`).
        handle: Value,
        /// The message name (e.g., "Increment").
        message_name: String,
        /// Optional payload pointer (typed as `Ptr`), or null.
        payload: Option<Value>,
    },

    /// Send a message and wait for a reply.
    ///
    /// `Ask(result, handle, message_name, payload)` — sends a message to the
    /// actor and blocks until a reply is received. The reply pointer is
    /// stored in `result`.
    Ask {
        /// The SSA value that receives the reply pointer (typed as `Ptr`).
        result: Value,
        /// The actor handle (typed as `Ptr`).
        handle: Value,
        /// The message name (e.g., "GetCount").
        message_name: String,
        /// Optional payload pointer (typed as `Ptr`), or null.
        payload: Option<Value>,
    },

    /// Initialize actor state.
    ///
    /// `ActorInit(initial_state)` — sets up the initial state for an actor.
    /// This is typically the first operation in an actor's constructor.
    ActorInit {
        /// The initial state value pointer.
        initial_state: Value,
    },
}
