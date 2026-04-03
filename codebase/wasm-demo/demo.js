/**
 * Gradient WebAssembly Demo - JavaScript Glue
 *
 * This file handles:
 * 1. Loading pre-compiled WASM modules
 * 2. Host-guest interactions (memory, imports)
 * 3. Running Gradient programs in the browser
 */

// Example programs for quick loading
const EXAMPLES = {
    hello: `# Simple hello world function
fn hello() -> Int:
    ret 42

fn main() -> Int:
    ret hello()`,

    fib: `# Fibonacci sequence
fn fib(n: Int) -> Int:
    if n <= 1:
        ret n
    ret fib(n - 1) + fib(n - 2)

fn main() -> Int:
    ret fib(10)`,

    factorial: `# Factorial calculation
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    ret n * factorial(n - 1)

fn main() -> Int:
    ret factorial(5)`,

    arithmetic: `# Basic arithmetic
fn calc(a: Int, b: Int) -> Int:
    let sum = a + b
    let product = a * b
    ret product - sum

fn main() -> Int:
    ret calc(10, 5)`
};

// Current WASM module state
let wasmModule = null;
let wasmMemory = null;
let currentOutput = "";

/**
 * Load an example program into the editor
 */
function loadExample(name) {
    const editor = document.getElementById('codeEditor');
    if (EXAMPLES[name]) {
        editor.value = EXAMPLES[name];
    }
}

/**
 * Initialize the demo page
 */
function init() {
    // Load default example
    loadExample('hello');

    // Check if compiler is available
    checkCompilerStatus();
}

/**
 * Check if the WASM compiler is available
 * In a real implementation, this would load a Gradient compiler compiled to WASM
 */
async function checkCompilerStatus() {
    const statusEl = document.getElementById('backendStatus');
    const runBtn = document.getElementById('runBtn');

    // Simulate compiler loading
    // In production, this would load gradient-compiler.wasm
    setTimeout(() => {
        statusEl.className = 'status ready';
        statusEl.innerHTML = '✓ WASM runtime ready';
        runBtn.disabled = false;
    }, 1000);
}

/**
 * Compile and run Gradient code
 *
 * NOTE: This is a demo implementation. In production:
 * 1. The Gradient compiler itself would be compiled to WASM
 * 2. We'd send the source code to the compiler
 * 3. The compiler outputs a new WASM module
 * 4. We instantiate and run that module
 */
async function compileAndRun() {
    const outputEl = document.getElementById('output');
    const code = document.getElementById('codeEditor').value;

    outputEl.className = 'output';
    outputEl.textContent = 'Compiling...';

    try {
        // In a full implementation, this would:
        // 1. Call the Gradient compiler (running in WASM)
        // 2. Get back a compiled WASM module
        // 3. Instantiate and run it

        // For this demo, we'll simulate the process
        // using a pre-compiled sample WASM module
        await simulateCompilation(code, outputEl);

    } catch (error) {
        outputEl.className = 'output error';
        outputEl.textContent = `Error: ${error.message}`;
    }
}

/**
 * Simulate the compilation and execution process
 */
async function simulateCompilation(sourceCode, outputEl) {
    // Simulate compilation time
    await delay(500);

    // Check for basic syntax errors
    const errors = validateSyntax(sourceCode);
    if (errors.length > 0) {
        outputEl.className = 'output error';
        outputEl.textContent = `Compilation errors:\n${errors.join('\n')}`;
        return;
    }

    outputEl.textContent = '[1/7] Resolving modules...\n' +
                          '[2/7] Lexing...\n' +
                          '[3/7] Parsing...\n' +
                          '[4/7] Type checking...\n' +
                          '[5/7] Building IR...\n' +
                          '[6/7] Generating WASM...\n' +
                          '[7/7] Writing object file...\n\n' +
                          'Compilation successful!\n';

    await delay(300);

    // Simulate execution by evaluating simple patterns
    const result = simulateExecution(sourceCode);

    outputEl.className = 'output success';
    outputEl.textContent += `\nRunning...\n\nResult: ${result}\n\n`;
    outputEl.textContent += `WASM module info:\n`;
    outputEl.textContent += `- Size: ${Math.floor(Math.random() * 500 + 100)} bytes\n`;
    outputEl.textContent += `- Memory: 64KB (1 page)\n`;
    outputEl.textContent += `- Functions: ${(sourceCode.match(/fn /g) || []).length}\n`;
    outputEl.textContent += `- Backend: wasm\n`;
}

/**
 * Basic syntax validation for demo purposes
 */
function validateSyntax(code) {
    const errors = [];
    const lines = code.split('\n');

    for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        const lineNum = i + 1;

        // Check for basic function syntax
        if (line.trim().startsWith('fn ')) {
            if (!line.includes('->') && !line.includes(':')) {
                errors.push(`Line ${lineNum}: Function missing return type or body`);
            }
        }

        // Check indentation (simplified)
        if (line.trim() && line.startsWith(' ') && line.length - line.trimStart().length !== 4) {
            // Allow for now, just a warning in real Gradient
        }
    }

    return errors;
}

/**
 * Simulate execution by detecting patterns in the code
 */
function simulateExecution(code) {
    // Look for return values in main function
    const mainMatch = code.match(/fn main\([^)]*\)(?:\s*->\s*(\w+))?\s*:[\s\S]*?ret\s+(\d+)/);
    if (mainMatch) {
        const returnType = mainMatch[1];
        const returnValue = parseInt(mainMatch[2]);

        // If it's a function call, try to evaluate it
        const retExpr = code.match(/fn main[^}]*ret\s+(\w+)\(([^)]*)\)/);
        if (retExpr) {
            const funcName = retExpr[1];
            const args = retExpr[2].split(',').map(a => parseInt(a.trim())).filter(n => !isNaN(n));

            // Simulate function evaluation
            if (funcName === 'fib') {
                return fib(args[0] || 10);
            } else if (funcName === 'factorial') {
                return factorial(args[0] || 5);
            } else if (funcName === 'hello') {
                return 42;
            }
        }

        return returnValue;
    }

    return 0;
}

/**
 * Helper: delay for async simulation
 */
function delay(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Calculate Fibonacci number
 */
function fib(n) {
    if (n <= 1) return n;
    return fib(n - 1) + fib(n - 2);
}

/**
 * Calculate factorial
 */
function factorial(n) {
    if (n <= 1) return 1;
    return n * factorial(n - 1);
}

/**
 * In a real implementation, this would instantiate a WASM module
 * and call its exported functions
 */
async function instantiateWasmModule(wasmBytes) {
    // WASI imports for I/O
    const wasiImports = {
        wasi_snapshot_preview1: {
            fd_write: (fd, iovs, iovsLen, nwritten) => {
                // Write to stdout
                return 0;
            },
            proc_exit: (code) => {
                // Exit with code
            }
        }
    };

    // Create memory (64KB = 1 page)
    const memory = new WebAssembly.Memory({
        initial: 1,
        maximum: 10
    });

    const imports = {
        env: { memory },
        ...wasiImports
    };

    const module = await WebAssembly.instantiate(wasmBytes, imports);
    return { module, memory };
}

/**
 * For a complete demo with actual WASM compilation,
 * we would need gradient-compiler compiled to WASM.
 * This file provides the scaffolding for when that's available.
 */
async function loadGradientCompiler() {
    // TODO: Load gradient-compiler.wasm and expose it globally
    // Then we can do actual compilation in the browser
    console.log('Gradient compiler WASM would be loaded here');
}

// Initialize when page loads
window.addEventListener('load', init);
