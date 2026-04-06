# Handoff: Agent Mode Implementation

**From:** Opus 4.6 (Claude Code)
**To:** Hermes Agent (Kimi K2.5 Turbo) or next coding agent
**Date:** 2026-04-05
**Branch:** feat/homelab-planning-panel (existing working branch)

---

## What Was Done

### Agent Mode: Persistent JSON-RPC 2.0 Server

Implemented the synthesis's top recommendation: expose the Query API via a persistent stdin/stdout JSON-RPC 2.0 protocol so agents make one `load` call and get everything, then follow up with targeted queries without re-parsing.

**Test count:** 1,035 → 1,058 (23 new agent mode tests, 0 regressions)

### Files Created

```
codebase/compiler/src/agent/
├── mod.rs          — Module exports + protocol documentation
├── protocol.rs     — JSON-RPC 2.0 types, request parsing, response building
├── server.rs       — stdin/stdout read loop, dispatch, session state
└── handlers.rs     — Method handlers (load, check, symbols, holes, etc.)

docs/superpowers/specs/
└── 2026-04-05-agent-mode-design.md — Full design spec
```

### Files Modified

| File | Change |
|------|--------|
| `compiler/src/lib.rs` | Added `pub mod agent;` |
| `compiler/src/main.rs` | Added `--agent` and `--pretty` flags, dispatches to `agent::server::run()` |

### No Files Deleted or Refactored

The agent module is a pure addition — a protocol adapter over the existing `Session` API. No compiler logic was duplicated or moved. All existing code paths are untouched.

---

## How It Works

### CLI Entry

```bash
gradient-compiler --agent              # Enter agent mode
gradient-compiler --agent --pretty     # Pretty-print JSON (debugging)
```

### Protocol

Newline-delimited JSON-RPC 2.0 over stdin/stdout.

### Session Lifecycle

1. Compiler sends `initialized` notification with version + capabilities list
2. Agent sends `load` with source string or file path
3. Compiler responds with `SessionReport` (bundled: diagnostics, symbols, holes, effects, summary)
4. Agent sends follow-up queries as needed (`symbols`, `holes`, `complete`, `context_budget`, `effects`, `inspect`, `call_graph`)
5. Agent sends `shutdown` or closes stdin

### The Bundle Principle

`load` and `check` return a `SessionReport` — everything an agent typically needs in one response:

```json
{
  "ok": true,
  "diagnostics": [...],
  "symbols": [...],
  "holes": [...],
  "effects": {...},
  "summary": {"functions": 5, "types": 2, "errors": 0, "warnings": 1, "holes": 1}
}
```

Most agents will only ever call `load` → read the result → done. The other methods exist for targeted follow-ups.

### Typed Holes Enhancement

Hole diagnostics are parsed from their note-string format into structured data:

```json
{
  "span": {"start": {"line": 2, "col": 5}, ...},
  "expected_type": "Int",
  "matching_bindings": [{"name": "a", "type": "Int"}, {"name": "b", "type": "Int"}],
  "matching_functions": [{"name": "max", "signature": "fn max(a: Int, b: Int) -> Int"}],
  "matching_variants": []
}
```

The extraction handles the actual note formats:
- `"expected type: Int"`
- `` "matching bindings in scope: `a` (Int), `b` (Int)" ``
- `` "matching functions: `max(a: Int, b: Int)` -> Int, ..." ``

---

## Design Decisions

### Why JSON-RPC 2.0 (not custom protocol, not LSP)

- **JSON-RPC 2.0** is a well-known standard that LSP is built on, but we define our own method set optimized for agent economics rather than IDE UX.
- **Not LSP** because LSP is verbose, designed for IDEs, requires many round trips, and has heavy implementation burden.
- **Not custom** because JSON-RPC has existing client libraries in every language.

### Why stdin/stdout (not HTTP, not sockets)

- Coding agents (Claude Code, Codex, etc.) naturally spawn subprocesses. stdin/stdout is zero-config, no port management.
- Process lifetime is tied to the agent's session — no zombie servers.
- HTTP/socket can be added later as a thin wrapper.

### Why bundled responses

- An LLM agent's context window is expensive. Fewer round trips = less overhead per tool call.
- `load` returns diagnostics + symbols + holes + effects + summary because that's what agents need 90% of the time.
- Targeted methods exist for the 10% case where an agent needs just one slice.

### Why extract holes from diagnostics (not add a new query method to Session)

- The typechecker already computes hole context and emits it as notes. Adding a parallel code path would duplicate logic.
- Extracting from notes is fragile if the note format changes, but it's contained in one function (`extract_holes`) that's easy to update.
- Future improvement: have the typechecker populate a structured `Vec<TypedHole>` directly on `CheckResult`. This would be the right long-term fix but requires typechecker changes.

---

## What Was NOT Done

### Out of scope per design (future work)

- **Multi-file project sessions** — Protocol designed to allow this later (just add `project` param to `load`)
- **Incremental re-checking** — Session is rebuilt from scratch on each `load`/`check`. For small files this is fast enough (<10ms). For large projects, incremental would matter.
- **Concurrent sessions** — Single session at a time. Multiple agents would need separate processes.
- **WebSocket/TCP transport** — Can be added as a wrapper over the same dispatch logic.

### Explicitly deferred per synthesis

- No syntax changes
- No new surface features
- No LLVM work
- No package registry
- No self-hosting as a release gate
- No session types / advanced capability algebra

### Issues I noticed but did not fix

1. **`Session::from_file` never returns `Err` for file-not-found** — it converts resolution errors into a session with type errors. This means agent mode reports file errors as type errors in the diagnostics, not as a JSON-RPC error. Acceptable behavior but could be cleaner.

2. **The matching_functions list in typed holes includes ALL builtins that return the expected type** — for `Int` holes this is 28+ functions. The agent gets a lot of noise. A relevance-ranked or truncated list would be better. This is a typechecker improvement, not an agent mode issue.

3. **Pre-existing warning:** `compiler/src/typechecker/checker.rs:1692` has unused variable `value_ty`. Not introduced by this work.

4. **Pre-existing dead code:** Parser recovery methods (`synchronize_to_any`, `synchronize_to_type`, `synchronize_to_delimiters`, `is_top_level_token`) are defined but never called. These were activated in Phase 2 but the caller was apparently removed or they're meant for future use.

---

## Verification

### Test Results

```
Before: 1,035 passed / 0 failed / 1 ignored
After:  1,058 passed / 0 failed / 1 ignored (5 ignored total across workspace)
New:    23 agent mode tests
```

### E2E Smoke Test

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"load","params":{"source":"fn add(a: Int, b: Int) -> Int:\n    a + b\n"}}
{"jsonrpc":"2.0","id":2,"method":"symbols"}
{"jsonrpc":"2.0","id":3,"method":"shutdown"}' | ./target/release/gradient-compiler --agent --pretty
```

This produces:
1. `initialized` notification with version and capabilities
2. Full `SessionReport` with ok=true, 1 function symbol, 0 errors, 0 holes
3. Symbol array (same as in the report, for targeted access)
4. Shutdown acknowledgment

### Typed Hole E2E

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"load","params":{"source":"fn pick(a: Int, b: Int) -> Int:\n    ?\n"}}' \
  | ./target/release/gradient-compiler --agent
```

Returns structured holes with `expected_type: "Int"`, bindings `a` and `b` with types, and 28 matching functions with full signatures.

---

## Recommended Next Steps

Per the synthesis, in priority order:

1. **Stabilize agent mode** — Use it in real agent workflows, find rough edges
2. **Phase 3: Gate experimental features** — Memory builtins, actor MVP, WASM subset
3. **Self-hosting regression** — Fix 0/10 parse failures (blocker: `expected field name; found :`)
4. **Future agent mode enhancements:**
   - Add `doc` method (calls `session.documentation()`)
   - Add `rename` method (calls `session.rename()`)
   - Add `type_at` method (calls `session.type_at()`)
   - Structured holes directly from typechecker (eliminate note-string parsing)
   - Multi-file project sessions
   - Relevance-ranked function suggestions for holes

---

## Essential Commands

```bash
# Build
cd /home/gray/TestingGround/Gradient/codebase
cargo build --release

# Test
cargo test --release

# Test agent mode specifically
cargo test --release -p gradient-compiler --lib agent

# Run agent mode
./target/release/gradient-compiler --agent
./target/release/gradient-compiler --agent --pretty
```
