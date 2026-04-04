# Phase 1.1 Implementation Plan: HashMap for Gradient

## Goal
Implement a production-quality HashMap[K, V] type for the Gradient standard library that can be used for compiler symbol tables.

## Requirements

### Functional Requirements
1. Generic over key (K) and value (V) types
2. Key must be hashable (Hash trait)
3. Key must be equatable (Eq trait)
4. Operations: insert, get, remove, contains_key, len, is_empty
5. Iteration support (keys, values, entries)
6. Resizable (grows when load factor exceeded)

### Performance Requirements
1. O(1) average case for insert, get, remove
2. Load factor: 0.75 (resize when 75% full)
3. Initial capacity: 16
4. Growth factor: 2x

### API Design

```gradient
mod std.collections:
    // HashMap type
    type HashMap[K, V]:
        buckets: List[Option[Bucket[K, V]]]
        size: i32
        capacity: i32

    type Bucket[K, V]:
        hash: i32
        key: K
        value: V
        next: Option[Bucket[K, V]]  // For chaining

    // Core operations
    fn new() -> HashMap[K, V]
    fn with_capacity(cap: i32) -> HashMap[K, V]

    fn insert(self, key: K, value: V) -> !{Pure} Option[V]  // Returns old value if present
    fn get(self, key: K) -> !{Pure} Option[V]
    fn remove(self, key: K) -> !{Pure} Option[V]
    fn contains_key(self, key: K) -> !{Pure} Bool

    fn len(self) -> !{Pure} i32
    fn is_empty(self) -> !{Pure} Bool
    fn clear(self) -> !{Pure} Unit

    // Iteration (Phase 1.2 when we have iterators)
    fn keys(self) -> !{Pure} List[K]
    fn values(self) -> !{Pure} List[V]
    fn entries(self) -> !{Pure} List[(K, V)]

// Required traits
trait Hash:
    fn hash(self) -> !{Pure} i32

trait Eq:
    fn eq(self, other: Self) -> !{Pure} Bool

// Implementations for built-in types
impl Hash for String:
    fn hash(self) -> !{Pure} i32: ...

impl Eq for String:
    fn eq(self, other: String) -> !{Pure} Bool: ...

impl Hash for i32:
    fn hash(self) -> !{Pure} i32: self

impl Eq for i32:
    fn eq(self, other: i32) -> !{Pure} Bool: self == other
```

## Implementation Strategy

### Approach: Separate Chaining with Dynamic Resizing

1. **Buckets**: Array of Option[Bucket] where Bucket is a linked list node
2. **Hash function**: FNV-1a or simple multiplicative hash for strings
3. **Collision resolution**: Linked list chaining
4. **Resizing**: When load factor > 0.75, double capacity and rehash

## File Structure

```
codebase/stdlib/
├── collections/
│   ├── hashmap.gr        # Main implementation
│   ├── hashmap_tests.gr  # Unit tests
│   └── mod.gr            # Module exports
├── traits/
│   ├── hash.gr           # Hash trait
│   └── eq.gr             # Eq trait
└── mod.gr                # stdlib root
```

## Implementation Details

### Hash Algorithm: FNV-1a (32-bit)

```gradient
fn fnv1a_32(data: String) -> i32:
    let hash = 0x811c9dc5  // FNV offset basis
    let prime = 0x01000193 // FNV prime

    for byte in data.bytes():
        hash = hash ^ byte
        hash = hash * prime

    return hash
```

### HashMap Operations

#### Insert
```
1. Compute hash of key
2. Compute index = hash % capacity
3. If bucket empty: create new bucket, size++, maybe_resize
4. If bucket occupied: traverse chain
   - If key found: update value, return old value
   - Else: append to chain, size++, maybe_resize
```

#### Get
```
1. Compute hash of key
2. Compute index = hash % capacity
3. If bucket empty: return None
4. Traverse chain, return value if key matches
5. Return None if not found
```

#### Resize
```
1. Double capacity
2. Create new buckets array
3. For each bucket in old array:
   - For each entry in chain:
     - Recompute index = hash % new_capacity
     - Insert into new buckets
4. Replace buckets array
```

## Test Plan

### Unit Tests

1. **Basic operations**
   - Insert and get single element
   - Insert multiple elements
   - Get non-existent key returns None
   - Update existing key

2. **Collision handling**
   - Insert keys with same hash (forced collision)
   - Verify both values retrievable
   - Remove one, verify other still present

3. **Resizing**
   - Insert many elements (trigger resize)
   - Verify all elements still accessible
   - Check capacity increased

4. **Edge cases**
   - Empty map operations
   - Single element map
   - Clear and reuse

5. **Different key types**
   - String keys
   - Int keys
   - Custom types with Hash+Eq

## Integration Points

1. **Type checker**: Add HashMap to builtin types
2. **Code generation**: Lower HashMap operations to runtime calls
3. **Runtime**: Implement HashMap in C runtime

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Performance worse than Rust | Benchmark early, consider Robin Hood hashing |
| Generic type issues | Test with multiple key/value combinations |
| Memory leaks in runtime | Careful C implementation, valgrind testing |
| Trait system not ready | Fallback: implement for String/i32 only initially |

## Definition of Done

- [ ] HashMap implementation in stdlib
- [ ] Hash trait with implementations for String, i32
- [ ] Eq trait with implementations for String, i32
- [ ] All unit tests passing
- [ ] Integrated with type checker
- [ ] Works in compiler symbol table use case
- [ ] Documentation and examples

## Success Criteria

```gradient
// Should work:
let map = HashMap[String, i32].new()
map.insert("x", 10)
map.insert("y", 20)
assert(map.get("x") == Some(10))
assert(map.get("z") == None)
assert(map.len() == 2)
```
