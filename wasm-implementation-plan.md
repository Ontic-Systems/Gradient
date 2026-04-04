# Gradient WASM Target - Refined Implementation Plan

## Current Status: 70% Complete ✅

The WASM backend already has substantial implementation:
- **723 lines** in `compiler/src/codegen/wasm.rs`
- **6/6 tests passing** in `wasm_tests.rs`
- **Integrated** into `BackendWrapper` enum
- **CLI accessible** via `--backend wasm` flag

## Remaining Work (3 Workstreams)

---

## Workstream 1: CLI & Output Handling (Priority: HIGH)

**Goal:** Proper WASM file output and user experience

**Files:**
- `compiler/src/main.rs` - Output file handling
- `compiler/src/codegen/wasm.rs` - Export section improvements

**Tasks:**
1. Auto-detect WASM output by file extension
   - If output file ends with `.wasm`, use WASM backend automatically
   - Example: `gradient input.gr output.wasm` should select WASM backend
   
2. Fix output messages for WASM target
   - Current: "Wrote object file: output.wasm"
   - Current: "Link with: cc ..." (wrong for WASM)
   - Should suggest: "Run with: wasmtime output.wasm"
   
3. Add `--target wasm32` flag as alternative to `--backend wasm`
   - More intuitive for users familiar with target triples
   - Can be extended later to `wasm64`, `x86_64`, etc.

**Deliverable:** `gradient input.gr output.wasm` produces valid, runnable WASM

---

## Workstream 2: WASI Runtime Integration (Priority: HIGH)

**Goal:** Working I/O and system calls in WASM environment

**Files:**
- `runtime/wasm_runtime.c` - WASI-compatible runtime (new file)
- `compiler/src/codegen/wasm.rs` - WASI import generation
- `compiler/src/codegen/wasm.rs` - Memory section setup

**Tasks:**
1. WASI Import Section
   ```rust
   // Import required WASI functions
   wasi_snapshot_preview1::fd_write  // for println
   wasi_snapshot_preview1::proc_exit // for exit
   wasi_snapshot_preview1::fd_read   // for input
   ```

2. Memory Section Enhancement
   - Export memory as "memory" (required by WASI)
   - Initial size: 1 page (64KB)
   - Maximum: optional (suggest 100 pages for ~6MB)
   
3. Bump Allocator in WASM
   - Global `__heap_ptr` at offset 64KB+ (after static data)
   - Simple malloc that bumps pointer
   - No free() needed for MVP (bump allocation only)

4. Println Implementation
   - Map `gradient_println` to WASI fd_write
   - Use iovec structure: [ptr, len]
   - Write to stdout (fd=1)

**Deliverable:** Hello-world program prints to stdout when run with wasmtime

---

## Workstream 3: Runtime Library & E2E Testing (Priority: MEDIUM)

**Goal:** Full gradient runtime working in WASM environment

**Files:**
- `runtime/wasm_runtime.c` - WASM-compatible runtime library
- `compiler/tests/wasm_e2e_tests.rs` - End-to-end tests (exists, may need updates)
- `.github/workflows/wasm.yml` - CI for WASM target

**Tasks:**
1. WASM-compatible runtime library
   - Port `runtime/gradient_runtime.c` to WASI
   - Replace file I/O with WASI equivalents
   - Keep memory allocation minimal (no libc malloc)
   
2. String handling in WASM
   - Encode string literals to data section
   - Pass strings as (ptr, len) pairs
   - UTF-8 validation before WASI fd_write
   
3. Actor runtime (WASM-compatible)
   - Single-threaded actor loop (WASM has no threads yet)
   - Message queue in linear memory
   - Async/await pattern for Ask operations

4. E2E Test Suite
   - Test: Compile and run simple arithmetic
   - Test: String concatenation and printing
   - Test: File I/O (via WASI)
   - Test: Actor spawn/send/receive
   
5. CI Integration
   - Install wasmtime in GitHub Actions
   - Run WASM tests on every PR
   - Validate output binary with wasm-validate

**Deliverable:** Full Gradient programs run correctly in wasmtime

---

## Integration Points

### Between Workstreams 1 & 2:
- Workstream 1 CLI outputs `.wasm` files
- Workstream 2 ensures those files work with wasmtime
- WASI imports must be declared before code section

### Between Workstreams 2 & 3:
- Workstream 2 provides fd_write for println
- Workstream 3 provides full stdlib (string, list, map ops)
- Runtime library must be linked (statically included)

---

## Testing Checklist

| Test | Status | Notes |
|------|--------|-------|
| Empty module emits valid WASM | ✅ | `test_wasm_backend_creation` |
| Simple arithmetic compiles | ✅ | `compile_simple_arithmetic` |
| WASM magic number correct | ✅ | `compile_simple_arithmetic` |
| Memory initialization | ✅ | `test_memory_initialization` |
| String data encoding | ✅ | `test_string_data_encoding` |
| WASI imports present | ✅ | `test_wasi_imports` |
| Println outputs to stdout | 🔄 | Needs E2E test with wasmtime |
| Full program runs | 🔄 | Needs integration test |
| Actor runtime works | 🔄 | Needs spawn/send/receive test |
| File I/O works | 🔄 | Needs WASI fd_read/fd_write test |

---

## Implementation Order

1. **Week 1:** Workstream 1 (CLI improvements)
   - Quick win, improves UX immediately
   - ~50 lines of code

2. **Week 1-2:** Workstream 2 (WASI integration)
   - Core functionality for real programs
   - ~200-300 lines of code
   - Depends on: Workstream 1 for testing

3. **Week 2-3:** Workstream 3 (E2E testing & runtime)
   - Full feature parity
   - ~300-400 lines of code
   - Depends on: Workstream 2 for I/O

---

## Commit Structure

```
feat(wasm): add CLI support for .wasm output

- Auto-select WASM backend for .wasm extension
- Add --target wasm32 flag
- Update output messages for WASM target

Refs: ONT-XX
```

```
feat(wasm): add WASI imports and memory section

- Import wasi_snapshot_preview1::fd_write
- Export linear memory with proper limits
- Implement bump allocator for heap
- Map gradient_println to WASI

Refs: ONT-XX
```

```
feat(wasm): end-to-end test suite and CI

- Add wasmtime-based E2E tests
- Create WASM-compatible runtime library
- Test file I/O, actors, strings
- Add GitHub Actions workflow

Refs: ONT-XX
```

---

## Open Questions

1. **Memory Model:** Should we support wasm64 (memory64 proposal) or stick to wasm32?
   - Recommendation: wasm32 for now (maximum compatibility)
   
2. **Actor Threads:** WASM doesn't have threads yet ( proposal still in progress)
   - Recommendation: Single-threaded actor runtime with async loop
   
3. **GC vs Manual Memory:** Should we add a simple GC for WASM?
   - Recommendation: Bump allocator for MVP, GC later if needed

4. **Browser vs Server:** Do we need browser-specific exports?
   - Recommendation: WASI target first (server-side), browser bindings later

---

## Success Criteria

✅ **Definition of Done:**
- [ ] `gradient input.gr output.wasm` works without `--backend` flag
- [ ] Output file runs with `wasmtime output.wasm` and produces correct result
- [ ] All 6 existing WASM tests still pass
- [ ] 3+ new E2E tests pass with wasmtime
- [ ] CI runs WASM tests on every PR
- [ ] Documentation updated with WASM usage examples
