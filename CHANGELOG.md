# Changelog

## [Unreleased]

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
