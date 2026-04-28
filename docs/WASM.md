# WebAssembly Backend

The Gradient WASM backend compiles Gradient IR to WebAssembly binary format and
is available via the `wasm` Cargo feature.

## Building with WASM support

```sh
cargo build --features wasm
```

When invoking the compiler, select the WASM backend with `--backend wasm`.

## Running WASM output

Use a runtime that enforces capability isolation, and supply only the directories
and environment variables the module actually needs:

```sh
wasmtime --dir=./data --env=HOME=/tmp output.wasm
```

Do **not** run WASM output with `--dir=/` or without explicit `--dir` / `--env`
restrictions.

## Security properties

- **Allocator safety (C-1)**: The bump allocator calls `memory.grow` before
  writing past the current page boundary and traps via `unreachable` if
  `memory.grow` returns -1. No `-1` sentinel pointer ever escapes into user code.

- **Minimal WASI imports (C-2)**: WASI imports (`fd_write`, `proc_exit`) are
  only emitted when the compiled module actually uses IO-effect builtins. A
  pure-function module emits zero WASI imports.
