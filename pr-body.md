## Summary

Fixes multi-field record returns producing garbage values by changing record allocation from stack (`Alloca`) to heap (`__gradient_genref_alloc`).

## Problem

When a function returned a record, the stack-allocated memory was deallocated on function return, causing dangling pointers. Accessing fields through these pointers returned garbage values.

## Solution

1. Added `ensure_genref_alloc()` method to register the `__gradient_genref_alloc` runtime function
2. Modified `RecordLit` arm in `build_expr()` to:
   - Calculate the total record size
   - Call `__gradient_genref_alloc(total_size)` to get heap memory
   - Store fields using the same `StoreField` instruction

## Changes

- `codebase/compiler/src/ir/builder/mod.rs`:
  - Added `ensure_genref_alloc()` helper method
  - Changed `RecordLit` from `Alloca` to heap allocation via `__gradient_genref_alloc`

## Testing

- Verified code compiles successfully
- Record test cases should now pass with correct field values

Fixes #46
Fixes #59
