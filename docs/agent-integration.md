# Agent Integration Guide

This document is the primary reference for AI agents integrating Gradient into their tool chains and infrastructure. It covers Gradient's machine-oriented design features, the structured output format, typed holes for iterative code generation, the LSP server with agent-specific extensions, and recommended integration patterns.

## Why Gradient for Agents

Gradient is an LLM-first programming language. Several design decisions make it unusually well-suited as an agent's target language.

**Token-efficient syntax.** Gradient uses minimal keywords, no redundant delimiters (no semicolons, no braces for blocks), and indentation-based blocks with colon delimiters. This reduces the number of tokens an agent must generate and parse, lowering latency and cost per interaction.

**Deterministic compilation.** The compiler pipeline is fully wired: source goes in, a native binary comes out. `gradient build` and `gradient run` work end-to-end. Agents can compile and test programs in a single command. The language supports conditionals, loops, recursion, mutable bindings, enum types (algebraic data types), and pattern matching (`match` on integers, booleans, enum variants, and wildcards).

**Static type checking.** The type checker catches errors before code generation. `gradient check` validates a program without producing a binary. The type checker reports all errors (not just the first), making fix-all-at-once workflows possible.

**Typed holes.** Writing `?hole` anywhere an expression is expected triggers hole-filling feedback. The compiler reports the expected type at that position, helping agents fill in expressions incrementally.

**LSP server.** The LSP server provides real-time diagnostics, hover information, and completions over JSON-RPC. The custom `gradient/batchDiagnostics` notification sends all diagnostics for a file in one message, designed for agents that process files atomically.

**Capability security.** Effects are declared in function signatures with `!{IO}`. The compiler enforces 5 canonical effects (IO, Net, FS, Mut, Time) and rejects unknown effects. The type checker verifies that pure functions do not call effectful ones -- `is_pure: true` in the symbol table means COMPILER PROVEN purity, not just the absence of an annotation. The `@cap(IO)` annotation constrains an entire module to only use IO effects, and `@cap()` requires the module to be entirely pure. An agent can statically determine what a piece of code is permitted to do before executing it.

## Structured Query API (Primary Agent Interface)

The compiler-as-library API is the most powerful way for agents to interact with Gradient. Instead of parsing CLI output, agents call structured Rust methods and receive JSON-serializable results.

- **`Session::from_source(code)`** -- one-call setup; creates a compiler session from a source string.
- **`Session::from_file(path)`** -- creates a compiler session from a file path; resolves `use` imports relative to the file's location for multi-file projects.
- **`session.check()`** -- returns `CheckResult` with JSON diagnostics (errors, warnings, per-phase counts).
- **`session.symbols()`** -- returns the symbol table with `is_pure`, `effects`, types, and signatures for every symbol.
- **`session.module_contract()`** -- returns a compact API summary including function signatures, effects, purity, capability ceiling, and call graph.
- **`session.effect_summary()`** -- returns per-function effect inference showing which effects each function body actually uses.
- **`session.rename(old, new)`** -- compiler-verified rename: updates all references, re-checks the program, and returns a `RenameResult` with locations and verification status.
- **`session.callees(fn_name)`** -- dependency query returning which functions a given function calls.

All results are serde-serializable and can be converted to JSON with a single call.

## Compilation Workflow

The simplest integration workflow is:

```
1. gradient new my-project     # Create a project
2. Write src/main.gr           # Generate Gradient source
3. gradient check              # Type-check (fast feedback)
4. gradient build              # Compile to native binary
5. gradient run                # Execute
```

**Multi-file support:** The compiler resolves `use` declarations to source files
on disk (`use math` resolves to `math.gr`, `use a.b` resolves to `a/b.gr`).
Agents can split code across multiple `.gr` files and the compiler will resolve
and compile them together. For multi-file projects, use `Session::from_file(path)`
instead of `Session::from_source(code)` -- it resolves imports relative to the
file's location and compiles all referenced modules.

Or in a tight loop:

```
Agent -> write .gr files -> gradient check -> fix errors -> gradient run -> check output -> done
```

## Machine-Readable Output

The compiler supports structured JSON output via CLI flags:
- `--check --json` -- structured diagnostics with per-phase error counts
- `--inspect --json` -- module contract (signatures, effects, purity, call graph)
- `--effects --json` -- per-function effect analysis

Compiler errors are also printed to stderr in human-readable form. Each error includes:
- The compiler phase that produced it (parse error, type error, IR error)
- A description of the problem (expected type, found type, etc.)

The LSP server provides structured diagnostics via standard LSP protocol and the custom `gradient/batchDiagnostics` notification (see below).

## Typed Holes for Agent-Assisted Coding

Typed holes are Gradient's mechanism for incremental, type-directed code generation. An agent writes a skeleton with `?hole_name` placeholders, compiles, reads the compiler's response, and fills the holes.

### Example

Source file:

```
fn process(data: Int) -> Int:
    let filtered = ?filter_step
    let result = ?aggregate_step
    ret result
```

Running `gradient check` on this file will report the expected types for each hole, helping the agent determine what expressions to fill in.

### How Agents Should Use Holes

1. Generate a function skeleton with holes at every decision point.
2. Run `gradient check` and read the error output.
3. Use the expected type information to constrain generation.
4. Fill holes one at a time, re-checking after each fill to propagate type information.
5. When `gradient check` returns zero errors, the function is complete.

This workflow is more reliable than generating an entire function body in one pass because each step is validated by the type checker.

## Language Server Protocol (LSP)

Gradient ships a working LSP server (`codebase/devtools/lsp/`). It communicates over stdio via JSON-RPC.

### Implemented Features

- **Diagnostics** -- errors from the lexer, parser, and type checker are published on file open, change, and save.
- **Hover** -- type and signature information for builtins, keywords, and user-defined functions.
- **Completion** -- context-aware suggestions: keywords and builtin function names with signatures.
- **`gradient/batchDiagnostics`** -- custom notification (see below).

### Agent-Specific: Batch Diagnostics

The custom `gradient/batchDiagnostics` notification sends all diagnostics for a file in a single message. The payload includes:

```json
{
  "uri": "file:///path/to/main.gr",
  "diagnostics": [ ... ],
  "lex_errors": 0,
  "parse_errors": 0,
  "type_errors": 1
}
```

This is preferred for agents that process files atomically -- no streaming, no incremental updates. The per-phase error counts let agents decide how to respond (e.g., if `lex_errors > 0`, the source is fundamentally malformed; if only `type_errors > 0`, the syntax is valid but types are wrong).

### Planned Extensions

| Method | Description | Status |
|---|---|---|
| `gradient/holeFill` | Request fill suggestions for a typed hole. | Planned |
| `gradient/effectQuery` | Query what effects a function or module requires. | **Implemented** via `session.effect_summary()` and `--effects --json` |
| `gradient/capabilityCheck` | Verify whether a code block stays within a capability budget. | **Implemented** via `@cap` annotations and `session.module_contract()` |
| `gradient/astDump` | Return the full AST as structured JSON. | Planned |
| `gradient/irDump` | Return the IR for a specific function as structured JSON. | Planned |

## CLI for Agents

The compiler supports JSON output flags for all major operations, making it easy for agents to parse results without scraping human-readable text:

```
gradient-compiler --check --json file.gr    # structured diagnostics
gradient-compiler --inspect --json file.gr  # module contract
gradient-compiler --effects --json file.gr  # effect analysis
```

All JSON output is serde-serialized and follows stable schemas. Agents should prefer these flags over parsing stderr text.

## Integration Patterns

### Pattern 1: Write, Check, Fix Loop

The simplest integration. The agent writes `.gr` files, runs the compiler, reads the diagnostics, fixes errors, and repeats.

```
Agent -> write .gr file -> gradient check -> read errors -> fix errors -> repeat
```

This pattern requires no LSP server and works with any agent framework that can invoke shell commands and read stderr.

### Pattern 2: Typed Holes for Scaffolding

The agent generates a skeleton with `?hole` placeholders, then uses the compiler's feedback to complete the implementation incrementally.

```
Agent -> write skeleton with ?holes -> gradient check -> read hole errors -> fill holes -> gradient check -> done
```

This pattern is best when the agent knows the high-level structure but is uncertain about specific expressions.

### Pattern 3: LSP for Interactive Development

The agent connects to the LSP server over stdio and uses it for real-time feedback during an extended coding session.

```
Agent <-> LSP server (JSON-RPC/stdio) -> real-time diagnostics, completions, hover
```

This pattern is best for agents that maintain a long-running session and make many small edits. The LSP server re-runs the compiler pipeline on every change.

### Pattern 4: Build and Execute

The agent compiles the program and inspects the output to verify correctness.

```
Agent -> gradient build -> gradient run -> capture stdout -> verify output -> done
```

This pattern is useful for agents that can validate program behavior by checking the output against expected results.

### Pattern 5: Structured API Integration

For Rust-based agents, the compiler-as-library API provides the richest integration. The agent creates a `Session` directly and calls structured query methods:

```rust
use gradient_compiler::Session;

let session = Session::from_source(r#"
    mod example
    fn add(a: Int, b: Int) -> Int:
        ret a + b
    fn main() -> !{IO} ():
        print_int(add(3, 4))
"#);

// Type-check and get structured diagnostics
let result = session.check();
assert!(result.errors.is_empty());

// Get the module contract (signatures, effects, purity, call graph)
let contract = session.module_contract();

// Query per-function effects
let effects = session.effect_summary();

// Query call graph
let callees = session.callees("main");

// Compiler-verified rename
let rename_result = session.rename("add", "sum");
```

This pattern gives agents full access to the compiler's analysis without any serialization overhead, and is the recommended approach for agents built in Rust.

## Built-in Functions Available to Agents

All builtins are available without imports:

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

String concatenation uses the `+` operator. Integer modulo uses the `%` operator.

## Getting Started as an Agent

1. Read `docs/language-guide.md` for syntax and semantics.
2. Read `resources/grammar.peg` for the formal grammar definition.
3. Study the test programs in `codebase/compiler/tests/` for working examples.
4. Use `gradient check` to validate generated code.
5. Use typed holes (`?name`) when unsure about specific expressions -- let the type checker guide generation.
6. Remember: every block-opening line (`fn`, `if`, `else`, `for`, `while`, `match`) must end with `:`.
