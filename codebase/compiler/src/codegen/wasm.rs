//! WebAssembly code generation backend for the Gradient compiler.
//!
//! This module implements the translation from Gradient IR to WebAssembly binary
//! format using the `wasm-encoder` crate. It produces .wasm files that can be
//! executed in browsers or standalone WASM runtimes like wasmtime.
//!
//! # Architecture
//!
//! ```text
//! Gradient IR
//!     |
//!     v
//! WasmBackend::compile_module()
//!     |
//!     +-- Map IR types to WASM valtypes
//!     +-- Encode IR instructions to WASM opcodes
//!     +-- Build function bodies with local variables
//!     |
//!     v
//! wasm-encoder::Module
//!     |
//!     v
//! Binary WASM file (.wasm)
//! ```

use crate::codegen::{CodegenBackend, CodegenError};
use crate::ir;
use crate::ir::{CmpOp, Instruction, Literal, Type, Value};
use std::collections::HashMap;
use wasm_encoder::Instruction as WasmInstr;
use wasm_encoder::{Function, Module, ValType};

/// Identifier for a string stored in the data section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

/// WASI import function descriptor.
#[derive(Debug, Clone)]
#[allow(dead_code)] // `name` is informational; only `module`/`field`/types are wired through encoder
struct WasiImport {
    name: String,
    module: String,
    field: String,
    param_types: Vec<ValType>,
    result_types: Vec<ValType>,
}

/// WebAssembly backend for compiling Gradient IR to WASM binary format.
///
/// This struct holds the state for WASM code generation, including the
/// underlying `wasm-encoder::Module`, function mappings, and local variable
/// tracking.
#[allow(dead_code)] // function_map / exports are scaffolding for upcoming codegen passes
pub struct WasmBackend {
    /// The WASM module being constructed.
    module: Module,

    /// Counter for generating unique function indices.
    function_count: u32,

    /// Maps function names to their WASM function indices.
    function_map: HashMap<String, u32>,

    /// Maps IR values to WASM local variable indices.
    /// Populated per-function during compilation.
    value_map: HashMap<Value, u32>,

    /// Counter for local variables within the current function.
    local_count: u32,

    /// Export section entries.
    exports: Vec<(String, wasm_encoder::ExportKind, u32)>,

    /// String storage: maps StringId to (offset, bytes)
    strings: HashMap<StringId, (u32, Vec<u8>)>,

    /// Next available string ID.
    next_string_id: u32,

    /// Current offset in data section for string storage.
    /// Starts at 1024 to leave room for stack/data structures.
    data_offset: u32,

    /// WASI imports to include in the module.
    wasi_imports: Vec<WasiImport>,

    /// Index of the next internal function (after imports).
    internal_function_base: u32,

    /// Builtin function indices.
    malloc_idx: Option<u32>,
    println_idx: Option<u32>,
}

#[allow(dead_code)] // helpers staged for upcoming codegen passes
impl WasmBackend {
    /// Create a new WASM backend with WASI imports and memory setup.
    pub fn new() -> Result<Self, CodegenError> {
        let module = Module::new();

        // Set up WASI imports
        let wasi_imports = vec![
            WasiImport {
                name: "fd_write".to_string(),
                module: "wasi_snapshot_preview1".to_string(),
                field: "fd_write".to_string(),
                param_types: vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
                result_types: vec![ValType::I32],
            },
            WasiImport {
                name: "proc_exit".to_string(),
                module: "wasi_snapshot_preview1".to_string(),
                field: "proc_exit".to_string(),
                param_types: vec![ValType::I32],
                result_types: vec![],
            },
        ];

        // Base index for internal functions (after WASI imports)
        let internal_function_base = wasi_imports.len() as u32;

        Ok(WasmBackend {
            module,
            function_count: 0,
            function_map: HashMap::new(),
            value_map: HashMap::new(),
            local_count: 0,
            exports: Vec::new(),
            strings: HashMap::new(),
            next_string_id: 0,
            data_offset: 1024, // Leave first 1KB for system use
            wasi_imports,
            internal_function_base,
            malloc_idx: None,
            println_idx: None,
        })
    }

    /// Maximum data section size: 1MB (before heap starts at 1MB)
    const MAX_DATA_SIZE: u32 = 1024 * 1024;

    /// Store a string in the data section and return its ID.
    pub fn emit_string(&mut self, s: &str) -> Result<StringId, CodegenError> {
        let id = StringId(self.next_string_id);
        self.next_string_id += 1;

        let bytes = s.as_bytes().to_vec();
        let offset = self.data_offset;
        // Align to 8 bytes for safe memory access
        let aligned_len = bytes.len().div_ceil(8) * 8;

        // Security: Check for data section overflow
        let new_offset = self
            .data_offset
            .checked_add(aligned_len as u32)
            .ok_or_else(|| CodegenError::from("Data section size overflow"))?;
        if new_offset > Self::MAX_DATA_SIZE {
            return Err(CodegenError::from(format!(
                "Data section exceeds maximum size of {} bytes",
                Self::MAX_DATA_SIZE
            )));
        }
        self.data_offset = new_offset;

        self.strings.insert(id, (offset, bytes));
        Ok(id)
    }

    /// Get the memory offset for a stored string.
    pub fn get_string_offset(&self, id: StringId) -> Option<u32> {
        self.strings.get(&id).map(|(offset, _)| *offset)
    }

    /// Get the string bytes for a stored string.
    pub fn get_string_bytes(&self, id: StringId) -> Option<&[u8]> {
        self.strings.get(&id).map(|(_, bytes)| bytes.as_slice())
    }

    /// Emit the malloc builtin function for bump allocation.
    /// Returns the function index of the malloc function.
    pub fn emit_malloc_builtin(&mut self) -> u32 {
        if let Some(idx) = self.malloc_idx {
            return idx;
        }

        let idx = self.internal_function_base + self.function_count;
        self.function_count += 1;
        self.malloc_idx = Some(idx);

        // Malloc function: (param size i32) (result ptr i32)
        // Uses global __heap_ptr which starts at data_offset
        // Returns old heap_ptr and increments by size

        // For now, we just record that malloc exists
        // The actual implementation will be generated during compile_module
        idx
    }

    /// Emit the println builtin function using WASI fd_write.
    /// Returns the function index.
    pub fn emit_println_builtin(&mut self) -> u32 {
        if let Some(idx) = self.println_idx {
            return idx;
        }

        let idx = self.internal_function_base + self.function_count;
        self.function_count += 1;
        self.println_idx = Some(idx);

        idx
    }

    /// Encode the data section with all stored strings.
    /// This should be called before finish() to include strings in the output.
    pub fn encode_data_section(&self) -> wasm_encoder::DataSection {
        let mut data_section = wasm_encoder::DataSection::new();

        for (offset, bytes) in self.strings.values() {
            // Add a data segment for each string at its offset
            data_section.active(
                0, // memory index
                &wasm_encoder::ConstExpr::i32_const(*offset as i32),
                bytes.clone(),
            );
        }

        data_section
    }

    /// Get the total size needed for the data section.
    fn data_section_size(&self) -> u32 {
        self.data_offset
    }

    /// Convert an IR type to a WASM value type.
    ///
    /// # Type Mapping
    /// - `Type::I32` → `ValType::I32`
    /// - `Type::I64` → `ValType::I64`
    /// - `Type::F32` → `ValType::F32`
    /// - `Type::F64` → `ValType::F64`
    /// - `Type::Ptr` → `ValType::I32` (wasm32 target)
    /// - `Type::Bool` → `ValType::I32` (boolean as i32)
    /// - `Type::Void` → None (no value type)
    fn ir_type_to_wasm(&self, ty: &Type) -> Option<ValType> {
        match ty {
            Type::I32 => Some(ValType::I32),
            Type::I64 => Some(ValType::I64),
            Type::F64 => Some(ValType::F64),
            Type::Ptr => Some(ValType::I32), // wasm32: pointers are i32
            Type::Bool => Some(ValType::I32), // boolean as i32 (0 or 1)
            Type::Void => None,
        }
    }

    /// Get the WASM local index for an IR value.
    /// Returns None if the value hasn't been mapped yet.
    fn get_local_index(&self, value: Value) -> Option<u32> {
        self.value_map.get(&value).copied()
    }

    /// Allocate a new local variable for an IR value.
    fn allocate_local(&mut self, value: Value, _ty: &Type) -> u32 {
        let index = self.local_count;
        self.value_map.insert(value, index);
        self.local_count += 1;
        index
    }

    /// Emit a single IR instruction as WASM instructions.
    ///
    /// This method translates Gradient IR instructions into their WASM equivalents.
    /// The `builder` is the wasm-encoder Function builder for the current function,
    /// and `value_map` tracks IR value → WASM local index mappings.
    fn emit_instruction(
        &self,
        builder: &mut Function,
        instr: &Instruction,
        value_map: &HashMap<Value, u32>,
    ) -> Result<(), CodegenError> {
        match instr {
            // Load a compile-time constant into a local variable
            Instruction::Const(result, literal) => {
                let local_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined value in Const"))?;

                match literal {
                    Literal::Int(n) => {
                        // Use i64.const for i64 values, i32.const for i32-sized values
                        if *n >= i32::MIN as i64 && *n <= i32::MAX as i64 {
                            builder.instruction(&WasmInstr::I32Const(*n as i32));
                        } else {
                            builder.instruction(&WasmInstr::I64Const(*n));
                        }
                    }
                    Literal::Float(f) => {
                        builder.instruction(&WasmInstr::F64Const(*f));
                    }
                    Literal::Bool(b) => {
                        builder.instruction(&WasmInstr::I32Const(if *b { 1 } else { 0 }));
                    }
                    Literal::Str(_s) => {
                        // String constants need to be stored in data section
                        // For now, push a placeholder pointer (offset 0)
                        // This will be resolved by the data section setup
                        builder.instruction(&WasmInstr::I32Const(0));
                    }
                }
                builder.instruction(&WasmInstr::LocalSet(local_idx));
            }

            // Integer addition: result = lhs + rhs
            Instruction::Add(result, lhs, rhs) => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result value in Add"))?;
                let lhs_idx = *value_map
                    .get(lhs)
                    .ok_or_else(|| CodegenError::from("Undefined lhs value in Add"))?;
                let rhs_idx = *value_map
                    .get(rhs)
                    .ok_or_else(|| CodegenError::from("Undefined rhs value in Add"))?;

                builder.instruction(&WasmInstr::LocalGet(lhs_idx));
                builder.instruction(&WasmInstr::LocalGet(rhs_idx));
                builder.instruction(&WasmInstr::I64Add);
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Integer subtraction: result = lhs - rhs
            Instruction::Sub(result, lhs, rhs) => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result value in Sub"))?;
                let lhs_idx = *value_map
                    .get(lhs)
                    .ok_or_else(|| CodegenError::from("Undefined lhs value in Sub"))?;
                let rhs_idx = *value_map
                    .get(rhs)
                    .ok_or_else(|| CodegenError::from("Undefined rhs value in Sub"))?;

                builder.instruction(&WasmInstr::LocalGet(lhs_idx));
                builder.instruction(&WasmInstr::LocalGet(rhs_idx));
                builder.instruction(&WasmInstr::I64Sub);
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Integer multiplication: result = lhs * rhs
            Instruction::Mul(result, lhs, rhs) => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result value in Mul"))?;
                let lhs_idx = *value_map
                    .get(lhs)
                    .ok_or_else(|| CodegenError::from("Undefined lhs value in Mul"))?;
                let rhs_idx = *value_map
                    .get(rhs)
                    .ok_or_else(|| CodegenError::from("Undefined rhs value in Mul"))?;

                builder.instruction(&WasmInstr::LocalGet(lhs_idx));
                builder.instruction(&WasmInstr::LocalGet(rhs_idx));
                builder.instruction(&WasmInstr::I64Mul);
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Integer division: result = lhs / rhs (signed)
            Instruction::Div(result, lhs, rhs) => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result value in Div"))?;
                let lhs_idx = *value_map
                    .get(lhs)
                    .ok_or_else(|| CodegenError::from("Undefined lhs value in Div"))?;
                let rhs_idx = *value_map
                    .get(rhs)
                    .ok_or_else(|| CodegenError::from("Undefined rhs value in Div"))?;

                builder.instruction(&WasmInstr::LocalGet(lhs_idx));
                builder.instruction(&WasmInstr::LocalGet(rhs_idx));
                builder.instruction(&WasmInstr::I64DivS);
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Memory load: result = *addr
            Instruction::Load(result, addr) => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result value in Load"))?;
                let addr_idx = *value_map
                    .get(addr)
                    .ok_or_else(|| CodegenError::from("Undefined addr value in Load"))?;

                builder.instruction(&WasmInstr::LocalGet(addr_idx));
                // i64.load with alignment 3 (8 bytes) and offset 0
                builder.instruction(&WasmInstr::I64Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Memory store: *addr = value
            Instruction::Store(value, addr) => {
                let value_idx = *value_map
                    .get(value)
                    .ok_or_else(|| CodegenError::from("Undefined value in Store"))?;
                let addr_idx = *value_map
                    .get(addr)
                    .ok_or_else(|| CodegenError::from("Undefined addr in Store"))?;

                builder.instruction(&WasmInstr::LocalGet(addr_idx));
                builder.instruction(&WasmInstr::LocalGet(value_idx));
                // i64.store with alignment 3 (8 bytes) and offset 0
                builder.instruction(&WasmInstr::I64Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
            }

            // Comparison: result = lhs op rhs
            Instruction::Cmp(result, op, lhs, rhs) => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result value in Cmp"))?;
                let lhs_idx = *value_map
                    .get(lhs)
                    .ok_or_else(|| CodegenError::from("Undefined lhs value in Cmp"))?;
                let rhs_idx = *value_map
                    .get(rhs)
                    .ok_or_else(|| CodegenError::from("Undefined rhs value in Cmp"))?;

                builder.instruction(&WasmInstr::LocalGet(lhs_idx));
                builder.instruction(&WasmInstr::LocalGet(rhs_idx));

                match op {
                    CmpOp::Eq => builder.instruction(&WasmInstr::I64Eq),
                    CmpOp::Ne => builder.instruction(&WasmInstr::I64Ne),
                    CmpOp::Lt => builder.instruction(&WasmInstr::I64LtS),
                    CmpOp::Le => builder.instruction(&WasmInstr::I64LeS),
                    CmpOp::Gt => builder.instruction(&WasmInstr::I64GtS),
                    CmpOp::Ge => builder.instruction(&WasmInstr::I64GeS),
                };

                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Boolean OR: result = lhs || rhs
            Instruction::Or(result, lhs, rhs) => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result value in Or"))?;
                let lhs_idx = *value_map
                    .get(lhs)
                    .ok_or_else(|| CodegenError::from("Undefined lhs value in Or"))?;
                let rhs_idx = *value_map
                    .get(rhs)
                    .ok_or_else(|| CodegenError::from("Undefined rhs value in Or"))?;

                // Convert i64 to i32 for boolean operations, OR them, then convert back
                builder.instruction(&WasmInstr::LocalGet(lhs_idx));
                builder.instruction(&WasmInstr::I32WrapI64); // i64 -> i32
                builder.instruction(&WasmInstr::LocalGet(rhs_idx));
                builder.instruction(&WasmInstr::I32WrapI64); // i64 -> i32
                builder.instruction(&WasmInstr::I32Or);
                builder.instruction(&WasmInstr::I64ExtendI32U); // i32 -> i64
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Function call: result = func(args...)
            Instruction::Call(result, func_ref, args) => {
                // Get function index from the func_ref
                let func_idx = func_ref.0;

                // Push arguments onto the stack
                for arg in args {
                    let arg_idx = *value_map
                        .get(arg)
                        .ok_or_else(|| CodegenError::from("Undefined arg value in Call"))?;
                    builder.instruction(&WasmInstr::LocalGet(arg_idx));
                }

                builder.instruction(&WasmInstr::Call(func_idx));
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result value in Call"))?;
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Return from function
            Instruction::Ret(value_opt) => {
                if let Some(value) = value_opt {
                    let value_idx = *value_map
                        .get(value)
                        .ok_or_else(|| CodegenError::from("Undefined return value"))?;
                    builder.instruction(&WasmInstr::LocalGet(value_idx));
                }
                builder.instruction(&WasmInstr::Return);
            }

            // Conditional branch: if cond then block_a else block_b
            Instruction::Branch(cond, then_block, else_block) => {
                let cond_idx = *value_map
                    .get(cond)
                    .ok_or_else(|| CodegenError::from("Undefined cond value in Branch"))?;

                builder.instruction(&WasmInstr::LocalGet(cond_idx));
                builder.instruction(&WasmInstr::BrIf(then_block.0));
                builder.instruction(&WasmInstr::Br(else_block.0));
            }

            // Unconditional jump
            Instruction::Jump(target) => {
                builder.instruction(&WasmInstr::Br(target.0));
            }

            // Phi nodes are handled separately (block parameters in WASM)
            Instruction::Phi(_result, _incoming) => {
                // Phi nodes don't directly emit instructions
                // They're handled by block parameters during block transitions
            }

            // Stack allocation
            Instruction::Alloca(_result, _ty) => {
                // Alloca is a no-op in WASM linear memory model
                // Memory is already allocated; we just use offsets
            }

            // Pointer casts
            Instruction::PtrToInt(result, ptr) => {
                // In wasm32, pointers are already i32, so this is just a copy
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in PtrToInt"))?;
                let ptr_idx = *value_map
                    .get(ptr)
                    .ok_or_else(|| CodegenError::from("Undefined ptr in PtrToInt"))?;
                builder.instruction(&WasmInstr::LocalGet(ptr_idx));
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            Instruction::IntToPtr(result, int) => {
                // In wasm32, integers are already valid pointers
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in IntToPtr"))?;
                let int_idx = *value_map
                    .get(int)
                    .ok_or_else(|| CodegenError::from("Undefined int in IntToPtr"))?;
                builder.instruction(&WasmInstr::LocalGet(int_idx));
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Get element pointer
            Instruction::GetElementPtr {
                result,
                base,
                offset,
                field_ty: _,
            } => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in GetElementPtr"))?;
                let base_idx = *value_map
                    .get(base)
                    .ok_or_else(|| CodegenError::from("Undefined base in GetElementPtr"))?;

                builder.instruction(&WasmInstr::LocalGet(base_idx));
                builder.instruction(&WasmInstr::I64Const(*offset));
                builder.instruction(&WasmInstr::I64Add);
                builder.instruction(&WasmInstr::I32WrapI64); // Convert i64 to i32 for wasm32
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Field address computation
            Instruction::FieldAddr {
                result,
                base,
                field_name: _,
                field_ty: _,
                offset,
            } => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in FieldAddr"))?;
                let base_idx = *value_map
                    .get(base)
                    .ok_or_else(|| CodegenError::from("Undefined base in FieldAddr"))?;

                builder.instruction(&WasmInstr::LocalGet(base_idx));
                builder.instruction(&WasmInstr::I64Const(*offset));
                builder.instruction(&WasmInstr::I64Add);
                builder.instruction(&WasmInstr::I32WrapI64); // Convert i64 to i32 for wasm32
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Enum operations - these require runtime support
            Instruction::ConstructVariant {
                result,
                tag,
                payload,
            } => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in ConstructVariant"))?;

                // For now, simplified: allocate space and store tag
                // Full implementation requires malloc from runtime
                builder.instruction(&WasmInstr::I32Const(0)); // Placeholder: heap offset

                // Store tag at offset 0
                builder.instruction(&WasmInstr::I32Const(*tag as i32));
                builder.instruction(&WasmInstr::I64Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));

                // Store payload values
                for (i, val) in payload.iter().enumerate() {
                    let val_idx = *value_map.get(val).ok_or_else(|| {
                        CodegenError::from("Undefined payload value in ConstructVariant")
                    })?;
                    builder.instruction(&WasmInstr::LocalGet(val_idx));
                    builder.instruction(&WasmInstr::I64Store(wasm_encoder::MemArg {
                        offset: ((i + 1) * 8) as u64,
                        align: 3,
                        memory_index: 0,
                    }));
                }

                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            Instruction::GetVariantTag { result, ptr } => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in GetVariantTag"))?;
                let ptr_idx = *value_map
                    .get(ptr)
                    .ok_or_else(|| CodegenError::from("Undefined ptr in GetVariantTag"))?;

                builder.instruction(&WasmInstr::LocalGet(ptr_idx));
                builder.instruction(&WasmInstr::I64Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                }));
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            Instruction::GetVariantField { result, ptr, index } => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in GetVariantField"))?;
                let ptr_idx = *value_map
                    .get(ptr)
                    .ok_or_else(|| CodegenError::from("Undefined ptr in GetVariantField"))?;

                builder.instruction(&WasmInstr::LocalGet(ptr_idx));
                builder.instruction(&WasmInstr::I64Load(wasm_encoder::MemArg {
                    offset: ((*index + 1) * 8) as u64,
                    align: 3,
                    memory_index: 0,
                }));
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            // Actor operations - these are runtime-specific
            Instruction::Spawn {
                result,
                actor_type_name: _,
            } => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in Spawn"))?;
                // Placeholder: return null pointer (runtime handles actual spawning)
                builder.instruction(&WasmInstr::I32Const(0));
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            Instruction::Send {
                handle,
                message_name: _,
                payload,
            } => {
                let handle_idx = *value_map
                    .get(handle)
                    .ok_or_else(|| CodegenError::from("Undefined handle in Send"))?;
                // Runtime-specific: just validate handle for now
                builder.instruction(&WasmInstr::LocalGet(handle_idx));
                builder.instruction(&WasmInstr::Drop);
                if let Some(_payload_val) = payload {
                    // Payload handling would go here
                    builder.instruction(&WasmInstr::Drop);
                }
            }

            Instruction::Ask {
                result,
                handle,
                message_name: _,
                payload: _,
            } => {
                let result_idx = *value_map
                    .get(result)
                    .ok_or_else(|| CodegenError::from("Undefined result in Ask"))?;
                let handle_idx = *value_map
                    .get(handle)
                    .ok_or_else(|| CodegenError::from("Undefined handle in Ask"))?;
                // Placeholder: return handle as reply pointer
                builder.instruction(&WasmInstr::LocalGet(handle_idx));
                builder.instruction(&WasmInstr::LocalSet(result_idx));
            }

            Instruction::ActorInit { initial_state: _ } => {
                // Actor initialization is handled by runtime
                // No WASM instructions needed at compile time
            }

            // LoadField loads a field from an enum payload by index. The
            // wasm backend does not yet model the heap layout for enum
            // payloads — this is implemented in the cranelift backend only.
            // Treat as unsupported here so the wasm build keeps compiling.
            Instruction::LoadField { .. } => {
                return Err(CodegenError::from(
                    "LoadField is not yet supported in the WASM backend",
                ));
            }

            // StoreField stores a field to an enum payload by index. Like
            // LoadField, this is implemented in the cranelift backend only.
            Instruction::StoreField { .. } => {
                return Err(CodegenError::from(
                    "StoreField is not yet supported in the WASM backend",
                ));
            }
        }

        Ok(())
    }

    /// Get the WASM bytes from the module (for backend compatibility).
    pub fn emit_bytes(self) -> Result<Vec<u8>, CodegenError> {
        Ok(self.module.finish())
    }
}

impl CodegenBackend for WasmBackend {
    fn compile_module(&mut self, module: &ir::Module) -> Result<(), CodegenError> {
        // ============================================
        // Phase 1: Type Section (for both imports and internal functions)
        // ============================================
        let mut type_section = wasm_encoder::TypeSection::new();

        // Add types for WASI imports
        for import in &self.wasi_imports {
            type_section.function(import.param_types.clone(), import.result_types.clone());
        }

        // Add type for malloc builtin: (i32) -> i32
        type_section.function(vec![ValType::I32], vec![ValType::I32]);

        // Add types for user functions
        for func in &module.functions {
            let param_types: Vec<ValType> = func
                .params
                .iter()
                .filter_map(|p| self.ir_type_to_wasm(p))
                .collect();
            let result_types: Vec<ValType> = self
                .ir_type_to_wasm(&func.return_type)
                .into_iter()
                .collect();

            type_section.function(param_types.clone(), result_types.clone());
        }

        self.module.section(&type_section);

        // ============================================
        // Phase 2: Import Section (WASI functions)
        // ============================================
        if !self.wasi_imports.is_empty() {
            let mut import_section = wasm_encoder::ImportSection::new();
            for (i, import) in self.wasi_imports.iter().enumerate() {
                import_section.import(
                    &import.module,
                    &import.field,
                    wasm_encoder::EntityType::Function(i as u32),
                );
            }
            self.module.section(&import_section);
        }

        // ============================================
        // Phase 3: Function Section (internal functions)
        // ============================================
        let mut func_section = wasm_encoder::FunctionSection::new();
        let num_wasi_imports = self.wasi_imports.len() as u32;

        // Builtin functions come first after imports
        // malloc is at type index = num_wasi_imports
        if self.malloc_idx.is_some() {
            func_section.function(num_wasi_imports); // type index for malloc
        }

        // User functions follow
        for (i, _func) in module.functions.iter().enumerate() {
            let type_idx = num_wasi_imports + 1 + i as u32; // +1 for malloc type
            func_section.function(type_idx);
        }

        self.module.section(&func_section);

        // ============================================
        // Phase 4: Memory Section
        // ============================================
        let mut memory_section = wasm_encoder::MemorySection::new();
        // Initial: 1 page (64KB), max: 100 pages (~6.4MB)
        memory_section.memory(wasm_encoder::MemoryType {
            minimum: 1,
            maximum: Some(100),
            memory64: false,
            shared: false,
        });
        self.module.section(&memory_section);

        // ============================================
        // Phase 5: Global Section (__heap_ptr)
        // ============================================
        let mut global_section = wasm_encoder::GlobalSection::new();
        // __heap_ptr: mutable i32, initialized to data_offset
        let global_type = wasm_encoder::GlobalType {
            val_type: ValType::I32,
            mutable: true,
        };
        global_section.global(
            global_type,
            &wasm_encoder::ConstExpr::i32_const(self.data_offset as i32),
        );
        self.module.section(&global_section);

        // ============================================
        // Phase 6: Export Section (memory and functions)
        // ============================================
        let mut export_section = wasm_encoder::ExportSection::new();
        // Export memory as "memory" (required by WASI)
        export_section.export("memory", wasm_encoder::ExportKind::Memory, 0);
        // Export heap pointer for runtime use
        export_section.export("__heap_ptr", wasm_encoder::ExportKind::Global, 0);
        self.module.section(&export_section);

        // ============================================
        // Phase 7: Data Section (string literals)
        // ============================================
        if !self.strings.is_empty() {
            let data_section = self.encode_data_section();
            self.module.section(&data_section);
        }

        // ============================================
        // Phase 8: Code Section (function bodies)
        // ============================================
        let mut code_section = wasm_encoder::CodeSection::new();

        // Emit malloc builtin if needed
        if self.malloc_idx.is_some() {
            let mut malloc_func = Function::new(vec![]);
            // Malloc: (param $size i32) (result i32)
            // locals: $size is local 0
            // Get current heap_ptr, increment by size, return old value

            // Get heap_ptr (global 0)
            malloc_func.instruction(&WasmInstr::GlobalGet(0));
            // Save to local for return
            malloc_func.instruction(&WasmInstr::LocalSet(1)); // Use local 1 as temp

            // Calculate new heap_ptr = heap_ptr + size
            malloc_func.instruction(&WasmInstr::GlobalGet(0));
            malloc_func.instruction(&WasmInstr::LocalGet(0)); // size param
            malloc_func.instruction(&WasmInstr::I32Add);
            malloc_func.instruction(&WasmInstr::GlobalSet(0));

            // Return old heap_ptr
            malloc_func.instruction(&WasmInstr::LocalGet(1));
            malloc_func.instruction(&WasmInstr::End);

            code_section.function(&malloc_func);
        }

        // Compile user functions
        for func in &module.functions {
            // Reset state for each function
            self.value_map.clear();
            self.local_count = func.params.len() as u32;

            // Build value → local index mapping for this function
            let mut next_local = self.local_count;
            for block in &func.blocks {
                for instr in &block.instructions {
                    match instr {
                        Instruction::Const(v, _)
                        | Instruction::Call(v, _, _)
                        | Instruction::Add(v, _, _)
                        | Instruction::Sub(v, _, _)
                        | Instruction::Mul(v, _, _)
                        | Instruction::Div(v, _, _)
                        | Instruction::Cmp(v, _, _, _)
                        | Instruction::Phi(v, _)
                        | Instruction::Alloca(v, _)
                        | Instruction::Load(v, _)
                        | Instruction::PtrToInt(v, _)
                        | Instruction::IntToPtr(v, _)
                        | Instruction::GetVariantTag { result: v, .. }
                        | Instruction::GetVariantField { result: v, .. }
                        | Instruction::Spawn { result: v, .. }
                        | Instruction::Ask { result: v, .. } => {
                            self.value_map.entry(*v).or_insert_with(|| {
                                let idx = next_local;
                                next_local += 1;
                                idx
                            });
                        }
                        Instruction::ConstructVariant { result: v, .. }
                        | Instruction::GetElementPtr { result: v, .. }
                        | Instruction::FieldAddr { result: v, .. } => {
                            self.value_map.entry(*v).or_insert_with(|| {
                                let idx = next_local;
                                next_local += 1;
                                idx
                            });
                        }
                        _ => {}
                    }
                }
            }

            // Count locals by type
            let mut i32_count = 0u32;
            let mut i64_count = 0u32;
            let f32_count = 0u32;
            let mut f64_count = 0u32;

            for value in self.value_map.keys() {
                if let Some(ty) = func.value_types.get(value) {
                    match ty {
                        Type::I32 | Type::Ptr | Type::Bool => i32_count += 1,
                        Type::I64 => i64_count += 1,
                        Type::F64 => f64_count += 1,
                        Type::Void => {}
                    }
                }
            }

            // Build locals list - wasm-encoder expects Vec<(u32, ValType)>
            // representing (count, type) pairs
            let mut locals: Vec<(u32, ValType)> = Vec::new();
            if i32_count > 0 {
                locals.push((i32_count, ValType::I32));
            }
            if i64_count > 0 {
                locals.push((i64_count, ValType::I64));
            }
            if f32_count > 0 {
                locals.push((f32_count, ValType::F32));
            }
            if f64_count > 0 {
                locals.push((f64_count, ValType::F64));
            }

            // Create function body
            let mut function = Function::new(locals);

            // Emit instructions
            for block in &func.blocks {
                for instr in &block.instructions {
                    self.emit_instruction(&mut function, instr, &self.value_map)?;
                }
            }

            // Function end
            function.instruction(&WasmInstr::End);

            code_section.function(&function);
        }

        self.module.section(&code_section);

        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<Vec<u8>, CodegenError> {
        Ok(self.module.finish())
    }

    fn name(&self) -> &str {
        "wasm"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_backend_creation() {
        let backend = WasmBackend::new();
        assert!(backend.is_ok());
    }

    #[test]
    fn test_ir_type_to_wasm() {
        let backend = WasmBackend::new().unwrap();

        assert_eq!(backend.ir_type_to_wasm(&Type::I32), Some(ValType::I32));
        assert_eq!(backend.ir_type_to_wasm(&Type::I64), Some(ValType::I64));
        assert_eq!(backend.ir_type_to_wasm(&Type::F64), Some(ValType::F64));
        assert_eq!(backend.ir_type_to_wasm(&Type::Ptr), Some(ValType::I32));
        assert_eq!(backend.ir_type_to_wasm(&Type::Bool), Some(ValType::I32));
        assert_eq!(backend.ir_type_to_wasm(&Type::Void), None);
    }
}
