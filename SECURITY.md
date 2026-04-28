# Security Policy

This document describes how to report vulnerabilities in **Gradient** and
what security guarantees the project ships today on `main`.

It is a *current-state* document, not a roadmap. Anything not enforced
in shipped code is explicitly called out as **not yet enforced**.

## Reporting a vulnerability

Please **do not** file public GitHub issues for suspected vulnerabilities.

- Open a private vulnerability report on this repository:
  <https://github.com/Ontic-Systems/Gradient/security/advisories/new>.
- Include a minimal reproduction, the affected version (commit SHA from
  `main` or the release tag), and the impact you believe it has.

We aim to acknowledge new reports within **7 calendar days**, and to
publish a fix or a documented mitigation within **30 days** for
confirmed High / Critical findings. Lower-severity findings may take
longer; we will say so in the acknowledgement.

## Supported versions

| Version | Supported |
|---------|-----------|
| `main` (current development branch) | ✅ — security fixes land here first |
| Tagged releases | ✅ — patch releases are issued for the most recent minor tag |
| Older releases | ❌ — please upgrade |

## What is hardened today on `main`

The following protections are implemented in shipped code as of
2026-04-28:

### Build / supply chain

- **`cargo deny` runs in CI** with a pinned `cargo-deny` version against
  a license/advisory/bans configuration committed at
  `codebase/deny.toml` (PR #171, #174).
- **`cargo audit`** runs in CI against `Cargo.lock` (PR #171).
- **OpenSSL is banned** from the dependency tree; `reqwest` is built
  with `rustls-tls` only (PR #171).
- **`wasmtime` install in CI is pinned to a version + verified against a
  known SHA-256** before extraction. The previous
  `curl … | bash` pattern is gone (PR #189).

### Package fetching / registry

- **ZIP archives** downloaded from the registry are extracted through a
  single hardened helper (`zip_safe::safe_extract`) that enforces:
  - max total uncompressed size (default 256 MiB)
  - max per-entry size (default 64 MiB)
  - max entry count (default 10 000)
  - max directory depth (default 32)
  - filesystem-free path validation (no string `..` checks; rejects
    Windows separators / drive prefixes / NUL / leading slash)
  - rejection of ZIP entries flagged as `S_IFLNK` symlinks
  - atomic temp-dir extraction + `fs::rename` install (PR #190).
- **GitHub registry dependencies are SHA-anchored**: tags resolve to a
  commit SHA at fetch time, the SHA and archive SHA-256 are recorded
  in the lockfile, and tag movement against an existing locked SHA is
  refused unless an explicit update is requested (PR #193).
- **Package and version names are validated** against a strict allowlist
  before being joined into any cache path; `safe_cache_path` walks
  components to verify the joined path stays under the cache root
  (PR #192).

### Compiler / Query API

- **Module imports are sandboxed** to a canonicalized source root.
  `..`, absolute paths, and symlinks pointing outside the root are
  rejected before the file is read; an opt-in stdlib allowlist permits
  imports from explicitly trusted roots (PR #191).
- **`Session::rename`** in the Query API rejects empty or
  non-identifier `old_name` (DoS regression) and walks UTF-8 source by
  character boundaries so non-ASCII identifiers no longer panic on
  byte-offset slices (PR #186).

### Runtime

- **`__gradient_file_read`** caps the total bytes read at a configurable
  limit (`GRADIENT_FILE_READ_MAX_BYTES`, default 64 MiB), uses
  saturating buffer growth on non-seekable input, and rejects oversize
  seekable files before any allocation. Hostile or accidental
  multi-GiB inputs (`/dev/zero`, large logs) no longer exhaust host
  memory (PR #187).
- **The arena allocator** uses checked arithmetic on every chunk /
  alignment / pointer addition; over-large allocations return `NULL`
  cleanly instead of wrapping (PR #188).
- **The actor runtime** serializes mailbox mutation with
  `pthread_mutex_t`, allocates message payloads in the target actor's
  arena under a per-actor lock, uses an atomic `_Atomic uint32_t`
  refcount, and stores tombstones (rather than `NULL`) on registry
  removal so linear-probe lookups stay correct. A zero-size memcpy
  guard removes the prior UB on actors with empty state (PR #195).

### WebAssembly target

- **The bump allocator emits explicit overflow guards** before each
  `i32.add` so `current_ptr + size` and the page-rounding addition can
  never wrap; the emitted memory carries an explicit
  `MemoryType.maximum` (default 4 096 pages = 256 MiB, configurable
  via `WasmBackend::with_max_pages`) so a guest cannot exhaust host
  RSS via unbounded `memory.grow` (PR #168, PR #194).

## What is NOT yet enforced

The following items are tracked but **not** part of the current security
guarantees. Do not assume them when threat-modelling against shipped
Gradient.

- **Effect-row enforcement at runtime.** Gradient's effect system is
  type-checked but the runtime does not yet validate effect rows on
  capability handles. (Tracked as a Wave-5 item.)
- **Spawn boundary.** Actor spawn does not yet enforce a capability
  boundary distinct from the parent's environment.
- **General contracts / runtime authority.** Pieces of contract
  parsing, type-checking, and SMT integration exist or are planned,
  but a generalised runtime-enforced contract layer is not shipped.
- **WASM as a sandbox.** Treat the WASM backend as **experimental** for
  untrusted code despite the hardening above. Additional review is
  required before relying on it as a security boundary.
- **Linear types as a runtime guarantee.** Currently a placeholder.

## Open security work

Tracking issues for currently in-flight or open security work on the
public tracker:

- #154 — replace self-hosted placeholder collection handles
- #156 — finish actor stateful integration semantics
- #158 — re-enable sanitizer + arena/genref regression tests

For follow-up to merged hardening (PR #168 / PR #171 / PR #190 /
PR #191 / etc.), see the project's recent merged history under the
`security` label on the issue tracker.

## Acknowledgements

Thank you to everyone who has reported issues responsibly. We will
credit reporters in release notes unless asked to remain anonymous.
