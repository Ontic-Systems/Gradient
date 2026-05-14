# ADR 0009: Capability-Passing in query.gr (#325)

## Status

Accepted — implemented in PR closing #325.

## Context

Gradient's capability model uses zero-sized typestate tokens (`cap Name`)
to gate access to effectful operations. The checker tracks capabilities as
linear values: they must be passed to callees and cannot be used after
consumption. This provides compile-time proof that code has authority to
perform side effects — without runtime overhead, since capabilities are
erased before codegen.

Prior to this change, capability declarations (`cap Name`) were only
recognized by the checker at the top level of a module. The self-hosted
compiler modules all wrap their code inside `mod <Name>:` blocks, which
meant `cap` declarations inside mod blocks were silently ignored — the
type was never registered, and any function parameter referencing the
capability type would produce "unknown type" errors.

## Decision

Migrate `compiler/query.gr` — the self-hosted Query API module — to
capability-passing as the first E3 dogfood candidate.

### Changes

1. **Checker: register `CapTypeDecl` inside mod blocks.**
   The `ModBlock` first-pass in `checker.rs` now handles `CapTypeDecl`
   items, mirroring the top-level pre-pass. Both qualified
   (`mod::CapName`) and unqualified (`CapName`) aliases are registered.

2. **query.gr: declare `cap FS` and require it for file I/O.**
   - `cap FS` declares the file-system capability type.
   - `new_session_from_file(fs: FS, path: String) -> !{Heap, FS} Session`
     now requires an FS capability token to prove the caller has file-read
     authority.
   - `bootstrap_query_read_file(path: String) -> !{FS} String` is the
     kernel extern that actually reads the file; it carries `!{FS}` so
     the effect propagates to callers.

3. **Kernel: `bootstrap_query_read_file` in bootstrap_query.rs.**
   Reads a file via `std::fs::read_to_string`, returning empty string on
   error (matching the safe-default pattern used by other bootstrap
   accessors). Registered in `env.rs` with `effects: vec!["FS"]`.

### Before / After

```
// BEFORE: no capability, stub implementation
fn new_session_from_file(path: String) -> !{Heap} Session:
    ret new_session("")

// AFTER: FS capability required, real file read
cap FS
fn new_session_from_file(fs: FS, path: String) -> !{Heap, FS} Session:
    let source = bootstrap_query_read_file(path)
    ret new_session(source)
```

Callers must now hold and pass an `FS` token:

```
// Caller must declare FS in its own effect row and pass the token
fn analyze_file(fs: FS, path: String) -> !{Heap, FS} CheckResult:
    let session = new_session_from_file(fs, path)
    ret check(session)
```

## Consequences

- Proves capability-passing ergonomics in real self-hosted code.
- `cap` declarations inside `mod` blocks now work (was a checker gap).
- The linear-use discipline applies: consuming the FS token (by passing
  it to a function that `consume()`s it) prevents further FS operations.
- `new_session_from_file` is no longer a stub — it actually reads files.
- Future capability-passing migrations (e.g. `cap Net` for network,
  `cap Unsafe` for FFI) follow the same pattern.
