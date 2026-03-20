# Gradient Test Framework

Test infrastructure for the Gradient programming language compiler (Stage 0 bootstrap).

## Test Strategy

### Unit Tests

Inline `#[cfg(test)]` modules colocated with each Rust source file in the compiler
crate. Run with `cargo test` from the compiler directory. Every module should have a
corresponding test submodule covering its core logic.

### Integration Tests

Located in `test-framework/tests/integration/`. These tests exercise interactions
across compiler subsystems (e.g., lexer -> parser -> AST, type-checker -> IR). They
link against the compiler crate as a library dependency and validate cross-module
contracts.

### Golden Tests

Located in `test-framework/tests/golden/`. Golden tests are snapshot-based:

1. A `.gr` source file is placed in `tests/golden/cases/`.
2. The compiler is invoked on that file.
3. Captured stdout and stderr are compared byte-for-byte against expected output files
   in `tests/golden/expected/` (`.stdout` and `.stderr` files).
4. Any mismatch is reported as a failure with a unified diff.

To update expected output after intentional changes, run with the environment variable:

```
UPDATE_GOLDEN=1 cargo test
```

This overwrites the expected files with the current compiler output.

### End-to-End Tests

Located in `test-framework/tests/e2e/`. These tests exercise the full pipeline:

1. Compile a `.gr` source file to a binary.
2. Execute the resulting binary.
3. Assert on exit code, stdout, and stderr.

E2E tests validate that the compiler produces correct, runnable programs.

## Directory Layout

```
test-framework/
  Cargo.toml
  README.md
  src/
    lib.rs             # Library root; re-exports modules
    harness.rs         # TestResult, TestCase, run_test_case(), run_suite()
    golden.rs          # Golden snapshot test runner
  tests/
    golden/
      cases/           # .gr input files
      expected/        # .stdout and .stderr expected output
    e2e/               # End-to-end compile-and-run tests
    integration/       # Cross-module integration tests
```

## Usage

```bash
# Run all tests
cargo test

# Run golden tests only
cargo test golden

# Update golden snapshots
UPDATE_GOLDEN=1 cargo test golden

# Run with verbose diff output
cargo test -- --nocapture
```
