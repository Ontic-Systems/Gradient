# Gradient Runtime Authority

**Date:** 2026-04-05  
**Status:** Critical Architecture Documentation  
**Purpose:** Resolve runtime ambiguity identified in adversarial synthesis

---

## The Ambiguity

Gradient has runtime code in two locations:

1. `codebase/compiler/runtime/gradient_runtime.c` — The current active runtime
2. `codebase/runtime/` — Newer runtime code (memory, arena, genref, actor VM)

This document clarifies which is authoritative and why.

---

## Authoritative Runtime: `codebase/compiler/runtime/gradient_runtime.c`

**This is the runtime actually linked when compiling Gradient programs.**

### Evidence

```bash
# Native compilation command
./target/release/gradient-compiler input.gr output.o --target native
gcc -o output output.o runtime/gradient_runtime.c -lcurl
```

The compiler's native backend expects symbols from this specific runtime file.

### What's In This Runtime

| Function Category | Examples | Status |
|-------------------|----------|--------|
| String operations | `__gradient_string_concat`, `__gradient_string_slice` | ✅ Active |
| I/O helpers | `__gradient_read_line`, `__gradient_file_read` | ✅ Active |
| HTTP | `__gradient_http_get`, `__gradient_http_post` | ✅ Active (requires -lcurl) |
| Math | `__gradient_random`, `__gradient_clamp_*` | ✅ Active |
| Stack/Queue/Map | `__gradient_stack_*`, `__gradient_queue_*`, `__gradient_map_*` | ✅ Active |
| JSON | `__gradient_json_parse`, `__gradient_json_stringify` | ✅ Active |
| DateTime | `__gradient_now`, `__gradient_datetime_*` | ✅ Active |
| Actor (minimal) | `__gradient_actor_spawn`, `__gradient_actor_send` | ⚠️ Entry points exist |

### Integration Method

The Cranelift backend:
1. Declares external functions from this runtime as `FuncRef`
2. Generates direct calls to these C functions
3. Links the runtime at compile time via GCC

---

## Non-Authoritative Runtime: `codebase/runtime/`

**This code exists but is NOT linked in native compilation.**

### Location

```
codebase/runtime/
├── memory/
│   ├── arena.c       # Arena allocator implementation
│   ├── arena.h
│   ├── genref.c      # Generational references
│   └── genref.h
└── vm/
    ├── actor.c       # Actor runtime
    ├── actor.h
    ├── scheduler.c   # Actor scheduler
    └── scheduler.h
```

### Status

| Component | Code Quality | Integration Status |
|-----------|--------------|-------------------|
| Arena allocator | Real implementation | ❌ Not linked |
| Generational references | Real implementation | ❌ Not linked |
| Actor VM | Real implementation | ❌ Not linked |
| Scheduler | Real implementation | ❌ Not linked |

### Why It Exists But Isn't Used

This runtime was developed as the "next generation" runtime with:
- Proper arena allocation
- Generational reference support
- Full actor VM with scheduler

However, integration into the compiler's code generation path is incomplete. The compiler still generates calls to the older runtime in `compiler/runtime/`.

---

## The Path Forward

### Short Term (v1.0)

Continue using `codebase/compiler/runtime/gradient_runtime.c` as the authoritative runtime. Document its limitations:
- No true arena allocation (uses malloc)
- No generational reference enforcement
- Actor support is minimal entry points only

### Medium Term (Post-v1.0)

Migrate to `codebase/runtime/`:
1. Update Cranelift backend to generate calls to new runtime
2. Update symbol names to match new runtime ABI
3. Add arena allocation primitives to language surface
4. Add generational reference builtins
5. Add actor spawn/send/receive integration

---

## For Compiler Developers

### Adding a New Runtime Function

**Current process:**
1. Add function to `codebase/compiler/runtime/gradient_runtime.c`
2. Declare in Cranelift backend's `declared_functions` map
3. Generate calls via `builder.ins().call()`

**Example:**
```rust
// In cranelift.rs
declared_functions.insert("__gradient_new_feature", "__gradient_new_feature");

// When generating code for the new feature
let func_ref = self.get_or_declare_func(builder, "__gradient_new_feature");
builder.ins().call(func_ref, &[arg1, arg2]);
```

### Testing Runtime Changes

```bash
# Rebuild runtime (no separate step needed—it's C)
cd codebase

# Test with a Gradient program
./target/release/gradient-compiler test.gr test.o --target native
gcc -o test test.o compiler/runtime/gradient_runtime.c -lcurl
./test
```

---

## Open Questions

1. **Should the newer runtime be integrated for v1.0?**
   - Synthesis recommendation: No, document current runtime as authoritative

2. **What's the migration path?**
   - Requires backend codegen changes to target new runtime
   - Requires language surface changes for arena/genref builtins
   - Estimated effort: 2-3 weeks

3. **Why was the new runtime developed separately?**
   - Likely to avoid destabilizing the working compilation path
   - Now requires integration effort to bridge the gap

---

## References

- Adversarial synthesis: `/home/gray/TestingGround/Research/gradient_v1_adversarial_synthesis_for_Hermes_agent.md`
- Current runtime: `codebase/compiler/runtime/gradient_runtime.c`
- New runtime: `codebase/runtime/`
- Cranelift backend: `codebase/compiler/src/codegen/cranelift.rs`
