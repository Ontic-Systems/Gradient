# Gradient

**A programming language designed for AI agents.**

Gradient is a statically-typed language that eliminates an entire class of errors before code ever runs—so LLMs write correct code the first time, not the tenth.

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

```bash
git clone https://github.com/graydeon/Gradient.git
cd Gradient/codebase
cargo build --release

# Create and run a project
./target/release/gradient new hello
cd hello
GRADIENT_COMPILER=../target/release/gradient-compiler ../target/release/gradient run
```

---

## What's Working Now

- **Full compilation pipeline** — source → native binary via Cranelift
- **Type system** — inference, generics, algebraic data types, pattern matching
- **Effects** — `!{IO}`, `!{Net}`, `!{FS}`, `!{Mut}`, `!{Time}` tracked and enforced
- **Contracts** — runtime-checked `@requires`/`@ensures` with `result` keyword
- **Multi-file modules** — `use math` resolves to `math.gr`
- **FFI** — `@extern("libm")` for C imports, `@export` for exports
- **Standard library** — strings, lists, maps, math, file I/O, CLI args
- **Test framework** — `@test` annotation with `gradient test`
- **Tooling** — LSP server, structured query API, `--json` output everywhere

**881 tests passing.** See `codebase/compiler/src/*/tests.rs`.

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

| Component | Status |
|-----------|--------|
| Lexer/Parser/Typechecker | ✅ 881 tests |
| Native code generation | ✅ Cranelift |
| LSP server | ✅ Built |
| Test runner | ✅ `gradient test` |
| LLVM backend | ⚠️ Feature flag (`--features llvm`) |
| SMT verification | ⚠️ Feature flag (`--features smt`) |
| Formatter | 🚧 Planned |
| REPL | 🚧 Planned |

---

## CLI Commands

```
gradient new <name>      Create project
gradient build           Compile to native binary
gradient run             Build and execute
gradient check           Type-check only
gradient test            Run @test functions
gradient add <path>      Add dependency
gradient update          Refresh lockfile
gradient-lsp             LSP server (stdio)
```

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
Cranelift → Object file → Linked with cc → Binary
```

---

## License

MIT — see [LICENSE](LICENSE)

---

<div align="center">
<sub>Built for agents. Verified by the compiler.</sub>
</div>
