# Gradient CLI Reference

`gradient` is the unified CLI for the Gradient programming language. All toolchain operations -- build, run, test, format -- go through this single command.

## Installation (from source)

```bash
cd codebase/build-system
cargo build --release
# Binary at: target/release/gradient
# Optionally: cp target/release/gradient ~/.local/bin/
```

## Commands

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

**Status:** Working.

**Behavior:** Creates a project directory with `gradient.toml` and `src/main.gr` containing a hello-world program. Prints instructions for building and running.

**Example:**

```bash
$ gradient new my-app
Created project 'my-app'

To get started:
  cd my-app
  gradient build
  gradient run
```

---

### `gradient build`

```
Usage: gradient build [OPTIONS]
```

Compile the current project to a native binary.

**Options:**

| Flag | Description |
|------|-------------|
| `--release` | Build in release mode (not yet differentiated from debug) |
| `--verbose` / `-v` | Show detailed compilation output (compiler and linker commands) |

**Status:** Working.

**Behavior:** Finds the project root by searching upward for `gradient.toml`. Invokes the compiler on `src/main.gr` to produce an object file. Links with `cc` to produce an executable at `target/debug/<project-name>` (or `target/release/<project-name>` with `--release`).

**Example:**

```bash
$ gradient build
[1/6] Lexing src/main.gr...
[2/6] Parsing...
[3/6] Type checking...
[4/6] Building IR...
[5/6] Generating code via Cranelift...
[6/6] Writing object file...
Compiled my-app -> target/debug/my-app
```

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

**Status:** Working.

**Behavior:** Equivalent to `gradient build` followed by executing the output binary. The binary's exit code is forwarded as the CLI's exit code. Build output is suppressed (only program output is shown).

**Example:**

```bash
$ gradient run
[1/6] Lexing src/main.gr...
[2/6] Parsing...
[3/6] Type checking...
[4/6] Building IR...
[5/6] Generating code via Cranelift...
[6/6] Writing object file...
Compiled my-app -> target/debug/my-app
Hello, Gradient!
```

---

### `gradient check`

```
Usage: gradient check [OPTIONS]
```

Type-check the project without producing a final binary.

**Options:**

| Flag | Description |
|------|-------------|
| `--verbose` / `-v` | Show detailed type-checking output |

**Status:** Working.

**Behavior:** Runs the full compiler pipeline (lex, parse, typecheck, IR, codegen) but discards the output object file. Reports success or failure. In a future version this will skip code generation; for now it runs the full pipeline to a temporary location.

**Example:**

```bash
$ gradient check
No errors found.
```

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

**Status:** Scaffolded -- not yet functional.

**Future behavior:** Discovers `@test`-annotated functions, runs them, and reports results.

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

**Status:** Scaffolded -- not yet functional.

**Future behavior:** Canonical formatter -- one way to format Gradient code.

---

### `gradient init`

```
Usage: gradient init
```

Initialize a Gradient project in the current directory.

**Status:** Scaffolded -- not yet functional.

**Future behavior:** Creates `gradient.toml` and `src/main.gr` in the current directory.

---

### `gradient repl`

```
Usage: gradient repl
```

Start the interactive Gradient REPL.

**Status:** Scaffolded -- not yet functional.

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
├── src/
│   └── main.gr
└── target/
    ├── debug/
    └── release/
```
