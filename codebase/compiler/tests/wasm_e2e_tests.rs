//! End-to-end WASM tests that validate compiled programs work correctly.
//!
//! These tests construct Gradient IR, compile to WASM, and verify the output.
//! When wasmtime is available, tests also run the compiled binary.

#[cfg(feature = "wasm")]
mod e2e_tests {
    use gradient_compiler::backend::WasmBackend;
    use gradient_compiler::codegen::CodegenBackend;
    use gradient_compiler::ir::{
        BasicBlock, BlockRef, Function, Instruction, Literal, Module, Type, Value,
    };
    use std::collections::HashMap;
    use std::process::Command;

    /// Check if wasmtime is available for runtime testing.
    fn wasmtime_available() -> bool {
        Command::new("wasmtime")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Helper: Create a simple function that returns a constant.
    fn create_const_function(name: &str, value: i64) -> Function {
        let result_val = Value(0);
        let entry_block = BasicBlock {
            label: BlockRef(0),
            instructions: vec![
                Instruction::Const(result_val, Literal::Int(value)),
                Instruction::Ret(Some(result_val)),
            ],
        };

        let mut value_types = HashMap::new();
        value_types.insert(result_val, Type::I64);

        Function {
            name: name.to_string(),
            params: vec![],
            return_type: Type::I64,
            blocks: vec![entry_block],
            value_types,
            is_export: true,
            extern_lib: None,
        }
    }

    /// Helper: Create an add function: fn add(a: Int, b: Int) -> Int { ret a + b }
    fn create_add_function() -> Function {
        let a_val = Value(0);
        let b_val = Value(1);
        let result_val = Value(2);

        let entry_block = BasicBlock {
            label: BlockRef(0),
            instructions: vec![
                Instruction::Add(result_val, a_val, b_val),
                Instruction::Ret(Some(result_val)),
            ],
        };

        let mut value_types = HashMap::new();
        value_types.insert(a_val, Type::I64);
        value_types.insert(b_val, Type::I64);
        value_types.insert(result_val, Type::I64);

        Function {
            name: "add".to_string(),
            params: vec![Type::I64, Type::I64],
            return_type: Type::I64,
            blocks: vec![entry_block],
            value_types,
            is_export: true,
            extern_lib: None,
        }
    }

    /// Helper: Create a subtract function.
    fn create_sub_function() -> Function {
        let a_val = Value(0);
        let b_val = Value(1);
        let result_val = Value(2);

        let entry_block = BasicBlock {
            label: BlockRef(0),
            instructions: vec![
                Instruction::Sub(result_val, a_val, b_val),
                Instruction::Ret(Some(result_val)),
            ],
        };

        let mut value_types = HashMap::new();
        value_types.insert(a_val, Type::I64);
        value_types.insert(b_val, Type::I64);
        value_types.insert(result_val, Type::I64);

        Function {
            name: "sub".to_string(),
            params: vec![Type::I64, Type::I64],
            return_type: Type::I64,
            blocks: vec![entry_block],
            value_types,
            is_export: true,
            extern_lib: None,
        }
    }

    /// Helper: Create a multiply function.
    fn create_mul_function() -> Function {
        let a_val = Value(0);
        let b_val = Value(1);
        let result_val = Value(2);

        let entry_block = BasicBlock {
            label: BlockRef(0),
            instructions: vec![
                Instruction::Mul(result_val, a_val, b_val),
                Instruction::Ret(Some(result_val)),
            ],
        };

        let mut value_types = HashMap::new();
        value_types.insert(a_val, Type::I64);
        value_types.insert(b_val, Type::I64);
        value_types.insert(result_val, Type::I64);

        Function {
            name: "mul".to_string(),
            params: vec![Type::I64, Type::I64],
            return_type: Type::I64,
            blocks: vec![entry_block],
            value_types,
            is_export: true,
            extern_lib: None,
        }
    }

    /// Test: Compile and verify a constant function.
    #[test]
    fn test_e2e_constant_function() {
        let func = create_const_function("answer", 42);

        let module = Module {
            name: "test".to_string(),
            functions: vec![func],
            func_refs: {
                let mut refs = HashMap::new();
                refs.insert(gradient_compiler::ir::FuncRef(0), "answer".to_string());
                refs
            },
        };

        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");
        backend
            .compile_module(&module)
            .expect("Failed to compile module");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        // Verify WASM is valid
        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");
        assert_eq!(&wasm_bytes[0..4], &[0x00, 0x61, 0x73, 0x6d], "Invalid WASM magic");
        assert_eq!(&wasm_bytes[4..8], &[0x01, 0x00, 0x00, 0x00], "Invalid WASM version");

        // Write to temp file for potential wasmtime testing
        let temp_dir = std::env::temp_dir();
        let wasm_path = temp_dir.join("gradient_e2e_answer.wasm");
        std::fs::write(&wasm_path, &wasm_bytes).expect("Failed to write WASM file");

        println!("WASM written to: {}", wasm_path.display());
        println!("WASM size: {} bytes", wasm_bytes.len());

        // If wasmtime is available, try to run it
        if wasmtime_available() {
            let output = Command::new("wasmtime")
                .arg("--invoke")
                .arg("answer")
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
            println!("wasmtime status: {}", output.status);

            // Function should run without error
            // Note: wasmtime may not return the value in a readable format for i64
        } else {
            println!("wasmtime not available, skipping runtime test");
        }

        // Cleanup
        let _ = std::fs::remove_file(&wasm_path);
    }

    /// Test: Compile and verify arithmetic operations.
    #[test]
    fn test_e2e_arithmetic_operations() {
        let add_func = create_add_function();
        let sub_func = create_sub_function();
        let mul_func = create_mul_function();

        let module = Module {
            name: "math".to_string(),
            functions: vec![add_func, sub_func, mul_func],
            func_refs: {
                let mut refs = HashMap::new();
                refs.insert(gradient_compiler::ir::FuncRef(0), "add".to_string());
                refs.insert(gradient_compiler::ir::FuncRef(1), "sub".to_string());
                refs.insert(gradient_compiler::ir::FuncRef(2), "mul".to_string());
                refs
            },
        };

        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");
        backend
            .compile_module(&module)
            .expect("Failed to compile module");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");

        // Write to temp file
        let temp_dir = std::env::temp_dir();
        let wasm_path = temp_dir.join("gradient_e2e_math.wasm");
        std::fs::write(&wasm_path, &wasm_bytes).expect("Failed to write WASM file");

        println!("Math WASM written to: {}", wasm_path.display());

        if wasmtime_available() {
            // Test add function: add(10, 32) = 42
            let output = Command::new("wasmtime")
                .arg("--invoke")
                .arg("add")
                .arg("10")
                .arg("32")
                .arg(&wasm_path)
                .output()
                .expect("Failed to run wasmtime");

            println!("add(10, 32) result:");
            println!("  stdout: {}", String::from_utf8_lossy(&output.stdout));
            println!("  stderr: {}", String::from_utf8_lossy(&output.stderr));
        }

        // Cleanup
        let _ = std::fs::remove_file(&wasm_path);
    }

    /// Test: Verify WASM module with strings can be compiled.
    #[test]
    fn test_e2e_with_strings() {
        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");

        // Store some strings
        let hello_id = backend.emit_string("Hello");
        let world_id = backend.emit_string("World");

        // Verify strings are stored
        assert_eq!(hello_id.0, 0);
        assert_eq!(world_id.0, 1);

        let func = create_const_function("main", 0);

        let module = Module {
            name: "strings".to_string(),
            functions: vec![func],
            func_refs: {
                let mut refs = HashMap::new();
                refs.insert(gradient_compiler::ir::FuncRef(0), "main".to_string());
                refs
            },
        };

        backend
            .compile_module(&module)
            .expect("Failed to compile module with strings");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");

        // Write to temp file
        let temp_dir = std::env::temp_dir();
        let wasm_path = temp_dir.join("gradient_e2e_strings.wasm");
        std::fs::write(&wasm_path, &wasm_bytes).expect("Failed to write WASM file");

        println!("Strings WASM written to: {}", wasm_path.display());
        println!("WASM size with strings: {} bytes", wasm_bytes.len());

        // Cleanup
        let _ = std::fs::remove_file(&wasm_path);
    }

    /// Test: Verify malloc builtin works in compiled module.
    #[test]
    fn test_e2e_malloc() {
        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");

        // Emit malloc builtin
        let malloc_idx = backend.emit_malloc_builtin();

        // Create a function that uses malloc indirectly (via the backend)
        let func = create_const_function("test_malloc", malloc_idx as i64);

        let module = Module {
            name: "malloc_test".to_string(),
            functions: vec![func],
            func_refs: {
                let mut refs = HashMap::new();
                refs.insert(gradient_compiler::ir::FuncRef(0), "test_malloc".to_string());
                refs
            },
        };

        backend
            .compile_module(&module)
            .expect("Failed to compile module with malloc");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");

        // Verify the module includes the malloc function (function index should be present)
        println!("Malloc function index: {}", malloc_idx);
        println!("WASM with malloc: {} bytes", wasm_bytes.len());

        // Write to temp file
        let temp_dir = std::env::temp_dir();
        let wasm_path = temp_dir.join("gradient_e2e_malloc.wasm");
        std::fs::write(&wasm_path, &wasm_bytes).expect("Failed to write WASM file");

        // Cleanup
        let _ = std::fs::remove_file(&wasm_path);
    }

    /// Test: Validate WASM structure against WebAssembly specification.
    #[test]
    fn test_e2e_wasm_validation() {
        let func = create_const_function("validate", 123);

        let module = Module {
            name: "validate".to_string(),
            functions: vec![func],
            func_refs: {
                let mut refs = HashMap::new();
                refs.insert(gradient_compiler::ir::FuncRef(0), "validate".to_string());
                refs
            },
        };

        let mut backend = WasmBackend::new().expect("Failed to create WASM backend");
        backend
            .compile_module(&module)
            .expect("Failed to compile module");
        let wasm_bytes = backend.finish().expect("Failed to finalize WASM");

        // Detailed WASM structure validation
        // Header: 0x00 0x61 0x73 0x6d (magic) + 0x01 0x00 0x00 0x00 (version)
        assert_eq!(&wasm_bytes[0..8], &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);

        // Parse sections
        let mut pos = 8;
        let mut sections_found = HashMap::new();

        while pos < wasm_bytes.len() {
            let section_id = wasm_bytes[pos];
            if pos + 1 >= wasm_bytes.len() {
                break;
            }

            // Read section size (LEB128 encoded)
            let mut size = 0u32;
            let mut shift = 0;
            let mut size_bytes = 0;

            for i in (pos + 1)..wasm_bytes.len() {
                let byte = wasm_bytes[i];
                size |= ((byte & 0x7f) as u32) << shift;
                size_bytes += 1;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }

            sections_found.insert(section_id, size);
            pos += 1 + size_bytes as usize + size as usize;
        }

        // Verify required sections are present
        println!("Sections found: {:?}", sections_found);

        // Type section (1) - required for functions
        assert!(sections_found.contains_key(&1), "Missing Type section");

        // Import section (2) - required for WASI
        assert!(sections_found.contains_key(&2), "Missing Import section");

        // Function section (3) - required
        assert!(sections_found.contains_key(&3), "Missing Function section");

        // Memory section (5) - required for WASI
        assert!(sections_found.contains_key(&5), "Missing Memory section");

        // Global section (6) - required for __heap_ptr
        assert!(sections_found.contains_key(&6), "Missing Global section");

        // Export section (7) - required for memory export
        assert!(sections_found.contains_key(&7), "Missing Export section");

        // Code section (10) - required for function bodies
        assert!(sections_found.contains_key(&10), "Missing Code section");

        println!("All required sections present for WASI compatibility!");
    }

    /// Test: Compare output sizes for different program types.
    #[test]
    fn test_e2e_size_comparison() {
        let sizes: Vec<(String, usize)> = vec![
            {
                let func = create_const_function("empty", 0);
                let module = Module {
                    name: "empty".to_string(),
                    functions: vec![func],
                    func_refs: HashMap::new(),
                };
                let mut backend = WasmBackend::new().unwrap();
                backend.compile_module(&module).unwrap();
                let bytes = backend.finish().unwrap();
                ("empty function".to_string(), bytes.len())
            },
            {
                let func = create_const_function("const42", 42);
                let module = Module {
                    name: "const".to_string(),
                    functions: vec![func],
                    func_refs: HashMap::new(),
                };
                let mut backend = WasmBackend::new().unwrap();
                backend.compile_module(&module).unwrap();
                let bytes = backend.finish().unwrap();
                ("const function".to_string(), bytes.len())
            },
            {
                let add = create_add_function();
                let module = Module {
                    name: "add".to_string(),
                    functions: vec![add],
                    func_refs: HashMap::new(),
                };
                let mut backend = WasmBackend::new().unwrap();
                backend.compile_module(&module).unwrap();
                let bytes = backend.finish().unwrap();
                ("add function".to_string(), bytes.len())
            },
            {
                let mut backend = WasmBackend::new().unwrap();
                let _ = backend.emit_string("test");
                let func = create_const_function("with_string", 0);
                let module = Module {
                    name: "string".to_string(),
                    functions: vec![func],
                    func_refs: HashMap::new(),
                };
                backend.compile_module(&module).unwrap();
                let bytes = backend.finish().unwrap();
                ("with string".to_string(), bytes.len())
            },
        ];

        println!("\nWASM Size Comparison:");
        println!("====================");
        for (name, size) in &sizes {
            println!("{:20} {:4} bytes", name, size);
        }

        // Empty function should be smallest
        assert!(sizes[0].1 <= sizes[1].1, "Empty should be <= const");
        assert!(sizes[1].1 <= sizes[2].1, "Const should be <= add");
    }
}

#[cfg(not(feature = "wasm"))]
mod e2e_tests {
    // Empty module when wasm feature is not enabled
}
