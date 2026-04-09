# Self-Hosting Roadmap: Gradient in Gradient

## Vision

Achieve **full self-hosting** where 95%+ of the Gradient compiler is written in Gradient, with a minimal Rust kernel (~2,000 lines) providing only critical primitives.

```
┌─────────────────────────────────────┐
│ RUST KERNEL (~2,000 lines)           │  ← Minimal primitives only
│ - String: char_at, length, etc      │
│ - File I/O: read, write, exists     │
│ - Memory: alloc, free               │
│ - Process: spawn, wait              │
│ - Codegen: Cranelift, WASM          │
└─────────────────────────────────────┘
           ↓ FFI
┌─────────────────────────────────────┐
│ GRADIENT COMPILER (~15,000 lines)   │  ← Full implementation
│ - Lexer (character scanning)         │
│ - Parser (recursive descent)         │
│ - Type Checker (HM inference)          │
│ - Queryable API (for agents!)       │  ← KEY: Agents use this
│ - LSP Server (JSON-RPC)             │  ← IDE integration
│ - IR Builder (SSA form)              │
│ - Optimizations                      │
│ - Codegen orchestration              │
└─────────────────────────────────────┘
```

## Why This Architecture?

### 1. Dogfooding
Agents develop Gradient in Gradient—the language is optimized for agentic workflows.

### 2. Self-Referential Improvement
```
┌────────────────────────────────────────┐
│ 1. Query API (in Gradient)            │
│    → Agent works faster                │
├────────────────────────────────────────┤
│ 2. Agent improves query API             │
│    → Even better for agents            │
├────────────────────────────────────────┤
│ 3. Repeat                               │
│    → Exponential improvement           │
└────────────────────────────────────────┘
```

### 3. Portability
- Minimal kernel = easy to port to new platforms
- Gradient code is platform-independent
- Only kernel needs platform-specific code

### 4. Safety
- Rust kernel handles memory safety
- Gradient handles logic
- Clear separation of concerns

## Current State

**Phase 3 COMPLETE:** Type definitions in self-hosted code
- ✅ 10 compiler modules: `token.gr`, `lexer.gr`, `parser.gr`, `types.gr`, `ir.gr`, `ir_builder.gr`, `checker.gr`, `compiler.gr`, `bootstrap.gr`, `types_positional.gr`
- ✅ ~4,077 lines of Gradient code
- ✅ All modules type-check successfully
- ❌ Implementations are stubs (need string primitives)

**Rust Compiler:** Production-ready, ~30,000 lines
- ✅ Full lexer, parser, type checker
- ✅ Queryable API (5,400 lines)
- ✅ LSP server
- ✅ Codegen (Cranelift + WASM)

## Roadmap

### Phase 0: String Primitives (CRITICAL BLOCKER) [#117](https://github.com/Ontic-Systems/Gradient/issues/117)
**Status:** 🔴 Not Started  
**Effort:** ~1 day, ~120 lines Rust

Add to Rust kernel:
- `string_length(s: String) -> Int`
- `string_char_at(s: String, idx: Int) -> Int`
- `string_substring(s: String, start: Int, end: Int) -> String`
- `string_append(a: String, b: String) -> String`

**Why Critical:** Self-hosted lexer cannot read source code without these.

---

### Phase 1: Lexer [#118](https://github.com/Ontic-Systems/Gradient/issues/118)
**Status:** ⏳ Blocked on #117  
**Effort:** ~3 days, ~800 lines Gradient

Implement actual character scanning in `compiler/lexer.gr`:
- `current_char(lex: Lexer) -> Int`
- `peek_char(lex: Lexer, offset: Int) -> Int`
- `next_token(lex: Lexer) -> (Lexer, Token)`
- Full tokenization of all token types

---

### Phase 2: Parser [#119](https://github.com/Ontic-Systems/Gradient/issues/119)
**Status:** ⏳ Blocked on #118  
**Effort:** ~5 days, ~1,300 lines Gradient

Implement recursive descent parser in `compiler/parser.gr`:
- Pratt parser for expressions (precedence climbing)
- Parse all statement types
- Parse module structure
- Error recovery

---

### Phase 3: Type Checker [#120](https://github.com/Ontic-Systems/Gradient/issues/120)
**Status:** ⏳ Blocked on #119  
**Effort:** ~7 days, ~2,500 lines Gradient

Implement full type checking in `compiler/checker.gr`:
- Hindley-Milner type inference
- Unification algorithm
- Effect inference
- Polymorphism
- Error reporting

---

### Phase 4: Queryable API in Gradient [#121](https://github.com/Ontic-Systems/Gradient/issues/121)
**Status:** ⏳ Blocked on #120  
**Effort:** ~5 days, ~1,500 lines Gradient

**NEW FILE:** `compiler/query.gr`

Implement the queryable API that enables agents:
- `session_from_source(source: String) -> Session`
- `session_check(sess: Session) -> CheckResult`
- `session_symbols(sess: Session) -> SymbolList`
- `session_type_at(sess: Session, line: Int, col: Int) -> TypeAtResult`
- `session_rename(sess: Session, old: String, new: String) -> RenameResult`
- `session_effects(sess: Session) -> EffectSummary`

**This is the KEY enabler for agentic workflows.**

---

### Phase 5: LSP in Gradient [#122](https://github.com/Ontic-Systems/Gradient/issues/122)
**Status:** ⏳ Blocked on #121  
**Effort:** ~7 days, ~2,500 lines Gradient

**NEW FILE:** `compiler/lsp.gr`

Implement LSP server in Gradient:
- JSON-RPC message handling
- textDocument/diagnostic (real-time errors)
- textDocument/hover (type info)
- textDocument/definition (go to def)
- textDocument/references (find refs)
- textDocument/rename (safe rename)
- textDocument/completion (autocomplete)

Uses `query.gr` for all data.

---

### Phase 6: IR Builder [#123](https://github.com/Ontic-Systems/Gradient/issues/123)
**Status:** ⏳ Blocked on #122  
**Effort:** ~5 days, ~1,500 lines Gradient

Implement IR generation in `compiler/ir_builder.gr`:
- Lower AST to SSA form
- Generate all instruction types
- Block/branch construction
- Type conversions

---

### Phase 7: Codegen [#124](https://github.com/Ontic-Systems/Gradient/issues/124)
**Status:** ⏳ Blocked on #123  
**Effort:** ~5 days, ~1,000 lines Gradient

Implement code generation orchestration:
- Call Rust kernel for Cranelift codegen
- Call Rust kernel for WASM codegen
- Linking and output

---

### Phase 8: Memory Primitives [#125](https://github.com/Ontic-Systems/Gradient/issues/125)
**Status:** 📋 Planned  
**Effort:** ~1 day, ~200 lines Rust

Add to Rust kernel:
- `alloc(size: Int) -> Int`
- `free(ptr: Int)`
- `list_create() -> Int`
- `list_push(lst: Int, item: Int)`

Needed for dynamic data structures.

---

## Critical Path

The phases MUST be completed in order. Each phase blocks the next:

```
#117 (String) → #118 (Lexer) → #119 (Parser) → #120 (Checker)
                                                      ↓
#124 (Codegen) ← #123 (IR) ← #122 (LSP) ← #121 (Query API)
```

## Definition of Done

- [ ] All phases complete
- [ ] Compiler can compile itself
- [ ] `gradient-compiler` is 95%+ Gradient code
- [ ] Rust kernel ≤2,000 lines
- [ ] Gradient compiler ≥15,000 lines
- [ ] All 1,058+ tests passing
- [ ] LSP working
- [ ] Queryable API working
- [ ] CI green

## Total Effort Estimate

| Component | Lines | Days |
|-----------|-------|------|
| Rust kernel additions | ~1,000 | 2 |
| Lexer | +800 | 3 |
| Parser | +1,300 | 5 |
| Type checker | +2,500 | 7 |
| Query API | +1,500 | 5 |
| LSP | +2,500 | 7 |
| IR builder | +1,500 | 5 |
| Codegen | +1,000 | 5 |
| **Total** | **~12,100** | **~39** |

## What Stays in Rust

Only critical primitives:
- String operations (memory safety)
- File I/O (OS syscalls)
- Memory allocation
- Process control
- Codegen backends (Cranelift, WASM)

**Everything else → Gradient**

## Benefits for Agents

Once complete, agents can:
1. Query the compiler programmatically (`Session` API in Gradient)
2. Get structured data (JSON) instead of parsing text
3. Check errors in ~10ms (in-memory, no file I/O)
4. Generate code correctly the first time (type-aware)
5. Refactor safely (automated, verified)
6. Understand effects (purity tracking)

**Result: 10-50x improvement in agentic coding workflows.**

## Resources

- Epic: [#116 - Full Self-Hosting](https://github.com/Ontic-Systems/Gradient/issues/116)
- Design doc: `SELF_HOSTING_STRATEGY.md` (in project root)
- Current code: `compiler/*.gr` (~4,077 lines)
- Rust compiler: `codebase/compiler/src/` (~30,000 lines)

---

**Status:** Phase 3 complete, Phase 0 in progress  
**Next milestone:** String primitives in Rust kernel  
**ETA:** ~39 days total effort
