# Gradient CLI Reference

`gradient` is the unified CLI for the Gradient programming language. All toolchain operations -- build, run, test, format -- go through this single command.

## Installation (from source)

```bash
cd codebase
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
| `--release` | Build in release mode using the LLVM backend (requires `llvm` cargo feature). Falls back to Cranelift if LLVM is not compiled in. Output goes to `target/release/`. |
| `--verbose` / `-v` | Show detailed compilation output (compiler and linker commands) |

**Status:** Working.

**Behavior:** Finds the project root by searching upward for `gradient.toml`. Resolves dependencies from `gradient.toml` and `gradient.lock`. Invokes the compiler on `src/main.gr` to produce an object file. In debug mode (default), uses the Cranelift backend and outputs to `target/debug/<project-name>`. With `--release`, selects the LLVM backend (when the `llvm` cargo feature is enabled) and outputs to `target/release/<project-name>`.

**Example:**

```bash
# Debug build (Cranelift, fast compilation)
$ gradient build
[1/6] Lexing src/main.gr...
[2/6] Parsing...
[3/6] Type checking...
[4/6] Building IR...
[5/6] Generating code via Cranelift...
[6/6] Writing object file...
Compiled my-app -> target/debug/my-app

# Release build (LLVM, optimized)
$ gradient build --release
[1/6] Lexing src/main.gr...
[2/6] Parsing...
[3/6] Type checking...
[4/6] Building IR...
[5/6] Generating code via LLVM...
[6/6] Writing object file...
Compiled my-app -> target/release/my-app
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

**Status:** Working.

**Behavior:** Discovers `@test`-annotated functions in project `.gr` files, generates a temporary harness for each test, compiles it, links it with the system C toolchain, executes it, and reports pass/fail status. `--filter` limits execution to tests whose names contain the provided pattern.

**Example:**

```bash
$ gradient test
Running 3 test(s)...

  PASS  test_add
  PASS  test_subtract
  PASS  test_multiply

test result: ok. 3 passed; 0 failed
```

---

### `gradient fmt`

```
Usage: gradient fmt [OPTIONS]
```

Format all Gradient source files in the current project's `src/` tree into canonical form.

**Options:**

| Flag | Description |
|------|-------------|
| `--check` | Check formatting without modifying files (exit 1 if changes are needed) |

**Status:** Working (experimental — the `gradient` wrapper passes `--experimental` automatically).

**Behavior:** Finds all `.gr` files under the current project's `src/` directory and formats them in place. With `--check`, it reports files that would change and exits non-zero. For single-file formatting or stdout output, use `gradient-compiler --fmt`.

**Example:**

```bash
# Format all project source files
$ gradient fmt

# Check formatting without writing (useful in CI)
$ gradient fmt --check

# Direct compiler invocation (requires --experimental flag)
$ gradient-compiler src/main.gr --fmt --experimental
$ gradient-compiler src/main.gr --fmt --write --experimental
```

---

### `gradient init`

```
Usage: gradient init
```

Initialize a Gradient project in the current directory.

**Status:** Working.

**Behavior:** Creates `gradient.toml` and `src/main.gr` in the current directory.

---

### `gradient repl`

```
Usage: gradient repl
       gradient-compiler --repl --experimental
```

Start the interactive Gradient REPL. Implemented as the `--repl` flag on `gradient-compiler`.

**Status:** Working (experimental — the `gradient` wrapper passes `--experimental` automatically).

**Behavior:** Starts a Cranelift-backed REPL session. Evaluates expressions and statements interactively, printing the result and inferred type for each input. When stdin is not a TTY (piped input), operates in non-interactive mode -- reads from stdin, evaluates, and prints results to stdout, then exits. This non-interactive mode is designed for agent piping.

**Example:**

```bash
# Interactive session (via wrapper — --experimental handled automatically)
$ gradient repl

# Direct compiler invocation (requires --experimental flag)
$ gradient-compiler --repl --experimental

# Non-interactive (piped) mode
$ echo "1 + 2" | gradient-compiler --repl --experimental
3 : Int
```

---

### `gradient add`

```
Usage: gradient add <ARG>
```

Add a dependency to the current project.

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<ARG>` | Dependency spec: local path, git URL, or registry package spec such as `name@version` |

**Status:** Working.

**Behavior:** Supports three dependency forms:

- local path dependencies such as `../my-lib`
- git dependencies such as `https://github.com/user/repo.git`
- registry dependencies such as `math@1.2.0`

The CLI updates `gradient.toml`, then re-resolves dependencies and refreshes `gradient.lock`. Path dependencies are the most mature path today; git and registry flows exist in the CLI surface but should be treated as less battle-tested.

**Example:**

```bash
$ gradient add ../my-lib
Added dependency 'my-lib' (path: ../my-lib)
Updated gradient.lock

$ gradient add https://github.com/user/repo.git
Added dependency 'repo' (git: https://github.com/user/repo.git)
```

---

### `gradient update`

```
Usage: gradient update
```

Re-resolve all dependencies and regenerate `gradient.lock`.

**Status:** Working.

**Behavior:** Reads the `[dependencies]` section from `gradient.toml`, resolves all dependencies (including transitive ones), detects cycles, deduplicates diamond dependencies, produces a topological ordering, and writes a fresh `gradient.lock` with SHA-256 checksums for all resolved packages.

**Example:**

```bash
$ gradient update
Resolved 3 dependencies
Updated gradient.lock
```

---

## Compiler Flags (`gradient-compiler`)

The `gradient-compiler` binary accepts flags directly for operations that are also accessible through the `gradient` CLI wrapper. These flags are useful for agents and scripts that invoke the compiler directly.

### `--fmt`

```
Usage: gradient-compiler <FILE> --fmt --experimental [--write]
```

Format a Gradient source file into canonical form. Requires `--experimental`.

| Flag | Description |
|------|-------------|
| `--fmt` | Format the file and print the result to stdout |
| `--fmt --write` | Format the file and overwrite it in place |
| `--experimental` | Required to enable this feature |

Without `--write`, the formatted output goes to stdout and the original file is unchanged. This is useful for diff-based checks and piping. The `gradient fmt` wrapper passes `--experimental` automatically.

### `--repl`

```
Usage: gradient-compiler --repl --experimental
```

Start the Gradient REPL. Requires `--experimental`.

| Flag | Description |
|------|-------------|
| `--repl` | Start an interactive evaluation session |

When stdin is a TTY, the REPL runs interactively with a prompt. When stdin is piped, it operates in non-interactive mode: reads expressions from stdin, evaluates each one, prints the result and inferred type to stdout, and exits when input is exhausted.

---

### `--complete`

```
Usage: gradient-compiler <FILE> --complete <LINE> <COL> [--json]
```

Return type-directed completion candidates at a cursor position.

**Note:** The file must be the first positional argument, before `--complete`.

| Flag | Description |
|------|-------------|
| `--complete <LINE> <COL>` | Query completion context at the given line and column |
| `--json` | Output as structured JSON (pretty-printed) |

**Status:** Working.

**Behavior:** Runs the compiler pipeline up through type checking, then returns completion context for the given cursor position. The result includes all in-scope bindings with inferred types plus any additional context available from the session.

**Example:**

```bash
# Get completion candidates at line 5, column 12
$ gradient-compiler src/main.gr --complete 5 12 --json
```

---

### `--context --budget`

```
Usage: gradient-compiler --context --budget <N> --function <NAME> <FILE>
```

Return relevance-ranked context for editing a function within a token budget.

| Flag | Description |
|------|-------------|
| `--context` | Enable context budget mode |
| `--budget <N>` | Maximum number of tokens in the returned context |
| `--function <NAME>` | The function to generate context for |

**Status:** Working.

**Behavior:** Analyzes the target function and returns the most relevant context items (function signatures, contracts, type definitions, capability ceilings) ranked by relevance to the target function, trimmed to fit within the specified token budget. Higher-relevance items are included first.

**Example:**

```bash
# Get optimal context for editing `process_data` within 1000 tokens
$ gradient-compiler --context --budget 1000 --function process_data src/main.gr
```

---

### `--inspect --index`

```
Usage: gradient-compiler --inspect --index <FILE>
```

Return a structural overview of the project.

| Flag | Description |
|------|-------------|
| `--inspect --index` | Generate a structural project index |

**Status:** Working.

**Behavior:** Produces a structural overview of the codebase including all modules, public function signatures, type definitions, and capability ceilings. This is the Gradient equivalent of Aider's RepoMap -- a compact, high-signal summary for navigating unfamiliar codebases.

**Example:**

```bash
$ gradient-compiler --inspect --index src/main.gr
```

---

## Project Manifest (`gradient.toml`)

```toml
[package]
name = "my-project"
version = "0.1.0"
edition = "2026"

[dependencies]
my-lib = { path = "../my-lib" }
utils = { path = "../utils" }
```

The `[dependencies]` section supports path-based dependencies. Each dependency must point to a directory containing its own `gradient.toml`. Dependencies can be added manually or via `gradient add <path>`.

## Lockfile (`gradient.lock`)

The lockfile records the resolved dependency graph with SHA-256 content-addressed checksums. It is generated automatically by `gradient build`, `gradient add`, and `gradient update`. The lockfile should be committed to version control.

## Project Layout

```
my-project/
├── gradient.toml        # Manifest with [dependencies]
├── gradient.lock         # Lockfile (SHA-256 checksums)
├── src/
│   └── main.gr
└── target/
    ├── debug/           # Cranelift backend output
    └── release/         # LLVM backend output
```
