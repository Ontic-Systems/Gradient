# Gradient WASM Target Implementation Plan

## Overview
Implement WebAssembly compilation target for Gradient compiler.

## Parallel Workstreams (3 Concurrent)

---

### Workstream 1: WASM Infrastructure & Dependencies
**Goal:** Set up module structure and core WASM encoder

**Files:**
- `codebase/compiler/Cargo.toml` - Add wasm-encoder dependency
- `codebase/compiler/src/codegen/wasm.rs` - New WASM backend module
- `codebase/compiler/src/codegen/mod.rs` - Add WasmBackend variant

**Key Tasks:**
1. Add `wasm-encoder = "0.200"` to Cargo.toml
2. Create `WasmBackend` struct with:
   - `module: Module` (wasm_encoder::Module)
   - `function_count: u32`
   - `local_count: u32`
   - `value_map: HashMap<Value, u32>` (IR value → WASM local index)
   - `function_map: HashMap<String, u32>` (func name → WASM func index)
3. Implement `Backend` trait methods:
   - `new()` - Create new WASM module
   - `emit_bytes()` - Return encoded WASM bytes
   - `compile_module()` - Main compilation entry point

**Deliverable:** WASM backend compiles, can emit empty module.

---

### Workstream 2: IR-to-WASM Instruction Translation
**Goal:** Map Gradient IR instructions to WASM opcodes

**Files:**
- `codebase/compiler/src/codegen/wasm.rs` - Instruction encoding methods

**Key Tasks:**
1. Implement type mapping:
   - `ir_type_to_wasm(Type) -> ValType`
   - Ptr → i32 (wasm32 target)
2. Implement instruction encoding for:
   - Constants (i32.const, i64.const, f32.const, f64.const)
   - Arithmetic (i32.add/sub/mul/div, i64.add/sub/mul/div)
   - Memory (i32.load/store, i64.load/store with alignment)
   - Comparison (i32.eq/lt/gt, i64.eq/lt/gt)
   - Control (block, end, br, br_if, call, return)
   - Locals (local.get, local.set, local.tee)
3. Implement function compilation:
   - Map IR values to WASM local indices
   - Generate function bodies

**Deliverable:** Can compile simple arithmetic functions to WASM.

---

### Workstream 3: Memory, Data & WASI Integration
**Goal:** Linear memory, string data, and I/O support

**Files:**
- `codebase/compiler/src/codegen/wasm.rs` - Memory and data sections
- `codebase/compiler/tests/wasm_tests.rs` - Integration tests

**Key Tasks:**
1. Linear memory setup:
   - Export memory with initial size (e.g., 64KB = 1 page)
   - Add memory import for WASI compatibility
2. String data encoding:
   - Encode string literals to data section
   - Map string data IDs to memory offsets
3. Bump allocator for heap:
   - Global `__heap_ptr` for next allocation
   - `malloc` function in WASM that bumps pointer
4. WASI imports:
   - Import `wasi_snapshot_preview1::fd_write` for println
   - Import `wasi_snapshot_preview1::proc_exit` for exit
5. Integration tests:
   - Test compiling simple program
   - Run with wasmtime
   - Verify output

**Deliverable:** Can compile and run hello-world in wasmtime.

---

## Integration Points

### Between Workstreams 1 & 2:
- Workstream 1 defines `WasmBackend` struct and `compile_function()` signature
- Workstream 2 implements the function body encoding that `compile_function()` calls

### Between Workstreams 2 & 3:
- Workstream 2 needs memory ops (load/store) that Workstream 3 sets up
- Workstream 3 needs instruction encoding from Workstream 2 for runtime functions

### All Workstreams → Main:
- Must update `BackendWrapper` enum in `codegen/mod.rs`
- Must add `wasm` feature flag to Cargo.toml

---

## Testing Checklist

- [ ] Empty module emits valid WASM
- [ ] Simple arithmetic compiles and runs
- [ ] Memory allocation works (malloc/free)
- [ ] String constants accessible
- [ ] Function calls work
- [ ] Println outputs to stdout via WASI
- [ ] Full program runs in wasmtime

---

## Commit Structure

```
feat(wasm): implement WebAssembly compilation target

Infrastructure:
- Add wasm-encoder dependency
- Create WasmBackend module
- Add wasm feature flag

Instruction Encoding:
- IR-to-WASM type mapping
- Arithmetic, memory, control instructions
- Function compilation pipeline

Memory & WASI:
- Linear memory setup and export
- String data section encoding
- Bump allocator for heap
- WASI imports for I/O

Tests:
- Unit tests for encoder
- Integration tests with wasmtime

Refs: ONT-XX
```
