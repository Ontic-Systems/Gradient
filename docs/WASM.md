# WebAssembly Backend (Unstable)

The Gradient WASM backend is currently **gated behind the `wasm-unstable` feature flag** and is not enabled in default or release builds.

## Why is it gated?

Two open security findings must be resolved before the WASM backend is considered production-ready:

- **C-1** — The bump allocator does not call `memory.grow` before writing past the initial page boundary, which can cause silent out-of-bounds writes that corrupt data-section content.
- **C-2** — WASI imports (`fd_write`, `proc_exit`, etc.) are emitted unconditionally regardless of which effects the compiled module actually uses. A pure function module should emit zero WASI imports.

Tracking issue: GRA-42 Security HUB.

## Enabling the WASM backend

```sh
cargo build --features wasm-unstable
```

When invoking the compiler, select the WASM backend with `--backend wasm`.

## Running WASM output

Use a runtime that enforces capability isolation, and always supply only the directories and environment variables the module actually needs:

```sh
wasmtime --dir=./data --env=HOME=/tmp output.wasm
```

Do **not** run WASM output with `--dir=/` or without explicit `--dir` / `--env` restrictions.

## Roadmap

The `wasm-unstable` gate will be removed once C-1 and C-2 fixes land and their test suite passes (see Wave 2 of the security remediation plan).
