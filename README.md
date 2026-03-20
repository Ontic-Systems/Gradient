<div align="center">

<img src="assets/banner.png" alt="Gradient" width="540"/>

<br/>
<br/>

**The world's first programming language designed from the ground up for autonomous AI agents.**

<br/>

[![Status](https://img.shields.io/badge/status-alpha-blueviolet?style=flat-square&labelColor=0d0d17)](https://github.com/graydeon/Gradient)
[![Language](https://img.shields.io/badge/impl-Rust-orange?style=flat-square&labelColor=0d0d17)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-4f8aff?style=flat-square&labelColor=0d0d17)](LICENSE)
[![Backend](https://img.shields.io/badge/backend-Cranelift-00e5ff?style=flat-square&labelColor=0d0d17)](https://cranelift.dev)
[![Tests](https://img.shields.io/badge/tests-194-brightgreen?style=flat-square&labelColor=0d0d17)](#status)

</div>

---

## What is Gradient?

Every programming language ever built was designed around **human cognition** — mnemonic keywords, visual indentation, memorable syntax, human-readable error messages. Gradient discards these assumptions entirely.

Gradient is a **statically-typed, arena-first, actor-based systems language** built for one specific programmer: an LLM operating under a context window budget, running generate-compile-fix loops at machine speed.

**The compiler exists and works.** Gradient programs compile to native binaries via Cranelift. Hello world, recursive factorial, fibonacci, arithmetic, string concatenation, and math builtins all compile and run today.

---

## Quick Start

```bash
# Build the toolchain from source
git clone https://github.com/graydeon/Gradient.git
cd Gradient/codebase/build-system
cargo build
cd ../compiler
cargo build

# Create, build, and run a project
gradient new my-project
cd my-project
gradient build
gradient run
```

This creates a project with the following `src/main.gr`:

```
mod main

fn main() -> !{IO} ():
    print("Hello, Gradient!")
```

The `gradient build` command compiles `src/main.gr` to a native binary via Cranelift, then links it with `cc`. The `gradient run` command builds and immediately executes the result.

---

## Programs That Compile Today

### Hello World

```
fn main() -> !{IO} ():
    print("Hello from Gradient!")
```

### Factorial (recursion)

```
mod factorial

fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

fn main() -> !{IO} ():
    let result: Int = factorial(5)
    print_int(result)
```

### Fibonacci

```
mod fibonacci

fn fib(n: Int) -> Int:
    if n <= 0:
        ret 0
    else if n == 1:
        ret 1
    else:
        let a: Int = fib(n - 1)
        let b: Int = fib(n - 2)
        ret a + b

fn main() -> !{IO} ():
    let result: Int = fib(10)
    print_int(result)
```

### String Concatenation

```
mod string_concat

fn main() -> !{IO} ():
    let greeting: String = "Hello" + ", " + "Gradient!"
    print(greeting)
```

### Math Builtins

```
mod math_builtins

fn main() -> !{IO} ():
    print_int(abs(-42))
    print_int(min(10, 3))
    print_int(max(10, 3))
    print_float(3.14)
    print_bool(true)
    print_int(17 % 5)
```

---

## Design Priorities

In strict priority order:

| # | Priority | What it means |
|---|---|---|
| 1 | **Token efficiency** | Every saved token is reclaimed context window and reduced inference cost |
| 2 | **Unambiguous parseability** | An LLM generates correct Gradient on the first pass, not the fifth |
| 3 | **Semantic density** | Maximum meaning per token -- terse, consistent, composable primitives |
| 4 | **Systems capability** | Kernel, drivers, OS userspace -- all within reach |
| 5 | **Agentic primitives** | Agent identity, capability delegation, supervision trees -- first class |

---

## Language Design

### Syntax
- **ASCII-only** -- no Unicode operators; every symbol is a single token in major LLM tokenizers
- **Indentation-significant** -- no braces for blocks, no semicolons, no redundant delimiters
- **Colon-delimited blocks** -- `fn`, `if`, `else`, `for` use `:` before their indented body
- **Keyword-led** -- `fn`, `let`, `if`, `for`, `ret`, `type`, `mod`, `use`, `impl`
- **One canonical form** per construct -- the formatter is a normalization function, not a style guide
- **LL(1)-parseable** -- context-free, unambiguous, enabling grammar-guided LLM decoding

### Type System
- Static type checking with inference for `let` bindings
- Five built-in types: `Int`, `Float`, `String`, `Bool`, `()`
- Effect annotations: `!{IO}` tracks side effects in function signatures
- **Typed holes** -- write `?hole`, get compiler feedback on the expected type
- Error recovery -- the type checker reports all errors, not just the first

### Built-in Functions

| Function | Signature |
|---|---|
| `print` | `print(value: String) -> !{IO} ()` |
| `println` | `println(value: String) -> !{IO} ()` |
| `print_int` | `print_int(value: Int) -> !{IO} ()` |
| `print_float` | `print_float(value: Float) -> !{IO} ()` |
| `print_bool` | `print_bool(value: Bool) -> !{IO} ()` |
| `abs` | `abs(n: Int) -> Int` |
| `min` | `min(a: Int, b: Int) -> Int` |
| `max` | `max(a: Int, b: Int) -> Int` |
| `mod_int` | `mod_int(a: Int, b: Int) -> Int` |
| `to_string` | `to_string(value: Int) -> String` |
| `int_to_string` | `int_to_string(value: Int) -> String` |
| `range` | `range(n: Int) -> Iterable` |

The `+` operator also performs string concatenation, and `%` performs integer modulo.

---

## Compiler Architecture

```
Source (.gr)
    |
    v
Lexer (61 tests) ---------- Token stream with INDENT/DEDENT injection
    |
    v
Parser + AST (46 tests) --- Recursive descent, error recovery
    |
    v
Type Checker (52 tests) --- Static types, inference, effect validation
    |
    v
IR Builder (27 tests) ----- AST to SSA-form intermediate representation
    |
    v
Cranelift Codegen ---------- Native object file (.o)
    |
    v
System Linker (cc) -------- Native executable binary
```

The full pipeline is wired end-to-end: `source.gr` goes in, a native binary comes out.

---

## Toolchain

Working CLI commands:

```
gradient new <name>      Create a new project (gradient.toml + src/main.gr)
gradient build           Compile to native binary (Cranelift backend)
gradient run             Build and execute
gradient check           Type-check without emitting a binary
```

Scaffolded (not yet functional):

```
gradient test            Run @test-annotated functions
gradient fmt             Canonical formatter
gradient init            Initialize project in current directory
gradient repl            Interactive session
```

### Project layout

```
my-project/
├── gradient.toml        # Manifest
├── src/
│   └── main.gr          # Entry point
└── target/
    └── debug/           # Build output
```

---

## LSP Server

Gradient ships an LSP server (`codebase/devtools/lsp/`) that provides:

- **Diagnostics** -- real-time lex, parse, and type-check errors on every file change
- **Hover** -- type and signature information for identifiers (builtins and user-defined functions)
- **Completions** -- keywords and builtin function names with signatures
- **`gradient/batchDiagnostics`** -- custom notification that sends all diagnostics for a file in one message, designed for AI agent consumers

The LSP server uses the same compiler pipeline as the CLI -- there is no separate parser or approximate analysis.

---

## Build Philosophy

Gradient is built **slow and modular**. At every point in development there is a working, testable artifact. Nothing is theoretical. Nothing ships without passing tests in a live environment.

The build roadmap is structured as progressive phases -- each one adding exactly one capability to the live system. The hard checkpoint was Phase 4: `gradient build` produces a real native binary from `.gr` source. That checkpoint is green.

---

## Status

Gradient is in **alpha**. The compiler works. Programs compile to native binaries. The test suite has **194 tests** across the lexer, parser, type checker, IR builder, and LSP server.

Phases 0 through 7 are **complete**. See the [roadmap](docs/roadmap.md) for details.

**What works:**
- Full compilation pipeline: source to native binary
- Recursion, arithmetic, conditionals, string concatenation
- Type checking with inference and effect validation
- Working CLI (`gradient new/build/run/check`)
- LSP server with diagnostics, hover, and completions

**What's next:**
- Pattern matching and algebraic data types
- LLVM release backend
- Package system and dependency resolution
- Effect system (row-polymorphic, Koka-inspired)
- Three-tier memory model

---

## Team

Gradient is built by Gray d'Eon.

---

## License

MIT -- see [LICENSE](LICENSE).

---

<div align="center">
<sub>built for the agents that will build everything else</sub>
</div>
