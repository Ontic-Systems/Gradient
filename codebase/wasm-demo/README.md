# Gradient WebAssembly Demo

A browser-based demo showing Gradient code compiling to WebAssembly and running in the browser.

## Quick Start

Open `index.html` in any modern browser:

```bash
cd /home/gray/TestingGround/Gradient/codebase/wasm-demo
python -m http.server 8080
# Then open http://localhost:8080
```

Or serve with any static file server.

## Current Status

This is a **visual demo** showing what Gradient → WASM compilation looks like.

### What's Implemented
- ✅ UI for editing Gradient code
- ✅ Example programs (Fibonacci, Factorial, etc.)
- ✅ Simulated compilation pipeline display
- ✅ Syntax validation
- ✅ Result simulation

### What's Coming
- 🔄 **Real WASM compilation**: Compile the Gradient compiler itself to WASM so it runs in the browser
- 🔄 **Live execution**: Actually instantiate and run compiled WASM modules
- 🔄 **WASI integration**: File I/O, stdin/stdout in the browser
- 🔄 **Debug output**: View generated WASM instructions

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Browser                                │
├─────────────────────────────────────────────────────────────┤
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐ │
│  │   Editor     │───▶│  Compiler    │───▶│   WASM VM    │ │
│  │  (Gradient)  │    │  (Gradient   │    │  (Web API)   │ │
│  │              │    │   → WASM)    │    │              │ │
│  └──────────────┘    └──────────────┘    └──────────────┘ │
│         │                    │                   │          │
│         └────────────────────┴───────────────────┘          │
│                         │                                   │
│                    ┌────┴────┐                              │
│                    │ Output  │                              │
│                    │ Panel   │                              │
│                    └─────────┘                              │
└─────────────────────────────────────────────────────────────┘
```

## Files

- `index.html` - Demo UI (dark theme, responsive)
- `demo.js` - JavaScript glue code
- `README.md` - This file

## Future: Self-Hosting Compiler

The end goal is to compile the Gradient compiler itself to WASM:

```bash
# 1. Build gradient-compiler.wasm
cargo build --features wasm --target wasm32-wasi

# 2. Use it in the browser
const compiler = await WebAssembly.instantiate(
    fetch('gradient-compiler.wasm'),
    wasiImports
);

# 3. Compile Gradient → WASM entirely in the browser
const wasmBytes = compiler.compile(sourceCode);
const program = await WebAssembly.instantiate(wasmBytes);
const result = program.exports.main();
```

This enables:
- **Zero-install development**: Try Gradient without installing anything
- **Sandboxed execution**: Run untrusted code safely
- **Edge deployment**: Deploy Gradient programs to CDNs/serverless
- **AI integration**: Run Gradient in ML pipelines (Python/JS interop)

## Related

- [WASM Backend Implementation](../compiler/src/codegen/wasm.rs)
- [CLI --backend flag](../compiler/src/main.rs)
- [WASM Tests](../compiler/tests/wasm_e2e_tests.rs)
