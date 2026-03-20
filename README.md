<div align="center">

<img src="assets/banner.png" alt="Gradient" width="540"/>

<br/>
<br/>

**The world's first programming language designed from the ground up for autonomous AI agents.**

<br/>

[![Status](https://img.shields.io/badge/status-pre--alpha-blueviolet?style=flat-square&labelColor=0d0d17)](https://github.com/graydeon/Gradient)
[![Language](https://img.shields.io/badge/impl-Rust-orange?style=flat-square&labelColor=0d0d17)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-4f8aff?style=flat-square&labelColor=0d0d17)](LICENSE)
[![Backend](https://img.shields.io/badge/backend-Cranelift%20%7C%20LLVM-00e5ff?style=flat-square&labelColor=0d0d17)](https://cranelift.dev)

</div>

---

## What is Gradient?

Every programming language ever built was designed around **human cognition** — mnemonic keywords, visual indentation, memorable syntax, human-readable error messages. Gradient discards these assumptions entirely.

Gradient is a **statically-typed, arena-first, actor-based systems language** built for one specific programmer: an LLM operating under a context window budget, running generate→compile→fix loops at machine speed.

It is capable of writing a kernel. It is capable of running an OS. And it does all of it in fewer tokens than any language that came before it.

---

## Design Priorities

In strict priority order:

| # | Priority | What it means |
|---|---|---|
| 1 | **Token efficiency** | Every saved token is reclaimed context window and reduced inference cost |
| 2 | **Unambiguous parseability** | An LLM generates correct Gradient on the first pass, not the fifth |
| 3 | **Semantic density** | Maximum meaning per token — terse, consistent, composable primitives |
| 4 | **Systems capability** | Kernel, drivers, OS userspace — all within reach |
| 5 | **Agentic primitives** | Agent identity, capability delegation, supervision trees — first class |

---

## Language Design

### Syntax
- **ASCII-only** — no Unicode operators; every symbol is a single token in major LLM tokenizers
- **Indentation-significant** — no braces, no semicolons, no redundant delimiters
- **Keyword-led** — `fn`, `let`, `if`, `for`, `ret`, `type`, `mod`, `use`, `impl`
- **One canonical form** per construct — the formatter is a normalization function, not a style guide
- **LL(1)-parseable** — context-free, unambiguous, enabling grammar-guided LLM decoding

```
fn fib(n: i32) -> i32 =
  if n < 2 then n
  else fib(n - 1) + fib(n - 2)
```

### Type System
- Bidirectional **Hindley-Milner inference** — annotate function signatures, infer everything inside
- **No lifetime annotations** — ever
- Algebraic data types with **exhaustive pattern matching**
- Row-polymorphic **effect system** — async is just an effect, no colored functions
- **Typed holes** — write `?hole`, get compiler-verified candidates back

### Memory Model
A three-tier model that provides safety without annotation burden:

| Tier | Mechanism | Coverage | Annotation cost |
|---|---|---|---|
| 1 | **Arena-first** (Zig/Odin-inspired) | ~80% of code | Zero |
| 2 | **Generational references** (Vale-inspired) | ~15% of code | Near-zero |
| 3 | **Linear types** (kernel/driver code only) | ~5% of code | Explicit, justified |

### Concurrency & Agents
- **Actors** as the primary concurrency primitive — each agent owns its memory
- **Algebraic effects** eliminate colored async/sync splits
- **Structured concurrency** — child agents cannot outlive their supervisor scope
- **Supervision trees** — `one_for_one`, `one_for_all`, `rest_for_one`
- **Resource budgets** as linear types — `Budget(tokens: 10000, memory: 100MB)`

### Security
- **Capability-based** from the ground up — capabilities are linear, unforgeable, non-duplicable
- **Module-level capability requirements** — `mod Foo requires {NetAccess, FileRead}`
- **Session types** — agent communication protocols verified at compile time
- Effect system enforces at the type level: an agent **cannot perform an undeclared effect**

---

## Compiler Architecture

```
Source (.gr)
    │
    ▼
Lexer ──────────────────── JSON token stream  (gradient lex)
    │
    ▼
Parser + AST ───────────── JSON AST dump      (gradient parse)
    │
    ▼
Name Resolution
    │
    ▼
Type Checker ──────────── Structured diagnostics + typed hole fits
    │
    ▼
IR (SSA) ──────────────── IR dump             (gradient ir)
    │
    ├──▶ Cranelift ──────── Debug builds   (<100ms incremental)
    └──▶ LLVM ───────────── Release builds (full optimization)
```

The compiler is a **collaborative agent**, not just a validator:
- Structured JSON diagnostics with causal chains, semantic context, and confidence-rated fix diffs
- **Typed holes** (`?hole`) return compiler-verified candidates in JSON
- **Query API** — type-at-position, functions-matching-type, incremental recheck — all sub-10ms

---

## Toolchain

All tools live under one CLI:

```
gradient new <name>      Create a new project
gradient build           Compile (Cranelift debug backend)
gradient build --release Compile (LLVM optimized)
gradient run             Build and execute
gradient test            Run @test-annotated functions
gradient fmt             Canonical formatter
gradient lint            Linter
gradient repl            Cranelift-backed interactive session
gradient doc             Generate documentation
gradient check           Type-check without emitting a binary
gradient bench           Run benchmarks
gradient pkg             Package manager
gradient query           Compiler query API (JSON)
```

### Project layout

```
my-project/
├── gradient.toml        # Manifest
├── gradient.lock        # Lockfile (committed)
├── src/
│   └── root.gr          # Package root module
└── .gradient/           # Local cache
```

---

## Build Philosophy

Gradient is built **slow and modular**. At every point in development there is a working, testable artifact. Nothing is theoretical. Nothing ships without passing tests in a live environment.

The build roadmap is structured as 18 progressive phases — each one adding exactly one capability to the live system, from an empty binary to a full agentic language runtime. The hard checkpoint is **Phase 6**: `fn main() -> i32 = 42` compiles, runs, and exits with code 42. Nothing beyond that starts until that's green.

---

## Status

Gradient is in **pre-alpha**. The compiler does not yet exist. The language specification is being written. Infrastructure is being set up.

We are currently on **Phase 0 — Foundation**.

---

## Team

Gradient is built by Gray d'Éon.

---

## License

MIT — see [LICENSE](LICENSE).

---

<div align="center">
<sub>∇ built for the agents that will build everything else</sub>
</div>
