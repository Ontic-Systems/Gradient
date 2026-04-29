# Gradient Roadmap

Gradient is an alpha-stage programming language and compiler stack built for AI-assisted software development.

The April 2026 research direction remains current:

- self-hosting is the highest-leverage long-term investment
- parser/checker parity gates are the immediate compiler bottleneck
- Cranelift remains the default backend for fast iteration
- LLVM is optional medium-term release work, not the current blocker
- production-grade WASM needs a deliberate backend plan, not assumption drift
- public claims must stay narrower than internal aspirations

## Current Product Shape

Stable today:

- native compilation through the Rust host compiler and Cranelift
- type checking with effects, contracts, generics, pattern matching, modules, traits, actors, lists, maps, and test support
- compiler-as-library query APIs in the Rust implementation
- LSP support backed by the Rust implementation
- CI-gated compiler, security, WASM, and end-to-end checks

In progress or experimental:

- self-hosted compiler modules in `compiler/*.gr`
- direct self-hosted parser execution
- self-hosted checker/IR/codegen/pipeline parity
- production-grade WASM strategy
- LLVM backend completion
- refinement types and session types
- registry-backed package distribution

## Current Self-Hosting Baseline

The active self-hosted compiler tree is `compiler/*.gr`.

Recent bootstrap substrate:

- #236: runtime-backed bootstrap collection handles
- #237: `lexer.gr` emits real `TokenList` values
- #238: `parser.gr` token access reads runtime-backed `TokenList` data
- #239: `parser.gr` stores real AST nodes/lists
- #240: `checker.gr` uses runtime-backed env storage and AST dispatch
- #242/#244: stale duplicate/dead code cleanup

This means the project has moved beyond pure stubs for lexer/parser/checker substrate. It does not mean the compiler is fully self-hosted.

Known current blockers:

- token payload access for identifiers/literals/errors
- newline/`INDENT`/`DEDENT` lexer parity
- direct `parser.gr` invocation through the Gradient runtime path
- checker differential parity against the Rust checker
- executable IR lowering, codegen, and compiler pipeline phases
- usable self-hosted driver, query service, and LSP backing
- end-to-end bootstrap trust checks and Rust-kernel boundary metrics

## Roadmap Principles

1. Protect the working Rust compiler.
2. Prioritize steps that unblock self-hosting.
3. Prefer verification and differential testing before broadening surface area.
4. Separate near-term compiler execution from long-term agent-language theory.
5. Keep public claims narrower than internal aspirations.

## Near-Term Roadmap

### Step 1: Direct parser execution and parser corpus expansion

Status: `Now`

Issues:

- #223: invoke `parser.gr` directly in the differential gate
- #224: expand parser parity corpus beyond bootstrap basics

Goal:

- prove self-hosted parser code runs through the intended Gradient runtime/comptime path
- prevent silent fallback to Rust-side bridge behavior
- expand corpus coverage to syntax used by `compiler/*.gr`

Required work:

- expose token payload accessors for identifiers, literals, strings, and errors
- add newline/indentation-sensitive lexer coverage
- distinguish real self-hosted execution from bridge fallback in test output
- add canonical normalized baselines for representative syntax families

Exit criteria:

- parser direct-exec gate fails if it silently falls back for the corpus
- corpus covers the current bootstrap subset plus representative compiler-module syntax

### Step 2: Checker differential parity

Status: `Now`

Issue:

- #226: add checker differential parity gate

Goal:

- compare self-hosted checker results against the Rust checker for a bounded corpus

Current substrate:

- #240 added runtime-backed checker env/fn/var storage
- #240 added AST dispatch via bootstrap expression/statement accessors

Required work:

- normalize checker output into comparable type/diagnostic results
- add positive and negative fixtures
- ensure the gate detects placeholder success and diagnostic drift

Exit criteria:

- bounded Rust-vs-self-hosted checker parity gate is CI-visible

### Step 3: IR lowering and IR parity

Status: `Next`

Issues:

- #227: make `ir_builder.gr` lower real AST to IR
- #228: add IR differential/golden parity tests

Goal:

- turn parsed/checked bootstrap AST into real self-hosted IR for a bounded subset

Exit criteria:

- self-hosted IR output can be compared against the Rust host for selected fixtures

### Step 4: Codegen and compiler pipeline execution

Status: `Next`

Issues:

- #229: implement executable codegen/emission slice
- #230: make `compiler.gr` pipeline execute real phases

Goal:

- connect self-hosted front-end work to an executable compilation pipeline

Exit criteria:

- a bounded source subset flows through parser/checker/IR/codegen orchestration without placeholder phase returns

### Step 5: Driver, query, and LSP backing

Status: `Next`

Issues:

- #231: make `main.gr` a usable bootstrap compiler driver
- #232: back `query.gr` with real sessions and diagnostics
- #233: back `lsp.gr` with query/session data

Goal:

- make self-hosted compiler services useful to users and agents

Exit criteria:

- driver behavior, query diagnostics, and LSP responses come from real session state for the documented subset

### Step 6: Bootstrap trust and Rust-kernel boundary

Status: `Next`

Issues:

- #234: add end-to-end bootstrap trust checks
- #235: define and shrink the Rust kernel boundary

Goal:

- measure what is still Rust-owned and prevent accidental host fallback

Exit criteria:

- trust checks prove which self-hosted phases executed
- Rust-kernel responsibilities are listed, measured, and intentionally retained

## Backend Track

Status: `Later`

Priority order:

1. Keep Cranelift as the default development backend.
2. Treat LLVM as an optional bounded release-backend completion project.
3. Treat production WASM as a separate backend initiative with an explicit design choice.

This track must not displace parser/checker/IR self-hosting work.

## Agent-Native Language Research Track

Status: `Parallel research track`

Research themes:

- typed tool and capability interfaces
- effect and authority tracking
- memory partitioning semantics
- contracts around actions and observations
- executable semantics
- multi-agent coordination primitives

Boundary:

- this should inform naming and design decisions
- it should not block parser/checker/IR execution work

## Milestone View

Near-term:

- direct parser execution prerequisites
- parser corpus expansion
- checker differential parity

Mid-term:

- executable self-hosted IR and codegen slices
- real compiler pipeline execution
- driver/query/LSP backing

Long-term:

- self-hosted compiler becomes the center of the Gradient development loop
- Rust kernel is measured, explicit, and small
- backend strategy is clarified without derailing self-hosting
