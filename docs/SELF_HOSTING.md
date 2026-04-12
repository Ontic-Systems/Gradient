# Self-Hosting Roadmap

Gradient's self-hosting plan is to move more of the compiler into Gradient while keeping a small Rust host kernel for platform-sensitive primitives and code generation.

This document is the detailed execution plan for that effort.

It is aligned with the April 2026 research review and with the public project roadmap.

## Objective

Target state:

- the Rust compiler remains the trusted host implementation
- the self-hosted compiler becomes the primary dogfooded compiler implementation
- the boundary between the two is explicit and small

Rust should keep:

- platform and runtime primitives
- file and process integration
- backend code generation engines
- bootstrap-critical low-level functionality

Gradient should increasingly own:

- compiler front-end logic
- semantic analysis
- agent-facing compiler services
- higher-level orchestration

## Current State

Stable facts from the repository and research:

- the Rust compiler is the production-leading implementation
- string primitives needed for self-hosted lexer/parser work are present
- self-hosted compiler files already exist in `compiler/*.gr`
- parser work is still the most important self-hosting bottleneck
- list/collection ergonomics remain a structural constraint for cleaner self-hosted implementations

## Execution Principles

1. Keep the bootstrap parser intentionally narrow.
2. Prefer local correctness and comparison tests over early completeness.
3. Localize temporary workarounds so they do not spread through later phases.
4. Do not block self-hosting on advanced type-system work beyond comptime polish.
5. Keep the Rust compiler usable and documented while self-hosting evolves.

## Step-By-Step Plan

### Step 1: Freeze the bootstrap parser subset

Purpose:

- define the minimum grammar slice required to bootstrap meaningful self-hosted progress

Must decide:

- which declarations are in the first milestone
- which expressions are in the first milestone
- whether initial parsing is scannerless or staged through a narrow token abstraction
- which temporary sequence representation will stand in for richer list support

Output:

- a parser bootstrap scope note
- a corpus of accepted source examples
- a written list of intentionally unsupported constructs for the first milestone

### Step 2: Implement parser state threading in `compiler/parser.gr`

Purpose:

- establish the parser architecture recommended by the research

Required characteristics:

- immutable parser state
- parse functions return updated state with result data
- precedence-aware expression parsing
- simple failure behavior first

Recommended first parser milestone:

- module shell
- function definitions
- let-bindings
- literals
- identifiers
- arithmetic and comparison expressions

Do not optimize for:

- rich recovery
- complete syntax parity
- elegant list accumulation

### Step 3: Constrain the temporary collection strategy

Purpose:

- avoid architectural drift while list primitives remain incomplete or awkward

Rules:

- use one temporary representation for parser-built sequences
- document where the representation enters and leaves the parser
- do not allow ad hoc sequence encodings to spread into checker and IR logic

Expected replacement path:

- once cleaner list support lands, replace the temporary representation behind stable boundaries

### Step 4: Add parser comparison infrastructure

Purpose:

- make self-hosted parser progress measurable

Build:

- a normalized AST or parse-output representation
- a shared test corpus between Rust and self-hosted parsers
- golden tests for syntax categories
- differential checks for the bootstrap subset

Success condition:

- self-hosted parser behavior can be compared automatically against the host parser for a known subset

### Step 5: Finish comptime polish in the Rust compiler

Purpose:

- land the highest-value near-term advanced-types work without derailing self-hosting

Focus:

- clearer diagnostics for non-comptime arguments
- explicit compile-time failure behavior
- evaluation budget limits or similar guardrails

Why inside the self-hosting roadmap:

- comptime strengthens the language used to write future self-hosted compiler code

### Step 6: Move into self-hosted semantic passes

Target files:

- `compiler/checker.gr`
- `compiler/ir.gr`
- `compiler/ir_builder.gr`
- `compiler/codegen.gr`

Prerequisites:

- parser bootstrap works
- parser outputs are testable
- temporary collection boundaries are understood

Success condition:

- self-hosted compilation covers meaningfully more than syntax

### Step 7: Build a repeatable bootstrap path

Purpose:

- make self-hosting practical, not just possible

Deliverables:

- documented stage ordering
- commands/scripts for bootstrap validation
- "same result" or equivalent trust checks where feasible
- failure diagnosis notes for stage mismatches

### Step 8: Expand the self-hosted compiler surface deliberately

After front-end stability improves, expand in this order:

1. query/compiler services useful to agents
2. orchestration and build flow
3. additional compiler subsystems where the self-hosted implementation is clearly paying for itself

Reason:

- the project's differentiator is not just self-hosting
- it is self-hosting in a compiler stack designed for agent use

## Dependencies And Ordering

The important dependency chain is:

1. parser scope
2. parser implementation
3. parser comparison infrastructure
4. self-hosted checker and IR work
5. repeatable bootstrap flow
6. broader self-hosted compiler services

Comptime can proceed in parallel because it is a contained host-compiler improvement with direct language value.

LLVM completion, production WASM strategy, refinement types, and session types should not sit on the critical path for this plan.

## Risks

### Risk: Parser scope expands too early

Impact:

- slower delivery
- more rewrite churn

Mitigation:

- freeze the bootstrap subset before implementation accelerates

### Risk: Temporary list workarounds leak everywhere

Impact:

- later checker/IR code becomes harder to replace cleanly

Mitigation:

- isolate the workaround behind parser-local boundaries

### Risk: Progress is judged by anecdotes instead of comparison tests

Impact:

- false confidence
- hard-to-debug semantic drift

Mitigation:

- build differential and golden tests early

### Risk: Advanced research pulls focus from execution

Impact:

- roadmap churn
- delayed self-hosting milestones

Mitigation:

- keep agent-language theory as a parallel design track, not a blocker

## What This Roadmap Does Not Assume

It does not assume:

- full parser parity before value is created
- immediate LLVM completion
- production-ready WASM output in the short term
- refinement or session types as near-term prerequisites

## Success Markers

Near-term success:

- bootstrap parser subset documented
- first parser milestone implemented
- parser comparison harness started
- comptime polish completed

Mid-term success:

- self-hosted checker and IR work progress on top of stable parser outputs
- bootstrap flow is reproducible

Long-term success:

- self-hosted compiler becomes a practical part of the project's own development loop
- the Rust compiler and self-hosted compiler have a clear, durable boundary

## Related Documents

- [Project Roadmap](./roadmap.md)
- [Architecture](./architecture.md)
- [Agent Integration](./agent-integration.md)
