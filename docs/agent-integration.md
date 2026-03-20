# Agent Integration Guide

This document is the primary reference for AI agents integrating Gradient into their tool chains and infrastructure. It covers Gradient's machine-oriented design features, the structured output format shared by all tooling, typed holes for iterative code generation, the planned LSP server with agent-specific extensions, and recommended integration patterns.

## Why Gradient for Agents

Gradient is an LLM-first programming language. Several design decisions make it unusually well-suited as an agent's target language.

**Token-efficient syntax.** Gradient uses minimal keywords, no redundant delimiters (no semicolons, no braces), and indentation-based blocks. This reduces the number of tokens an agent must generate and parse, lowering latency and cost per interaction.

**Machine-readable errors.** Every compiler error emits structured JSON to stderr alongside the human-readable message. The JSON includes the expected type, the found type, the enclosing function, all relevant declarations with source spans, confidence-rated fix suggestions formatted as diffs, and a causal chain linking the error back to its root cause. Agents never need to scrape or pattern-match unstructured text.

**Typed holes.** Writing `?hole` anywhere an expression is expected triggers hole-filling assistance. The compiler returns a JSON object containing the expected type, the bindings in scope, and suggested fills. This lets agents generate code incrementally, guided by the type system.

**Deterministic builds.** The same source input always produces the same compiled output. Builds are reproducible via `gradient.lock`. Agents can cache and diff build artifacts reliably.

**Capability security.** Effects and capabilities are explicit in Gradient's type system. An agent can statically determine what a piece of code is permitted to do -- filesystem access, network calls, mutation -- before executing it. This is critical for sandboxed or safety-constrained agent architectures.

## Machine-Readable Output Format

All Gradient tools (compiler, LSP server, formatter, package manager) emit diagnostics in a shared JSON envelope. The format is stable and versioned.

```json
{
  "source_phase": "parser",
  "severity": "error",
  "code": "E0001",
  "message": "expected expression, found keyword 'fn'",
  "span": {
    "file": "src/main.gr",
    "start": {"line": 5, "col": 12},
    "end": {"line": 5, "col": 14}
  },
  "suggestions": [
    {
      "message": "did you mean to define a function?",
      "confidence": 0.85,
      "diff": "..."
    }
  ],
  "context": {
    "enclosing_function": "main",
    "expected_type": "Int",
    "found_type": null
  }
}
```

### Field Reference

| Field | Type | Description |
|---|---|---|
| `source_phase` | string | Compiler stage that produced the diagnostic: `lexer`, `parser`, `typechecker`, `ir`, `codegen`. |
| `severity` | string | One of `error`, `warning`, `info`, `hint`. |
| `code` | string | Stable error code. Can be used for programmatic matching. |
| `message` | string | Human-readable description. |
| `span` | object | Source location with file path, start position, and end position. Lines and columns are 1-indexed. |
| `suggestions` | array | Zero or more fix suggestions. Each carries a `message`, a `confidence` score (0.0 to 1.0), and a `diff` that can be applied directly. |
| `context` | object | Additional structured context. Contents vary by `source_phase`. Common fields: `enclosing_function`, `expected_type`, `found_type`, `related_declarations`. |

Agents should filter on `source_phase` and `code` to decide how to respond. The `confidence` field on suggestions is calibrated: values above 0.8 are almost always correct; values below 0.5 are speculative.

## Typed Holes for Agent-Assisted Coding

Typed holes are Gradient's mechanism for incremental, type-directed code generation. An agent writes a skeleton with `?hole_name` placeholders, compiles, reads the compiler's structured response, and fills the holes.

### Example

Source file:

```
fn process(data: List[Int]) -> Int
    let filtered = ?filter_step
    let result = ?aggregate_step
    ret result
```

Running `gradient check` on this file produces one JSON diagnostic per hole. For `?filter_step`:

```json
{
  "hole": "filter_step",
  "expected_type": "List[Int]",
  "available_bindings": [
    {"name": "data", "type": "List[Int]"}
  ],
  "suggested_fills": [
    "data.filter(fn(x: Int) -> Bool => x > 0)",
    "data"
  ]
}
```

### How Agents Should Use Holes

1. Generate a function skeleton with holes at every decision point.
2. Run `gradient check` and parse the hole diagnostics.
3. Use the `expected_type` and `available_bindings` to constrain generation.
4. Fill holes one at a time, re-checking after each fill to propagate type information.
5. When all holes are filled and `gradient check` returns zero diagnostics, the function is complete.

This workflow is more reliable than generating an entire function body in one pass because each step is validated by the type checker. It reduces hallucination of type-incorrect code.

## Language Server Protocol (LSP) -- Planned

Gradient's LSP server is designed with AI agent consumption as a first-class use case, not an afterthought.

### Standard LSP Features

The server will implement the standard LSP specification for the following capabilities:

- **Diagnostics** -- errors and warnings pushed to the client on file change.
- **Go to definition / references** -- resolve symbol locations across the codebase.
- **Hover information** -- types, documentation, and inferred constraints for any expression.
- **Completion** -- context-aware, type-directed suggestions ranked by fit.
- **Code actions** -- suggested fixes sourced directly from the compiler's suggestion engine.
- **Formatting** -- canonical formatting with zero configuration. One format only.

### Agent-Specific Extensions

These custom LSP methods go beyond the standard protocol to serve agent workflows.

| Method | Description |
|---|---|
| `gradient/holeFill` | Request fill suggestions for a typed hole. Returns the same structured JSON as `gradient check` but scoped to a single hole and with richer context from the live compilation state. |
| `gradient/effectQuery` | Query what effects a function or module requires. Returns a list of effect types (e.g., `IO`, `Mut`, `Net`) with the source spans where each effect is introduced. |
| `gradient/capabilityCheck` | Verify whether a code block stays within a capability budget. The agent sends a block of code and a set of allowed capabilities; the server returns pass/fail with a list of violations. |
| `gradient/batchDiagnostics` | Retrieve all diagnostics for a file in a single response. Unlike the standard `textDocument/publishDiagnostics`, this is a request/response pair -- no streaming, no incremental updates. Preferred for agents that process files atomically. |
| `gradient/astDump` | Return the full abstract syntax tree for a file as structured JSON. Useful for agents that need to reason about code structure beyond what diagnostics provide. |
| `gradient/irDump` | Return the intermediate representation for a specific function as structured JSON. Useful for agents that need to verify optimization decisions or understand lowered code. |

### Design Decisions

- The LSP server uses the compiler's query API directly. There is no second parser, no approximate analysis. The LSP and the CLI compiler produce identical results.
- All responses are JSON-first, human-readable second.
- Batch operations are preferred over streaming for agent efficiency. An agent can request everything it needs in one round trip.
- The LSP server will live in `codebase/devtools/lsp/`.

## Integration Patterns

### Pattern 1: Write, Check, Fix Loop

The simplest integration. The agent writes `.gr` files, runs the compiler, reads the JSON diagnostics, fixes errors, and repeats until the check passes.

```
Agent -> write .gr file -> gradient check -> read JSON diagnostics -> fix errors -> repeat
```

This pattern requires no LSP server and works with any agent framework that can invoke shell commands and parse JSON.

### Pattern 2: Typed Holes for Scaffolding

The agent generates a skeleton with `?hole` placeholders, then uses the compiler's hole-filling suggestions to complete the implementation incrementally.

```
Agent -> write skeleton with ?holes -> gradient check -> read hole suggestions -> fill holes -> gradient check -> done
```

This pattern is best when the agent knows the high-level structure but is uncertain about specific expressions. The type checker constrains each decision point.

### Pattern 3: LSP for Interactive Development

The agent connects to the LSP server over JSON-RPC and uses it for real-time feedback during an extended coding session.

```
Agent <-> LSP server (JSON-RPC) -> real-time diagnostics, completions, refactoring
```

This pattern is best for agents that maintain a long-running session and make many small edits. The LSP server maintains compilation state between requests, avoiding redundant work.

### Pattern 4: Build and Inspect

The agent compiles the program and inspects the compiled output to verify correctness or understand optimization behavior.

```
Agent -> gradient build -> read IR dump -> verify optimization -> gradient run -> check output
```

This pattern is useful for agents that need to reason about performance or verify that high-level intent is preserved through compilation.

## File Formats for Agents

Gradient will provide several machine-oriented file formats alongside the source code.

| File | Status | Description |
|---|---|---|
| `llms.txt` | Planned | Concise project summary sized to fit in an LLM context window. Covers module structure, key types, and public API surface. |
| `llms-full.txt` | Planned | Complete API reference formatted for LLM consumption. Every public symbol, its type, its documentation, and usage examples. |
| JSON symbol index | Planned | Machine-readable index of all exported symbols with types, module paths, and source spans. Designed for programmatic lookup, not reading. |
| Deprecation manifests | Planned | Structured list of deprecated APIs with migration paths, replacement symbols, and version metadata. Agents can use these to automatically update code that depends on deprecated APIs. |

## Getting Started as an Agent

1. Read `docs/language-guide.md` for syntax and semantics.
2. Read `resources/grammar.peg` for the formal grammar definition.
3. Study the examples in `resources/v0.1-examples/` for idiomatic patterns.
4. Use `gradient check` to validate generated code (when available).
5. Use typed holes (`?name`) when unsure about specific expressions -- let the type checker guide generation.
