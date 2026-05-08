# Gradient Roadmap

> **STATUS:** partial — Roadmap reflects current alpha state and locked vision. Epic-level details now tracked in GitHub Epics #294-#304 + #116; this doc summarizes.

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

## Vision Roadmap (Locked 2026-05-02)

The 2026-05-02 alignment session locked Gradient's positioning as **agent-native + systems-first generalist**: a language an agent can use to emit any tier of software, from bare-metal up to applications, without the LLM-hostile failure modes of borrow-checker dialogue. Self-hosting is treated as philosophy + trust artifact (see [README](../README.md#self-hosting-as-philosophy)).

Pattern lock: **everything is an effect**. Memory, concurrency, errors, FFI, and trust all surface as effect rows on function signatures, so the same mental model scales across tiers and is machine-readable for agents.

### Epics

| # | Epic | GH Issue | Status |
|---|---|---|---|
| 1 | Doc honesty pass | [#294](https://github.com/Ontic-Systems/Gradient/issues/294) | partial — banner/wording PRs landing now |
| 2 | Effect-tier foundation (`!{Heap}`/`!{Stack}`/`!{Static}`/`!{Async}`/`!{Atomic}`/`!{Volatile}`/`!{Throws}`) + `@panic(abort\|unwind\|none)` module attribute — see [ADR 0001](adr/0001-effect-tier-foundation.md) | [#295](https://github.com/Ontic-Systems/Gradient/issues/295) | in progress (ADR 0001 accepted; `!{Heap}` gates heap allocation; `!{Stack}`/`!{Static}` are known marker effects; `@panic(abort\|unwind\|none)` parses + checker rejects integer division/modulo under `@panic(none)` #318) |
| 3 | Capability + arena memory model (typestate caps, arenas, C ABI, `Unsafe` gate on `extern`) — see [ADR 0002](adr/0002-arenas-capabilities.md) | [#296](https://github.com/Ontic-Systems/Gradient/issues/296) | in progress (ADR 0002 accepted; `FFI(C)` audit-trail effect on every `extern fn` landed [#322](https://github.com/Ontic-Systems/Gradient/issues/322) launch tier) |
| 4 | Tiered contracts (runtime + `@verified` SMT + `@runtime_only` opt-out) — see [ADR 0003](adr/0003-tiered-contracts.md) | [#297](https://github.com/Ontic-Systems/Gradient/issues/297) | in progress (ADR 0003 accepted; `@verified` annotation parses + checker recognizes #327; VC generator emits SMT-LIB for the supported subset #328; Z3 subprocess discharger + counterexample diagnostics #329 — opt-in via `GRADIENT_VC_VERIFY=1`; `@runtime_only(off_in_release)` opt-out + release audit #330; stdlib `@verified` pilot expanded to seventeen modules — `core_math.gr` (10 fns / 17 obligations) + `core_bool.gr` (6 fns / 6 obligations) + `core_compare.gr` (10 fns / 11 obligations) + `core_int_ops.gr` (10 fns / 11 obligations) + `core_arith_ops.gr` (10 fns / 10 obligations) + `core_order_ops.gr` (10 fns / 14 obligations) + `core_pair_ops.gr` (10 fns / 17 obligations) + `core_select_ops.gr` (10 fns / 11 obligations) + `core_chain_ops.gr` (10 fns / 10 obligations) + `core_witness_ops.gr` (10 fns / 15 obligations) + `core_parity_ops.gr` (10 fns / 17 obligations) + `core_interval_ops.gr` (10 fns / 21 obligations) + `core_neg_ops.gr` (10 fns / 10 obligations) + `core_inequality_chain_ops.gr` (10 fns / 10 obligations) + `core_assoc_ops.gr` (10 fns / 10 obligations) + `core_swap_ops.gr` (10 fns / 11 obligations) + `core_zero_one_ops.gr` (10 fns / 11 obligations) — discharged on every CI run #331) |
| 5 | Modular runtime (refcount/actors/async/allocator/panic as effect-driven linkable units) | [#298](https://github.com/Ontic-Systems/Gradient/issues/298) | planned |
| 6 | Backend split (Cranelift dev / LLVM release + cross-compile + DWARF) — see [ADR 0004](adr/0004-cranelift-llvm-split.md) | [#299](https://github.com/Ontic-Systems/Gradient/issues/299) | planned (ADR 0004 accepted) |
| 7 | Stdlib `core`/`alloc`/`std` split, effect-gated — see [ADR 0005](adr/0005-stdlib-split.md) | [#300](https://github.com/Ontic-Systems/Gradient/issues/300) | in progress (ADR 0005 accepted; effect-row annotation pass kicked off via [#346](https://github.com/Ontic-Systems/Gradient/issues/346) — wave 1 landed: random number generation + process introspection (`random`, `random_int`, `random_float`, `seed_random`, `process_id`) carry `IO`; allocating string/string-conversion builtins (`int_to_string`, `string_substring`, `string_trim`, `string_to_upper`, `string_to_lower`, `string_replace`, `string_char_at`, `string_append`) carry `Heap`; wave 2 landed: JSON allocators (`json_parse`, `json_stringify`, `json_type`, `json_keys`, `json_get`, `json_array_get`, `json_as_string`, `json_as_int`, `json_as_float`, `json_as_bool`) + remaining string allocators (`string_format`, `string_join`, `string_repeat`, `string_pad_left`, `string_pad_right`, `string_strip`, `string_strip_prefix`, `string_strip_suffix`, `string_reverse`, `string_slice`) + `float_to_string`/`bool_to_string` carry `Heap`; wave 3 landed: container/stringbuilder/iterator allocators (`hashmap_insert`, `hashmap_remove`, `list_iter`, `stringbuilder_append`, `stringbuilder_append_char`, `stringbuilder_append_int`, `stringbuilder_to_string`, `set_add`, `set_remove`, `set_union`, `set_intersection`, `set_to_list`, `queue_new`, `queue_enqueue`, `queue_dequeue`, `queue_peek`) carry `Heap`; wave 4 landed: `to_string(Int)` convenience builtin carries `Heap` — 3 self-hosted call sites (`format_bootstrap_result`, `format_result`, `type_to_string`) updated to declare `!{Heap}` accordingly; **wave 5 (final audit pass) landed**: remaining heap-allocator leaks (`genref_alloc`, `string_to_int`, `string_to_float`, `string_find`, `range_iter`, `iter_next`) carry `Heap`; pure-stays-pure tests pin `json_is_null`, `json_has`, `json_len`, `hashmap_get`, `hashmap_contains`, `hashmap_len`, `set_contains`, `set_size`, `stringbuilder_length`, `stringbuilder_capacity`, `iter_has_next`, `iter_count`, `genref_get`, `string_compare`, `datetime_year` as no-allocation accessors; **#345 scaffold landed (post-#346)**: tier classifier (`StdlibTier { Core, Alloc, Std }`) plus `TypeEnv::lookup_fn_tier` plus migration guide [`docs/stdlib-migration.md`](stdlib-migration.md) — every registered builtin now classifies into a tier derived from its effect row; `#347` (no_std test matrix) and `#348` (`import std` rejection) consume this scaffold next; **#347 software-smoke landed (post-#345)**: `cargo test -p gradient-compiler --test no_std_smoke` lex+parse+type-checks every fixture under `codebase/compiler/tests/no_std_corpus/` (today: arithmetic, control flow, basic no-alloc data structures via tuples + Option/Result pure decomposers) and asserts every fn's effect closure classifies at `core` — the cross-compile target-triple matrix the issue body also names is parked behind E5/E6 per the issue's "Blocked by" line and grows as a parallel matrix when those land; **#348 `@no_std` rejection landed (post-#347)**: parser accepts top-of-file `@no_std` module attribute (no args), checker rejects any call propagating `!{Heap}` / `!{IO}` / `!{FS}` / `!{Net}` / `!{Time}` / `!{Mut}` with a structured diagnostic naming the in-scope function, call site, offending effect, classified tier, declared ceiling, and a fix-it; out-of-axis effects (`Stack`/`Atomic`/`Volatile`/`Async`/`Send`/`Throws(_)`/`FFI(_)`) stay orthogonal — closes the entire user-facing E7 tier story for the launch tier) |
| 8 | Inference engine + `@app`/`@system` modes — see [ADR 0006](adr/0006-inference-modes.md) | [#301](https://github.com/Ontic-Systems/Gradient/issues/301) | planned (ADR 0006 accepted) |
| 9 | Threat model + `@trusted`/`@untrusted` + sigstore-prep + sandbox + fuzz + DDC + reproducible builds | [#302](https://github.com/Ontic-Systems/Gradient/issues/302) | planned |
| 10 | Package registry (sigstore + capability-scoped manifests) — see [ADR 0007](adr/0007-registry-trust.md) | [#303](https://github.com/Ontic-Systems/Gradient/issues/303) | planned (ADR 0007 accepted) |
| 11 | Tooling suite (bench/doc/asm/cross-compile/bindgen/DWARF + plugin spec) | [#304](https://github.com/Ontic-Systems/Gradient/issues/304) | planned |
| 12 | Self-hosting acceleration (body-flips, `main.gr` wrap, trust-gate expansion, public LoC metric) | [#116](https://github.com/Ontic-Systems/Gradient/issues/116) | partial — bootstrap stage active |

### Dependency graph

```
1 (doc honesty) ─────────── independent, ship now
2 (effect tier) ───┬─→ 3 (cap+arena) ──┬─→ 9 (threat) ──→ 10 (registry)
                   ├─→ 4 (contracts)   │
                   ├─→ 5 (runtime)     │
                   ├─→ 7 (stdlib)      │
                   └─→ 8 (inference) ──┘
6 (backend split) ──── parallel, blocks 5 cross-compile only
11 (tooling) ──────── parallel, partial blocked on 6/7
12 (self-host) ──── parallel, dogfoods all above
```

### Current implementation level vs target

| Tier | Today | Target |
|---|---|---|
| Memory | refcount + COW heap, no `no_std` | effect-gated `!{Heap}`/`!{Stack}`/`!{Static}`, arenas, `no_std` via no `!{Heap}` |
| Concurrency | actor runtime (experimental) | actors as `!{Async}+!{Send}`, plus atomics + memory ordering primitives |
| Safety | no borrowck, no lifetime annotations, refcount handles aliasing | arenas + capability tokens (no lifetime annotations) |
| FFI | `extern fn` ungated | C ABI + `Unsafe` capability + `!{FFI}` effect, header gen via `gradient bindgen` |
| Contracts | runtime asserts only | runtime default + `@verified` SMT + `@runtime_only` opt-out |
| Backend | Cranelift only | Cranelift dev / LLVM release, GPU deferred post-1.0 |
| Stdlib | flat builtins | `core` / `alloc` / `std` effect-gated |
| Errors | `Result[T,E]`, no panic strategy doc | `!{Throws(E)}` + `@panic(abort\|unwind\|none)` module attr |
| Runtime | bundled | modular, effect-driven linker DCE |
| Self-host | bootstrap stage, several modules delegate via kernel | ~95% Gradient, Rust kernel small and measured |
| Threat model | `SECURITY.md` stub | full agent-threat-model + `@trusted`/`@untrusted` + sigstore + sandbox + fuzz + DDC |
| Registry | unimplemented | sigstore-signed + capability-scoped manifests |
| Tooling | `build`/`run`/`check`/`test`/`fmt`/`repl`/LSP | + `bench`/`doc`/`asm`/`bindgen`/`--target`/DWARF, plugin spec for fuzz/miri/profile/debugger |

### Sequencing

- Sprint 0: epic 1 (doc honesty) — independent, ship now.
- Sprint 1: epic 2 (effects), epic 6 (backend split), epic 11 (tooling) — all unblocked. Epic 12 (self-host) opportunistic body-flips.
- Sprint 2: epics 3, 4, 5, 7 — need epic 2.
- Sprint 3: epics 8, 9 — need epics 2 and 3.
- Sprint 4: epic 10 — needs epics 3 and 9.

### Public-facing posture

We are building this in public. The repository is public, issues are public, PRs are public, but **active promotion** (HN posts, conference talks, social-media pushes) waits until either (a) the language is real enough to be useful, or (b) a breakthrough requires immediate sharing. Until then, the alpha label, the `STATUS:` banners across `docs/`, and explicit per-feature epic numbers keep the public claim narrower than the internal aspiration.

No dated public gates. Sprints are advisory; CI-green on every PR is the only enforced gate.

### Architecture Decision Records

Significant architectural decisions land as ADRs under [`docs/adr/`](adr/README.md). Seven are accepted: [ADR 0001: Effect-tier foundation](adr/0001-effect-tier-foundation.md) (Epic E2 [#295](https://github.com/Ontic-Systems/Gradient/issues/295)), [ADR 0002: Arenas + capabilities (no lifetime annotations)](adr/0002-arenas-capabilities.md) (Epic E3 [#296](https://github.com/Ontic-Systems/Gradient/issues/296)), [ADR 0003: Tiered contract enforcement](adr/0003-tiered-contracts.md) (Epic E4 [#297](https://github.com/Ontic-Systems/Gradient/issues/297)), [ADR 0004: Cranelift dev / LLVM release backend split](adr/0004-cranelift-llvm-split.md) (Epic E6 [#299](https://github.com/Ontic-Systems/Gradient/issues/299)), [ADR 0005: Stdlib core/alloc/std split with effect gating](adr/0005-stdlib-split.md) (Epic E7 [#300](https://github.com/Ontic-Systems/Gradient/issues/300)), [ADR 0006: Inference engine + @app/@system modes](adr/0006-inference-modes.md) (Epic E8 [#301](https://github.com/Ontic-Systems/Gradient/issues/301)), and [ADR 0007: Registry trust model](adr/0007-registry-trust.md) (Epic E10 [#303](https://github.com/Ontic-Systems/Gradient/issues/303)). Together they anchor Sprint 1 (effects + backend), the capability + arena memory model and tiered contracts Sprint 2 consumes, the stdlib partition and inference engine that follow, and the registry trust model Sprint 4 closes the supply chain on. The pre-allocated ADR-numbering roster is now fully accepted; future ADRs are filed as new architectural decisions arise.
