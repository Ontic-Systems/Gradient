# Changelog

## [Unreleased]

### Security

#### Wave 2 — CRITICAL WASM Hardening (2026-04-23)

- **C-1** (`backend/wasm.rs`, `codegen/wasm.rs`): Rewrote the WASM bump allocator
  (`emit_malloc_builtin` / code-section malloc body) to use `memory.size` +
  `memory.grow` + `unreachable` trap on OOM. No `-1` sentinel pointer ever
  escapes into user code. Memory cap removed; the runtime grows pages on demand.
  `heap_start` is derived from `data_end_offset` (8-byte aligned) so the static
  data region and heap cannot alias.

- **C-2** (`backend/wasm.rs`, `codegen/wasm.rs`): WASI imports (`fd_write`,
  `proc_exit`) are now lazy/conditional. A pure-function module that references
  no IO-effect builtins emits zero WASI imports. The `wasm-unstable` feature
  gate is **removed** (C-1 and C-2 are resolved); the feature is renamed back to
  `wasm` with no instability marker.

  Tests added: `pure_module_emits_no_imports`, `alloc_grows_module_compiles`,
  `data_heap_no_alias`, `test_wasi_imports_lazy`.

### Security

#### Wave 1 — Emergency Stops (2026-04-23)

- **C-3** (`runtime/gradient_runtime.c`): Guard `malloc(size+1)` in
  `__gradient_file_read` against `ftell()` returning −1 on non-seekable files
  (pipes, `/proc/*`).  Falls back to incremental buffered read instead of
  passing a wrapped `size_t` value to `malloc`.

- **C-5** (`runtime/gradient_runtime.c`): Harden `__gradient_http_get` and
  `http_post_impl` against protocol-downgrade and SSRF attacks:
  `CURLOPT_PROTOCOLS_STR="https"`, `CURLOPT_REDIR_PROTOCOLS_STR="https"`,
  `CURLOPT_MAXREDIRS=5`, `CURLOPT_SSL_VERIFYPEER=1`,
  `CURLOPT_SSL_VERIFYHOST=2`.

- **H-3** (`runtime/gradient_runtime.c`): Introduce `safe_realloc(ptr, size)`
  wrapper that `free()`s the original pointer and calls `abort()` on `NULL`
  return. Replaced all seven raw `realloc` call sites (map growth, curl
  receive buffer, JSON string/array buffers, `json_buf_append`,
  `stringbuilder_grow`).

- **H-4** (`runtime/gradient_runtime.c`): Add `depth` counter to `JsonParser`
  and a `MAX_JSON_DEPTH = 128` guard in `json_parse_array` and
  `json_parse_object`. Deeply-nested inputs (depth-bomb payloads) now return a
  parse error instead of consuming unbounded stack.

- **M-2** (`scripts/install.sh`): Add `--locked` to the `cargo build` invocation
  so installs are reproducible and cannot silently resolve different dependency
  versions than those in `Cargo.lock`.

- **L-4** (`build-system/src/commands/build.rs`): Replace the fixed
  `/tmp/gradient_stdin_output.o` path with `tempfile::NamedTempFile` so
  concurrent invocations and unprivileged users cannot race or predict the
  output path.

- **L-7** (`compiler/src/codegen/cranelift.rs`): Enable Cranelift's built-in IR
  verifier (`enable_verifier = true`) in debug builds (`#[cfg(debug_assertions)]`)
  to catch malformed IR early during development.

- **WASM gate**: Renamed feature `wasm` → `wasm-unstable` in `Cargo.toml` and
  all `#[cfg]` sites. The WASM backend is gated until C-1 (allocator OOB) and
  C-2 (unconstrained WASI imports) are resolved. See `docs/WASM.md`.

#### Wave 2 — CRITICAL WASM Hardening (2026-04-23)

- **C-1** (`compiler/src/backend/wasm.rs`): Harden WASM allocator to prevent OOB
  access: emit `memory.grow` with trap-on-fail in `__alloc`, reserve fixed
  static-data region above `heap_start`, and add runtime bounds check on every
  `i32.store` with attacker-derived offsets. No `-1` pointer ever escapes.

- **C-2** (`compiler/src/backend/wasm.rs`): Drive WASI imports from effect row.
  Modules without `!{FS}` emit no `fd_write`; without `!{IO}` emit no
  `proc_exit`. Pure modules now compile to zero imports.

#### Wave 3 — Supply Chain & Build Hardening (2026-04-23)

- **H-1** (`build-system/src/manifest.rs`, `resolver.rs`, `lockfile.rs`,
  `commands/add.rs`): Require commit SHA (`rev`) for all git dependencies.
  Lockfile now records `archive_sha256` (SHA256 of downloaded archive bytes,
  pre-extraction). Dependency syntax: `dep = { git = "...", rev = "<40-char-sha>" }`.
  CLI: `gradient add https://github.com/user/repo.git#<sha>`.

- **H-2** (`build-system/src/resolver.rs`, `commands/fetch.rs`): Harden ZIP
  extractor: reject symlink entries (via `unix_mode()` S_IFLNK check), reject
  backslash separators and absolute Windows paths (`C:\`), canonicalize output
  paths and verify they stay within destination directory.

- **M-1** (`build-system/src/manifest.rs`): Validate `[package].name` against
  `^[a-zA-Z][a-zA-Z0-9_-]{0,63}$`. Reject flag-shaped names starting with `-`.

- **M-5** (`compiler/src/typechecker/env.rs`, `runtime/gradient_runtime.c`):
  Replace removed `system()` builtin with `spawn(program, args)` — executes
  programs directly via `posix_spawnp()` or `fork()`+`execvp()` without shell
  invocation, eliminating shell injection vulnerabilities.

- **M-6** (`deny.toml`, `.github/workflows/ci.yml`): Add `cargo-deny` to CI
  for license compliance, security advisories, and banned crate checks.
  Add weekly `cargo audit` scheduled workflow. Pin Cranelift dependencies.

- **M-7** (`codebase/build-system/Cargo.toml`): Upgrade reqwest 0.11 → 0.12
  with `default-features = false` and `rustls-tls` feature. Drop OpenSSL
  transitive dependency.

- **L-2** (`scripts/install.sh`): Generate `SHA256SUMS` for release binaries
  during install. Print installed binary SHA256 hashes post-install.
