# Gradient Development Roadmap

## Vision

Gradient is the world's first programming language designed for autonomous AI agents. Every decision — from syntax to error messages to the build system — is optimized for LLM consumption and agentic workflows.

---

## Phase 0 — Foundation (COMPLETE)

- **Formal PEG grammar** (`resources/grammar.peg`) — 348-line specification covering modules, functions, let bindings, control flow, types, and expressions
- **Language reference** (`resources/language-reference.md`) — complete v0.1 language documentation
- **`gradient` CLI** (`codebase/build-system/`) — unified toolchain entry point with 8 subcommands (build, run, test, check, fmt, new, init, repl) — all scaffolded with clap
- **Cranelift codegen PoC** (`codebase/compiler/`) — proof that IR to Cranelift to native binary works. Produces a working "Hello from Gradient!" binary.
- **IR type system** (`codebase/compiler/src/ir/`) — SSA-form IR with 14 instruction variants, type definitions, value references
- **Test framework** (`codebase/test-framework/`) — golden test runner, test harness, 4-tier test strategy (unit, integration, golden, e2e)
- **Example programs** (`resources/v0.1-examples/`) — hello.gr and factorial.gr

## Phase 1 — Compiler Frontend (PLANNED)

- Hand-written lexer with INDENT/DEDENT token injection
- Recursive descent parser implementing the PEG grammar
- Typed AST with source spans on every node
- Error recovery (report multiple errors per compilation)
- Machine-readable error output (JSON to stderr)

## Phase 2 — Type System (PLANNED)

- Bidirectional Hindley-Milner type inference
- Basic type checking (Int, Float, String, Bool, ())
- Function signature checking
- Exhaustive pattern match validation
- Machine-readable type errors with causal chains

## Phase 3 — IR Generation (PLANNED)

- AST to SSA IR translation
- Scope resolution and name binding
- Effect annotation propagation
- Arena allocation site tracking

## Phase 4 — Full Pipeline (PLANNED)

- Wire lexer to parser to typechecker to IR to Cranelift
- `gradient build` produces a real binary from `.gr` source
- `gradient run` compiles and executes
- **HARD CHECKPOINT**: First real Gradient binary compiled from source

## Phase 5 — Standard Library Core (PLANNED)

- `core.io` — print, read, file operations
- `core.math` — basic math functions
- `core.string` — string manipulation
- `core.collections` — list, map, set

## Phase 6 — Package System (PLANNED)

- `gradient new` creates real projects
- `gradient.toml` manifest parsing
- Dependency resolution
- Content-addressed caching

## Phase 7 — Developer Tooling (PLANNED)

- LSP server (backed by compiler query API)
- Canonical formatter (`gradient fmt`)
- Linter
- REPL (Cranelift-backed)

## Phase 8+ — Advanced Features (FUTURE)

- Effect system (row-polymorphic, Koka-inspired)
- Refinement types (SMT-backed)
- Actor runtime with supervision trees
- Three-tier memory model
- LLVM release backend
- FFI bridges (C, Rust, Python)
- Documentation generator

---

## Status Key

| Status        | Meaning                               |
|---------------|---------------------------------------|
| COMPLETE      | Shipped and working                   |
| IN PROGRESS   | Actively being built                  |
| PLANNED       | Designed but not started              |
| FUTURE        | On the roadmap but not yet designed   |
