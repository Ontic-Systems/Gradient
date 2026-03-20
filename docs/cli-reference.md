# Gradient CLI Reference

`gradient` is the unified CLI for the Gradient programming language. All toolchain operations — build, run, test, format, package management — go through this single command.

## Installation (from source)

```bash
cd codebase/build-system
cargo build --release
# Binary at: target/release/gradient
# Optionally: cp target/release/gradient ~/.local/bin/
```

## Commands

---

### `gradient build`

```
Usage: gradient build [OPTIONS]
```

Compile the current project.

**Options:**

| Flag | Description |
|------|-------------|
| `--release` | Build in release mode (LLVM backend, optimized) |
| `--verbose` | Show detailed compilation output |

**Status:** Stubbed — prints diagnostic message.

**Future behavior:** Reads `gradient.toml`, resolves dependencies, invokes the compiler pipeline, and outputs a binary to `target/debug/` or `target/release/`.

---

### `gradient run`

```
Usage: gradient run [OPTIONS]
```

Compile and run the current project.

**Options:**

| Flag | Description |
|------|-------------|
| `--release` | Build in release mode before running |

**Status:** Stubbed.

**Future behavior:** Equivalent to `gradient build` followed by executing the output binary.

---

### `gradient test`

```
Usage: gradient test [OPTIONS]
```

Run tests for the current project.

**Options:**

| Flag | Description |
|------|-------------|
| `--filter <PATTERN>` | Only run tests matching this pattern |

**Status:** Stubbed.

**Future behavior:** Discovers `@test`-annotated functions, runs them, and reports results.

---

### `gradient check`

```
Usage: gradient check [OPTIONS]
```

Type-check the project without code generation.

**Options:**

| Flag | Description |
|------|-------------|
| `--verbose` | Show detailed type-checking output |

**Status:** Stubbed.

**Future behavior:** Runs lexer, parser, and type checker. Reports errors without performing codegen.

---

### `gradient fmt`

```
Usage: gradient fmt [OPTIONS]
```

Format Gradient source files.

**Options:**

| Flag | Description |
|------|-------------|
| `--check` | Check formatting without modifying files (exit 1 if changes needed) |

**Status:** Stubbed.

**Future behavior:** Canonical formatter — one way to format Gradient code.

---

### `gradient new <name>`

```
Usage: gradient new <NAME>
```

Create a new Gradient project.

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<NAME>` | Project name (creates directory) |

**Status:** Stubbed.

**Future behavior:** Creates a project directory with `gradient.toml` and `src/main.gr`.

---

### `gradient init`

```
Usage: gradient init
```

Initialize a Gradient project in the current directory.

**Status:** Stubbed.

**Future behavior:** Creates `gradient.toml` and `src/main.gr` in the current directory.

---

### `gradient repl`

```
Usage: gradient repl
```

Start the interactive Gradient REPL.

**Status:** Stubbed.

**Future behavior:** Cranelift-backed REPL for interactive evaluation.

---

## Project Manifest (`gradient.toml`)

```toml
[package]
name = "my-project"
version = "0.1.0"
edition = "2026"

[dependencies]
```

## Project Layout

```
my-project/
├── gradient.toml
├── gradient.lock
├── src/
│   └── main.gr
├── tests/
└── target/
    ├── debug/
    └── release/
```
