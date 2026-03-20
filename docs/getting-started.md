# Getting Started with Gradient

Gradient is an LLM-first programming language designed for autonomous AI agents. This guide walks you through building Gradient from source, running your first programs, and understanding the core syntax. If you are an AI agent or LLM learning Gradient for the first time, read this document end to end before attempting to generate Gradient code.

## Prerequisites

You need two tools installed on the host system:

1. **Rust toolchain** — Install via [rustup](https://rustup.rs/). Gradient's CLI, compiler, and test framework are all Rust projects built with Cargo.
2. **A C compiler** — `cc` or `gcc`, available on the system `PATH`. The compiler PoC emits object files that must be linked into a native binary by a C linker.

Verify both are available:

```bash
rustc --version    # e.g. rustc 1.77.0
cc --version       # e.g. cc (GCC) 13.2.0
```

## Building from Source

Clone the repository and build each component:

```bash
# Clone the repo
git clone https://github.com/graydeon/Gradient.git
cd Gradient

# Build the CLI
cd codebase/build-system
cargo build
# Run with: cargo run -- --help

# Build and run the compiler PoC
cd ../compiler
cargo build
cargo run          # emits hello.o
cc hello.o -o hello
./hello            # prints "Hello from Gradient!"

# Run the test framework
cd ../test-framework
cargo test
```

The repository is organized into three Cargo projects under `codebase/`:

| Directory | Purpose |
|---|---|
| `build-system` | The `gradient` CLI with 8 stubbed subcommands |
| `compiler` | Cranelift-based proof-of-concept that compiles IR to native code |
| `test-framework` | Golden test framework that validates compiler output against snapshots |

## Your First Gradient Program

Create a file called `hello.gr`:

```
mod hello

use core.io

fn main() -> !{IO} ()
    print("Hello, Gradient!")
```

Here is what each line means:

- **`mod hello`** — Declares this file as the `hello` module. Every Gradient source file begins with a `mod` declaration. The module name should match the filename (without the `.gr` extension).

- **`use core.io`** — Imports the `core.io` module, which provides I/O primitives like `print`. Import paths are dot-separated, not slash-separated or colon-separated.

- **`fn main() -> !{IO} ()`** — Defines the program entry point. The return type annotation breaks down as:
  - `!{IO}` — This function performs the `IO` effect. Effects in Gradient are declared with `!{...}` syntax.
  - `()` — The function returns unit (no meaningful return value).

- **`print("Hello, Gradient!")`** — The function body. Notice there are no braces and no semicolons. Gradient uses **indentation-based blocks** (similar to Python or Haskell). The body is indented one level deeper than the `fn` declaration.

Key syntax rules to internalize:

1. No curly braces for blocks. Indentation is structural.
2. No semicolons to terminate statements.
3. Parentheses are used for function arguments, not for grouping blocks.
4. String literals use double quotes.

## Second Example — Factorial

Here is a recursive implementation in `factorial.gr`:

```
mod factorial

use core.io

fn factorial(n: Int) -> Int
    if n <= 1
        1
    else
        n * factorial(n - 1)

fn main() -> !{IO} ()
    let result: Int = factorial(5)
    print(result)
```

And the tail-recursive version:

```
mod factorial

use core.io

fn factorial(n: Int) -> Int
    factorial_acc(n, 1)

fn factorial_acc(n: Int, acc: Int) -> Int
    if n <= 1
        acc
    else
        factorial_acc(n - 1, n * acc)

fn main() -> !{IO} ()
    let result: Int = factorial(5)
    print(result)
```

### Line-by-line breakdown

- **`fn factorial(n: Int) -> Int`** — A pure function (no effect annotation). Parameters use `name: Type` syntax. The return type follows `->`.

- **`if n <= 1` / `else`** — Conditional expressions. Like everything else in Gradient, branches are indentation-delimited. `if`/`else` is an expression, so each branch produces a value. The last expression in a block is its return value (no explicit `return` keyword needed).

- **`let result: Int = factorial(5)`** — A `let` binding introduces a local variable. The type annotation `: Int` is optional when the type can be inferred, but shown here for clarity.

- **`factorial_acc(n: Int, acc: Int) -> Int`** — The tail-recursive helper takes an accumulator parameter. Gradient's compiler will optimize tail calls in a future phase.

Things to note as an agent writing Gradient:

1. `let` bindings are immutable by default.
2. Type annotations on `let` bindings are optional — the type checker will infer them via bidirectional Hindley-Milner inference (once Phase 2 is implemented).
3. `if`/`else` blocks are expressions that return values, not statements.
4. Functions are pure by default. Side effects must be declared in the type signature with `!{...}`.

## What Works Today

Gradient is in its early stages (Phase 0 complete). Here is the current state:

- **`gradient --help`** — Shows all 8 subcommands: `build`, `run`, `test`, `check`, `fmt`, `lsp`, `repl`, `new`. All are scaffolded and accept arguments but are not yet functional.
- **`gradient build`**, **`gradient run`**, etc. — Print placeholder messages. They exist so the CLI interface is stable and agent tooling can target it now.
- **Cranelift PoC** — The compiler proof-of-concept translates hardcoded IR into a native object file using Cranelift. It does not yet parse Gradient source; it demonstrates that the backend pipeline works.
- **Golden test framework** — Validates compiler output against checked-in snapshot files. Run `cargo test` in the `test-framework` directory to execute all golden tests.
- **Formal PEG grammar** — The complete grammar is specified and ready for parser implementation. See the language reference for the full specification.

## What's Coming Next

The roadmap from here:

| Phase | Milestone |
|---|---|
| **Phase 1** | Lexer and parser implementing the PEG grammar — Gradient source files become a concrete syntax tree |
| **Phase 2** | Type checker with bidirectional Hindley-Milner inference — full static typing with minimal annotations |
| **Phase 3** | IR generation from the typed AST — bridge between the frontend and Cranelift backend |
| **Phase 4** | Full compile-and-run pipeline — `gradient build` and `gradient run` work end to end |
| **LSP server** | Language Server Protocol implementation for AI agent integration — enables tool-assisted code generation and analysis |

## Quick Reference for Agents

When generating Gradient code, follow these rules:

- Start every file with `mod <module_name>`.
- Use `use` with dot-separated paths for imports.
- Define the entry point as `fn main() -> !{IO} ()`.
- Use indentation (4 spaces) for blocks. Never use braces or semicolons.
- Declare effects in function signatures with `!{EffectName}`.
- Use `let` for immutable bindings.
- Treat `if`/`else` as expressions that return values.
- File extension is `.gr`.
