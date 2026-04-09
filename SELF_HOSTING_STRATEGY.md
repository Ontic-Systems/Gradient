# Strategy: Full Self-Hosting with Minimal Rust Kernel

## Vision

**Goal:** Move 95%+ of the compiler to Gradient, keeping only a minimal Rust runtime as the "kernel" for critical primitives.

**Why:** Gradient is optimized for agentic workflows. Agents should develop Gradient in Gradient.

## Current State

```
┌─────────────────────────────────────────────────────────────┐
│  CURRENT ARCHITECTURE                                       │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  RUST COMPILER (codebase/)                          │   │
│  │  - Full lexer (working)                             │   │
│  │  - Full parser (working)                            │   │
│  │  - Full type checker (working)                      │   │
│  │  - Full queryable API (working)                     │   │
│  │  - Full LSP (working)                               │   │
│  │  - Codegen (Cranelift + WASM)                       │   │
│  │  - ~30,000 lines of Rust                            │   │
│  └─────────────────────────────────────────────────────┘   │
│                          │                                  │
│  ┌───────────────────────┼─────────────────────────────┐   │
│  │  SELF-HOSTED (compiler/*.gr)                         │   │
│  │  - Type definitions (complete)                       │   │
│  │  - API signatures (complete)                       │   │
│  │  - Implementations (stubs)                           │   │
│  │  - ~4,077 lines of Gradient                        │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## Target Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  TARGET ARCHITECTURE: Minimal Rust Kernel                    │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  RUST KERNEL (~2,000 lines)                         │   │
│  │  - String primitives (char_at, length, substring)   │   │
│  │  - File I/O (read, write, exists)                 │   │
│  │  - Memory management (alloc, free)                │   │
│  │  - Process control (spawn, wait)                  │   │
│  │  - Basic runtime (panic handling)                 │   │
│  │  - FFI bridge to Gradient                          │   │
│  └─────────────────────────────────────────────────────┘   │
│                          │                                  │
│  ┌───────────────────────▼─────────────────────────────┐   │
│  │  GRADIENT COMPILER (~15,000 lines)                  │   │
│  │  - Lexer (actual implementation)                    │   │
│  │  - Parser (actual implementation)                   │   │
│  │  - Type Checker (actual implementation)             │   │
│  │  - Queryable API (implemented in Gradient!)         │   │
│  │  - IR Builder (actual implementation)               │   │
│  │  - Codegen (orchestrates Rust kernel)               │   │
│  │  - LSP Server (implemented in Gradient!)             │   │
│  │  - Optimizations                                    │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## What Moves to Gradient

### ✅ Can Move to Gradient (High Priority)

| Component | Current | Target | Lines Est. |
|-----------|---------|--------|------------|
| **Lexer** | Rust stub | Full impl | ~1,000 |
| **Parser** | Rust stub | Full impl | ~2,000 |
| **Type Checker** | Rust stub | Full impl | ~3,000 |
| **Queryable API** | Rust | Gradient | ~2,000 |
| **IR Builder** | Rust stub | Full impl | ~2,000 |
| **LSP Server** | Rust | Gradient | ~3,000 |
| **Optimizations** | None | Gradient | ~2,000 |
| **Documentation** | Rust | Gradient | ~500 |
| **Total** | ~4,077 stubs | ~15,500 full | +11,500 |

### ⚠️ Must Stay in Rust (Kernel Primitives)

| Primitive | Why in Rust |
|-----------|-------------|
| `string_char_at` | Memory safety, bounds checking |
| `string_length` | O(1) string metadata |
| `file_read` | OS syscall interface |
| `file_write` | OS syscall interface |
| `alloc` | Memory management |
| `spawn_process` | OS process control |
| `tcp_connect` | Network syscalls |
| `panic_handler` | Crash safety |

## The Bootstrap Path

### Phase 1: Expose String Primitives (BLOCKER NOW)

**Current blocker:** Self-hosted code can't access string characters.

**Solution:** Add to Rust kernel:
```rust
// Expose to Gradient extern
fn string_length(s: String) -> Int;
fn string_char_at(s: String, idx: Int) -> Int;
fn string_substring(s: String, start: Int, end: Int) -> String;
fn string_append(a: String, b: String) -> String;
```

**Impact:** Unblocks lexer implementation.

### Phase 2: Implement Actual Lexer

**File:** `compiler/lexer.gr`

**Current:** ~206 lines (stubs)
**Target:** ~1,000 lines (full implementation)

**Implementation:**
```gradient
fn next_token(lex: Lexer) -> (Lexer, Token):
    // Actually scan characters
    let ch = current_char(lex)
    if is_whitespace(ch):
        ret skip_whitespace(lex)
    if is_digit(ch):
        ret read_number(lex)
    if is_ident_start(ch):
        ret read_identifier(lex)
    // ... etc
```

### Phase 3: Implement Actual Parser

**File:** `compiler/parser.gr`

**Current:** ~706 lines (stubs)
**Target:** ~2,000 lines (full recursive descent)

### Phase 4: Implement Type Checker

**File:** `compiler/checker.gr`

**Current:** ~465 lines (stubs)
**Target:** ~3,000 lines (full inference)

### Phase 5: Implement Queryable API in Gradient

**NEW FILE:** `compiler/query.gr`

**Implementation:**
```gradient
mod query:
    type Session:
        source: String
        tokens: TokenList
        ast: AstModule
        type_cache: TypeCache

    fn session_from_source(source: String) -> Session:
        let tokens = tokenize(source, 0)
        let ast = parse_module(tokens)
        let checked = check_module(ast)
        ret Session { source: source, tokens: tokens, ast: ast, type_cache: build_type_cache(checked) }

    fn session_check(sess: Session) -> CheckResult:
        // Return structured errors
        ret collect_errors(sess.ast)

    fn session_symbols(sess: Session) -> SymbolList:
        // Return exact symbol table
        ret extract_symbols(sess.ast)

    fn session_type_at(sess: Session, line: Int, col: Int) -> TypeResult:
        // Precise type lookup
        ret lookup_type(sess.type_cache, line, col)
```

### Phase 6: Implement LSP in Gradient

**NEW FILE:** `compiler/lsp.gr`

**Implementation:**
```gradient
mod lsp:
    // JSON-RPC message handling
    // Uses query.gr for all data
    // Runs as daemon, responds to IDE
```

### Phase 7: Self-Hosting Complete

**The compiler can compile itself:**
```bash
# Self-compilation
gradient-compiler compiler/*.gr -o gradient-self-hosted

# New binary uses Rust kernel + Gradient implementation
./gradient-self-hosted myapp.gr
```

## Critical Blockers & Solutions

### Blocker 1: String Operations (CURRENT)

**Problem:** `lexer.gr` can't access string characters.

**Solution:** Add 4 functions to Rust kernel:
- `string_length` 
- `string_char_at`
- `string_substring`
- `string_append`

**Effort:** ~100 lines of Rust

### Blocker 2: Memory Management

**Problem:** Self-hosted code needs to allocate/deallocate.

**Solution:** Add to Rust kernel:
- `alloc(size: Int) -> Ptr`
- `free(ptr: Ptr)`
- `realloc(ptr: Ptr, new_size: Int) -> Ptr`

**Effort:** ~200 lines of Rust

### Blocker 3: Data Structure Primitives

**Problem:** Lists, maps need backing implementation.

**Solution:** Add to Rust kernel:
- `list_create()`
- `list_push(lst: Int, item: Int)`
- `map_create()`
- `map_insert(m: Int, key: String, value: Int)`

**Effort:** ~500 lines of Rust

### Blocker 4: FFI Bridge

**Problem:** Gradient needs to call Rust kernel.

**Solution:** Already exists! The `extern fn` mechanism.

**Status:** ✅ Working today

## Total Effort Estimate

| Component | Lines | Complexity | Priority |
|-----------|-------|------------|----------|
| **Rust kernel additions** | ~1,000 | Low | 🔴 Critical |
| **String builtins** | ~100 | Low | 🔴 Blocker |
| **Lexer** | +800 | Medium | 🟡 Phase 1 |
| **Parser** | +1,300 | High | 🟡 Phase 2 |
| **Type checker** | +2,500 | High | 🟡 Phase 3 |
| **Queryable API** | +1,500 | Medium | 🟢 Phase 4 |
| **LSP** | +2,500 | Medium | 🟢 Phase 4 |
| **IR builder** | +1,500 | Medium | 🟢 Phase 5 |
| **Codegen** | +1,000 | High | 🟢 Phase 6 |
| **Total new code** | ~11,200 | | |

## Benefits of This Strategy

### 1. Dogfooding
- Agents develop Gradient in Gradient
- Real-world testing of the language
- Continuous improvement

### 2. Optimization for Agents
- Queryable API written in agent-optimized language
- Self-referential improvements
- Faster iteration

### 3. Safety
- Rust kernel handles critical primitives
- Memory safety at the boundary
- Fallback if self-hosted fails

### 4. Portability
- Minimal kernel = easy to port
- New architectures: just reimplement kernel
- Gradient code is portable

## The Vision: Gradient Writing Gradient

**The ultimate goal:**
```
┌────────────────────────────────────┐
│  AGENT (me) asks to improve        │
│  the queryable API                 │
├────────────────────────────────────┤
│                                    │
│  1. I query the current API        │
│     → Session::symbols()          │
│     → Session::type_at()            │
│                                    │
│  2. I understand the code          │
│     → Exact symbol tables          │
│     → Precise type information     │
│                                    │
│  3. I generate improvements        │
│     → In Gradient                  │
│     → Using query results          │
│                                    │
│  4. I verify correctness           │
│     → In-memory check              │
│     → Real-time feedback           │
│                                    │
│  5. I apply changes                │
│     → Compile with Rust kernel     │
│     → New binary works             │
│                                    │
└────────────────────────────────────┘
```

**This creates a virtuous cycle:**
1. Better query API → Agent works faster
2. Agent improves query API → Even better for agents
3. Repeat → Exponential improvement

## Immediate Next Steps

### Step 1: Add String Builtins (1 day)
```bash
# Add to Rust kernel:
# - string_length
# - string_char_at  
# - string_substring
# - string_append
```

### Step 2: Implement Lexer (3 days)
```bash
# Expand compiler/lexer.gr:
# - Read characters from source
# - Handle all token types
# - Proper error handling
```

### Step 3: Verify End-to-End (1 day)
```bash
# Test: gradient-compiler --check compiler/lexer.gr
# Test: gradient-compiler compiler/test.gr
```

## Conclusion

**YES - We can get 95%+ of the project in Gradient.**

**Strategy:**
1. Minimal Rust kernel (~2,000 lines) for primitives
2. Everything else in Gradient (~15,000 lines)
3. Agents develop Gradient in Gradient
4. Virtuous cycle of improvement

**Current blocker:** String primitives in Rust kernel.

**Ready to implement this?**
