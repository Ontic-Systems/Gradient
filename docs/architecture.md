# Gradient Architecture Guide

This document describes the internals of the Gradient compiler and toolchain. It is intended for AI agents working on or integrating with Gradient.

---

## Compiler Pipeline

```
Source (.gr) -> Lexer -> Parser -> AST -> Type Checker (+ Contracts, Generics, Effects, FFI) -> IR (SSA) -> CodegenBackend (Cranelift or LLVM) -> Linker -> Binary
```

The full pipeline is wired end-to-end and produces native executables from Gradient source files.

### 1. Lexer

Hand-written tokenizer that emits a flat token stream. The lexer tracks indentation depth and synthesizes `INDENT` and `DEDENT` tokens to represent indentation-based block structure (similar to Python, but with stricter rules).

Token categories: keywords (17), identifiers, literals (int, float, string, bool), operators (including `%` for modulo), delimiters (including `[` and `]` for generics), `INDENT`, `DEDENT`, `NEWLINE`, and error tokens for recovery.

All tokens are ASCII-only. No Unicode operators are permitted.

**Location:** `codebase/compiler/src/lexer/` -- **71 tests**

### 2. Parser

Recursive descent, LL(1). Implements the PEG grammar defined in `resources/grammar.peg`. Produces a fully typed AST where every node carries source location information (`Span` with file id, start position, end position).

Error recovery is implemented. The parser uses synchronization tokens (newlines, dedents, keywords) to resync and continue parsing, collecting all diagnostics for a single pass.

Key grammar features:
- Module declarations (`mod path`) and use imports (`use path`, `use path.{items}`)
- Function definitions with colon-delimited blocks: `fn name(params) -> RetType:`
- Generic function definitions: `fn identity[T](x: T) -> T:`
- Generic enum type declarations: `type Option[T] = Some(T) | None`
- Extern function declarations (`@extern`, `@extern("libm")`) with no body
- Export function annotations (`@export`) for C-compatible linkage
- Let bindings with optional type annotations; mutable bindings (`let mut`)
- Assignment statements (`name = expr`) for mutable bindings
- If/else if/else expressions (with `:` before each branch block)
- For loops (`for var in expr:`)
- While loops (`while condition:`)
- Match expressions with integer, boolean, enum variant, and wildcard patterns
- Enum type declarations (`type Color = Red | Green | Blue`)
- All arithmetic, comparison, and logical operators with correct precedence
- Typed holes (`?name`)
- Annotations (`@name(args)`) including `@cap`, `@budget`, `@extern`, and `@export`
- Field access (`expr.field`)

**Location:** `codebase/compiler/src/parser/` -- **82 tests**

### 3. AST

Typed AST nodes follow the Rust enum + struct pattern. Every node is wrapped in `Spanned<T>` which carries a `Span` value encoding its source location:

```
Span { file_id: u32, start: Position, end: Position }
Position { line: u32, col: u32, offset: u32 }
```

The AST is organized into modules:
- `module.rs` -- `Module`, `ModuleDecl`, `UseDecl`
- `item.rs` -- `FnDef`, `ExternFnDecl`, `Param`, `Annotation`, `ItemKind`
- `stmt.rs` -- `StmtKind::Let`, `StmtKind::Assign`, `StmtKind::Ret`, `StmtKind::Expr`
- `expr.rs` -- `ExprKind` variants (literals, binary/unary ops, calls, if, for, typed holes, field access, etc.)
- `types.rs` -- `TypeExpr` (Named, Unit, Fn), `EffectSet`
- `block.rs` -- `Block` (a spanned list of statements)
- `span.rs` -- `Span`, `Position`, `Spanned<T>`

**Location:** `codebase/compiler/src/ast/`

### 4. Type Checker

Static type checker with inference for let bindings. Key properties:

- **Five built-in types:** `Int`, `Float`, `String`, `Bool`, `()`
- **Generics:** type parameters on functions (`fn identity[T](x: T) -> T`) and enums (`type Option[T] = Some(T) | None`) with bidirectional type inference via unification at call sites.
- **Enum types:** user-defined algebraic data types with variant matching.
- **Forward references:** all function signatures are registered before any bodies are checked, allowing mutual recursion.
- **Type inference:** let bindings without explicit type annotations have their types inferred from the right-hand side.
- **Mutable bindings:** `let mut` bindings are tracked; assignment to immutable bindings is rejected.
- **Effect validation:** calling a function with `!{IO}` from a pure function is a type error. The effect system is enforced -- all 5 canonical effects (IO, Net, FS, Mut, Time) are recognized and unknown effects are rejected. Module-level `@cap` ceilings are checked.
- **Effect polymorphism:** lowercase effect variables (`!{e}`) resolve at call sites (pure callbacks -> empty, effectful callbacks -> concrete effects). `is_effect_polymorphic` exposed in the query API.
- **Design-by-contract:** `@requires`/`@ensures` annotations are validated during type checking. The `result` keyword is recognized in postconditions. Contract conditions must be boolean expressions.
- **Budget annotations:** `@budget(cpu: 5s, mem: 100mb)` on functions. The compiler checks budget containment -- a callee's budget must not exceed the caller's budget.
- **FFI type validation:** `@extern` and `@export` functions are checked to ensure only FFI-compatible types (`Int`, `Float`, `Bool`, `String`, `()`) appear in their signatures.
- **Lexical scoping:** scope stack with push/pop for blocks.
- **Error recovery:** `Ty::Error` sentinel suppresses cascading diagnostics.
- **String concatenation:** `+` on `String` operands is type-checked as concatenation.
- **Builtin function registry:** `print`, `println`, `print_int`, `print_float`, `print_bool`, `abs`, `min`, `max`, `mod_int`, `to_string`, `int_to_string`, `range` are all pre-loaded.

The type checker does **not** modify the AST. It reads the AST and produces a list of `TypeError`s.

**Sub-modules:**
- `checker.rs` -- Main type-checking logic
- `effects.rs` -- Effect system: canonical effects, validation, and `@cap` enforcement
- `env.rs` -- Type environment and scope management
- `error.rs` -- Diagnostic types for type errors
- `types.rs` -- Internal type representations

**Location:** `codebase/compiler/src/typechecker/` -- **115 tests**

### 5. IR

SSA (Static Single Assignment) form. The IR builder translates the typed AST into an intermediate representation suitable for code generation.

**Instruction set:**

| Instruction | Description                          |
|-------------|--------------------------------------|
| `Const`     | Load a constant value                |
| `Call`      | Function call                        |
| `Ret`       | Return from function                 |
| `Add`       | Integer/float addition               |
| `Sub`       | Integer/float subtraction            |
| `Mul`       | Integer/float multiplication         |
| `Div`       | Integer/float division               |
| `Mod`       | Integer modulo                       |
| `Cmp`       | Comparison (returns bool)            |
| `Branch`    | Conditional branch                   |
| `Jump`      | Unconditional branch                 |
| `Phi`       | SSA phi node (merge point)           |
| `Alloca`    | Stack allocation                     |
| `Load`      | Load from memory                     |
| `Store`     | Store to memory                      |
| `Neg`       | Unary negation                       |
| `Not`       | Logical negation                     |
| `StringConcat` | String concatenation              |

**Location:** `codebase/compiler/src/ir/` -- **29 tests** (in `builder/tests.rs`)

### 6. Codegen

The codegen layer is abstracted behind the `CodegenBackend` trait. Both backends consume the same SSA IR and produce object files.

**`CodegenBackend` trait:**
- Defines the interface that all codegen backends must implement.
- The build system selects the backend based on the `--release` flag.

**Cranelift (debug backend, default):**
- Translates IR to native machine code via Cranelift.
- Emits an object file (`.o`) that is then linked by the system's `cc` to produce an executable.
- Fast compilation, suitable for development and iteration.
- Working and producing real binaries.

**LLVM (release backend, feature-gated):**
- Behind the `llvm` cargo feature flag.
- Selected when `--release` is passed and the feature is compiled in.
- Stub implementation until LLVM libraries are available on the build host.

**FFI linkage:**
- `@extern` functions produce `Linkage::Import` in the IR, resolved at link time.
- `@export` functions produce `Linkage::Export` in the IR, making symbols visible to C callers.
- FFI type validation ensures only compatible types (`Int`, `Float`, `Bool`, `String`, `()`) cross the boundary.

**Location:** `codebase/compiler/src/codegen/`

### 7. Module Resolver

Multi-file module resolution. Handles resolving `use` declarations to source files on disk, parsing dependent modules, and building a combined type environment. Detects circular imports.

- `use math` resolves to `math.gr` in the same directory.
- `use a.b` resolves to `a/b.gr` relative to the source root.

**Location:** `codebase/compiler/src/resolve.rs` -- **8 tests**

### 8. Formatter

Canonical code formatter (AST pretty-printer). Parses the source, walks the AST, and emits canonically formatted text with consistent 4-space indentation, operator spacing, and normalized line breaks. Guarantees one canonical form for every program.

> **Limitation:** Comments are not preserved (they are stripped during lexing).

**Location:** `codebase/compiler/src/fmt.rs` -- **25 tests**

### 9. REPL

Interactive Read-Eval-Print Loop. Operates in check mode: each input is type-checked (not compiled) and the inferred type or errors are reported immediately. Supports both interactive (TTY with prompt) and non-interactive (piped stdin) modes. Handles expressions, `let` bindings, and function definitions.

**Location:** `codebase/compiler/src/repl.rs` -- **30 tests**

### 10. Query API

Structured query API that turns the compiler into a queryable service. Agents call `Session::from_source` and query for structured, JSON-serializable data (diagnostics, module contracts, symbol tables, contracts, completion context, context budgets). Contract annotations (`@requires`/`@ensures`) are included in symbol entries and module contracts. FFI metadata (`@extern`/`@export`, linkage) is included in symbol entries and module contracts. Completion context provides type-directed suggestions at any cursor position. Context budget tooling returns relevance-ranked items within a token budget. Project index provides a structural overview.

**Key methods:**
- `session.check()` -- type-check and return diagnostics
- `session.symbols()` -- symbol table with types, effects, contracts
- `session.module_contract()` -- public API surface including call graph and budgets
- `session.completion_context(line, col)` -- type-directed completion candidates
- `session.context_budget(fn_name, budget)` -- relevance-ranked context within token budget
- `session.project_index()` -- structural overview (modules, signatures, types)
- `session.rename(old, new)` -- compiler-verified rename
- `session.callees(fn_name)` -- call graph query

**Location:** `codebase/compiler/src/query.rs` -- **74 tests**

---

## Project Structure

```
Gradient/
├── assets/              # Logo, banner
├── codebase/
│   ├── build-system/    # `gradient` CLI (Rust, clap)
│   │   └── src/
│   │       ├── main.rs          # CLI entry point
│   │       ├── commands/        # build, run, check, new, init, fmt, test, repl, query, add, update
│   │       ├── manifest.rs      # gradient.toml parsing (including [dependencies])
│   │       ├── lockfile.rs      # gradient.lock generation and parsing (SHA-256 checksums)
│   │       ├── resolver.rs      # Dependency resolver (cycle detection, diamond dedup, topo sort)
│   │       └── project.rs       # Project discovery and paths
│   ├── compiler/        # Compiler pipeline (Rust, Cranelift)
│   │   └── src/
│   │       ├── main.rs          # Compiler driver
│   │       ├── lib.rs           # Library crate root
│   │       ├── lexer/           # Tokenizer with INDENT/DEDENT
│   │       ├── parser/          # Recursive descent parser
│   │       ├── ast/             # AST node definitions
│   │       ├── typechecker/     # Type checker, effects, and environment
│   │       ├── ir/              # SSA IR and builder
│   │       ├── codegen/         # CodegenBackend trait, Cranelift + LLVM backends
│   │       ├── resolve.rs       # Multi-file module resolution
│   │       ├── fmt.rs           # Canonical code formatter
│   │       ├── repl.rs          # Interactive REPL (check mode)
│   │       └── query.rs         # Structured query API for agents
│   ├── devtools/
│   │   └── lsp/         # LSP server (tower-lsp, tokio)
│   │       └── src/
│   │           ├── main.rs          # Server entry point
│   │           ├── backend.rs       # LanguageServer implementation
│   │           └── diagnostics.rs   # Compiler integration
│   ├── test-framework/  # Golden test framework
│   └── ...              # runtime, stdlib, etc. (planned)
├── docs/                # Documentation (this file lives here)
└── resources/           # Grammar, language reference, examples
```

---

## Build System

### CLI

`gradient` is the unified entry point for all toolchain operations. Built in Rust with `clap`.

Working commands:
- `gradient new <name>` -- creates a project directory with `gradient.toml` and `src/main.gr`
- `gradient init` -- initializes a project in the current directory
- `gradient build` -- finds the project root, resolves dependencies, invokes the compiler on `src/main.gr`, links with `cc`, outputs binary to `target/debug/<name>`
- `gradient build --release` -- same as above but selects the LLVM backend (when compiled with the `llvm` feature) and outputs to `target/release/<name>`
- `gradient run` -- builds then executes the binary, forwarding the exit code
- `gradient check` -- invokes the compiler for type checking, discards the object file
- `gradient fmt` -- canonically formats Gradient source files
- `gradient test` -- runs the project's test suite
- `gradient repl` -- starts an interactive REPL (check mode)
- `gradient add <path>` -- adds a path-based dependency to `gradient.toml` and updates `gradient.lock`
- `gradient update` -- re-resolves all dependencies and regenerates `gradient.lock`

The build system finds the project root by searching upward for `gradient.toml`. It locates the compiler binary relative to its own path.

### Package System

The package system manages project dependencies through `gradient.toml` and `gradient.lock`.

**`gradient.toml` `[dependencies]` section:**
- Path-based dependencies: `my-lib = { path = "../my-lib" }`
- Each dependency must have its own `gradient.toml`

**`gradient.lock` lockfile:**
- SHA-256 content-addressed checksums for all resolved packages
- Generated automatically on build, add, and update
- Should be committed to version control

**Dependency resolver:**
- Cycle detection: rejects circular dependency graphs
- Diamond dedup: shared transitive dependencies are resolved once
- Topological ordering: dependencies are built in correct order
- Build integration: `gradient build` resolves dependencies before compilation

### Manifest

`gradient.toml` defines a project:

```toml
[package]
name = "my-project"
version = "0.1.0"

[dependencies]
my-lib = { path = "../my-lib" }
```

### Project layout

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

---

## LSP Server

The LSP server (`codebase/devtools/lsp/`) provides IDE-like features for Gradient. It communicates over stdio via JSON-RPC using `tower-lsp`.

### Capabilities

- **Text document sync:** full document sync on open, change, and save.
- **Diagnostics:** runs the full compiler pipeline (lex, parse, typecheck) on every change and publishes diagnostics.
- **Hover:** shows type/signature information for builtins, keywords, and user-defined functions (by parsing the AST).
- **Completion:** offers keywords and builtin function names with their signatures.
- **Custom `gradient/batchDiagnostics`:** sends all diagnostics for a file in one notification with per-phase error counts (`lex_errors`, `parse_errors`, `type_errors`).

The LSP server uses the same compiler library crate as the CLI compiler -- there is no separate parser or approximate analysis.

---

## Design Principles

### LLM-First

Every design decision optimizes for AI agent consumption:

- Token efficiency: the language syntax minimizes token count for common patterns.
- Machine-readable output is the default; human-readable output is a formatting layer on top.
- The LSP `gradient/batchDiagnostics` notification provides the complete diagnostic picture in one round trip.

### ASCII-Only

Every token in Gradient source code is a printable ASCII character. No Unicode operators, no special symbols. This eliminates encoding issues and ensures every agent and tool can process Gradient source without character set complications.

### No Hidden Magic

- Effects are declared in function signatures and tracked by the type checker.
- All function signatures are explicit (parameter types and return types required).
- There is no implicit allocation, no implicit effect propagation, and no implicit capability granting.

### One Way to Do It

No syntactic sugar is introduced unless it carries distinct semantics. If two syntactic forms would compile to identical IR, only one is permitted. This makes code written by different agents (or humans) structurally consistent and eliminates style-based ambiguity.
