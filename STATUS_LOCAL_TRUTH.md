# Gradient Local Truth Report
**Generated:** 2026-04-05  
**Repo:** /home/gray/TestingGround/Gradient  
**Commit:** 4060dc3 (main branch, clean working tree)

---

## Executive Summary

| Category | Status | Notes |
|----------|--------|-------|
| Build | ✅ PASS | Release build succeeds with warnings |
| Tests | ✅ 1,030 passing / 0 failed / 1 ignored | Local test count confirmed |
| Clippy | ⚠️ WARNINGS | 3 warnings (unused vars, too many args) |
| E2E Native | ✅ WORKING | Cranelift → GCC link → run successful |
| Self-Hosting | ❌ BLOCKED | 0/10 files parse completely |
| Formatter | ⚠️ CODE EXISTS | 1,297 lines, CLI stubbed |
| REPL | ⚠️ CODE EXISTS | 960 lines, exits silently |
| WASM | ❌ NOT COMPILED | Code exists, feature flag disabled |
| LLVM | ❌ BROKEN | Disabled in CI, Polly linking issue |
| Actors | ⚠️ TYPECHECKER | 21 tests pass, runtime integration unclear |
| Memory | ⚠️ TYPECHECKER | IR tests pass, runtime integration unclear |
| Packages | ⚠️ PARTIAL | Path deps likely work, untested |

---

## Detailed Verification Results

### 1. Build/Test Matrix

```bash
cargo build --release          # ✅ SUCCESS (warnings only)
cargo test --release           # ✅ 1,030 passed / 0 failed / 1 ignored
cargo clippy --release         # ⚠️ 3 warnings (not failures)
```

**Claude-v1 claim validated:** The local tree IS green with 1,030 tests.
**Codex-v1 concern:** Public CI shows failures - this is a CI/public vs local divergence.

### 2. E2E Native Compilation

**Test:** Hello world with IO effects  
**Result:** ✅ SUCCESS

Full pipeline verified:
1. Parse: ✅
2. Typecheck (effect tracking): ✅
3. IR build: ✅
4. Cranelift codegen: ✅
5. Native object output: ✅
6. GCC link with runtime: ✅ (requires `-lcurl`)
7. Execute: ✅

**Runtime linked:** `codebase/compiler/runtime/gradient_runtime.c`

### 3. Self-Hosted Compiler Files (The Blocker)

| File | Lines | Parse Status | Error Location |
|------|-------|--------------|----------------|
| bootstrap.gr | 662 | ❌ FAIL | Line 126: expected variable name, found `state` |
| checker.gr | 915 | ❌ FAIL | Line 229: expected expression, found `:` |
| compiler.gr | 507 | ❌ FAIL | Line 87: expected field name, found `:` |
| ir_builder.gr | 352 | ❌ FAIL | Line 44: expected field name, found `:` |
| ir.gr | 772 | ❌ FAIL | Line 566: expected field name, found `:` |
| lexer.gr | 575 | ❌ FAIL | Line 9: unexpected token in mod block |
| parser.gr | 997 | ❌ FAIL | Line 249: unexpected character `\` |
| token.gr | 489 | ❌ FAIL | Line 151: expected expression, found `{` |
| types.gr | 666 | ❌ STUCK | Module resolution hangs |
| types_positional.gr | 592 | ❌ STUCK | Module resolution hangs |

**Pass rate: 0/10 files (0%)**

**Note:** Previous handoff claimed 2/10 files passing. Current verification shows 0/10. This may indicate a regression or environment difference.

### 4. Runtime Authority Resolution

**Conflict identified in adversarial synthesis:**

| Location | Purpose | Status |
|----------|---------|--------|
| `codebase/compiler/runtime/gradient_runtime.c` | **Linked in native builds** | ✅ Active |
| `codebase/runtime/memory/arena.c` | Arena allocation runtime | ⚠️ Exists, not linked |
| `codebase/runtime/memory/genref.c` | Generational references | ⚠️ Exists, not linked |
| `codebase/runtime/vm/scheduler.c` | Actor scheduler | ⚠️ Exists, not linked |
| `codebase/runtime/vm/actor.c` | Actor runtime | ⚠️ Exists, not linked |

**Resolution:** The "newer" runtime in `codebase/runtime/` is NOT the one being linked. The authoritative runtime for native compilation is `codebase/compiler/runtime/gradient_runtime.c`.

This is the critical ambiguity that must be documented.

### 5. Backend Status

| Backend | Status | Evidence |
|---------|--------|----------|
| Cranelift | ✅ PRIMARY | Default, tested, working |
| WASM | ❌ DISABLED | Code exists, compiled without `wasm` feature |
| LLVM | ❌ BROKEN | CI disabled, Polly linking issue |

### 6. Formatter / REPL

| Component | Code Lines | CLI Status | Notes |
|-----------|------------|------------|-------|
| Formatter | 1,297 | `[planned]` | Implementation exists but not wired to CLI |
| REPL | 960 | Silent exit | Implementation exists but not functional |

The roadmap labels these as "stubbed" but substantial code exists.

### 7. Actor Support

**Typechecker tests:** 21 passing
- actor_spawn_valid
- actor_send_valid_message
- actor_ask_returns_correct_type
- etc.

**Runtime integration:** Unclear. Actor runtime code exists in `codebase/runtime/vm/actor.c` but this is NOT the runtime being linked in native builds.

### 8. Memory Model

**Typechecker tests:** 2 passing (ir_memory_instructions, ir_builder_memory_emit)

**Runtime integration:** Arena/genref code exists but not linked in native builds.

### 9. Package/Dependency Support

**Likely working:** Path dependencies (untested in this verification)

**Experimental/untested:** Git dependencies, registry dependencies

**Not working:** Registry server does not exist

---

## Open Questions from Synthesis - Local Answers

| Question | Local Answer |
|----------|--------------|
| Is local tree green? | ✅ YES - 1,030 tests pass |
| What is authoritative runtime? | `codebase/compiler/runtime/gradient_runtime.c` |
| Does git dependency work? | Untested, likely partial |
| Actor path close to MVP? | Typechecker yes, runtime integration unclear |
| Self-hosting progress? | 0/10 files parse, blocked on various issues |

---

## Synthesis Alignment

**Which report was right?**

| Claim | Claude-v1 | Codex-v1 | Local Truth |
|-------|-----------|----------|-------------|
| 1,030 tests passing | ✅ | ❌ (didn't claim) | ✅ Claude correct |
| Build green | ✅ | ❌ (CI failing) | ✅ Claude correct (local) |
| Runtime exists | ✅ | ✅ (more skeptical) | ✅ Both correct |
| Formatter/REPL real | ✅ | ⚠️ | ✅ Claude correct |
| Self-hosting 2/10 | ✅ | ❌ | ❌ Neither - currently 0/10 |
| Public CI failing | ❌ | ✅ | ✅ Codex correct |

**Resolution:** Claude-v1 accurately described the local state. Codex-v1 accurately described the public CI state. Both are true - there's a divergence between local and public.

---

## Recommended Immediate Actions

### P0: Truth Surface Fixes
1. Update README test count to 1,030
2. Add CI/local divergence explanation
3. Document runtime authority
4. Update roadmap formatter/REPL labels

### P1: Self-Hosting Blocker
Current blocker pattern: Field name parsing with `:`
- Multiple files fail with `expected field name; found :`
- This suggests typed expression/record literal disambiguation issues

### P2: Runtime Integration
- Document which runtime is authoritative
- Create migration path for arena/genref/actor runtimes

---

## Files Modified Since Last Truth Check

```
4060dc3 docs: add handoff for deep research integration
69d7633 docs: update handoff with enum block fix status
e89194b fix(parser): add enum block syntax support for type declarations
0e54785 fix(parser): handle type application and constructor syntax...
de1a1c1 feat(parser): add enum variant constructor syntax with named fields
```

---

**Report generated by:** Hermes Agent  
**Method:** Direct local verification per adversarial synthesis Phase 0
