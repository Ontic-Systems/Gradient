# Gradient Architecture Guide

This document describes the internals of the Gradient compiler and toolchain. It is intended for AI agents working on or integrating with Gradient.

---

## Compiler Pipeline

```
Source (.gr) -> Lexer -> Parser -> AST -> Type Checker -> IR (SSA) -> Cranelift Codegen -> Linker -> Binary
```

The full pipeline is wired end-to-end and produces native executables from Gradient source files.

### 1. Lexer

Hand-written tokenizer that emits a flat token stream. The lexer tracks indentation depth and synthesizes `INDENT` and `DEDENT` tokens to represent indentation-based block structure (similar to Python, but with stricter rules).

Token categories: keywords (17), identifiers, literals (int, float, string, bool), operators (including `%` for modulo), delimiters, `INDENT`, `DEDENT`, `NEWLINE`, and error tokens for recovery.

All tokens are ASCII-only. No Unicode operators are permitted.

**Location:** `codebase/compiler/src/lexer/` -- **61 tests**

### 2. Parser

Recursive descent, LL(1). Implements the PEG grammar defined in `resources/grammar.peg`. Produces a fully typed AST where every node carries source location information (`Span` with file id, start position, end position).

Error recovery is implemented. The parser uses synchronization tokens (newlines, dedents, keywords) to resync and continue parsing, collecting all diagnostics for a single pass.

Key grammar features:
- Module declarations (`mod path`) and use imports (`use path`, `use path.{items}`)
- Function definitions with colon-delimited blocks: `fn name(params) -> RetType:`
- Extern function declarations (no body)
- Let bindings with optional type annotations
- If/else if/else expressions (with `:` before each branch block)
- For loops (`for var in expr:`)
- All arithmetic, comparison, and logical operators with correct precedence
- Typed holes (`?name`)
- Annotations (`@name(args)`)
- Field access (`expr.field`)

**Location:** `codebase/compiler/src/parser/` -- **46 tests**

### 3. AST

Typed AST nodes follow the Rust enum + struct pattern. Every node is wrapped in `Spanned<T>` which carries a `Span` value encoding its source location:

```
Span { file_id: u32, start: Position, end: Position }
Position { line: u32, col: u32, offset: u32 }
```

The AST is organized into modules:
- `module.rs` -- `Module`, `ModuleDecl`, `UseDecl`
- `item.rs` -- `FnDef`, `ExternFnDecl`, `Param`, `Annotation`, `ItemKind`
- `stmt.rs` -- `StmtKind::Let`, `StmtKind::Ret`, `StmtKind::Expr`
- `expr.rs` -- `ExprKind` variants (literals, binary/unary ops, calls, if, for, typed holes, field access, etc.)
- `types.rs` -- `TypeExpr` (Named, Unit, Fn), `EffectSet`
- `block.rs` -- `Block` (a spanned list of statements)
- `span.rs` -- `Span`, `Position`, `Spanned<T>`

**Location:** `codebase/compiler/src/ast/`

### 4. Type Checker

Static type checker with inference for let bindings. Key properties:

- **Five built-in types:** `Int`, `Float`, `String`, `Bool`, `()`
- **Forward references:** all function signatures are registered before any bodies are checked, allowing mutual recursion.
- **Type inference:** let bindings without explicit type annotations have their types inferred from the right-hand side.
- **Effect validation:** calling a function with `!{IO}` from a pure function is a type error.
- **Lexical scoping:** scope stack with push/pop for blocks.
- **Error recovery:** `Ty::Error` sentinel suppresses cascading diagnostics.
- **String concatenation:** `+` on `String` operands is type-checked as concatenation.
- **Builtin function registry:** `print`, `println`, `print_int`, `print_float`, `print_bool`, `abs`, `min`, `max`, `mod_int`, `to_string`, `int_to_string`, `range` are all pre-loaded.

The type checker does **not** modify the AST. It reads the AST and produces a list of `TypeError`s.

**Location:** `codebase/compiler/src/typechecker/` -- **52 tests**

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

**Location:** `codebase/compiler/src/ir/` -- **27 tests** (in `builder/tests.rs`)

### 6. Codegen

**Cranelift (debug builds):**
- Translates IR to native machine code via Cranelift.
- Emits an object file (`.o`) that is then linked by the system's `cc` to produce an executable.
- Working and producing real binaries.

**LLVM (release builds):**
- Not yet implemented.

**Location:** `codebase/compiler/src/codegen/`

---

## Project Structure

```
Gradient/
├── assets/              # Logo, banner
├── codebase/
│   ├── build-system/    # `gradient` CLI (Rust, clap)
│   │   └── src/
│   │       ├── main.rs          # CLI entry point
│   │       ├── commands/        # build, run, check, new, init, fmt, test, repl
│   │       ├── manifest.rs      # gradient.toml parsing
│   │       └── project.rs       # Project discovery and paths
│   ├── compiler/        # Compiler pipeline (Rust, Cranelift)
│   │   └── src/
│   │       ├── main.rs          # Compiler driver
│   │       ├── lib.rs           # Library crate root
│   │       ├── lexer/           # Tokenizer with INDENT/DEDENT
│   │       ├── parser/          # Recursive descent parser
│   │       ├── ast/             # AST node definitions
│   │       ├── typechecker/     # Type checker and environment
│   │       ├── ir/              # SSA IR and builder
│   │       └── codegen/         # Cranelift backend
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
- `gradient build` -- finds the project root, invokes the compiler on `src/main.gr`, links with `cc`, outputs binary to `target/debug/<name>`
- `gradient run` -- builds then executes the binary, forwarding the exit code
- `gradient check` -- invokes the compiler for type checking, discards the object file

The build system finds the project root by searching upward for `gradient.toml`. It locates the compiler binary relative to its own path.

### Manifest

`gradient.toml` defines a project:

```toml
[package]
name = "my-project"
version = "0.1.0"
```

### Project layout

```
my-project/
├── gradient.toml
├── src/
│   └── main.gr
└── target/
    ├── debug/
    └── release/
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
