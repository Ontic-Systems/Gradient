<div align="center">

<img src="assets/banner.png" alt="Gradient" width="540"/>

<br/>
<br/>

**A statically-typed language with compiler-enforced effect tracking and a structured query API, designed for autonomous AI agents.**

<br/>

[![Status](https://img.shields.io/badge/status-alpha-blueviolet?style=flat-square&labelColor=0d0d17)](https://github.com/graydeon/Gradient)
[![Language](https://img.shields.io/badge/impl-Rust-orange?style=flat-square&labelColor=0d0d17)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-4f8aff?style=flat-square&labelColor=0d0d17)](LICENSE)
[![Backend](https://img.shields.io/badge/backend-Cranelift-00e5ff?style=flat-square&labelColor=0d0d17)](https://cranelift.dev)
[![Tests](https://img.shields.io/badge/tests-232-brightgreen?style=flat-square&labelColor=0d0d17)](#status)

</div>

---

## What is Gradient?

Every programming language ever built was designed around **human cognition** — mnemonic keywords, visual indentation, memorable syntax, human-readable error messages. Gradient discards these assumptions entirely.

Gradient is a **statically-typed language with compiler-enforced effect tracking and a structured query API**, built for one specific programmer: an LLM operating under a context window budget, running generate-compile-fix loops at machine speed.

What actually makes Gradient different:

- **Enforced effect system** -- every side effect (`IO`, `Net`, `FS`, `Mut`, `Time`) is tracked in the type system. Functions are pure by default, and the compiler *proves* purity.
- **Compiler-as-library API** -- agents don't scrape CLI output. They call `Session::from_source`, `check()`, `symbols()`, `module_contract()` and get structured data back.
- **Module capabilities** -- `@cap` annotations restrict what effects a module is allowed to use. The compiler enforces the boundary.
- **Call graph analysis** -- the compiler builds and exposes the full call graph, enabling dependency analysis, dead code detection, and impact analysis for agents.
- **Compiler-verified rename** -- rename a symbol and the compiler guarantees correctness across the codebase.

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
| 1 | **Provable purity** | Functions are pure by default; the compiler proves it via enforced effect tracking |
| 2 | **Enforced effects** | Five effects (`IO`, `Net`, `FS`, `Mut`, `Time`) tracked in the type system -- no silent side effects |
| 3 | **Structured compiler API** | Agents interact with the compiler through typed queries, not string parsing |
| 4 | **Capability-based sandboxing** | Modules declare their allowed effects with `@cap`; the compiler enforces the boundary |
| 5 | **Token efficiency** | Every saved token is reclaimed context window and reduced inference cost |
| 6 | **Unambiguous parseability** | An LLM generates correct Gradient on the first pass, not the fifth |

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

## Agent-First Features

### Structured Query API

Agents interact with the compiler as a library, not by parsing CLI output.

```rust
let session = Session::from_source(src);
let diags    = session.check();       // type errors, effect mismatches
let syms     = session.symbols();     // every symbol with type + span
let contract = session.module_contract(); // public API surface
```

All results are structured data. No regex. No scraping.

### Enforced Effect System

Gradient tracks five effects: **IO**, **Net**, **FS**, **Mut**, **Time**.

Functions are **pure by default**. If a function performs IO, it must declare `!{IO}` in its signature. If it doesn't declare effects and doesn't call anything effectful, the compiler *proves* it is pure.

```
fn add(a: Int, b: Int) -> Int:       // proven pure -- no effects
    ret a + b

fn greet(name: String) -> !{IO} ():  // must declare IO
    print("Hello, " + name)
```

### Module Capabilities

Modules declare their allowed effects with `@cap`:

```
@cap(IO, Net)
mod http_client

fn fetch(url: String) -> !{IO, Net} String:
    ...
```

If a module tries to use an effect it hasn't declared, the compiler rejects it.

### Call Graph and Dependency Analysis

The compiler builds the full call graph and exposes it to agents. This enables:

- **Impact analysis** -- which functions are affected by a change?
- **Dead code detection** -- which functions are never called?
- **Dependency tracking** -- what does this function transitively depend on?

### Compiler-Verified Rename

Rename a symbol and the compiler guarantees correctness. The rename operation uses the type system and call graph to find every reference, including across module boundaries.

### CLI JSON Mode

Every analysis command supports `--json` for structured agent consumption:

```bash
gradient check --json        # type errors as JSON
gradient inspect --json      # symbols, types, spans as JSON
gradient effects --json      # effect annotations as JSON
```

---

## Compiler Architecture

```
Source (.gr)
    |
    v
Lexer (61 tests) ---------- Token stream with INDENT/DEDENT injection
    |
    v
Parser + AST (47 tests) --- Recursive descent, error recovery
    |
    v
Type Checker (59 tests) --- Static types, inference, effect validation
    |
    v
IR Builder (27 tests) ----- AST to SSA-form intermediate representation
    |
    v
Query API (33 tests) ------ Structured queries: symbols, contracts, call graph
    |
    v
Effect System (2 tests) --- Enforced effect tracking, purity proofs
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

Gradient is in **alpha**. The compiler works. Programs compile to native binaries. The test suite has **232 tests** across the lexer, parser, type checker, IR builder, query API, effect system, and LSP server.

Phases A through E are **complete**. See the [roadmap](docs/roadmap.md) for details.

**What works:**
- Full compilation pipeline: source to native binary
- Recursion, arithmetic, conditionals, string concatenation
- Type checking with inference and effect validation
- Enforced effect system with 5 effects (IO, Net, FS, Mut, Time)
- Structured query API (Session::from_source, check, symbols, module_contract)
- Module capability constraints (`@cap` annotations)
- Call graph and dependency analysis
- Compiler-verified rename
- Working CLI (`gradient new/build/run/check`) with `--json` output
- LSP server with diagnostics, hover, and completions

**What's next:**
- Row-polymorphic effect inference
- Pattern matching and algebraic data types
- Effect handlers (resume/abort)
- Package system and dependency resolution
- Expand call graph analysis to cross-module boundaries

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
