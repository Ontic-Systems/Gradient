# Gradient Roadmap

Gradient is an alpha-stage programming language and compiler stack built for AI-assisted software development.

The roadmap below reflects the current repository state and the April 2026 research synthesis.

The main conclusion from that research is straightforward:

- self-hosting remains the highest-leverage long-term investment
- the parser is the immediate compiler bottleneck
- comptime is the best short-term advanced-types task
- Cranelift remains the default backend for fast iteration
- LLVM is optional medium-term release work, not the current blocker
- production-grade WASM needs a deliberate backend plan, not assumption drift

## Current Product Shape

What is stable today:

- native compilation through the Rust host compiler and Cranelift
- type checking with effects, contracts, generics, pattern matching, modules, traits, actors, lists, maps, and test support
- compiler-as-library query APIs
- LSP support

What is still in-progress or experimental:

- the self-hosted compiler in `compiler/*.gr`
- production-grade WASM strategy
- LLVM backend completion
- refinement types and session types
- registry-backed package distribution

## Roadmap Principles

Every roadmap decision below follows five constraints:

1. Protect the working Rust compiler.
2. Prioritize steps that unblock self-hosting.
3. Prefer verification and differential testing before broadening surface area.
4. Separate "near-term compiler execution" from "long-term agent-language theory."
5. Keep public claims narrower than internal aspirations.

## Step-By-Step Roadmap

### Step 1: Lock the self-hosted parser bootstrap subset

Status: `Now`

Goal:

- define the exact source forms the first self-hosted parser must accept

Deliverables:

- parser subset specification
- explicit statement on temporary collection/list workaround strategy
- decision on initial scannerless strategy versus token-stream staging

Why first:

- current research converges on parser work as the immediate bottleneck
- the bootstrap parser should be intentionally smaller than full Rust-parser parity

Exit criteria:

- one written bootstrap scope doc
- one accepted temporary AST-sequence representation
- one first-milestone acceptance corpus

### Step 2: Implement the self-hosted parser milestone

Status: `Now`

Goal:

- make `compiler/parser.gr` handle a constrained but useful program subset

Target milestone:

- function definitions
- let-bindings
- literals
- arithmetic expressions
- module-level structure needed for early compiler files

Implementation guidance from research:

- immutable state threading
- `(result, state)` style parser functions
- recursive descent first
- fail-fast error behavior first
- defer richer recovery until later

Exit criteria:

- parser accepts the bootstrap subset
- outputs are stable enough for comparison testing
- temporary list workaround remains localized

### Step 3: Build the parser testing bridge early

Status: `Now`

Goal:

- reduce risk before downstream self-hosting work multiplies it

Deliverables:

- shared parser corpus between Rust and self-hosted implementations
- AST serialization or comparable normalized output
- golden tests for representative syntax families
- initial differential tests against the host parser

Why this early:

- the research strongly supports differential testing as high ROI
- parser confidence should not depend on manual spot checks

Exit criteria:

- at least one automated Rust-vs-self-hosted parser comparison path
- golden output checked in for the bootstrap subset

### Step 4: Finish comptime polish

Status: `Now`

Goal:

- close the remaining comptime quality gaps without expanding scope

Deliverables:

- improved error reporting for runtime values passed to comptime parameters
- explicit compile-time failure surfaces
- evaluation budget or termination guardrails

Why now:

- comptime is the shortest advanced-types task with direct compiler value
- it improves the language without destabilizing the self-hosting critical path

Exit criteria:

- current TODOs closed
- tests updated
- comptime documented as complete enough for current roadmap purposes

### Step 5: Complete self-hosted semantic passes

Status: `Next`

Goal:

- move from parser bootstrap to a useful self-hosted compiler front end

Scope:

- `compiler/checker.gr`
- `compiler/ir.gr`
- `compiler/ir_builder.gr`
- `compiler/codegen.gr`

Dependency note:

- this step should start only once parser shape and comparison testing are credible

Exit criteria:

- self-hosted compiler can process meaningful Gradient programs beyond tokenization/parsing
- bootstrap flow is documented and repeatable

### Step 6: Harden the public compiler workflow

Status: `Next`

Goal:

- keep the Rust host compiler clearly production-leading while self-hosting advances

Deliverables:

- clearer CI expectations
- stronger local-vs-CI parity
- improved docs for supported versus experimental features
- regression tracking for parser, typechecker, and build-system workflows

Exit criteria:

- stable public docs
- fewer ambiguous "works locally but not in CI" claims
- public roadmap and README remain aligned

### Step 7: Revisit backend expansion in the right order

Status: `Later`

Priority order:

1. Cranelift remains the default development backend.
2. LLVM is an optional bounded release-backend completion project.
3. production WASM is a separate backend initiative with an explicit design choice.

What this means in practice:

- do not let LLVM displace parser/self-hosting work
- do not market WASM as fully mature until the backend path is hardened
- treat backend comparison as an engineering track, not the main narrative

Exit criteria:

- written backend strategy update
- explicit decision between direct WASM emission, LLVM-to-WASM reuse, or another dedicated route

### Step 8: Formalize Gradient's agent-native language core

Status: `Parallel research track`

Goal:

- turn the broader research thesis into coherent language design direction

Core themes from research:

- typed tool and capability interfaces
- effect and authority tracking
- memory partitioning semantics
- contracts around actions and observations
- executable semantics
- multi-agent coordination primitives

Important boundary:

- this work should inform naming and design decisions now
- it should not block parser and self-hosting execution

Exit criteria:

- one design memo for agent-native language primitives
- clear distinction between current features, near-term plans, and long-term research

## Milestone View

### Near-Term

- parser bootstrap scope locked
- first self-hosted parser milestone implemented
- parser differential testing started
- comptime polished

### Mid-Term

- self-hosted checker and IR work meaningfully underway
- public docs and CI status tightened
- backend strategy clarified without derailing self-hosting

### Long-Term

- self-hosted compiler becomes the center of the Gradient development loop
- production-grade WASM strategy lands
- agent-native language features move from theory into concrete design and implementation

## Notable Non-Goals Right Now

- broadening the language surface before self-hosting bottlenecks are reduced
- marketing LLVM as imminent
- treating experimental WASM support as production-ready
- starting refinement or session types ahead of parser/comptime/testing priorities

## Related Documents

- [Self-Hosting Roadmap](./SELF_HOSTING.md)
- [Agent Integration](./agent-integration.md)
- [Architecture](./architecture.md)
