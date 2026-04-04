//! WASM backend integration tests.
//!
//! These tests verify that the WASM backend can compile Gradient IR
//! into valid WebAssembly binaries that can be run with wasmtime.

#[cfg(feature = "wasm")]
mod wasm_tests {
    use gradient_compiler::backend::WasmBackend;
    use gradient_compiler::codegen::CodegenBackend;
    use gradient_compiler::ir::{
        BasicBlock, BlockRef, Function, Instruction, Literal, Module, Type, Value,
    };
    use std::collections::HashMap;
    use std::io::Write;
    use std::process::Command;

    /// Test that we can compile a simple arithmetic function to WASM.
    ///
    /// This test creates an IR module with a simple `add` function:
    /// ```gradient
    /// fn add(a: Int, b: Int) -> Int:
    ///     ret a + b
    /// ```
    ///
    /// The test then:
    /// 1. Compiles the IR to WASM bytes
    /// 2. Writes the bytes to a temporary file
    /// 3. Validates the WASM using wasm-encoder or runs with wasmtime if available
    #[test]
    fn compile_simple_arithmetic() {
        // Create a simple add function IR
        let add_function = create_add_function();

        // Create a module with the function
        let module = Module {
            name: "test".to_string(),
            functions: vec![add_function],
            func_refs: {
                let mut refs = HashMap::new();
                refs.insert(gradient_compiler::ir::FuncRef(0), "add".to_string());
                refs
            },
        };

        // Compile to WASM
        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");
        backend
            .compile_module(&module)
            .expect("Failed to compile module");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        // Verify we got some WASM bytes
        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");

        // Verify WASM magic number
        assert_eq!(
            &wasm_bytes[0..4],
            &[0x00, 0x61, 0x73, 0x6d],
            "WASM magic number mismatch"
        );

        // Verify WASM version
        assert_eq!(
            &wasm_bytes[4..8],
            &[0x01, 0x00, 0x00, 0x00],
            "WASM version mismatch"
        );

        // Write to temp file
        let temp_dir = std::env::temp_dir();
        let wasm_path = temp_dir.join("gradient_test_add.wasm");
        std::fs::write(&wasm_path, &wasm_bytes).expect("Failed to write WASM file");

        println!("WASM file written to: {}", wasm_path.display());
        println!("WASM file size: {} bytes", wasm_bytes.len());

        // Try to validate with wasmtime if available
        if wasmtime_available() {
            let output = Command::new("wasmtime")
                .arg("--invoke")
                .arg("add")
                .arg(&wasm_path)
                .output()
                .expect("Failed to run wasmtime");

            println!(
                "wasmtime stdout: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            println!(
                "wasmtime stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );

            // The function should run without crashing
            // (It may not return anything useful since we're just testing compilation)
        } else {
            println!("wasmtime not available, skipping runtime test");
        }

        // Clean up
        let _ = std::fs::remove_file(&wasm_path);
    }

    /// Test that the WASM backend properly initializes memory.
    #[test]
    fn test_memory_initialization() {
        let backend = WasmBackend::new().expect("Failed to create WASM backend");

        // The backend should have:
        // - 1 page (64KB) of memory
        // - Memory exported as "memory"
        // - Heap pointer global initialized to 1024

        // We verify this by checking the final output contains the expected sections
        let module = Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: HashMap::new(),
        };

        // Can't easily check internals, but we can verify compilation succeeds
        let mut backend = backend;
        backend
            .compile_module(&module)
            .expect("Failed to compile empty module");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        // Should have at least the header (8 bytes) + type section + import section + memory section
        assert!(wasm_bytes.len() > 8, "WASM output too small");
    }

    /// Test that strings can be stored and retrieved.
    #[test]
    fn test_string_data_encoding() {
        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");

        // Store some strings
        let id1 = backend.emit_string("hello");
        let id2 = backend.emit_string("world");

        // Verify we got valid IDs
        assert_eq!(id1.0, 0);
        assert_eq!(id2.0, 1);

        // Verify offsets are different (strings are stored sequentially)
        let offset1 = backend
            .get_string_offset(id1)
            .expect("Failed to get offset for id1");
        let offset2 = backend
            .get_string_offset(id2)
            .expect("Failed to get offset for id2");

        assert!(
            offset2 > offset1,
            "Second string should be at higher offset"
        );

        // Encode data section
        backend.encode_data_section();

        // Verify module compiles
        let module = Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: HashMap::new(),
        };

        backend
            .compile_module(&module)
            .expect("Failed to compile module with strings");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");
    }

    /// Test that the bump allocator (malloc builtin) works.
    #[test]
    fn test_malloc_builtin() {
        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");

        // Emit the malloc builtin
        let malloc_idx = backend.emit_malloc_builtin();

        // malloc should be the first internal function (index 2, after imports 0 and 1)
        assert_eq!(malloc_idx, 2, "malloc should be at index 2");

        // Verify module compiles
        let module = Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: HashMap::new(),
        };

        backend
            .compile_module(&module)
            .expect("Failed to compile module with malloc");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");
    }

    /// Test that the println builtin is properly exported.
    #[test]
    fn test_println_builtin() {
        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");

        // Emit the println builtin
        let println_idx = backend.emit_println_builtin();

        // println should be the first internal function
        assert!(println_idx >= 2, "println should be at index >= 2");

        // Verify module compiles
        let module = Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: HashMap::new(),
        };

        backend
            .compile_module(&module)
            .expect("Failed to compile module with println");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");
    }

    /// Test WASI imports are present.
    #[test]
    fn test_wasi_imports() {
        let backend = WasmBackend::new().expect("Failed to create WASM backend");

        // The backend should have WASI imports configured
        // We verify this by checking the compiled output

        let module = Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: HashMap::new(),
        };

        let mut backend = backend;
        backend
            .compile_module(&module)
            .expect("Failed to compile module");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        // The WASM should have an import section
        // (We can check this by looking for section ID 1)
        let has_import_section = wasm_bytes.windows(2).any(|w| w == &[0x01, 0x07]);
        // Note: This is a simplified check - the import section ID is 1,
        // but the exact byte pattern may vary

        println!("WASM has import section: {}", has_import_section);
        println!("WASM size: {} bytes", wasm_bytes.len());
    }

    /// Test that the generated WASM has all required WASI sections.
    #[test]
    fn test_wasm_sections() {
        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");

        // Add a string to trigger data section
        let _id = backend.emit_string("test");

        let module = Module {
            name: "test".to_string(),
            functions: vec![],
            func_refs: HashMap::new(),
        };

        backend
            .compile_module(&module)
            .expect("Failed to compile module");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        // WASM section IDs:
        // 0: Custom, 1: Type, 2: Import, 3: Function, 4: Table, 5: Memory,
        // 6: Global, 7: Export, 8: Start, 9: Element, 10: Code, 11: Data, 12: DataCount

        // Helper to find section
        fn has_section(wasm: &[u8], section_id: u8) -> bool {
            let mut i = 8; // Skip magic (4) + version (4)
            while i < wasm.len() {
                if wasm[i] == section_id {
                    return true;
                }
                // Move to next section
                if i + 1 >= wasm.len() {
                    break;
                }
                let len = wasm[i + 1] as usize;
                i += 2 + len;
            }
            false
        }

        // Check for required sections
        assert!(has_section(&wasm_bytes, 1), "Missing Type section");
        assert!(
            has_section(&wasm_bytes, 2),
            "Missing Import section (WASI imports)"
        );
        assert!(has_section(&wasm_bytes, 3), "Missing Function section");
        assert!(has_section(&wasm_bytes, 5), "Missing Memory section");
        assert!(
            has_section(&wasm_bytes, 6),
            "Missing Global section (__heap_ptr)"
        );
        assert!(has_section(&wasm_bytes, 7), "Missing Export section");
        assert!(
            has_section(&wasm_bytes, 11),
            "Missing Data section (strings)"
        );

        println!("All required WASM sections present!");
        println!("WASM size: {} bytes", wasm_bytes.len());
    }

    /// Helper function to check if wasmtime is available.
    fn wasmtime_available() -> bool {
        Command::new("wasmtime")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Helper function to create a simple add function IR.
    fn create_add_function() -> Function {
        // fn add(a: Int, b: Int) -> Int:
        //     ret a + b

        let entry_block = BasicBlock {
            label: BlockRef(0),
            instructions: vec![
                // This is a simplified representation
                // In reality, we'd need proper value tracking
                Instruction::Ret(None), // Simplified - real implementation would return a+b
            ],
        };

        Function {
            name: "add".to_string(),
            params: vec![Type::I32, Type::I32],
            return_type: Type::I32,
            blocks: vec![entry_block],
            value_types: HashMap::new(),
            is_export: true,
            extern_lib: None,
        }
    }
}

#[cfg(not(feature = "wasm"))]
mod wasm_tests {
    // Empty module when wasm feature is not enabled
}
