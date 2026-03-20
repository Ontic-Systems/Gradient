# Gradient Architecture Guide

This document describes the internals of the Gradient compiler and toolchain. It is intended for AI agents working on or integrating with Gradient.

---

## Compiler Pipeline

```
Source (.gr) -> Lexer -> Parser -> AST -> Name Resolution -> Type Checker -> IR (SSA) -> Codegen -> Binary
```

### 1. Lexer

Hand-written tokenizer that emits a flat token stream. The lexer tracks indentation depth and synthesizes `INDENT` and `DEDENT` tokens to represent indentation-based block structure (similar to Python, but with stricter rules).

Token categories: keywords, identifiers, literals (int, float, string, bool), operators, delimiters, `INDENT`, `DEDENT`, `NEWLINE`.

All tokens are ASCII-only. No Unicode operators are permitted.

**Location:** `codebase/compiler/lexer/` (not yet implemented)

### 2. Parser

Recursive descent, LL(1). Implements the PEG grammar defined in `resources/grammar.peg`. Produces a fully typed AST where every node carries source location information.

Error recovery is mandatory. The parser must never bail on the first error. It uses synchronization tokens (newlines, dedents, keywords) to resync and continue parsing, collecting all diagnostics for a single pass.

**Location:** `codebase/compiler/parser/` (not yet implemented)

### 3. AST

Typed AST nodes follow the Rust enum + struct pattern. Every node carries a `Span` value encoding its source location:

```
Span { file: FileId, start: (line, col), end: (line, col) }
```

The AST is the single source of truth for all downstream compiler stages before lowering to IR.

**Location:** `codebase/compiler/ast/` (not yet implemented)

### 4. Name Resolution

Runs over the AST to resolve all identifiers to their binding sites. Produces a side table mapping each use-site `NodeId` to its definition `NodeId`. Detects duplicate definitions, unresolved names, and shadowing violations.

### 5. Type Checker

Bidirectional type inference based on Hindley-Milner. Key properties:

- **No lifetime annotations.** Memory safety is handled by the three-tier memory model (see below), not by a borrow checker.
- **Structural interfaces.** Types satisfy interfaces by structure, not by explicit `impl` declarations.
- **Algebraic data types.** Sum types with exhaustive pattern matching enforced at compile time.
- **Row-polymorphic effect system.** Effects are tracked in the type system. Functions declare their effects; the type checker verifies that all effects are handled.
- **Refinement types.** Backed by an SMT solver. Allows expressing constraints like `x: Int where x > 0` that are verified at compile time.

**Location:** `codebase/compiler/typechecker/` (not yet implemented)

### 6. IR

SSA (Static Single Assignment) form. The IR makes several things explicit that are implicit in the source language:

- **Capability tokens** — access to IO, network, filesystem, etc. is mediated by explicit capability values threaded through the IR.
- **Effect labels** — every call site is annotated with the effects it may perform.
- **Arena allocation sites** — allocations are explicit IR nodes, not hidden behind syntactic sugar.

**Instruction set:**

| Instruction | Description                          |
|-------------|--------------------------------------|
| `Const`     | Load a constant value                |
| `Call`      | Function call (with effect labels)   |
| `Ret`       | Return from function                 |
| `Add`       | Integer/float addition               |
| `Sub`       | Integer/float subtraction            |
| `Mul`       | Integer/float multiplication         |
| `Div`       | Integer/float division               |
| `Cmp`       | Comparison (returns bool)            |
| `Branch`    | Conditional branch                   |
| `Jump`      | Unconditional branch                 |
| `Phi`       | SSA phi node (merge point)           |
| `Alloca`    | Stack allocation                     |
| `Load`      | Load from memory                     |
| `Store`     | Store to memory                      |

Placeholder types are currently defined in `codebase/compiler/src/ir/`.

### 7. Codegen

Two backends, selected by build profile:

**Cranelift (debug builds):**
- Targets fast compilation. Sub-100ms incremental compilation is the goal.
- Proof-of-concept is working in `codebase/compiler/src/codegen/cranelift.rs`.
- Used for development iteration and REPL.

**LLVM (release builds):**
- Full optimization pipeline for production binaries.
- Not yet implemented.

---

## Project Structure

```
Gradient/
├── assets/              # Logo, banner
├── codebase/
│   ├── build-system/    # `gradient` CLI (Rust, clap)
│   ├── compiler/        # Compiler pipeline (Rust, Cranelift)
│   │   └── src/
│   │       ├── ir/          # IR types and instructions
│   │       └── codegen/     # Cranelift backend
│   ├── devtools/        # LSP, formatter, linter, REPL (planned)
│   ├── docs/            # Doc generator (planned)
│   ├── runtime/         # VM, memory, GC (planned)
│   │   ├── gc/
│   │   ├── memory/
│   │   └── vm/
│   ├── stdlib/          # Standard library (planned)
│   ├── stdlib-core/     # Core stdlib (planned)
│   └── test-framework/  # Test harness and golden tests
├── docs/                # Documentation (this file lives here)
└── resources/           # Grammar, language reference, examples
```

---

## Memory Model (Three-Tier)

Gradient does not use a borrow checker or garbage collector as the primary memory strategy. Instead, it provides three tiers of memory management, selected per-allocation by the programmer (with Tier 1 as the default).

### Tier 1 -- Arena (default)

Bump allocator with deferred bulk-free. Memory is allocated by advancing a pointer; individual frees are not possible. The entire arena is reset when the owning scope exits.

- Zero per-object overhead.
- Expected to cover approximately 80% of allocations in typical code.
- Arena selection is an explicit IR node, visible in the compiler output.

### Tier 2 -- Generational References

Per-slot generation counter with 8 bytes of overhead per allocation. On each access, the runtime checks that the stored generation matches the slot generation. A mismatch means the referent has been freed, and the program traps.

- Use when data must outlive the arena that would naturally own it.
- Provides use-after-free safety without garbage collection.

### Tier 3 -- Linear Types

Enforced by the type system. Values with linear types must be used exactly once. The runtime provides raw `alloc`/`free` for these values.

- Used for capabilities, file handles, network sockets, and other resources that require deterministic cleanup.
- The type checker rejects programs that drop or duplicate linear values.

---

## Actor Runtime (planned)

Gradient's concurrency model is based on isolated actors (called "agents" in Gradient terminology).

- **Per-agent memory isolation.** Each agent has its own arena hierarchy. No shared mutable state between agents.
- **Typed mailboxes.** Message passing through bounded ring buffers. The type system ensures messages are sendable (no arena-local references leak across agent boundaries).
- **Cooperative scheduling.** The compiler inserts yield points at function calls and loop back-edges. No preemption.
- **Work-stealing M:N scheduler.** N agents are multiplexed onto M OS threads. Idle threads steal work from busy threads' run queues.
- **Supervision trees.** OTP-inspired hierarchy. A supervisor agent monitors its children and restarts them on failure according to a declared strategy (one-for-one, one-for-all, rest-for-one).

---

## Build System

### CLI

`gradient` is the unified entry point for all toolchain operations. Built in Rust with `clap`.

### Manifest

`gradient.toml` defines a project:

```toml
[package]
name = "my-project"
version = "0.1.0"

[dependencies]
# dependency-name = "version"
```

### Lockfile

`gradient.lock` pins exact dependency versions for reproducible builds.

### Build Cache

`.gradient/` is the local build cache directory. Stores compiled artifacts, incremental compilation state, and dependency downloads.

---

## Design Principles

### LLM-First

Every design decision optimizes for AI agent consumption:

- Compiler errors are structured (JSON-serializable) with deterministic formatting.
- Token efficiency: the language syntax minimizes token count for common patterns.
- Machine-readable output is the default; human-readable output is a formatting layer on top.

### ASCII-Only

Every token in Gradient source code is a printable ASCII character. No Unicode operators, no special symbols. This eliminates encoding issues and ensures every agent and tool can process Gradient source without character set complications.

### No Hidden Magic

- Allocations are visible as explicit nodes in the IR.
- Effects are declared in function signatures and tracked by the type checker.
- Capabilities (IO, network, filesystem) are explicit values that must be threaded through call chains.
- There is no implicit allocation, no implicit effect propagation, and no implicit capability granting.

### One Way to Do It

No syntactic sugar is introduced unless it carries distinct semantics. If two syntactic forms would compile to identical IR, only one is permitted. This makes code written by different agents (or humans) structurally consistent and eliminates style-based ambiguity.
