# Agent Integration Guide

This document is the primary reference for AI agents integrating Gradient into their tool chains and infrastructure. It covers Gradient's machine-oriented design features, the structured output format, typed holes for iterative code generation, the LSP server with agent-specific extensions, and recommended integration patterns.

## Why Gradient for Agents

Gradient is an LLM-first programming language. Its design is driven by empirical research on how LLMs generate, check, and verify code. Every major feature maps to a published result showing measurable improvement in agent-generated code quality.

**Grammar-constrained decoding ready.** Gradient's grammar is LL(1)-parseable, which means constrained decoding engines (XGrammar, vLLM) can enforce it token-by-token to guarantee syntactically valid output. SynCode (Ugare et al., 2024) demonstrated that grammar-constrained decoding eliminates all syntax errors at near-zero latency overhead. Gradient is designed from the ground up to be compatible with this approach.

**Enforced effects = trustable signatures.** Effects are declared in function signatures with `!{IO}`. The compiler enforces 5 canonical effects (IO, Net, FS, Mut, Time) and rejects unknown effects. The type checker verifies that pure functions do not call effectful ones -- `is_pure: true` in the symbol table means COMPILER PROVEN purity, not just the absence of an annotation. An agent reads `fn compute(x: Int) -> Int` and KNOWS it is pure -- compiler-proven. No other mainstream language provides this guarantee. The `@cap(IO)` annotation constrains an entire module to only use IO effects, and `@cap()` requires the module to be entirely pure. An agent can statically determine what a piece of code is permitted to do before executing it.

**Structured compiler API.** CoCoGen (Li et al., ACL 2024) showed that structured compiler feedback improves LLM code generation by 80%+ compared to raw error messages. Gradient's query API (`session.check()`, `session.symbols()`, `session.module_contract()`) provides exactly this: JSON-serializable, semantically rich compiler output that agents can consume directly.

**Typed holes for generation.** Blinn et al. (OOPSLA 2024) showed that typed holes are the most effective form of context for LLM code generation, outperforming docstrings and example-based prompting. Writing `?hole` anywhere an expression is expected triggers hole-filling feedback. The compiler reports the expected type at that position, enabling incremental, type-directed generation.

**Coming: Design-by-contract.** Research shows LLMs achieve 82-96% verification success rates on Dafny-style specifications (Chakraborty et al., 2024; Sun et al., 2024). Gradient is adding `@requires`/`@ensures` annotations to enable the generate-verify workflow -- agents generate code, compilers verify contracts hold.

**Token-efficient syntax.** Gradient uses minimal keywords, no redundant delimiters (no semicolons, no braces for blocks), and indentation-based blocks with colon delimiters. This reduces the number of tokens an agent must generate and parse, lowering latency and cost per interaction.

**Deterministic compilation.** The compiler pipeline is fully wired: source goes in, a native binary comes out. `gradient build` and `gradient run` work end-to-end. Agents can compile and test programs in a single command. The language supports conditionals, loops, recursion, mutable bindings, enum types (algebraic data types), and pattern matching (`match` on integers, booleans, enum variants, and wildcards).

**LSP server.** The LSP server provides real-time diagnostics, hover information, and completions over JSON-RPC. The custom `gradient/batchDiagnostics` notification sends all diagnostics for a file in one message, designed for agents that process files atomically.

## Research Foundation

Gradient's roadmap is driven by a systematic literature review of how LLMs interact with programming languages, compilers, and verification tools. The review synthesized findings from over 30 papers into 8 design principles:

1. **Grammar-constrained decoding** -- LL(1) grammars enable token-level enforcement of syntax validity.
2. **Structured compiler feedback** -- JSON diagnostics with semantic context outperform raw error text.
3. **Type-directed completion** -- Typed holes provide the most effective generation context.
4. **Effect tracking** -- Compiler-enforced purity enables trustable function signatures.
5. **Design-by-contract** -- Formal specifications unlock the generate-verify workflow.
6. **Incremental verification** -- Check early, check often, check structurally.
7. **Token efficiency** -- Fewer tokens per construct means lower latency and cost.
8. **Machine-first output** -- Every compiler output should be JSON-serializable.

Each feature in Gradient maps to one or more of these principles, and each principle is backed by published empirical results. The full synthesis is available in `resources/research-synthesis.md` in the repository.

## Upcoming: The Generate-Verify Workflow

The research points to a vision where agent-generated code is not just compiler-CHECKED but compiler-VERIFIED. Gradient is building toward this workflow:

1. **Agent generates Gradient code with grammar-constrained decoding.** Because Gradient's grammar is LL(1), constrained decoding engines can enforce it token-by-token. Result: zero syntax errors (SynCode, 2024).
2. **Compiler provides type-directed completion context.** Typed holes and structured diagnostics give the agent precise type information at every decision point. Result: 75% fewer type errors (Blinn et al., OOPSLA 2024).
3. **Functions have `@requires`/`@ensures` contracts.** Agents generate not just implementations but specifications. The contracts are machine-checkable declarations of intent.
4. **Compiler verifies contracts hold.** The compiler proves that implementations satisfy their contracts. Research shows 82-96% verification success rates on LLM-generated code with Dafny-style specs.
5. **Effect system guarantees no undeclared side effects.** A function without `!{IO}` in its signature is compiler-proven pure. No runtime surprises.
6. **Result: agent-generated code that is compiler-VERIFIED, not just compiler-CHECKED.** The combination of grammar constraints, type checking, contract verification, and effect enforcement means the compiler can vouch for correctness, not just well-formedness.

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

## Canonical Formatter

The `--fmt` flag on `gradient-compiler` normalizes Gradient source into canonical form. This is useful for agents that generate code and want to ensure consistent formatting before committing, diffing, or feeding code back into a context window.

```bash
# Print formatted output to stdout (no file modification)
gradient-compiler --fmt file.gr

# Format and overwrite the file in place
gradient-compiler --fmt --write file.gr
```

**Agent use cases:**

- **Normalize before diffing.** Format both files before comparing to eliminate spurious whitespace differences.
- **Post-generation cleanup.** After generating `.gr` code, pipe it through `--fmt` to guarantee canonical output without manually tracking indentation rules.
- **CI checks.** Run `--fmt` and compare against the original to enforce consistent style across a codebase.

The formatter produces one canonical form per construct. There are no configuration options or style knobs -- the output is deterministic.

## Interactive REPL

The `--repl` flag on `gradient-compiler` starts an interactive evaluation session backed by Cranelift. Agents can use the REPL to quickly evaluate expressions and inspect inferred types without creating a full project.

```bash
# Interactive mode (for human or agent terminal sessions)
gradient-compiler --repl

# Non-interactive mode: pipe expressions via stdin
echo "1 + 2" | gradient-compiler --repl
```

**Non-interactive mode for agent piping.** When stdin is not a TTY (i.e., input is piped), the REPL reads from stdin, evaluates each line, and prints results to stdout. This allows agents to programmatically query the type checker and evaluator without maintaining an interactive session.

**Agent use cases:**

- **Type inference queries.** Pipe an expression to `--repl` to get its inferred type without writing a full program.
- **Quick evaluation.** Verify the result of an arithmetic or string expression before embedding it in generated code.
- **Exploratory prototyping.** Test small code fragments in isolation before assembling them into a module.

## CLI for Agents

The compiler supports JSON output flags for all major operations, making it easy for agents to parse results without scraping human-readable text:

```
gradient-compiler --check --json file.gr    # structured diagnostics
gradient-compiler --inspect --json file.gr  # module contract
gradient-compiler --effects --json file.gr  # effect analysis
gradient-compiler --fmt file.gr             # canonical formatting to stdout
gradient-compiler --fmt --write file.gr     # canonical formatting in place
echo "expr" | gradient-compiler --repl      # evaluate expression, get type + value
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

### Pattern 6: Generate-Verify Loop (Upcoming)

The full generate-verify workflow combines grammar-constrained decoding, type checking, and contract verification into a single pipeline. Each stage eliminates a class of errors, so the agent never wastes tokens on code that fails a later stage.

```
Agent -> generate with grammar constraint -> type-check -> verify contracts -> trust result
```

Step by step:

1. **Generate.** The agent generates Gradient source using a grammar-constrained decoding engine (XGrammar, vLLM). The LL(1) grammar guarantees the output is syntactically valid. Zero parse errors.
2. **Type-check.** The agent runs `gradient check --json` and reads structured diagnostics. Typed holes provide completion context for any remaining gaps. The agent fixes type errors using the compiler's feedback.
3. **Verify contracts.** The agent runs contract verification (once available) to prove that `@requires`/`@ensures` annotations hold. If verification fails, the compiler provides a structured counterexample.
4. **Trust result.** If all three stages pass, the code is compiler-verified: syntactically valid, well-typed, contract-compliant, and effect-safe. The agent (or a downstream system) can trust the result without additional testing for the properties covered by the contracts.

This pattern is not yet fully available -- contract verification is on the roadmap. The grammar-constrained decoding and type-checking stages work today.

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
