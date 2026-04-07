<div align="center">

<img src="assets/banner.png" alt="Gradient" width="540"/>

<br/>
<br/>

**A programming language designed for AI agents.**

<br/>

[![Status](https://img.shields.io/badge/status-alpha-blueviolet?style=flat-square&labelColor=0d0d17)](https://github.com/graydeon/Gradient)
[![Language](https://img.shields.io/badge/impl-Rust-orange?style=flat-square&labelColor=0d0d17)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-4f8aff?style=flat-square&labelColor=0d0d17)](LICENSE)
[![Backends](https://img.shields.io/badge/backends-Cranelift%20|%20WASM-00e5ff?style=flat-square&labelColor=0d0d17)](#webassembly-support)
[![Tests](https://img.shields.io/badge/tests-1058-brightgreen?style=flat-square&labelColor=0d0d17)](#status)

</div>

---

Gradient eliminates an entire class of errors before code ever runs—so LLMs write correct code the first time, not the tenth.

```
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)
```

---

## Why Gradient Exists

Current LLM coding workflows waste tokens on generate-compile-fix loops. Gradient cuts this waste through:

| Technique | What It Saves |
|-----------|---------------|
| **Grammar-constrained generation** | Zero syntax errors via XGrammar/llguidance integration |
| **Enforced effects** | Pure functions proven at compile time—no silent side effects |
| **Type-directed completion** | Agents know expected types before generating code |
| **Contracts** | `@requires`/`@ensures` for generate-verify instead of generate-fix |

**Result:** Fewer iterations, smaller context windows, lower inference costs.

---

## Quick Start

### Prerequisites

- **Rust** 1.75 or later
- For WebAssembly builds: `wasm32-unknown-unknown` target

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add WASM target (for WebAssembly builds)
rustup target add wasm32-unknown-unknown
```

### Building

```bash
git clone https://github.com/Ontic-Systems/Gradient.git
cd Gradient/codebase

# Build (native only)
cargo build --release

# Build with WebAssembly support
cargo build --release --features wasm

# Create and run a project
./target/release/gradient new hello
cd hello
GRADIENT_COMPILER=../target/release/gradient-compiler ../target/release/gradient run

# Or compile to WebAssembly
./target/release/gradient-compiler hello.gr hello.wasm --backend wasm
wasmtime hello.wasm  # Run with wasmtime
```

---

## What's Working Now

- **Full compilation pipeline** — source → native binary via Cranelift
- **Type system** — inference, generics, algebraic data types, pattern matching
- **Effects** — `!{IO}`, `!{Net}`, `!{FS}`, `!{Mut}`, `!{Time}` tracked and enforced
- **Contracts** — runtime-checked `@requires`/`@ensures` with `result` keyword
- **Multi-file modules** — `use math` resolves to `math.gr`
- **FFI** — `@extern("libm")` for C imports, `@export` for exports
- **WebAssembly** — compile to WASM for browser/edge deployment (`--backend wasm`)
- **Package Dependencies** — Path dependencies stable (`gradient add ../local`)
- **Standard library** — strings, lists, maps, math, file I/O, CLI args
- **Test framework** — `@test` annotation with `gradient test`
- **Tooling** — LSP server, structured query API, `--json` output everywhere

**1,058 tests passing locally.** See `codebase/compiler/src/*/tests.rs`. Public CI currently shows failures due to environment differences—local builds pass.

---

## Language Highlights

### Effects Are Part of the Type

```gradient
fn add(a: Int, b: Int) -> Int:        # proven pure
    ret a + b

fn greet(name: String) -> !{IO} ():   # must declare IO
    print("Hello, " + name)
```

### Contracts for Verification

```gradient
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1: ret 1
    else: ret n * factorial(n - 1)
```

### Generics with Inference

```gradient
fn identity[T](x: T) -> T:
    ret x

let x: Int = identity(42)        # T inferred as Int
let y: String = identity("hi")     # T inferred as String
```

### Pattern Matching

```gradient
type Option[T] = Some(T) | None

fn unwrap[T](opt: Option[T], default: T) -> T:
    match opt:
        Some(val): val
        None: default
```

---

## Project Status

**Alpha.** The compiler works. Programs compile and run.

| Component | Status | Notes |
|-----------|--------|-------|
| Lexer/Parser/Typechecker | ✅ **Stable** | 1,058 tests passing |
| Native code generation | ✅ **Stable** | Cranelift backend (default) |
| LSP server | ✅ **Stable** | Built, functional |
| Test runner | ✅ **Stable** | `gradient test` works |
| WebAssembly backend | 🧪 **Experimental** | Code exists, compile with `--features wasm` |
| LLVM backend | ❌ **Broken** | Disabled in CI (Polly linking issue) |
| SMT verification | 🚧 **Planned** | Feature flag only, not functional |
| Formatter | 🧪 **Experimental** | Code exists, CLI not wired |
| REPL | 🧪 **Experimental** | Code exists, not functional |
| Package registry | 🚧 **Planned** | Not implemented |
| Git dependencies | 🧪 **Experimental** | Unverified end-to-end |

---

## CLI Commands

```
gradient new <name>          Create project
gradient build               Compile to native binary
gradient run                 Build and execute
gradient check               Type-check only
gradient test                Run @test functions
gradient add <spec>          Add dependency (path, git, or registry)
gradient update              Refresh lockfile and dependencies
gradient-lsp                 LSP server (stdio)

gradient-compiler flags:
  --backend <cranelift|llvm|wasm>   Select code generation backend
  --release                         Use LLVM backend (optimized)
  --check                           Type-check only (no codegen)
  --json                            JSON output format
```

### Package Management

Add dependencies from local paths:

```bash
# From local path (stable)
gradient add ../local-package

# From git repository (experimental, unverified)
gradient add https://github.com/user/repo
gradient add https://github.com/user/repo@v1.0.0
```

**Status:** Path dependencies are the only fully supported dependency type. Git dependencies have CLI support but lack end-to-end verification. Registry dependencies (`gradient add math@1.2.0`) require a registry server that does not yet exist.

---

## Architecture

```
Source (.gr)
    ↓
Lexer → Parser → AST
    ↓
Type Checker (effects, contracts, inference)
    ↓
IR Builder (SSA)
    ↓
┌─────────────────────────────────────────────┐
│           Cranelift (default)               │
│              ✅ Stable                        │
└─────────────────────────────────────────────┘
    ↓
Binary .exe

Experimental: WASM backend (--features wasm)
Not working: LLVM backend (Polly linking issue)
```

---

## WebAssembly Support (Experimental)

Gradient has experimental WebAssembly support. Code exists but requires compiling with the `wasm` feature flag:

```bash
# Build with WASM support (required)
cargo build --release --features wasm

# Compile to WASM
./target/release/gradient-compiler input.gr output.wasm --backend wasm

# Run with wasmtime
wasmtime output.wasm
```

**Status:** WASM backend code is complete but not included in default builds. It has not undergone the same testing as the Cranelift backend.

**Features (when enabled):**
- Linear memory export for host interaction
- WASI imports for I/O (`fd_write`, `proc_exit`)
- String data in passive data segments
- Bump allocator for heap allocations

**Use cases:**
- **Browser-based agents** — Run Gradient in the browser
- **Edge deployment** — Deploy to Cloudflare Workers, Deno Deploy
- **Sandboxed execution** — Safe execution of untrusted code

**Browser demo:** `codebase/wasm-demo/index.html` (requires WASM feature build)

---

## License

MIT — see [LICENSE](LICENSE)

---

<div align="center">
<sub>Built for agents. Verified by the compiler.</sub>
</div>
