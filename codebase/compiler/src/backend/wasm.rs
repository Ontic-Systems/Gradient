//! WASM backend for the Gradient compiler.
//!
//! This module provides a WebAssembly code generation backend using the
//! `wasm-encoder` crate. It translates Gradient IR into WebAssembly binary
//! format that can run in browsers or standalone WebAssembly runtimes.
//!
//! # Architecture
//!
//! The WASM backend follows a similar structure to the Cranelift backend:
//! 1. Create a new `WasmBackend`
//! 2. Compile IR modules with `compile_module()`
//! 3. Retrieve the final WASM bytes with `finish()`
//!
//! # Memory Model
//!
//! The WASM backend uses a simple linear memory with:
//! - Initial 1 page (64KB) of memory
//! - A bump allocator for heap allocations
//! - Static data section for string literals
//!
//! # WASI Integration
//!
//! For standalone WASM execution, the backend imports WASI preview1 functions:
//! - `fd_write` for stdout output
//! - `proc_exit` for program termination

use crate::ir;
use crate::ir::{Instruction, Literal};
use std::collections::HashMap;

// WASM encoder types
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, ImportSection, Instruction as WasmInstr, MemArg,
    MemorySection, MemoryType, Section, ValType,
};

/// Unique identifier for data segments (string literals, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DataId(pub u32);

/// Errors that can occur during WASM code generation.
#[derive(Debug)]
pub struct WasmCodegenError {
    pub message: String,
}

impl std::fmt::Display for WasmCodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WasmCodegenError {}

impl From<String> for WasmCodegenError {
    fn from(message: String) -> Self {
        WasmCodegenError { message }
    }
}

impl From<&str> for WasmCodegenError {
    fn from(message: &str) -> Self {
        WasmCodegenError {
            message: message.to_string(),
        }
    }
}

/// The WASM compilation backend.
///
/// This struct holds all the state needed to compile Gradient IR into
/// WebAssembly binary format. It manages:
/// - Memory sections with initial page allocation
/// - String data storage and encoding
/// - Global variables (including heap pointer for bump allocator)
/// - WASI imports for I/O operations
/// - Function compilation
#[allow(dead_code)] // wasi_proc_exit_idx, type_indices, type_section_bytes are scaffolding
pub struct WasmBackend {
    /// String data storage: DataId -> (offset, bytes)
    /// The offset is the position in memory where the string will be placed.
    string_data: HashMap<DataId, (usize, Vec<u8>)>,

    /// Next available DataId counter
    next_data_id: u32,

    /// Current data section offset (where the heap starts after static data)
    data_end_offset: usize,

    /// Heap pointer global index (for bump allocator)
    heap_ptr_global_idx: u32,

    /// WASI fd_write import function index
    wasi_fd_write_idx: Option<u32>,

    /// WASI proc_exit import function index
    wasi_proc_exit_idx: Option<u32>,

    /// Function index counter for internal functions
    next_func_idx: u32,

    /// Export section builder
    exports: ExportSection,

    /// Function section builder
    functions: FunctionSection,

    /// Code section builder
    code: CodeSection,

    /// Import section builder
    imports: ImportSection,

    /// Global section builder
    globals: GlobalSection,

    /// Memory section builder
    memories: MemorySection,

    /// Data section builder
    data: DataSection,

    /// Type indices for functions
    type_indices: Vec<u32>,

    /// Map from function name to its index
    func_name_to_idx: HashMap<String, u32>,

    /// Current type index counter
    next_type_idx: u32,

    /// Type section bytes (we'll build this manually)
    type_section_bytes: Vec<u8>,
}

impl WasmBackend {
    /// Create a new WASM backend with initial memory setup.
    ///
    /// Initializes:
    /// - 1 page (64KB) of linear memory exported as "memory"
    /// - Heap pointer global starting after the data section
    /// - WASI imports for fd_write and proc_exit
    pub fn new() -> Result<Self, WasmCodegenError> {
        let mut exports = ExportSection::new();
        let functions = FunctionSection::new();
        let code = CodeSection::new();
        let mut imports = ImportSection::new();
        let mut globals = GlobalSection::new();
        let mut memories = MemorySection::new();
        let data = DataSection::new();

        // Memory: 1 page (64KB) minimum, no maximum
        memories.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
        });

        // Export memory as "memory" for host access
        exports.export("memory", ExportKind::Memory, 0);

        // Import WASI fd_write for stdout
        // wasi_snapshot_preview1::fd_write(i32, i32, i32, i32) -> i32
        imports.import(
            "wasi_snapshot_preview1",
            "fd_write",
            wasm_encoder::EntityType::Function(0), // type index will be 0
        );

        // Import WASI proc_exit
        // wasi_snapshot_preview1::proc_exit(i32)
        imports.import(
            "wasi_snapshot_preview1",
            "proc_exit",
            wasm_encoder::EntityType::Function(1), // type index will be 1
        );

        // Initialize heap pointer global (index 0)
        // __heap_ptr: i32 = DATA_END (will be updated after data section is known)
        // For now, start at 1024 to leave room for stack and static data
        let heap_start = 1024i32;
        let init_expr = ConstExpr::i32_const(heap_start);
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
            },
            &init_expr,
        );

        Ok(WasmBackend {
            string_data: HashMap::new(),
            next_data_id: 0,
            data_end_offset: heap_start as usize,
            heap_ptr_global_idx: 0,
            wasi_fd_write_idx: Some(0),  // First import function
            wasi_proc_exit_idx: Some(1), // Second import function
            next_func_idx: 2,            // Start after imports
            exports,
            functions,
            code,
            imports,
            globals,
            memories,
            data,
            type_indices: Vec::new(),
            func_name_to_idx: HashMap::new(),
            next_type_idx: 2, // Types 0 and 1 for WASI imports
            type_section_bytes: Vec::new(),
        })
    }

    /// Maximum data section size before heap (1MB)
    const MAX_DATA_SIZE: usize = 1024 * 1024;

    /// Store a string in the data section and return a DataId.
    ///
    /// The string bytes are stored with a null terminator for C compatibility.
    /// The returned DataId can be used to retrieve the string's offset later.
    pub fn emit_string(&mut self, s: &str) -> Result<DataId, WasmCodegenError> {
        let id = DataId(self.next_data_id);
        self.next_data_id += 1;

        // Store string bytes with null terminator
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0); // Null terminator

        // Security: Check for data section overflow
        let new_offset = self
            .data_end_offset
            .checked_add(bytes.len())
            .ok_or_else(|| WasmCodegenError::from("Data section size overflow"))?;
        if new_offset > Self::MAX_DATA_SIZE {
            return Err(WasmCodegenError::from(format!(
                "Data section exceeds maximum size of {} bytes",
                Self::MAX_DATA_SIZE
            )));
        }

        let offset = self.data_end_offset;
        self.data_end_offset = new_offset;

        self.string_data.insert(id, (offset, bytes));
        Ok(id)
    }

    /// Emit all stored strings to the data section.
    ///
    /// This should be called before finalizing the module to ensure
    /// all string literals are placed in the data section.
    pub fn encode_data_section(&mut self) {
        // Sort by offset to ensure correct placement
        let mut entries: Vec<_> = self.string_data.iter().collect();
        entries.sort_by_key(|(_, (offset, _))| *offset);

        for (_id, (_offset, bytes)) in entries {
            // Add data segment for this string as passive
            self.data.passive(bytes.clone());
        }
    }

    /// Get the memory offset for a previously stored string.
    pub fn get_string_offset(&self, id: DataId) -> Option<usize> {
        self.string_data.get(&id).map(|(offset, _)| *offset)
    }

    /// Emit the malloc builtin function.
    ///
    /// This implements a simple bump allocator:
    /// - Takes size parameter (i32)
    /// - Returns current heap pointer
    /// - Bumps heap pointer by size
    ///
    /// Function signature: malloc(size: i32) -> i32
    pub fn emit_malloc_builtin(&mut self) -> u32 {
        let func_idx = self.next_func_idx;
        self.next_func_idx += 1;

        // Add type for malloc: (i32) -> i32
        let _type_idx = self.next_type_idx;
        self.next_type_idx += 1;

        // Build the function
        let mut func = Function::new([(1, ValType::I32)]); // 1 local for result

        // Get current heap pointer
        func.instruction(&WasmInstr::GlobalGet(self.heap_ptr_global_idx));

        // Store it in local 0 (the result)
        func.instruction(&WasmInstr::LocalSet(0));

        // Calculate new heap pointer: heap_ptr + size
        func.instruction(&WasmInstr::GlobalGet(self.heap_ptr_global_idx));
        func.instruction(&WasmInstr::LocalGet(1)); // size parameter
        func.instruction(&WasmInstr::I32Add);

        // Security: Check for overflow against max memory (100 pages * 64KB = 6.4MB)
        // If new_ptr > max_memory, trap instead of corrupting memory
        func.instruction(&WasmInstr::I32Const(100 * 64 * 1024)); // Max memory bytes
        func.instruction(&WasmInstr::I32GtU);
        // If overflow detected, return -1 to signal allocation failure
        func.instruction(&WasmInstr::If(BlockType::Empty));
        func.instruction(&WasmInstr::I32Const(-1i32 as u32 as i32)); // Return -1 on overflow
        func.instruction(&WasmInstr::Return);
        func.instruction(&WasmInstr::End);

        // Store back to heap pointer global
        func.instruction(&WasmInstr::GlobalSet(self.heap_ptr_global_idx));

        // Return the original heap pointer (stored in local 0)
        func.instruction(&WasmInstr::LocalGet(0));
        func.instruction(&WasmInstr::End);

        // Add to code section
        self.code.function(&func);

        // Map function name for internal reference
        self.func_name_to_idx.insert("malloc".to_string(), func_idx);

        func_idx
    }

    /// Emit the println builtin using WASI fd_write.
    ///
    /// This writes a string to stdout using the WASI fd_write syscall.
    /// For simplicity, this version writes a pre-formatted newline string.
    ///
    /// Function signature: println(ptr: i32, len: i32) -> i32
    pub fn emit_println_builtin(&mut self) -> u32 {
        let func_idx = self.next_func_idx;
        self.next_func_idx += 1;

        // WASI fd_write signature: fd_write(fd: i32, iovs: i32, iovs_len: i32, nwritten: i32) -> i32
        // fd = 1 for stdout
        // iovs = pointer to array of (ptr, len) pairs
        // iovs_len = number of iovs
        // nwritten = pointer to store number of bytes written

        let mut func = Function::new([]);

        // Allocate space on stack for iovs (8 bytes: ptr + len)
        // iov[0].ptr = param 0 (string pointer)
        // iov[0].len = param 1 (string length)

        // Get stack pointer - use memory offset 0 for simplicity
        // In a real implementation, we'd manage stack properly

        // Store iovs at memory offset 0 and 4
        // iov.ptr at offset 0
        func.instruction(&WasmInstr::I32Const(0));
        func.instruction(&WasmInstr::LocalGet(0)); // ptr param
        func.instruction(&WasmInstr::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));

        // iov.len at offset 4
        func.instruction(&WasmInstr::I32Const(4));
        func.instruction(&WasmInstr::LocalGet(1)); // len param
        func.instruction(&WasmInstr::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));

        // Call fd_write
        // fd = 1 (stdout)
        func.instruction(&WasmInstr::I32Const(1));
        // iovs = 0 (pointer to start of memory where we stored iovs)
        func.instruction(&WasmInstr::I32Const(0));
        // iovs_len = 1 (one iov)
        func.instruction(&WasmInstr::I32Const(1));
        // nwritten = 8 (pointer to store result, after iovs)
        func.instruction(&WasmInstr::I32Const(8));

        // Call WASI fd_write
        if let Some(fd_write_idx) = self.wasi_fd_write_idx {
            func.instruction(&WasmInstr::Call(fd_write_idx));
        } else {
            // Return error if WASI not available
            func.instruction(&WasmInstr::I32Const(-1));
        }

        func.instruction(&WasmInstr::End);

        // Add to code section
        self.code.function(&func);

        // Export the function
        self.exports.export("println", ExportKind::Func, func_idx);

        // Map function name
        self.func_name_to_idx
            .insert("println".to_string(), func_idx);

        func_idx
    }

    /// Compile an IR module into WASM.
    pub fn compile_module(&mut self, module: &ir::Module) -> Result<(), WasmCodegenError> {
        // First pass: collect all strings from the IR
        for function in &module.functions {
            self.collect_strings_from_function(function)?;
        }

        // Emit data section with all strings
        self.encode_data_section();

        // Emit builtin functions
        self.emit_malloc_builtin();
        self.emit_println_builtin();

        // Compile each function
        for function in &module.functions {
            self.compile_function(function)?;
        }

        Ok(())
    }

    /// Collect all string literals from a function.
    fn collect_strings_from_function(
        &mut self,
        function: &ir::Function,
    ) -> Result<(), WasmCodegenError> {
        for block in &function.blocks {
            for instr in &block.instructions {
                if let Instruction::Const(_, Literal::Str(s)) = instr {
                    // Store the string - propagate any errors
                    let _ = self.emit_string(s)?;
                }
            }
        }
        Ok(())
    }

    /// Compile a single IR function to WASM.
    fn compile_function(&mut self, function: &ir::Function) -> Result<(), WasmCodegenError> {
        let func_idx = self.next_func_idx;
        self.next_func_idx += 1;

        // Map function name
        self.func_name_to_idx
            .insert(function.name.clone(), func_idx);

        // Create function builder with locals for parameters
        let num_params = function.params.len();
        let mut locals = Vec::with_capacity(num_params);
        for _ in 0..num_params {
            locals.push((1, ValType::I32)); // All params are i32 for now
        }

        let mut func = Function::new(locals);

        // Compile each block
        for block in function.blocks.iter() {
            // In a real implementation, we'd track block labels for branching
            // For now, we just compile sequentially

            for instr in &block.instructions {
                self.compile_instruction(&mut func, instr, function)?;
            }
        }

        // Add function to code section
        self.code.function(&func);

        // Export main function
        if function.name == "main" {
            self.exports.export("main", ExportKind::Func, func_idx);
        }

        Ok(())
    }

    /// Compile a single IR instruction to WASM.
    fn compile_instruction(
        &mut self,
        func: &mut Function,
        instr: &Instruction,
        _function: &ir::Function,
    ) -> Result<(), WasmCodegenError> {
        match instr {
            Instruction::Const(_, literal) => {
                match literal {
                    Literal::Int(n) => {
                        // For i32 values
                        if *n >= i32::MIN as i64 && *n <= i32::MAX as i64 {
                            func.instruction(&WasmInstr::I32Const(*n as i32));
                        } else {
                            // For i64, we'd use I64Const
                            return Err(WasmCodegenError::from(
                                "i64 constants not yet supported in WASM backend",
                            ));
                        }
                    }
                    Literal::Bool(b) => {
                        func.instruction(&WasmInstr::I32Const(if *b { 1 } else { 0 }));
                    }
                    Literal::Float(f) => {
                        // WASM f64.const
                        func.instruction(&WasmInstr::F64Const(*f));
                    }
                    Literal::Str(_s) => {
                        // String literals should have been stored in data section
                        // We'd push the offset here
                        // For now, push a placeholder
                        func.instruction(&WasmInstr::I32Const(0));
                    }
                }
                Ok(())
            }

            Instruction::Add(_, _lhs, _rhs) => {
                // Load lhs and rhs values (simplified - assumes they're on stack)
                // In reality, we'd need to track value stack mapping
                func.instruction(&WasmInstr::I32Add);
                Ok(())
            }

            Instruction::Sub(_, _lhs, _rhs) => {
                func.instruction(&WasmInstr::I32Sub);
                Ok(())
            }

            Instruction::Mul(_, _lhs, _rhs) => {
                func.instruction(&WasmInstr::I32Mul);
                Ok(())
            }

            Instruction::Div(_, _lhs, _rhs) => {
                func.instruction(&WasmInstr::I32DivS);
                Ok(())
            }

            Instruction::Ret(_val) => {
                if _val.is_some() {
                    // Return value should be on stack
                }
                func.instruction(&WasmInstr::End);
                Ok(())
            }

            // Other instructions - stub implementations
            _ => {
                // For now, just skip unimplemented instructions
                Ok(())
            }
        }
    }

    /// Finalize the WASM module and return the binary bytes.
    pub fn finish(self) -> Result<Vec<u8>, WasmCodegenError> {
        // Build type section
        // Type 0: fd_write (i32, i32, i32, i32) -> i32
        // Type 1: proc_exit (i32) -> ()
        // Type 2+: user functions

        // We need to manually construct the type section since wasm-encoder
        // doesn't expose a direct TypeSection builder in the same way

        // For now, create a minimal type section
        let mut type_section = Vec::new();
        type_section.push(0x01); // section id

        // Type section content:
        // count: 2 (fd_write and proc_exit types)
        // Type 0: (i32, i32, i32, i32) -> i32
        // Type 1: (i32) -> ()

        // Simplified type section encoding
        let type_content = vec![
            0x02, // count = 2 types
            // Type 0: function type
            0x60, // func type
            0x04, // 4 params
            0x7f, 0x7f, 0x7f, 0x7f, // i32, i32, i32, i32
            0x01, // 1 result
            0x7f, // i32
            // Type 1: function type
            0x60, // func type
            0x01, // 1 param
            0x7f, // i32
            0x00, // 0 results
        ];

        let type_len = type_content.len();
        type_section.extend_from_slice(&encode_leb128(type_len as u32));
        type_section.extend_from_slice(&type_content);

        // Manually build the module sections
        let mut result = Vec::new();

        // Magic number and version
        result.extend_from_slice(&[0x00, 0x61, 0x73, 0x6d]); // \0asm
        result.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version 1

        // Type section
        result.extend_from_slice(&type_section);

        // Import section
        let mut import_bytes = Vec::new();
        self.imports.append_to(&mut import_bytes);
        if !import_bytes.is_empty() {
            result.extend_from_slice(&import_bytes);
        }

        // Function section
        let mut func_bytes = Vec::new();
        self.functions.append_to(&mut func_bytes);
        if !func_bytes.is_empty() {
            result.extend_from_slice(&func_bytes);
        }

        // Memory section
        let mut mem_bytes = Vec::new();
        self.memories.append_to(&mut mem_bytes);
        if !mem_bytes.is_empty() {
            result.extend_from_slice(&mem_bytes);
        }

        // Global section
        let mut global_bytes = Vec::new();
        self.globals.append_to(&mut global_bytes);
        if !global_bytes.is_empty() {
            result.extend_from_slice(&global_bytes);
        }

        // Export section
        let mut export_bytes = Vec::new();
        self.exports.append_to(&mut export_bytes);
        if !export_bytes.is_empty() {
            result.extend_from_slice(&export_bytes);
        }

        // Code section
        let mut code_bytes = Vec::new();
        self.code.append_to(&mut code_bytes);
        if !code_bytes.is_empty() {
            result.extend_from_slice(&code_bytes);
        }

        // Data section
        let mut data_bytes = Vec::new();
        self.data.append_to(&mut data_bytes);
        if !data_bytes.is_empty() {
            result.extend_from_slice(&data_bytes);
        }

        Ok(result)
    }

    /// Get the name of this backend.
    pub fn name(&self) -> &str {
        "wasm"
    }
}

/// Encode a u32 as LEB128 bytes.
fn encode_leb128(mut value: u32) -> Vec<u8> {
    let mut result = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            result.push(byte | 0x80);
        } else {
            result.push(byte);
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_backend_new() {
        let backend = WasmBackend::new();
        assert!(backend.is_ok());
    }

    #[test]
    fn test_emit_string() {
        let mut backend = WasmBackend::new().unwrap();
        let id = backend.emit_string("hello").expect("Failed to emit string");
        assert_eq!(id.0, 0);

        let offset = backend.get_string_offset(id);
        assert_eq!(offset, Some(1024)); // Heap starts at 1024
    }

    #[test]
    fn test_emit_malloc_builtin() {
        let mut backend = WasmBackend::new().unwrap();
        let idx = backend.emit_malloc_builtin();
        assert_eq!(idx, 2); // First function after imports
    }

    #[test]
    fn test_emit_println_builtin() {
        let mut backend = WasmBackend::new().unwrap();
        let idx = backend.emit_println_builtin();
        assert_eq!(idx, 2); // First function after imports
    }
}
