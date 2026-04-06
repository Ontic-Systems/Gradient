# Agent Mode Design Spec

**Date:** 2026-04-05
**Status:** Approved
**Author:** Opus 4.6 (Claude Code)

## Summary

Add a persistent JSON-RPC 2.0 agent mode to `gradient-compiler` that turns the compiler into a queryable service over stdin/stdout. Agents spawn the process once, send structured requests, and receive rich bundled responses — minimizing round trips and re-parse overhead.

## Motivation

The Query API (`query.rs`, 5,266 lines, 113 tests) is Gradient's strongest differentiator per the adversarial synthesis. Currently agents must invoke the compiler binary repeatedly with different flags (`--check`, `--inspect`, `--complete`, etc.), each time re-parsing and re-typechecking the source. Agent mode eliminates this overhead by holding a `Session` in memory and serving queries over a persistent connection.

## Protocol

**Transport:** Newline-delimited JSON-RPC 2.0 over stdin/stdout.

**Request format:**
```json
{"jsonrpc":"2.0","id":1,"method":"load","params":{"source":"fn main() -> !{IO} ():\n    print(\"hello\")"}}
```

**Response format:**
```json
{"jsonrpc":"2.0","id":1,"result":{...}}
```

**Error format:**
```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"No active session"}}
```

## Methods

### `load` — Primary entry point

Load source (inline or from file), parse, typecheck, return comprehensive bundle.

**Params:**
- `source` (string, optional) — inline Gradient source
- `file` (string, optional) — path to .gr file

One of `source` or `file` is required.

**Returns:** `SessionReport` (see below).

### `check` — Reload with updated source

Identical to `load` in behavior. Semantically signals "I changed the code."

**Params:** Same as `load`.
**Returns:** `SessionReport`.

### `symbols` — Symbol table

**Params:** None.
**Returns:** Array of `SymbolInfo` objects.

### `holes` — Typed holes with structured context

**Params:** None.
**Returns:** Array of `HoleInfo` objects with expected type, matching bindings, matching functions, matching variants.

### `complete` — Completion context at position

**Params:**
- `line` (u32) — 1-based line number
- `col` (u32) — 1-based column number

**Returns:** `CompletionContext` object.

### `context_budget` — Optimal context for editing a function

**Params:**
- `function` (string) — function name
- `budget` (u32) — token budget

**Returns:** `ContextBudget` object.

### `effects` — Effect summary

**Params:** None.
**Returns:** `ModuleEffectSummary` object.

### `inspect` — Module contract

**Params:** None.
**Returns:** `ModuleContract` object.

### `call_graph` — Call graph

**Params:** None.
**Returns:** Array of `CallGraphEntry` objects.

### `shutdown` — Clean exit

**Params:** None.
**Returns:** `{"ok": true}`. Process exits after sending response.

## SessionReport

The bundled response from `load` and `check`:

```json
{
  "ok": true,
  "diagnostics": [
    {
      "severity": "error",
      "phase": "typechecker",
      "message": "type mismatch: expected Int, found String",
      "span": {"start": {"line": 1, "col": 5}, "end": {"line": 1, "col": 10}},
      "notes": ["expected type comes from return annotation"],
      "related": [{"message": "return type declared here", "span": {...}}]
    }
  ],
  "symbols": [...],
  "holes": [
    {
      "span": {"start": {"line": 2, "col": 5}, "end": {"line": 2, "col": 6}},
      "expected_type": "Int",
      "matching_bindings": [{"name": "x", "type": "Int"}],
      "matching_functions": [{"name": "helper", "signature": "fn() -> Int"}],
      "matching_variants": []
    }
  ],
  "effects": {...},
  "summary": {
    "functions": 5,
    "types": 2,
    "errors": 0,
    "warnings": 1,
    "holes": 1
  }
}
```

## HoleInfo (Enhanced Typed Holes)

Current hole diagnostics embed context in string notes. Agent mode extracts this into structured data:

```json
{
  "span": {"start": {"line": 2, "col": 5}, "end": {"line": 2, "col": 6}},
  "expected_type": "Int",
  "matching_bindings": [
    {"name": "x", "type": "Int"},
    {"name": "y", "type": "Int"}
  ],
  "matching_functions": [
    {"name": "helper", "signature": "fn() -> Int"}
  ],
  "matching_variants": []
}
```

## Architecture

```
compiler/src/
  agent/
    mod.rs          — module exports
    protocol.rs     — JSON-RPC 2.0 types, parse/serialize
    server.rs       — stdin/stdout loop, dispatch, session state
    handlers.rs     — method handlers calling into Session API
  query.rs          — existing Session API (unchanged)
  main.rs           — --agent flag enters agent::server::run()
```

The agent module is a protocol adapter over the existing `Session` API. No compiler logic is duplicated.

## CLI

```
gradient-compiler --agent          # Enter agent mode
gradient-compiler --agent --pretty # Pretty-print JSON (debugging)
```

## Session Lifecycle

1. Agent spawns `gradient-compiler --agent`
2. Compiler sends `initialized` notification with version and capabilities
3. Agent sends `load` with source
4. Compiler responds with `SessionReport`
5. Agent sends follow-up queries as needed
6. Agent sends `shutdown` or closes stdin -> compiler exits

## Error Codes

| Code | Meaning |
|------|---------|
| -32700 | Parse error (invalid JSON) |
| -32600 | Invalid request |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32001 | No active session |
| -32002 | File not found |

## Scope

**In scope:**
- Single-file session support
- All existing query.rs methods exposed via JSON-RPC
- Enhanced typed hole extraction
- Diagnostic grouping with related spans
- Tests for protocol, handlers, lifecycle

**Out of scope (future):**
- Multi-file project sessions (design protocol to allow later)
- Incremental re-checking
- Concurrent sessions
- WebSocket/TCP transport

## Dependencies

`serde_json` is already a dependency. No new crates required.
