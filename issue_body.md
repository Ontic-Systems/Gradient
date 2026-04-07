## Problem

Records are currently allocated on the stack using `Alloca`, but when a function returns a record, the stack frame is deallocated, making the record pointer dangling. This causes garbage values when accessing fields after the function returns.

## Root Cause

In `ir/builder/mod.rs`, the `RecordLit` arm uses:
```rust
let record_ptr = self.fresh_value(Type::Ptr);
self.emit(Instruction::Alloca(record_ptr, Type::I64));
```

`Alloca` allocates on the current function's stack. When the function returns, that memory is reclaimed.

## Solution

Change record allocation from `Alloca` (stack) to heap allocation using the runtime's `__gradient_genref_alloc` function, which uses the GC-managed heap.

## Files to Modify

- `codebase/compiler/src/ir/builder/mod.rs`:
  1. Add `ensure_genref_alloc()` method (similar to `ensure_malloc()`)
  2. Modify `RecordLit` arm to use `__gradient_genref_alloc` instead of `Alloca`

## Verification

After the fix, the example from issue #46 should work and print `42 99`.

Fixes #46
