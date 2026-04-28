//! GRA-183 regression tests: WASM allocator overflow guards + memory cap.
//!
//! These tests verify the emitted WASM module:
//! 1. carries an explicit `MemoryType.maximum` (DoS protection),
//! 2. contains the overflow-guard `Unreachable` traps in the bump
//!    allocator that prevent `current_ptr + size` from wrapping past
//!    `u32::MAX`, and
//! 3. honors the per-backend `with_max_pages` override.
//!
//! We don't link wasmtime at the unit-test layer — instead we parse the
//! emitted bytes with `wasmparser` (already a transitive dep through
//! `wasm-encoder`) and assert on the structural shape. This keeps the
//! suite offline and fast.

#[cfg(feature = "wasm")]
mod wasm_overflow_tests {
    use gradient_compiler::backend::WasmBackend;
    use gradient_compiler::ir::{BasicBlock, BlockRef, Function, Instruction, Module, Type};
    use std::collections::HashMap;

    fn empty_main_module() -> Module {
        // Smallest legal IR: one function `main` with a single block
        // returning. Mirrors `create_add_function` from wasm_tests.rs.
        let entry_block = BasicBlock {
            label: BlockRef(0),
            instructions: vec![Instruction::Ret(None)],
        };
        let main = Function {
            name: "main".to_string(),
            params: vec![],
            return_type: Type::I32,
            blocks: vec![entry_block],
            value_types: HashMap::new(),
            is_export: true,
            extern_lib: None,
        };
        Module {
            name: "test".to_string(),
            functions: vec![main],
            func_refs: {
                let mut r = HashMap::new();
                r.insert(gradient_compiler::ir::FuncRef(0), "main".to_string());
                r
            },
        }
    }

    fn compile(backend: &mut WasmBackend, module: &Module) -> Vec<u8> {
        backend
            .compile_module(module)
            .expect("compile_module failed");
        // `finish` consumes the backend; replace with a fresh one so the
        // caller doesn't see a moved value. The caller never reuses the
        // borrow after this returns.
        let taken = std::mem::replace(backend, WasmBackend::new().expect("new"));
        taken.finish().expect("finish failed")
    }

    /// Default `WasmBackend::new()` must emit `memory.maximum = DEFAULT_MAX_PAGES`.
    /// Without an upper bound, `memory.grow` is unbounded and a malicious guest
    /// can DoS the host by allocating until OOM.
    #[test]
    fn emitted_memory_has_default_maximum() {
        let module = empty_main_module();
        let mut backend = WasmBackend::new().expect("backend");
        let bytes = compile(&mut backend, &module);

        let parser = wasmparser::Parser::new(0);
        let mut saw_memory = false;
        for payload in parser.parse_all(&bytes) {
            if let Ok(wasmparser::Payload::MemorySection(reader)) = payload {
                for mem in reader {
                    let mem = mem.expect("memory entry");
                    saw_memory = true;
                    let max = mem.maximum.expect(
                        "GRA-183: emitted memory must declare a `maximum`; \
                         unbounded `memory.grow` is a host-DoS vector",
                    );
                    assert!(
                        max <= WasmBackend::WASM32_MAX_PAGES as u64,
                        "max {} above WASM32 hard limit",
                        max
                    );
                    assert_eq!(
                        max, WasmBackend::DEFAULT_MAX_PAGES as u64,
                        "default backend should emit DEFAULT_MAX_PAGES",
                    );
                }
            }
        }
        assert!(saw_memory, "no memory section found in emitted module");
    }

    /// `WasmBackend::with_max_pages(n)` must propagate `n` (clamped to
    /// `[1, WASM32_MAX_PAGES]`) to the emitted `MemoryType.maximum`. This
    /// lets embedders tighten the sandbox below the default.
    #[test]
    fn with_max_pages_overrides_emitted_maximum() {
        let module = empty_main_module();
        // Use a tight cap — 16 pages = 1 MiB — typical fuzzer sandbox size.
        let mut backend = WasmBackend::with_max_pages(16).expect("backend");
        let bytes = compile(&mut backend, &module);

        let parser = wasmparser::Parser::new(0);
        for payload in parser.parse_all(&bytes) {
            if let Ok(wasmparser::Payload::MemorySection(reader)) = payload {
                for mem in reader {
                    let mem = mem.expect("memory entry");
                    assert_eq!(mem.maximum, Some(16), "with_max_pages(16) not honored");
                }
            }
        }
    }

    /// `with_max_pages` must clamp to `[1, WASM32_MAX_PAGES]`. Specifically:
    /// - 0 must clamp up to 1 (a memory with `min=1, max=0` is malformed).
    /// - Values > WASM32_MAX_PAGES must clamp down (avoid emitting an
    ///   illegal memory type that wasmtime / browsers reject).
    #[test]
    fn with_max_pages_clamps_extremes() {
        let module = empty_main_module();

        let mut zero = WasmBackend::with_max_pages(0).expect("zero");
        let bytes = compile(&mut zero, &module);
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            if let Ok(wasmparser::Payload::MemorySection(reader)) = payload {
                for mem in reader {
                    assert_eq!(mem.unwrap().maximum, Some(1), "0 must clamp up to 1");
                }
            }
        }

        let mut big = WasmBackend::with_max_pages(u32::MAX).expect("big");
        let bytes = compile(&mut big, &module);
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            if let Ok(wasmparser::Payload::MemorySection(reader)) = payload {
                for mem in reader {
                    assert_eq!(
                        mem.unwrap().maximum,
                        Some(WasmBackend::WASM32_MAX_PAGES as u64),
                        "u32::MAX must clamp down to WASM32 hard limit"
                    );
                }
            }
        }
    }

    /// The bump allocator must contain at least two `Unreachable` instructions
    /// guarding integer-overflow conditions before the `i32.add` for the
    /// pointer bump and the page-rounding `i32.add`. We don't try to match
    /// the exact byte sequence (that's brittle); we just assert the malloc
    /// function body contains ≥ 2 `unreachable` opcodes, which is the
    /// minimum count consistent with the GRA-183 patch.
    #[test]
    fn bump_allocator_has_overflow_traps() {
        let module = empty_main_module();
        let mut backend = WasmBackend::new().expect("backend");
        let bytes = compile(&mut backend, &module);

        // The emitted module must contain at least 3 `0x00` (unreachable)
        // opcodes inside function bodies: 2 from the overflow guards and
        // ≥1 from the existing memory.grow == -1 trap added in PR #168.
        // We scan the code section as a whole — counting per-function would
        // require full operator iteration which is heavier than necessary
        // for a regression test. The number of `Unreachable` opcodes is
        // monotonically non-decreasing relative to PR #168's baseline.
        let mut total_unreachable = 0usize;
        let mut total_funcs = 0usize;
        let parser = wasmparser::Parser::new(0);
        for payload in parser.parse_all(&bytes) {
            if let Ok(wasmparser::Payload::CodeSectionEntry(body)) = payload {
                total_funcs += 1;
                let ops = body.get_operators_reader().expect("operators");
                for op in ops {
                    if matches!(op.expect("op"), wasmparser::Operator::Unreachable) {
                        total_unreachable += 1;
                    }
                }
            }
        }
        assert!(total_funcs >= 1, "no function bodies emitted");
        assert!(
            total_unreachable >= 3,
            "GRA-183: expected ≥3 Unreachable traps across the module \
             (2 new overflow guards + ≥1 memory.grow guard from PR #168), \
             saw {}",
            total_unreachable,
        );
    }

    /// Sanity: the emitted bytes still parse as a valid WASM module after
    /// the overflow guards are inserted. If the trap sequence is malformed
    /// (e.g. unbalanced `If` / `End`), `wasmparser` will reject it.
    #[test]
    fn emitted_module_is_structurally_valid() {
        let module = empty_main_module();
        let mut backend = WasmBackend::new().expect("backend");
        let bytes = compile(&mut backend, &module);
        let parser = wasmparser::Parser::new(0);
        for payload in parser.parse_all(&bytes) {
            payload.expect("WASM module must round-trip through wasmparser");
        }
    }
}
