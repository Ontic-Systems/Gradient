# ADR 0001: Effect-tier foundation

- Status: Accepted (locked 2026-05-02)
- Deciders: Gradient core (alignment session Q1–Q19)
- Epic: [#295](https://github.com/Ontic-Systems/Gradient/issues/295)
- Tracking issue: [#319](https://github.com/Ontic-Systems/Gradient/issues/319)
- Related epics: capabilities ([#296](https://github.com/Ontic-Systems/Gradient/issues/296)), contracts ([#297](https://github.com/Ontic-Systems/Gradient/issues/297)), runtime ([#298](https://github.com/Ontic-Systems/Gradient/issues/298)), stdlib ([#300](https://github.com/Ontic-Systems/Gradient/issues/300)), inference ([#301](https://github.com/Ontic-Systems/Gradient/issues/301)), threat model ([#302](https://github.com/Ontic-Systems/Gradient/issues/302))
- Reference: [`gradient-effect-system-workarounds`](../../#) skill (extern function effect cascades)

## Context

Gradient targets the **agent-native + systems-first generalist** position locked in the 2026-05-02 alignment session. The language must be usable by an LLM agent to emit any tier of software, from bare-metal drivers and kernels up through standard application code, without exposing the LLM-hostile failure modes of borrow-checker dialogue or hidden global state.

Three classes of decision sit on the critical path between "useful for apps" and "useful for systems":

1. **Memory tier.** Where data lives — heap, stack, static — and whether allocation is permitted at all (`no_std`).
2. **Concurrency tier.** Whether code may suspend (`async`), atomically observe shared state, or interact with hardware-visible memory (`volatile`).
3. **Error tier.** Whether code may raise typed errors (`Throws(E)`) and what happens on unrecoverable failure (`@panic(abort|unwind|none)`).

Each of these decisions is per-function and must compose: a `no_std` driver writing to a memory-mapped register is `!{Static, Volatile}` and forbidden from `!{Heap}` or `!{Async}`. An app-tier RPC handler is `!{Heap, Async, Throws(RpcError)}`.

We need a single mechanism that:

- Surfaces all three classes uniformly so agents and humans only learn one mental model.
- Composes through call chains without per-feature magic.
- Is machine-readable (effect rows on signatures) and machine-checkable (effect inference + diff against module-tier annotations).
- Forbids a tier mismatch at compile time (no-std cannot call into a `!{Heap}` function).
- Carries through to codegen so the linker can DCE behavior the program never invokes (refcount/actors/async/allocator/panic; see [#298](https://github.com/Ontic-Systems/Gradient/issues/298)).

## Decision

**Everything is an effect.** Memory tier, concurrency tier, error tier, FFI, and trust posture are all expressed as effect rows on function signatures. The single mental model is: "a function lists what it _does_; the checker propagates and forbids what's incompatible."

This ADR formalizes the **effect-tier foundation**: the concurrency/memory/error effects added under Epic [#295](https://github.com/Ontic-Systems/Gradient/issues/295), plus the `@panic` module-tier strategy attribute. It does NOT cover capabilities (Epic [#296](https://github.com/Ontic-Systems/Gradient/issues/296)), contracts (Epic [#297](https://github.com/Ontic-Systems/Gradient/issues/297)), or trust labels (Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302)) — those layer on top of the foundation defined here.

### Effect catalog (Epic E2)

Each row below is implemented as a distinct sub-issue under Epic [#295](https://github.com/Ontic-Systems/Gradient/issues/295). Names use the established `!{...}` syntax already accepted by the parser (see existing `!{IO, Net, FS, Mut, Time}` in `EXTERN_DEFAULT_EFFECTS`).

| Effect | Class | Sub-issue | Semantics | Gates |
|---|---|---|---|---|
| `!{Heap}` | memory | [#313](https://github.com/Ontic-Systems/Gradient/issues/313) | Function may dynamically allocate from the global allocator. Touched by record/list construction, closures that escape, refcount/COW operations. | Required to call any allocating builtin. Forbidden in modules whose `@panic` strategy or capability set excludes the allocator. `no_std` is defined as "no `!{Heap}` in the closure of `main`". |
| `!{Stack}` | memory | [#314](https://github.com/Ontic-Systems/Gradient/issues/314) | Function uses stack-only storage; values do not outlive the call frame. Default for value-only arithmetic / control flow. | Pure leaf; no gating. Marker for inference + linker DCE. |
| `!{Static}` | memory | [#314](https://github.com/Ontic-Systems/Gradient/issues/314) | Function reads or writes module-tier static storage (`@static` items, MMIO-style locations). | Required to access any `@static`. Combines with `!{Volatile}` for hardware registers. |
| `!{Async}` | concurrency | [#315](https://github.com/Ontic-Systems/Gradient/issues/315) | Function may suspend on an awaitable or cross an actor scheduling boundary. Async fns infect callers transitively (per the standard async-effect rules). | Required to `await`. Required by `spawn`/`send`/`ask`. |
| `!{Send}` | concurrency | [#315](https://github.com/Ontic-Systems/Gradient/issues/315) | Function transfers a value/message across a task or actor boundary. Complements capability sendability checks from Epic E3. | Required by actor `spawn`/`send`/`ask`; payload/handle sendability remains a capability/type rule. |
| `!{Atomic}` | concurrency | [#315](https://github.com/Ontic-Systems/Gradient/issues/315) | Function performs an atomic load/store/RMW. Carries an inner ordering parameter (`Relaxed`/`Acquire`/`Release`/`AcqRel`/`SeqCst`). | Required to call any atomic builtin. Composes with `!{Volatile}` only in well-defined hardware-fence patterns. |
| `!{Volatile}` | concurrency | [#316](https://github.com/Ontic-Systems/Gradient/issues/316) | Function performs a volatile load/store — accesses cannot be elided, reordered across the access, or coalesced. Used for MMIO and signal handlers. | Required to access any `@volatile` item. Distinct from `!{Atomic}` — volatile is about elision, atomic is about racing. |
| `!{Throws(E)}` | errors | [#317](https://github.com/Ontic-Systems/Gradient/issues/317) | Function may raise a typed error of kind `E`. Composes through the call chain; `Result[T,E]` desugars to `T !{Throws(E)}` and back at the boundary. | Caller must handle (`try`/`catch`/`?` propagation) or propagate. Module-tier `@panic` strategy decides what `!{Throws(E)}` means at the binary edge. |

### Module-tier panic strategy (sub-issue [#318](https://github.com/Ontic-Systems/Gradient/issues/318))

`@panic(strategy)` is a module attribute (not a per-function effect) because the choice is binary-tier: a unwind-able panic must be supported by the entire link unit's ABI.

| Strategy | Semantics | Use case |
|---|---|---|
| `@panic(abort)` | Panic terminates the process immediately via `abort `. Smallest binary, no unwind tables, no `catch_unwind`. | `no_std` firmware, kernels, FFI consumers that cannot tolerate Rust-style unwinding. |
| `@panic(unwind)` | Panic unwinds through frames, runs destructors, and is catchable at thread/actor boundaries. Default for app-tier code. | Standard apps where a single fault should not bring down a multi-task process. |
| `@panic(none)` | Panics are statically forbidden — any code path that could panic is a checker error. | Hard real-time / safety-critical modules. Composes with `@verified` (Epic E4) for full coverage. |

A module's `@panic` strategy interacts with `!{Throws(E)}`:

- `@panic(abort)`: an unhandled `!{Throws(E)}` at the binary boundary is a CI-time error (no implicit `expect `).
- `@panic(unwind)`: unhandled `!{Throws(E)}` desugars to a runtime panic at the boundary.
- `@panic(none)`: `!{Throws(E)}` is forbidden at the boundary; all errors must be handled or reified as `Result[T,E]` returns.

### Forbidden combinations

The checker rejects these at the call site:

- `!{Heap}` callee inside a `no_std` module (which by definition has zero `!{Heap}` in its `main` closure).
- `!{Async}` callee from a non-`!{Async}` caller without an explicit blocking-bridge primitive.
- `!{Send}` actor/task transfer from a caller whose effect row does not permit cross-boundary sends.
- `!{Volatile}` access without a corresponding `@volatile` annotation on the storage.
- `!{Throws(E)}` callee inside `@panic(none)` module without a handler in scope.

### Inference defaults (handed off to Epic E8)

The bidirectional inference engine ([#350](https://github.com/Ontic-Systems/Gradient/issues/350)) computes the minimal effect row for each fn body and compares against the declared signature.

- `@app` modules: inference defaults to `!{Heap, Async, Throws(_)}` permitted; explicit denial via `!{}` row.
- `@system` modules: inference defaults to **deny `!{Heap}`**; explicit grant required. `!{Stack, Static, Volatile, Atomic}` permitted by default.

## Consequences

### Positive

- **Single mental model.** Memory, concurrency, errors all surface as effect rows. An agent learns one annotation grammar.
- **Composability.** Effects propagate through the call graph; the checker tells you exactly which line forces an effect into the row.
- **Linker DCE alignment.** Epic E5 ([#298](https://github.com/Ontic-Systems/Gradient/issues/298)) splits the runtime into refcount / actors / async / allocator / panic crates, each gated on the corresponding effect. A program that never uses `!{Async}` does not link the async runtime.
- **`no_std` is definable, not declared.** A module is `no_std` iff the closure of its `main` carries no `!{Heap}`. No special keyword; the existing rule does the work.
- **Agent-friendly errors.** When the checker rejects a call, it points at the missing effect — a structurally simpler diagnostic than borrow-checker output, and one that LLMs can act on without dialogue.

### Negative

- **More noise on signatures** for code that touches multiple effect classes. Mitigated by inference (E8) plus `@app`/`@system` mode defaults: in `@app` mode you rarely write `!{Heap, Async}` — it's inferred.
- **Backwards-incompatible** with existing `compiler/*.gr` and `examples/` once the checker enforces the new effects. Migration is staged: each effect lands behind a checker flag, dogfooded across `compiler/*.gr` (sub-issues [#382](https://github.com/Ontic-Systems/Gradient/issues/382), [#383](https://github.com/Ontic-Systems/Gradient/issues/383)), then the flag flips on by default.
- **`!{Throws(E)}` is not Rust's `Result`.** The desugaring is real — agents must understand both shapes — but it follows the OCaml/Koka tradition rather than introducing a novel construct.

### Neutral / deferred

- **GPU effect (`!{GPU(_)}`).** Deferred post-1.0 per Q7. A future ADR may extend this catalog.
- **Capability typestate.** Lives in Epic E3 ([#296](https://github.com/Ontic-Systems/Gradient/issues/296)) and ADR 0002. Effects answer "what does this fn do"; capabilities answer "what tokens does this fn need". The two compose but are decided separately.
- **Trust labels (`@trusted`/`@untrusted`).** Live in Epic E9 ([#302](https://github.com/Ontic-Systems/Gradient/issues/302)) and ADR (TBD). Trust answers "should we believe the source"; effects answer "what does the source say it does."

## Implementation order

Sub-issues land in this order so each adds a checker rule and at least one dogfooded use:

1. [#313](https://github.com/Ontic-Systems/Gradient/issues/313) `!{Heap}` — touches the most code, shake out checker plumbing first.
2. [#314](https://github.com/Ontic-Systems/Gradient/issues/314) `!{Stack}` + `!{Static}` — adds the marker effects that don't gate but inform inference.
3. [#317](https://github.com/Ontic-Systems/Gradient/issues/317) `!{Throws(E)}` — error tier first because every existing fn that returns `Result[T,E]` migrates trivially.
4. [#318](https://github.com/Ontic-Systems/Gradient/issues/318) `@panic(abort|unwind|none)` — module attribute, depends on `!{Throws(E)}` for the `@panic(none)` rule.
5. [#315](https://github.com/Ontic-Systems/Gradient/issues/315) `!{Async}` + `!{Atomic}` — concurrency tier; depends on the actor runtime split (Epic E5) for clean codegen.
6. [#316](https://github.com/Ontic-Systems/Gradient/issues/316) `!{Volatile}` + memory ordering primitives — last because it interacts with `!{Atomic}` rules.

Each sub-issue includes:

- Parser support if new syntax is required (most reuse the existing `!{...}` grammar).
- Checker rule + diagnostic with the canonical "missing effect on signature" error.
- Codegen path (no-op for marker effects; real lowering for `!{Async}`, `!{Send}`, `!{Atomic}`, `!{Volatile}`, `!{Throws(E)}`).
- At least one stdlib annotation update.
- At least one self-hosted module dogfood under [#382](https://github.com/Ontic-Systems/Gradient/issues/382).

## Related

- Epic E2 [#295](https://github.com/Ontic-Systems/Gradient/issues/295) — this ADR's parent.
- Epic E3 [#296](https://github.com/Ontic-Systems/Gradient/issues/296) — capabilities; consumes effect rows.
- Epic E4 [#297](https://github.com/Ontic-Systems/Gradient/issues/297) — contracts; `@panic(none)` integrates with `@verified`.
- Epic E5 [#298](https://github.com/Ontic-Systems/Gradient/issues/298) — modular runtime; effect rows drive linker DCE.
- Epic E7 [#300](https://github.com/Ontic-Systems/Gradient/issues/300) — stdlib core/alloc/std split; `!{Heap}` is the gate.
- Epic E8 [#301](https://github.com/Ontic-Systems/Gradient/issues/301) — inference engine; computes minimal effect rows.
- Epic E12 [#116](https://github.com/Ontic-Systems/Gradient/issues/116) — self-hosting; dogfoods every effect.
- Roadmap: [`docs/roadmap.md` § Vision Roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02).

## Notes

The Q-numbered decisions referenced inline (Q2, Q3, Q7, Q14) are the alignment-session questions that locked each tier. The session log is internal-only; this ADR is the canonical public record of what was decided and why.
