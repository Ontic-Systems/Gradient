# Agent Integration Guide

> **STATUS:** partial — Grammar-constrained decoding, runtime contracts, effects, and the Query API are implemented. Static `@verified` SMT discharge, `@untrusted` source mode, and capability-scoped manifests are planned (Epics #297, #302, #303).

This document is the primary reference for AI agents integrating Gradient into their tool chains. It covers structured output formats, typed holes, the LSP server with agent-specific extensions, and recommended integration patterns.

For Gradient's design principles and research foundation, see [README.md](../README.md). This guide focuses on practical integration details.

## Why Gradient for Agents

Gradient is purpose-built for agent code generation:

- **Grammar-constrained decoding ready** — LL(1) grammar compatible with XGrammar, llguidance, Outlines for token-level syntax enforcement
- **Compiler-enforced effects** — `!{IO}` annotations are verified, not just convention; pure functions are compiler-proven
- **Structured compiler API** — JSON-serializable diagnostics via `session.check()`, `session.symbols()`, `session.module_contract()`
- **Typed holes** — `?hole` placeholders provide type-directed completion context
- **Contracts** — `@requires`/`@ensures` annotations enable generate-verify workflows with runtime checking

See [README.md](../README.md) for the full research foundation and design principles.

## The Generate-Verify Workflow

Agent-generated code is not just compiler-CHECKED but compiler-VERIFIED. Gradient delivers this workflow today:

1. **Agent generates Gradient code with grammar-constrained decoding.** The formal EBNF grammar (`resources/gradient.ebnf`) is compatible with XGrammar, llguidance, and Outlines. Constrained decoding engines enforce it token-by-token. Result: zero syntax errors (SynCode, 2024).
2. **Compiler provides type-directed completion context.** Typed holes and structured diagnostics give the agent precise type information at every decision point. Result: 75% fewer type errors (Blinn et al., OOPSLA 2024).
3. **Functions have `@requires`/`@ensures` contracts.** Agents generate not just implementations but specifications. The contracts are machine-checkable declarations of intent. The `result` keyword in postconditions references the return value.
4. **Compiler verifies contracts at runtime.** The compiler inserts assertion checks on function entry (preconditions) and exit (postconditions). Contract violations produce structured error messages. Research shows 82-96% verification success rates on LLM-generated code with Dafny-style specs.
5. **Effect system guarantees no undeclared side effects.** A function without `!{IO}` in its signature is compiler-proven pure. No runtime surprises.
6. **Result: agent-generated code that is compiler-VERIFIED, not just compiler-CHECKED.** The combination of grammar constraints, type checking, contract verification, and effect enforcement means the compiler can vouch for correctness, not just well-formedness.

## Design-by-Contract for Agents

Design-by-contract is the single highest-leverage feature for agent code generation. Research shows LLMs achieve 82-96% first-pass success rates when generating code against formal specifications.

### Contract Annotations

Gradient supports two contract annotations on functions:

- **`@requires(condition)`** -- precondition checked on function entry
- **`@ensures(condition)`** -- postcondition checked on function exit

The `result` keyword in `@ensures` refers to the function's return value.

```
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)
```

Multiple contracts can be stacked:

```
@requires(a > 0)
@requires(b > 0)
@ensures(result > 0)
fn multiply_positive(a: Int, b: Int) -> Int:
    ret a * b
```

### Runtime Checking

Contracts are enforced at runtime via assertion checks:

- **Preconditions** are checked on function entry. If a `@requires` condition is false, the program halts with a structured contract violation error.
- **Postconditions** are checked on function exit. If an `@ensures` condition is false after the function body executes, the program halts with a structured error.

Contract violations produce structured error messages that agents can parse and act on.

### Contracts in the Query API

Contracts are visible through the structured query API:

- **`session.symbols()`** -- each function's symbol entry includes its contracts
- **`session.module_contract()`** -- the module contract includes contracts for all public functions

This means agents can read a module's contracts without parsing source code -- they get structured JSON describing every function's preconditions and postconditions.

### Agent Workflow: Generate with Contracts

The recommended workflow for agents generating Gradient code with contracts:

1. **Specify contracts first.** Write the `@requires`/`@ensures` annotations before the function body. This declares intent.
2. **Generate the implementation.** Fill in the function body to satisfy the contracts.
3. **Type-check.** Run `gradient-compiler --check --json file.gr` to verify the code is well-typed with structured diagnostics.
4. **Run.** Execute the program. If a contract is violated at runtime, the structured error message tells the agent exactly which contract failed and why.
5. **Iterate.** Fix the implementation until all contracts pass.

This workflow maps directly to the "vericoding" pattern from the research literature: generate code against a formal specification, verify it holds, trust the result.

### Contracts for Code Review

When an agent reviews Gradient code, contracts provide a machine-readable summary of expected behavior. Instead of reading and reasoning about an entire function body, the agent can:

1. Read the contracts via `session.module_contract()`.
2. Verify that the contracts capture the intended behavior.
3. Trust that the runtime will enforce compliance.

This reduces the review task from "understand what this code does" to "verify that the contracts match the specification."

---

## Type-Directed Completion Context

The compiler provides rich completion context at any cursor position, giving agents exactly the information needed to generate type-correct code. This is backed by research showing type-directed generation reduces compile errors by 75% (ETH Zurich, PLDI '25).

### API

```rust
let ctx = session.completion_context(line, col);
// Returns:
//   expected_type: the type expected at the cursor position
//   bindings: all bindings in scope with their types
//   matching_functions: functions whose return type matches the expected type
//   matching_variants: enum variants that match the expected type
//   matching_builtins: builtin functions that match the expected type
```

### CLI

```bash
gradient-compiler file.gr --complete 5 12 --json
```

Returns a JSON object with all completion candidates ranked by relevance.

### Enhanced Typed Hole Diagnostics

When the compiler encounters a typed hole (`?`), it now reports not just the expected type but also all in-scope bindings, functions, and enum variants that would satisfy the hole. This turns every `?` into a type-directed menu of valid completions.

### Agent Workflow: Type-Directed Generation

1. **Write a skeleton with typed holes.** Place `?` at every decision point.
2. **Query completion context.** Run `--complete line col --json` at each hole.
3. **Select from candidates.** The compiler provides ranked candidates -- bindings in scope, functions with matching return types, and enum variants.
4. **Fill and re-check.** Replace the hole with the selected candidate and re-run `gradient check`.

This workflow is strictly more powerful than untyped hole-filling because the agent receives a curated set of type-valid options rather than generating from scratch.

## Context Budget Tooling

Agents operate under context window budgets. Sending too much code degrades performance ("context rot"). Sending too little misses critical information. Context budget tooling solves this by letting agents request exactly the right amount of context for a given task.

### API

```rust
// Get relevance-ranked context for editing a function within a token budget
let ctx = session.context_budget("process_data", 1000);
// Returns: function signature, called functions' contracts, relevant type
// definitions, capability ceiling -- ranked by relevance, trimmed to budget

// Get a structural overview of the entire project
let index = session.project_index();
// Returns: all modules, all public functions with signatures, type definitions,
// capability ceilings -- a RepoMap-style index for navigation
```

### CLI

```bash
# Get optimal context for editing `main` within a 1000-token budget
gradient-compiler --context --budget 1000 --function main file.gr

# Get a structural index of the project
gradient-compiler --inspect --index file.gr
```

### What Context Includes

The context budget API returns items ranked by relevance to the target function:

1. **The function's own signature and contracts** (highest priority)
2. **Signatures of functions it calls** (direct dependencies)
3. **Type definitions used in the function** (parameter and return types)
4. **Module capability ceiling** (what effects are allowed)
5. **Contracts of called functions** (pre/postconditions)

Items are trimmed to fit the requested token budget. The most relevant items are always included first.

### Agent Workflow: Budget-Aware Editing

1. **Request context.** Call `session.context_budget("target_fn", budget)` with a token budget appropriate for your model's context window.
2. **Include in prompt.** Insert the returned context into the LLM prompt. It is already ranked and trimmed -- no further processing needed.
3. **Generate code.** The LLM generates with exactly the right amount of context.
4. **Verify.** Run `gradient check` to confirm correctness.

### Project Index for Navigation

`session.project_index()` (or `--inspect --index` via CLI) returns a structural overview of the entire codebase -- module names, public function signatures, type definitions, and capability ceilings. This is the Gradient equivalent of Aider's RepoMap: a compact, high-signal summary that helps agents navigate unfamiliar codebases without reading every source file.

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
- **`session.completion_context(line, col)`** -- type-directed completion context at a cursor position: expected type, in-scope bindings, matching functions, enum variants, and builtins.
- **`session.context_budget(fn_name, budget)`** -- relevance-ranked context for editing a function, trimmed to the given token budget.
- **`session.project_index()`** -- structural overview of the project (modules, public signatures, types, capabilities).

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
- `gradient-compiler --check --json file.gr` -- structured diagnostics with per-phase error counts
- `--inspect --json` -- module contract (signatures, effects, purity, call graph)
- `--effects --json` -- per-function effect analysis
- `--complete <line> <col> --json` -- type-directed completion candidates at a cursor position (file must be the first positional arg)
- `--context --budget N --function name` -- relevance-ranked context within a token budget
- `--inspect --index` -- structural project index (modules, signatures, types)

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
gradient-compiler file.gr --check --json                        # structured diagnostics
gradient-compiler file.gr --inspect --json                      # module contract
gradient-compiler file.gr --effects --json                      # effect analysis
gradient-compiler file.gr --complete 5 12 --json                # type-directed completion at line 5, col 12
gradient-compiler file.gr --context --budget 1000 --function main  # context budget for editing main
gradient-compiler file.gr --inspect --index                     # structural project index
gradient-compiler file.gr --fmt --experimental                  # canonical formatting to stdout
gradient-compiler file.gr --fmt --write --experimental          # canonical formatting in place
echo "expr" | gradient-compiler --repl --experimental           # evaluate expression, get type + value
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

### Pattern 6: Generate-Verify Loop

The full generate-verify workflow combines grammar-constrained decoding, type checking, and contract verification into a single pipeline. Each stage eliminates a class of errors, so the agent never wastes tokens on code that fails a later stage.

```
Agent -> generate with grammar constraint -> type-check -> run with contracts -> trust result
```

Step by step:

1. **Generate.** The agent generates Gradient source using a grammar-constrained decoding engine (XGrammar, vLLM, Outlines) with the formal EBNF grammar (`resources/gradient.ebnf`). The LL(1) grammar guarantees the output is syntactically valid. Zero parse errors.
2. **Type-check.** The agent runs `gradient-compiler --check --json file.gr` and reads structured diagnostics. Typed holes provide completion context for any remaining gaps. The agent fixes type errors using the compiler's feedback.
3. **Verify contracts.** The agent writes `@requires`/`@ensures` annotations on functions and runs the program. Runtime contract checking asserts preconditions on entry and postconditions on exit. If a contract is violated, a structured error message identifies exactly which contract failed.
4. **Trust result.** If all three stages pass, the code is compiler-verified: syntactically valid, well-typed, contract-compliant, and effect-safe. The agent (or a downstream system) can trust the result without additional testing for the properties covered by the contracts.

All stages of this pattern are working today. Grammar-constrained decoding eliminates syntax errors. Type checking catches type mismatches. Runtime contract checking enforces `@requires`/`@ensures` annotations. The effect system guarantees no undeclared side effects.

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
