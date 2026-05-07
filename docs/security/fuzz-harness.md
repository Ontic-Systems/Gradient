# Fuzz harness

> Issues: [#357](https://github.com/Ontic-Systems/Gradient/issues/357) (lexer + parser) + [#358](https://github.com/Ontic-Systems/Gradient/issues/358) (checker + IR builder) — full closure of adversarial-review **F3 (HIGH)**.
> Epic: [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (threat model).

The Gradient compiler ships a [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) harness with libFuzzer-driven coverage-guided fuzzing of every parser-tier through IR-tier entry point. Together with #357 and #358 this closes the F3 deliverable: "no panics in 24h soak required before public push."

## Scope

Four fuzz targets cover the entire frontend pipeline:

| Target | Entry point | Acceptance |
|---|---|---|
| `lex_random_bytes` | `Lexer::tokenize()` | UTF-8 input → token stream; must not panic |
| `parse_random_text` | `parser::parse(tokens, file_id)` | UTF-8 input → (Module, Vec<ParseError>); must not panic |
| `check_random_module` | `typechecker::check_module(...)` | UTF-8 input → Vec<TypeError>; must not panic |
| `lower_random_module` | `IrBuilder::build_module(...)` | UTF-8 input → (ir::Module, Vec<String>); must not panic |

## Layout

```
codebase/fuzz/
├── Cargo.toml          # has its own [workspace] so libfuzzer-sys
│                       # doesn't leak into normal builds
└── fuzz_targets/
    ├── lex_random_bytes.rs
    ├── parse_random_text.rs
    ├── check_random_module.rs
    └── lower_random_module.rs
```

The fuzz crate is **excluded from the main workspace** (`codebase/Cargo.toml [workspace] exclude = ["fuzz"]`). Run targets via `cargo fuzz` from inside `codebase/fuzz/`.

## Run locally

cargo-fuzz requires nightly Rust:

```bash
rustup install nightly
cargo install --locked cargo-fuzz

cd codebase/fuzz
# 30s smoke — fast feedback that scaffolding compiles and runs.
cargo +nightly fuzz run lex_random_bytes -- -max_total_time=30
cargo +nightly fuzz run parse_random_text -- -max_total_time=30

# Full soak — what the nightly CI runs.
cargo +nightly fuzz run lex_random_bytes -- -max_total_time=14400
```

When a target finds a crash, libFuzzer writes the offending input to `codebase/fuzz/artifacts/<target>/crash-<hash>` and exits non-zero. CI uploads `artifacts/` so the input is downloadable.

## CI

[`.github/workflows/fuzz.yml`](../../.github/workflows/fuzz.yml) defines two run modes:

1. **Nightly soak** — cron `0 2 * * *`. Two parallel jobs (one per target), each running 4 hours (14400s). Per-job timeout 290 minutes (under the 6-hour GHA hard cap with buffer). Per the F3 acceptance: "Zero panics in 24hr soak required before public push" — six consecutive nightly greens cover that.

2. **PR smoke** — runs only when `codebase/fuzz/**` or the workflow file itself changes. Compiles and runs each target for 30 seconds. Catches build breakage in the fuzz scaffolding without paying the full soak cost on every unrelated PR.

Crash artifacts are uploaded via `actions/upload-artifact@v4` for triage.

## Acceptance — closes #357

- [x] Two fuzz targets compile and run (`lex_random_bytes`, `parse_random_text`).
- [x] CI nightly job runs each for ≥4hr (cron + 14400s `max_total_time`).
- [ ] Zero panics in 24hr soak required before public push — pending six consecutive nightly greens. Tracked here; flip to ✓ when achieved.

## When a fuzz crash fires

1. CI uploads the crash artifact. Download from the failed workflow run.
2. Reproduce locally: `cargo +nightly fuzz run <target> codebase/fuzz/artifacts/<target>/crash-<hash>`.
3. Minimize: `cargo +nightly fuzz tmin <target> codebase/fuzz/artifacts/<target>/crash-<hash>`.
4. File a `parser-bug` / `lexer-bug` issue, attach the minimized input, link the failing run.
5. Fix the panic, add the input to `codebase/fuzz/corpus/<target>/` so it becomes a regression seed.

## Cross-references

- [`docs/security/threat-model.md`](threat-model.md) row TF1 (fuzz harness).
- [Epic #302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model umbrella.
- cargo-fuzz docs: <https://rust-fuzz.github.io/book/cargo-fuzz.html>
- libFuzzer options: <https://llvm.org/docs/LibFuzzer.html#options>
