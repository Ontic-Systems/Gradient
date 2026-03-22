<div align="center">

<img src="assets/banner.png" alt="Gradient" width="540"/>

<br/>
<br/>

**The first programming language where the compiler proves what your code can and cannot do.**

<br/>

[![Status](https://img.shields.io/badge/status-alpha-blueviolet?style=flat-square&labelColor=0d0d17)](https://github.com/graydeon/Gradient)
[![Language](https://img.shields.io/badge/impl-Rust-orange?style=flat-square&labelColor=0d0d17)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-4f8aff?style=flat-square&labelColor=0d0d17)](LICENSE)
[![Backend](https://img.shields.io/badge/backend-Cranelift-00e5ff?style=flat-square&labelColor=0d0d17)](https://cranelift.dev)
[![Tests](https://img.shields.io/badge/tests-386-brightgreen?style=flat-square&labelColor=0d0d17)](#status)

</div>

---

## What is Gradient?

The research is clear on what makes AI code generation work:

- **Grammar-constrained decoding** eliminates syntax errors entirely (SynCode, XGrammar)
- **Type-directed generation** reduces compile errors by 75% (ETH Zurich, PLDI '25)
- **Enforced effects** let agents trust function signatures without reading implementations
- **Design-by-contract** enables generate-verify loops with 82--96% success rates (Dafny research)

Gradient is being built to deliver **all of these** in a single language. It is a statically-typed language designed for one specific programmer: an LLM operating under a context window budget, running generate-compile-fix loops at machine speed.

**What works today:**

- **Design-by-contract** -- `@requires`/`@ensures` annotations with runtime contract checking. The `result` keyword in postconditions references the return value. Contract violations produce structured error messages. This enables the generate-verify workflow.
- **Grammar for constrained decoding** -- formal EBNF grammar (`resources/gradient.ebnf`) compatible with XGrammar, llguidance, and Outlines. Agents using Gradient through an inference engine can guarantee syntactically valid output.
- **Enforced effect system** -- every side effect (`IO`, `Net`, `FS`, `Mut`, `Time`) is tracked in the type system. Functions are pure by default, and the compiler *proves* purity.
- **Structured compiler API** -- agents call `Session::from_source`, `check()`, `symbols()`, `module_contract()` and get structured data back. No CLI scraping, no regex.
- **Module capabilities** -- `@cap` annotations restrict what effects a module is allowed to use. The compiler enforces the boundary.
- **Call graph analysis** -- the compiler builds and exposes the full call graph, enabling dependency analysis, dead code detection, and impact analysis.
- **Canonical formatter** -- one representation per program, eliminating style ambiguity for generators.
- **Compiler-verified rename** -- rename a symbol and the compiler guarantees correctness across the codebase.

**Coming next (Tier 1 research-driven priorities):**

- **Type-directed completion context** -- the compiler tells the agent exactly what types are valid at any cursor position
- **Generics with bidirectional type inference** -- fewer annotations, richer type information for generation

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

### While Loop with Mutable Bindings

```
mod countdown

fn main() -> !{IO} ():
    let mut i: Int = 5
    while i > 0:
        print_int(i)
        i = i - 1
    print("Liftoff!")
```

### Match Expression

```
mod match_demo

fn describe(n: Int) -> String:
    let label: String = match n:
        0:
            "zero"
        1:
            "one"
        _:
            "other"
    ret label

fn main() -> !{IO} ():
    print(describe(0))
    print(describe(1))
    print(describe(42))
```

### Enum Types

```
mod traffic

type Light = Red | Yellow | Green

fn action(light: Light) -> String:
    match light:
        Red:
            "stop"
        Yellow:
            "caution"
        Green:
            "go"

fn main() -> !{IO} ():
    print(action(Green))
```

### Design-by-Contract

```
mod contracts

@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

@requires(a > 0)
@requires(b > 0)
@ensures(result > 0)
fn multiply_positive(a: Int, b: Int) -> Int:
    ret a * b

fn main() -> !{IO} ():
    print_int(factorial(5))
    print_int(multiply_positive(3, 4))
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

Eight research-validated principles, in priority order:

| # | Priority | What it means |
|---|---|---|
| 1 | **Verifiable correctness** | Enforced effects and design-by-contract (`@requires`/`@ensures`) shipped. Agents prove code correct, not just plausible |
| 2 | **Grammar-constrained generation** | LL(1) grammar with EBNF export for XGrammar/vLLM/Outlines -- structurally eliminates syntax errors |
| 3 | **Type-directed completion** | Rich type context at every cursor position guides generation; reduces compile errors by 75% (ETH PLDI '25) |
| 4 | **Structured compiler API** | Agents interact with the compiler through typed queries, not string parsing |
| 5 | **Token efficiency** | Every saved token is reclaimed context window and reduced inference cost |
| 6 | **Capability-based sandboxing** | Modules declare their allowed effects with `@cap`; the compiler enforces the boundary |
| 7 | **Enforced effects** | Five effects (`IO`, `Net`, `FS`, `Mut`, `Time`) tracked in the type system -- no silent side effects |
| 8 | **Unambiguous parseability** | One canonical form per construct; the formatter is a normalization function, not a style guide |

---

## Language Design

### Syntax
- **ASCII-only** -- no Unicode operators; every symbol is a single token in major LLM tokenizers
- **Indentation-significant** -- no braces for blocks, no semicolons, no redundant delimiters
- **Colon-delimited blocks** -- `fn`, `if`, `else`, `for`, `while`, `match` use `:` before their indented body
- **Keyword-led** -- `fn`, `let`, `if`, `for`, `while`, `match`, `ret`, `type`, `mod`, `use`, `impl`
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
Lexer (70 tests) ---------- Token stream with INDENT/DEDENT injection
    |
    v
Parser + AST (61 tests) --- Recursive descent, error recovery
    |
    v
Type Checker (94 tests) --- Static types, inference, effect validation, contracts
    |
    v
IR Builder (29 tests) ----- AST to SSA-form intermediate representation
    |
    v
Query API (43 tests) ------ Structured queries: symbols, contracts, call graph
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
gradient fmt             Canonical formatter (--fmt flag on gradient-compiler)
gradient repl            Interactive session (--repl flag on gradient-compiler)
```

Scaffolded (not yet functional):

```
gradient test            Run @test-annotated functions
gradient init            Initialize project in current directory
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

## Research Foundation

Gradient's roadmap is driven by a systematic literature review of 60+ papers spanning constrained decoding, type-directed synthesis, formal verification, and LLM code generation. The key finding: LLMs achieve 82--96% first-pass success rates when generating code against formal specifications (demonstrated in Dafny research), making design-by-contract the single highest-leverage feature to build. Grammar-constrained decoding (SynCode, XGrammar) and type-directed generation (ETH Zurich PLDI '25) round out the top tier. See the [full roadmap](docs/roadmap.md) for the prioritized research-backed feature list.

---

## Build Philosophy

Gradient is built **slow and modular**. At every point in development there is a working, testable artifact. Nothing is theoretical. Nothing ships without passing tests in a live environment.

The build roadmap is structured as progressive phases -- each one adding exactly one capability to the live system. The hard checkpoint was Phase 4: `gradient build` produces a real native binary from `.gr` source. That checkpoint is green.

---

## Status

Gradient is in **alpha**. The compiler works. Programs compile to native binaries. The test suite has **386 tests** (384 unit + 2 integration) across the lexer, parser, type checker, IR builder, query API, effect system, LSP server, formatter, and REPL.

Phases 0 through M are **complete**. See the [roadmap](docs/roadmap.md) for details.

**What works:**
- Full compilation pipeline: source to native binary, including multi-file compilation
- Multi-file module resolution: `use math` resolves to `math.gr`, `use a.b` resolves to `a/b.gr`, with qualified calls across modules
- Recursion, arithmetic, conditionals, string concatenation, mutable bindings, while loops, pattern matching (match on int/bool/enum variants with wildcard)
- Enum types (algebraic data types) with unit variants; tuple variant payloads parsed but codegen deferred
- Type checking with inference and effect validation
- Enforced effect system with 5 effects (IO, Net, FS, Mut, Time)
- Design-by-contract: `@requires`/`@ensures` annotations with runtime contract checking, `result` keyword in postconditions, structured contract violation errors
- Grammar for constrained decoding: formal EBNF grammar for XGrammar/llguidance/Outlines integration
- Structured query API (Session::from_source, check, symbols, module_contract)
- Module capability constraints (`@cap` annotations)
- Call graph and dependency analysis
- Compiler-verified rename
- Working CLI (`gradient new/build/run/check`) with `--json` output
- LSP server with diagnostics, hover, and completions
- Canonical formatter (`gradient fmt` / `--fmt`) with `--write` mode for in-place updates
- Interactive REPL (`gradient repl` / `--repl`) with type inference feedback and non-interactive piping support

**What's next (Tier 1 research-driven priorities):**
- Type-directed completion context
- Generics and bidirectional type inference

---

## Team

Gradient is built by Gray d'Eon.

---

## License

MIT -- see [LICENSE](LICENSE).

---

<div align="center">
<sub>built on research, verified by the compiler, trusted by agents</sub>
</div>
