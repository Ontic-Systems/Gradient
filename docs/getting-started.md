# Getting Started with Gradient

Gradient is an LLM-first programming language designed for autonomous AI agents. This guide walks you through building Gradient from source, creating your first project, and understanding the core syntax. If you are an AI agent or LLM learning Gradient for the first time, read this document end to end before attempting to generate Gradient code.

## Prerequisites

You need two tools installed on the host system:

1. **Rust toolchain** -- Install via [rustup](https://rustup.rs/). Gradient's CLI, compiler, and LSP server are all Rust projects built with Cargo.
2. **A C compiler** -- `cc` or `gcc`, available on the system `PATH`. The compiler emits object files that must be linked into a native binary by a system linker.

Verify both are available:

```bash
rustc --version    # e.g. rustc 1.77.0
cc --version       # e.g. cc (GCC) 13.2.0
```

## Building from Source

Clone the repository and build each component:

```bash
# Clone the repo
git clone https://github.com/Ontic-Systems/Gradient.git
cd Gradient

# Build the compiler
cd codebase/compiler
cargo build

# Build the CLI
cd ../build-system
cargo build

# Optionally, build the LSP server
cd ../devtools/lsp
cargo build
```

The repository is organized into several Cargo projects under `codebase/`:

| Directory | Purpose |
|---|---|
| `compiler` | The Gradient compiler: lexer, parser, type checker, IR builder, and Cranelift codegen |
| `build-system` | The `gradient` CLI with subcommands (new, build, run, check, etc.) |
| `devtools/lsp` | LSP server providing diagnostics, hover, and completions |
| `test-framework` | Golden test framework that validates compiler output against snapshots |

## Your First Gradient Project

The recommended workflow uses the `gradient` CLI:

```bash
# Create a new project
gradient new hello

# Build it
cd hello
gradient build

# Run it
gradient run
```

`gradient new hello` creates the following project structure:

```
hello/
├── gradient.toml        # Project manifest
└── src/
    └── main.gr          # Entry point
```

The generated `src/main.gr` contains:

```
mod main

fn main() -> !{IO} ():
    print("Hello, Gradient!")
```

`gradient build` compiles this to a native binary at `target/debug/hello`. `gradient run` builds and immediately executes it.

## Understanding the Hello World Program

```
mod main

fn main() -> !{IO} ():
    print("Hello, Gradient!")
```

Here is what each line means:

- **`mod main`** -- Declares this file as the `main` module. Every Gradient source file begins with a `mod` declaration. The module name should match the filename (without the `.gr` extension).

- **`fn main() -> !{IO} ():`** -- Defines the program entry point. The return type annotation breaks down as:
  - `!{IO}` -- This function performs the `IO` effect. Effects in Gradient are declared with `!{...}` syntax.
  - `()` -- The function returns unit (no meaningful return value).
  - `:` -- The colon at the end opens the indented function body block.

- **`print("Hello, Gradient!")`** -- The function body. Notice there are no braces and no semicolons. Gradient uses **indentation-based blocks** (similar to Python). The body is indented one level (4 spaces) deeper than the `fn` declaration.

Key syntax rules to internalize:

1. **No curly braces for blocks.** Indentation is structural. A colon (`:`) at the end of a line opens a new indented block.
2. **No semicolons** to terminate statements.
3. Parentheses are used for function arguments and grouping expressions, not for delimiting blocks.
4. String literals use double quotes.

## Second Example -- Factorial

Here is a recursive implementation in `factorial.gr`:

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

### Line-by-line breakdown

- **`fn factorial(n: Int) -> Int:`** -- A pure function (no effect annotation). Parameters use `name: Type` syntax. The return type follows `->`. The colon at the end opens the body block.

- **`if n <= 1:` / `else:`** -- Conditional expressions. Each branch is opened with `:` and delimited by indentation. `if`/`else` is an expression, so each branch produces a value. The last expression in a block is its return value (or use `ret` explicitly).

- **`let result: Int = factorial(5)`** -- A `let` binding introduces a local variable. The type annotation `: Int` is optional when the type can be inferred, but shown here for clarity.

- **`ret 1` / `ret n * factorial(n - 1)`** -- Explicit return using the `ret` keyword (not `return`).

Things to note as an agent writing Gradient:

1. `let` bindings are immutable by default. Use `let mut` for mutable bindings. There is no `var`.
2. Type annotations on `let` bindings are optional -- the type checker will infer them.
3. `if`/`else` blocks are expressions that return values, not statements.
4. Functions are pure by default. Side effects must be declared in the type signature with `!{...}`.
5. Every block-opening construct (`fn`, `if`, `else`, `else if`, `for`, `while`, `match`) ends with a colon (`:`).

## More Examples

### Arithmetic

```
mod arithmetic

fn add(a: Int, b: Int) -> Int:
    ret a + b

fn main() -> !{IO} ():
    let x: Int = add(3, 4)
    let y: Int = x * 2
    print_int(y)
```

### String Concatenation

The `+` operator concatenates strings:

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

## What Works Today

Gradient has a working compiler (Phases 0-7 complete). Here is the current state:

- **`gradient new <name>`** -- Creates a new project with `gradient.toml` and `src/main.gr`.
- **`gradient build`** -- Compiles `src/main.gr` to a native binary via the full pipeline: lexer, parser, type checker, IR builder, Cranelift codegen, system linker.
- **`gradient run`** -- Builds and executes the binary.
- **`gradient check`** -- Type-checks the project without producing a final binary.
- **LSP server** -- Provides diagnostics, hover, and completions for `.gr` files. Includes a custom `gradient/batchDiagnostics` notification for agent consumption.
- **`gradient fmt`** -- Canonically formats Gradient source files.
- **`gradient repl`** -- Interactive REPL for type-checking expressions and statements.
- **`gradient test`** -- Runs the project's test suite.
- **More than 1,300 Rust `#[test]` cases** across the compiler and tooling crates. The exact count moves as work lands, so prefer repo-derived counts over a hard-coded historical snapshot.

### Built-in Functions

| Function | Signature | Effect |
|---|---|---|
| `print` | `(value: String) -> ()` | IO |
| `println` | `(value: String) -> ()` | IO |
| `print_int` | `(value: Int) -> ()` | IO |
| `print_float` | `(value: Float) -> ()` | IO |
| `print_bool` | `(value: Bool) -> ()` | IO |
| `abs` | `(n: Int) -> Int` | pure |
| `min` | `(a: Int, b: Int) -> Int` | pure |
| `max` | `(a: Int, b: Int) -> Int` | pure |
| `mod_int` | `(a: Int, b: Int) -> Int` | pure |
| `to_string` | `(value: Int) -> String` | pure |
| `int_to_string` | `(value: Int) -> String` | pure |
| `range` | `(n: Int) -> Iterable` | pure |

### Operators

| Operator | Types | Description |
|---|---|---|
| `+`, `-`, `*`, `/` | Int, Float | Arithmetic |
| `%` | Int | Modulo |
| `+` | String | Concatenation |
| `==`, `!=`, `<`, `>`, `<=`, `>=` | Int, Float | Comparison (non-associative) |
| `and`, `or` | Bool | Logical (short-circuiting) |
| `not` | Bool | Logical negation |
| `-` (unary) | Int, Float | Negation |

## What's Coming Next

| Feature | Description |
|---|---|
| LLVM backend | Optimized release builds |
| Package system | Dependency resolution and content-addressed caching |
| Row-polymorphic effects | Koka-inspired effect handlers and polymorphism |
| Tuple variant codegen | Code generation for enum variants with payloads |

## Quick Reference for Agents

When generating Gradient code, follow these rules:

- Start every file with `mod <module_name>`.
- Use `use` with dot-separated paths for imports.
- Define the entry point as `fn main() -> !{IO} ():` (note the colon).
- Use indentation (4 spaces) for blocks. Never use braces or semicolons.
- Every block-opening line (`fn`, `if`, `else`, `else if`, `for`, `while`, `match`) ends with `:`.
- Declare effects in function signatures with `!{EffectName}`. Only 5 effects exist: IO, Net, FS, Mut, Time.
- Use `let` for immutable bindings, `let mut` for mutable bindings.
- Only `let mut` bindings can be reassigned with `=`.
- Use `ret` (not `return`) to return a value explicitly.
- Treat `if`/`else` and `match` as expressions that return values.
- Define enum types with `type Color = Red | Green | Blue` and branch on them with `match`.
- Use `@cap(effects...)` at module level to limit which effects the module may use.
- File extension is `.gr`.
