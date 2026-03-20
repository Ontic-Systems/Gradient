//! Gradient compiler — proof-of-concept binary.
//!
//! This is a temporary entry point that demonstrates the end-to-end toolchain:
//!
//!   Hardcoded program -> Cranelift -> Object file -> Link -> Native binary
//!
//! It will be replaced by a proper compiler driver that reads source files,
//! runs them through the frontend pipeline, and invokes the codegen backend.
//!
//! # Usage
//!
//! ```sh
//! cargo run                  # Produces hello.o
//! cc hello.o -o hello        # Link with libc
//! ./hello                    # Prints "Hello from Gradient!"
//! ```

use gradient_compiler::codegen::CraneliftCodegen;

fn main() {
    println!("Gradient Compiler — Proof of Concept");
    println!("====================================");
    println!();

    // Step 1: Create the Cranelift code generator targeting the host platform.
    println!("[1/3] Initializing Cranelift codegen for host target...");
    let mut codegen = match CraneliftCodegen::new() {
        Ok(cg) => cg,
        Err(e) => {
            eprintln!("Error: Failed to initialize codegen: {}", e);
            std::process::exit(1);
        }
    };

    // Step 2: Emit the hardcoded "Hello from Gradient!" program.
    //
    // In the future, this will be replaced by:
    //   1. Parse source file(s) into AST
    //   2. Type-check the AST
    //   3. Lower AST to IR (ir::Module)
    //   4. Compile each ir::Function via codegen.compile_function()
    println!("[2/3] Compiling hardcoded 'Hello from Gradient!' program...");
    if let Err(e) = codegen.emit_hello_world() {
        eprintln!("Error: Failed to compile: {}", e);
        std::process::exit(1);
    }

    // Step 3: Write the object file to disk.
    let output_path = "hello.o";
    println!("[3/3] Writing object file...");
    if let Err(e) = codegen.finalize(output_path) {
        eprintln!("Error: Failed to write object file: {}", e);
        std::process::exit(1);
    }

    // Print instructions for the user to complete the pipeline.
    println!();
    println!("Success! To produce and run the final executable:");
    println!();
    println!("  cc hello.o -o hello");
    println!("  ./hello");
    println!();
    println!("Expected output: Hello from Gradient!");
}
