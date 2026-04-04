# Phase 1.2 Implementation Plan: Iterator Protocol

## Goal
Implement an Iterator protocol for Gradient that enables `for item in collection` syntax and provides functional-style iteration adapters (map, filter, fold).

## Requirements

### Functional Requirements
1. **Trait-based design**: `Iterator[T]` trait with `next()` method
2. **for-loop support**: `for x in list { ... }` desugars to iterator protocol
3. **Builtin iterators**: List, Range, HashMap (keys, values, entries)
4. **Adapter functions**: map, filter, fold, collect
5. **Lazy evaluation**: Adapters chain without intermediate collections

### API Design

```gradient
// Core trait
trait Iterator[T]:
    fn next(self) -> !{Pure} Option[T]
    fn has_next(self) -> !{Pure} Bool

// Extension methods (via trait default impls or builtins)
fn map[T, U](self, f: fn(T) -> U) -> Iterator[U]
fn filter[T](self, pred: fn(T) -> Bool) -> Iterator[T]
fn fold[T, U](self, init: U, f: fn(U, T) -> U) -> U
fn collect[T](self) -> !{Pure} List[T]
fn count[T](self) -> !{Pure} Int
fn find[T](self, pred: fn(T) -> Bool) -> !{Pure} Option[T]

// Builtin iteration functions
fn list_iter[T](list: List[T]) -> Iterator[T]
fn range_iter(start: Int, end: Int) -> Iterator[Int]
fn hashmap_keys[K, V](map: HashMap[K, V]) -> Iterator[K]
fn hashmap_values[K, V](map: HashMap[K, V]) -> Iterator[V]

// For loops (syntactic sugar)
// for x in list { body }  →  desugars to iterator protocol
```

## Implementation Strategy

### Phase A: Core Iterator Type and Trait

1. **Add `Ty::Iterator(Box<Ty>)`** to types.rs
2. **Add `Iterator` trait** to env.rs with `next` method
3. **Add type resolution** for `Iterator[T]` syntax in checker.rs

### Phase B: List Iterator (Concrete Implementation)

The simplest iterator - maintains index into list.

Runtime representation:
```c
typedef struct {
    void** data;       // Pointer to list data
    int64_t len;       // List length
    int64_t index;     // Current position
    int ref_count;     // For COW
} GradientListIter;
```

Functions:
- `__gradient_list_iter_new(list)` → `Iterator[T]`
- `__gradient_list_iter_next(iter)` → `Option[T]`

### Phase C: For-Loop Desugaring

Transform `for x in expr { body }` into:

```gradient
{
    let __iter = expr  // Or: list_iter(expr) if expr is List
    loop:
        let __next = __iter.next()
        match __next:
            | Some(x) => { body; continue }
            | None => break
}
```

This requires:
1. Parser: Recognize for-loop syntax (already exists)
2. Lowering/Desugaring pass in IR builder or AST transformation

### Phase D: Iterator Adapters

Implement as builtin functions that return new iterator types:

```c
// Map iterator - applies function to each element
typedef struct {
    void* inner_iter;     // Source iterator
    void* fn_ptr;         // Function to apply
    int ref_count;
} GradientMapIter;

// Filter iterator - skips elements not matching predicate
typedef struct {
    void* inner_iter;
    void* pred_ptr;
    int ref_count;
} GradientFilterIter;
```

Functions:
- `__gradient_iter_map(iter, fn_ptr)` → `Iterator[U]`
- `__gradient_iter_filter(iter, pred_ptr)` → `Iterator[T]`
- `__gradient_iter_fold(iter, init, fn_ptr)` → `U`
- `__gradient_iter_collect(iter)` → `List[T]`

## File Structure Changes

```
codebase/compiler/src/
├── typechecker/
│   ├── types.rs          # Add Ty::Iterator
│   ├── env.rs            # Add Iterator trait, iter builtins
│   └── checker.rs        # Add Iterator[T] type resolution
├── ir/
│   └── builder/
│       └── mod.rs        # Add for-loop desugaring
└── codegen/
    └── ...               # Add iterator runtime calls

runtime/
└── gradient_runtime.c    # Add iterator C implementations
```

## Implementation Order

1. **Type system** (30 min)
   - Add `Ty::Iterator(Box<Ty>)`
   - Add Display impl
   - Add type resolution

2. **Iterator trait** (30 min)
   - Add `Iterator` trait to env.rs
   - Add `next` method signature

3. **List iterator** (1 hour)
   - C runtime: `GradientListIter` struct
   - C runtime: `__gradient_list_iter_new/next`
   - Type checker: `list_iter[T]` builtin

4. **For-loop desugaring** (2 hours)
   - Add lowering pass in IR builder
   - Generate iterator protocol code

5. **Range iterator** (30 min)
   - Similar to List iterator
   - For `for i in 0..10` syntax

6. **Adapters** (2 hours)
   - Map iterator
   - Filter iterator
   - Fold (eager - not lazy)
   - Collect

7. **Tests** (1 hour)
   - Unit tests for each component
   - Integration tests

## Testing Plan

```gradient
// Test 1: Basic list iteration
fn test_list_iter():
    let list = [1, 2, 3]
    let iter = list_iter(list)
    assert(iter.next() == Some(1))
    assert(iter.next() == Some(2))
    assert(iter.next() == Some(3))
    assert(iter.next() == None)

// Test 2: For loop
fn test_for_loop():
    let sum = 0
    for x in [1, 2, 3]:
        sum = sum + x
    assert(sum == 6)

// Test 3: Map adapter
fn test_map():
    let list = [1, 2, 3]
    let doubled = list_iter(list).map(fn(x) -> x * 2).collect()
    assert(doubled == [2, 4, 6])

// Test 4: Filter adapter
fn test_filter():
    let list = [1, 2, 3, 4, 5]
    let evens = list_iter(list).filter(fn(x) -> x % 2 == 0).collect()
    assert(evens == [2, 4])

// Test 5: Fold
fn test_fold():
    let list = [1, 2, 3, 4]
    let sum = list_iter(list).fold(0, fn(acc, x) -> acc + x)
    assert(sum == 10)
```

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Closures in adapters complex | Start with function pointers only |
| For-loop desugaring tricky | Test desugared form first |
| Performance overhead | Lazy evaluation, minimize allocations |
| C runtime complexity | Reference count all iterators |

## Definition of Done

- [ ] `Iterator[T]` type recognized by type checker
- [ ] `list_iter[T]` works and returns `Iterator[T]`
- [ ] `for x in list { ... }` compiles and runs
- [ ] `map`, `filter`, `fold`, `collect` implemented
- [ ] All iterator tests passing
- [ ] No regressions in existing 897 tests
- [ ] Documentation updated

## Timeline Estimate

- Phase A (Type system): 30 min
- Phase B (List iterator): 1 hour  
- Phase C (For-loop): 2 hours
- Phase D (Adapters): 2 hours
- Tests & polish: 1 hour

**Total: ~6-7 hours**
