<div align="center">

<img src="assets/banner.png" alt="Gradient" width="540"/>

<br/>
<br/>

**An agent-native programming language and compiler stack.**

<br/>

[![Status](https://img.shields.io/badge/status-alpha-blueviolet?style=flat-square&labelColor=0d0d17)](https://github.com/Ontic-Systems/Gradient)
[![Language](https://img.shields.io/badge/impl-Rust-orange?style=flat-square&labelColor=0d0d17)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-4f8aff?style=flat-square&labelColor=0d0d17)](LICENSE)
[![Backend](https://img.shields.io/badge/backend-Cranelift-00e5ff?style=flat-square&labelColor=0d0d17)](#project-status)
[![Stage](https://img.shields.io/badge/self--hosting-in_progress-2ecc71?style=flat-square&labelColor=0d0d17)](#roadmap)

</div>

---

Gradient is a programming language designed for AI-assisted development.

It combines a typed language, a compiler with structured query surfaces, and an active commitment to writing the compiler in itself — self-hosting as proof that the language is real for systems work, not a feature aimed at end users.

## What Gradient Is For

Gradient targets four overlapping use cases:

1. **Agent-assisted coding.** Give coding agents a language with explicit effects, contracts, and a machine-readable compiler surface instead of relying on fragile prompt-only loops.
2. **Compiler-verified automation.** Move from generate-compile-fix toward generate-check-verify by making syntax, type expectations, and side effects more explicit.
3. **Tooling for agent workflows.** Expose compiler services, diagnostics, and query APIs that are easier for agents and IDEs to consume than raw terminal text.
4. **Research on agent-native languages.** Explore how tools, authority, effects, contracts, and eventually memory/protocol abstractions can become first-class language concepts.

## Why Gradient Exists

Most current LLM coding workflows burn tokens and time on avoidable failure loops:

- syntax mistakes
- missing type context
- hidden side effects
- weak tooling interfaces
- repeated compile-fix retries

Gradient is built to reduce that waste through:

| Technique | Why It Matters |
|-----------|----------------|
| **Grammar-constrained generation** | pushes syntax validity earlier in the generation loop |
| **Effect tracking** | makes side effects explicit instead of implicit |
| **Contracts** | supports generate-check-verify workflows |
| **Structured compiler queries** | gives agents machine-readable diagnostics and symbol data |
| **Self-hosting (philosophy + trust artifact)** | the compiler is being written in Gradient — proof the language is real for systems work, and a live dogfooding loop that pressures the design |

## What Works Today

The Rust host compiler is the stable center of the project today.

Available now:

- native compilation via **Cranelift**
- static type checking with inference
- algebraic data types and pattern matching
- generics
- modules
- contracts via `@requires` / `@ensures`
- effect tracking
- lists, maps, tuples, closures, traits, and actor syntax
- compiler-as-library query APIs
- LSP support
- `gradient build`, `run`, `check`, `test`, `fmt`, `repl`, and dependency workflows

Supporting docs:

- [Language Guide](docs/language-guide.md)
- [CLI Reference](docs/cli-reference.md)
- [Agent Integration](docs/agent-integration.md)
- [Architecture](docs/architecture.md)

## What Is Still Experimental

Gradient is still alpha software.

These areas are real, but not yet production-grade:

- the self-hosted compiler in `compiler/*.gr`
- WebAssembly support
- LLVM backend completion
- registry-backed package distribution
- refinement types and session types

Public docs should be read with that distinction in mind:

- **Cranelift** is the working default backend
- **WASM** exists as an experimental path, not the primary production path
- **LLVM** is a medium-term engineering option, not the current focus

## Quick Example

```gradient
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)
```

Gradient's direction is visible in one small example:

- the function is statically typed
- the intent is declared with contracts
- the compiler can expose this structure to tools and agents

## Quick Start

### Prerequisites

- Rust 1.75+
- optional: `wasm32-unknown-unknown` target for experimental WASM work

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add wasm32-unknown-unknown
```

### Build

```bash
git clone https://github.com/Ontic-Systems/Gradient.git
cd Gradient/codebase
cargo build --release
```

### Create and run a project

```bash
./target/release/gradient new hello
cd hello
../target/release/gradient run
```

### Experimental WASM path

```bash
cargo build --release --features wasm
./target/release/gradient-compiler hello.gr hello.wasm --backend wasm
wasmtime hello.wasm
```

Use the WASM path as an experiment, not yet as the default deployment story.

## Language Highlights

### Effects are explicit

```gradient
fn add(a: Int, b: Int) -> Int:
    ret a + b

fn greet(name: String) -> !{IO} ():
    print("Hello, " + name)
```

### Contracts are part of the surface language

```gradient
@requires(b != 0)
@ensures(result * b == a)
fn div_exact(a: Int, b: Int) -> Int:
    ret a / b
```

### Generics and algebraic data types are built in

```gradient
type Option[T] = Some(T) | None

fn unwrap_or[T](opt: Option[T], default: T) -> T:
    match opt:
        Some(value): value
        None: default
```

## Project Status

### Stable core

| Area | Status | Notes |
|------|--------|-------|
| Rust host compiler | `Stable core` | primary implementation |
| Cranelift backend | `Stable core` | default codegen path |
| Type system | `Stable core` | inference, effects, contracts, generics |
| Query API | `Stable core` | machine-readable compiler services |
| LSP | `Stable core` | editor and agent integration surface |
| Modules and build flow | `Stable core` | multi-file project support |

### Experimental / in progress

| Area | Status | Notes |
|------|--------|-------|
| Self-hosted compiler | `In progress` | major strategic focus |
| WebAssembly backend | `Experimental` | useful path, not yet the default story |
| LLVM backend | `Incomplete` | not on the immediate critical path |
| Package registry | `Planned` | path dependencies are the practical option today |
| Refinement and session types | `Planned` | research-backed, not near-term execution work |

## Roadmap

The roadmap has been updated to reflect the current research consensus.

Short version:

1. lock the bootstrap parser subset
2. implement the first self-hosted parser milestone
3. build parser comparison and differential tests early
4. finish comptime polish
5. continue self-hosted checker / IR / bootstrap work
6. revisit LLVM and production WASM after self-hosting pressure drops
7. keep advancing the longer-term agent-native language design agenda in parallel

Detailed docs:

- [Project Roadmap](docs/roadmap.md)
- [Self-Hosting Roadmap](docs/SELF_HOSTING.md)

## Self-Hosting as Philosophy

Gradient is being written in Gradient. The Rust host compiler is the trusted kernel today; `compiler/*.gr` is the active self-hosted tree, and the kernel boundary is being progressively shrunk.

We treat self-hosting as **philosophy + trust artifact**, not a user-facing feature:

- if a language is going to claim systems-tier credibility, the compiler should be expressible in it
- writing the compiler in Gradient is the most aggressive dogfooding loop available: every effect, capability, contract, and ergonomic decision has to survive use in the compiler itself before it ships to anyone else
- the Rust-vs-Gradient lines-of-code ratio is a public metric that tracks the trajectory honestly (planned, see Epic #116)

This is **not** the same claim as "agents will edit the compiler." Self-hosting is a discipline we apply to ourselves; it is not something we expect downstream agents or users to depend on.

## Intended Positioning

Gradient is not trying to be "just another general-purpose language."

The project is aimed at a narrower and more ambitious intersection:

- a language that agents can generate more reliably
- a compiler that agents can query directly
- a workflow that moves verification closer to generation
- a long-term language design program for agent-native software systems

That means the near-term value is practical tooling and compiler behavior.

The long-term value is a language whose semantics are shaped around tools, authority, verification, and machine-assisted development from the start.

## Repository Layout

```text
Gradient/
├── codebase/    Rust host compiler and toolchain
├── compiler/    Self-hosted Gradient compiler work
├── docs/        Public documentation
├── examples/    Example programs
├── resources/   Grammar and language reference material
└── assets/      Project assets
```

## License

MIT. See [LICENSE](LICENSE).

<div align="center">
<sub>Built for agents. Grounded in the compiler.</sub>
</div>
