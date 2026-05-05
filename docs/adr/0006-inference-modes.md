# ADR 0006: Inference engine + @app/@system modes

- Status: Accepted (locked 2026-05-02)
- Deciders: Gradient core (alignment session Q9)
- Epic: [#301](https://github.com/Ontic-Systems/Gradient/issues/301)
- Tracking issue: [#354](https://github.com/Ontic-Systems/Gradient/issues/354)
- Depends on: [ADR 0001 — effect-tier foundation](0001-effect-tier-foundation.md)
- Related: [ADR 0005 — stdlib core/alloc/std split with effect gating](0005-stdlib-split.md), future ADR 0002 (capabilities)

## Context

ADR 0001 locks "everything is an effect" and ADR 0005 derives stdlib tier from a module's effect closure. Both are precise. Both are noisy.

Without inference, every function declares its full effect row at the signature site. A trivial app-tier helper that builds a string and prints it carries `!{Heap, IO}` in its signature; a recursive arithmetic helper carries `!{}`; a function calling either picks up the union. Annotation grows linearly with the call graph in size and quadratically in the number of distinct effect rows the program touches. For human authors the cost is annoying but bounded; for LLM agents emitting Gradient code, the **token cost is the bottleneck** — every line of body comes with a line of effect annotation, the ratio inverts, and the LLM's reasoning budget gets eaten by syntactic boilerplate it could derive deterministically.

Q9 of the alignment session locked **inference by default** as the agent-ergonomic answer. The same mechanism must serve a second audience — systems-tier authors who want the explicit-everywhere discipline that catches "I accidentally allocated in the kernel" at review time, not at production crash time. A single switch can't satisfy both; a per-module mode attribute can.

The decisions to make:

- What does inference compute, and how does it interact with declared signatures?
- Where does inference stop? (Public API boundaries? Module boundaries? Crate boundaries?)
- What's the default mode, what's the opt-out, and what changes between them?
- How does the user see the inferred result so they can review it?

## Decision

Gradient ships a **bidirectional inference engine** that computes the minimal sound effect row and capability set for every function body, plus a **per-module mode attribute** (`@app` default, `@system` opt-in) that controls whether inference fills in missing annotations or treats them as errors. Public API boundaries always require explicit annotations regardless of mode, matching industry consensus on inference scope.

### What inference computes

For every fn body, the engine computes:

1. **Effect row** — the union of effects from every called fn, every builtin, every awaited expression, every raised error. The minimal row is the smallest effect set such that every call in the body is permitted. If the body is empty (or pure arithmetic with no calls), the row is `!{}`.
2. **Capability set** (Epic E3 / future ADR 0002) — the set of capability tokens the body consumes or borrows. Bidirectional flow tracks consume/borrow states across the body's control flow.
3. **Throws shape** — for `!{Throws(E)}`, the engine infers `E` as the disjoint union of every error type raised in the body that isn't caught locally.

The result is compared against the function's declared signature:

- **Signature declares an effect row.** The declared row must be a superset of the inferred row. If declared is exactly minimal (declared == inferred), the diagnostic is silent. If declared is strictly larger, the checker emits a `note: declared !{...} but body uses only !{...}` (informational; not an error — over-declaration is a user choice, e.g. for forward-compat).
- **Signature omits the effect row.** Behavior depends on the module's mode attribute (see below).

The same rule applies to capability sets and throws shapes.

### Module mode attribute

`@app` and `@system` are mutually exclusive module-tier attributes. They control how the checker handles **omitted** annotations on **non-public** functions:

| Mode | Default? | Behavior on omitted effect row | Behavior on omitted capability set | Public APIs |
|---|---|---|---|---|
| `@app` | Yes | Inferred minimal row used. No diagnostic. | Inferred consume/borrow set used. No diagnostic. | Always explicit (see below). |
| `@system` | Opt-in | Error: `effect row required on @system fn <name>`. The engine still computes the inferred row and includes it in the diagnostic as a fix-it (`hint: try !{Heap}`). | Error: `capability set required on @system fn <name>`. Same fix-it pattern. | Always explicit. |

Mode is set via `@app` or `@system` at the top of the module file (or the `mod <name>:` block). Modules without an explicit mode default to `@app`. Crate-tier or workspace-tier defaults can override via `gradient.toml` once the manifest format ([#365](https://github.com/Ontic-Systems/Gradient/issues/365)) lands; until then, the per-module attribute is the only knob.

### Public-API boundary rule

Inference does NOT cross public-API boundaries. Specifically:

- A `pub fn` declared in either mode **must** carry an explicit effect row, capability set, and throws shape on its signature. Omission is an error in both modes.
- The engine still computes an inferred row for the body; the diagnostic on a `pub fn` with no signature includes the inferred row as a fix-it.
- Cross-module calls **read** the public signature, not the body. The callee's actual effect-row inference is checked against its declared signature internally — what callers see is always the signature.

Rationale: this matches the industry consensus across Rust, OCaml, Haskell, F#, and TypeScript. Inferring across the API boundary makes the public surface fragile (a downstream-invisible refactor can change a public signature) and trades author convenience for downstream debugging cost. Inside a module, inference is safe because the maintainer of the inferred body is the same person who writes the callers.

### Soundness sketch

The inference engine is sound (computed rows are always subsets of any sound declaration) under the following invariants:

1. **Builtins are ground truth.** Every builtin in `env.rs` has an explicit effect row. The inference engine uses that row directly; it does not infer through the FFI boundary.
2. **Cross-module signatures are ground truth.** A call to a `pub fn` in another module uses the declared signature, not the inferred body row.
3. **Recursion uses fixed-point iteration.** Mutually recursive functions are inferred together via the standard worklist algorithm: assume `!{}` for each, iterate until rows stabilize, take the union.
4. **Effect rows are downward-closed under sub-effecting.** If a declaration says `!{Heap, IO}` and the body uses only `!{Heap}`, the body is sound — extra effects on the declaration are conservative over-approximation. This is the same direction as Rust's `Send + Sync` bound widening.

The capability inference (Epic E3) reuses the same fixed-point iteration on a bounded lattice (consume/borrow/none per token); soundness is preserved by the same monotonicity argument. The full proof sketch lands in the implementation PR for [#350](https://github.com/Ontic-Systems/Gradient/issues/350) / [#351](https://github.com/Ontic-Systems/Gradient/issues/351); this ADR commits to it as a goal.

### Surfacing inferred signatures

Sub-issue [#353](https://github.com/Ontic-Systems/Gradient/issues/353) adds a Query API entry that returns the inferred signature for any fn:

```text
gradient-compiler --query inferred-signature --function <name> input.gr
```

The CLI emits the same shape as `gradient-compiler --doc` (existing per #425) — JSON or text. The inferred signature is what the LLM agent or human reviewer sees BEFORE deciding whether to commit to the inferred row or write it explicitly.

The agent-emit corpus that Epic E12 dogfoods ([#382](https://github.com/Ontic-Systems/Gradient/issues/382) / [#383](https://github.com/Ontic-Systems/Gradient/issues/383)) uses this Query API: agent emits a bare fn body, queries the inferred signature, decides whether the row matches intent, then commits.

## Consequences

### Positive

- **Agent-ergonomic.** A typical app-tier function body has zero annotation overhead; the LLM emits the body and lets the engine derive the signature. Token budget recovered.
- **Reviewable.** The inferred row is always queryable — the agent (or a human reviewer) can check what the engine computed without compiling-and-reading-errors.
- **Sound by construction.** Builtins and cross-module signatures are ground truth; recursion uses fixed-point iteration; sub-effecting is monotone. The minimal row is computable in finite time on any well-formed program.
- **Two modes, one mechanism.** `@app` and `@system` differ only in how omitted annotations are handled; they share the inference engine, the diagnostics, and the fix-it format. Switching a module from one to the other is a one-attribute change.
- **Public-API stability.** Downstream callers read declared signatures, not inferred bodies. A maintainer can refactor a `pub fn` body without changing its visible effect row.

### Negative

- **Inference is computation.** Effect-row inference is cheap (linear in body size for the simple lattice), capability inference is more expensive (typestate-flavored, fixed-point on a bounded lattice with multiple states per token). For very large modules the cost is non-trivial. We accept this for two reasons: (a) inference runs in the type-checker phase, which already dominates compile time; (b) incremental compilation can cache per-fn inference across edits.
- **`@system` mode is more annotation than authors are used to.** A systems-tier author has to write effect rows, capability sets, AND throws shapes on every non-pub fn. We mitigate by emitting fix-its with the inferred row in the diagnostic — copy-paste the hint and you're done.
- **Inference can hide intent.** A bug in a fn body that incidentally adds a `!{Heap}` slips into the inferred row silently. We mitigate by surfacing inferred signatures via Query API ([#353](https://github.com/Ontic-Systems/Gradient/issues/353)) and by `gradient doc` output (per #425) including the inferred row in non-pub fn entries.
- **Two paths to specify intent** (declare vs let infer) is a documentation burden. The language guide must explain the modes and the boundary rule clearly.

### Neutral / deferred

- **Crate / workspace mode defaults.** Today the mode is per-module. Once `gradient.toml` ([#365](https://github.com/Ontic-Systems/Gradient/issues/365)) lands, a crate-tier `default-mode = "system"` becomes useful for embedded/firmware projects.
- **Inference for trait methods.** Traits are not yet a focus; when they land, the rule will likely match: `pub trait` methods carry explicit signatures; private trait methods can be inferred. A future ADR formalizes.
- **`@verified`-mode interaction.** ADR 0003 (contracts) governs SMT-verified functions; verification consumes the inferred or declared row directly. No interaction here beyond that.
- **GPU / async-runtime-specific inference.** Deferred until the underlying effects (`!{GPU(_)}`, runtime-specific `!{Async}` flavors) are pinned.

## Implementation order

Sub-issues land in this order so each step ships value independently:

1. [#350](https://github.com/Ontic-Systems/Gradient/issues/350) — bidirectional effect inference. The smaller and simpler half; depends only on Epic E2's effect rows being in place. Includes the recursion fixed-point and the sub-effecting check.
2. [#352](https://github.com/Ontic-Systems/Gradient/issues/352) — `@app` / `@system` module attribute. Adds the mode parser/checker plumbing; behavior is "inference enabled; @system errors on omitted rows."
3. [#353](https://github.com/Ontic-Systems/Gradient/issues/353) — Query API surface for inferred signatures. Unblocks the agent-emit dogfood corpus.
4. [#351](https://github.com/Ontic-Systems/Gradient/issues/351) — bidirectional capability inference. Depends on Epic E3's capability typestate engine being in place.

Each sub-issue includes:

- Checker plumbing (or Query API plumbing for [#353](https://github.com/Ontic-Systems/Gradient/issues/353)).
- Diagnostics with structured fix-it for the `@system` no-row case.
- Self-hosted dogfood under [#382](https://github.com/Ontic-Systems/Gradient/issues/382) / [#383](https://github.com/Ontic-Systems/Gradient/issues/383).
- Test corpus expansion (parser/checker corpora; see existing patterns under `gradient-checker-differential-parity-gate.md`).

## Related

- Epic E8 [#301](https://github.com/Ontic-Systems/Gradient/issues/301) — this ADR's parent.
- Sub-issues [#350](https://github.com/Ontic-Systems/Gradient/issues/350) – [#353](https://github.com/Ontic-Systems/Gradient/issues/353).
- Epic E2 [#295](https://github.com/Ontic-Systems/Gradient/issues/295) — effects; ADR 0001 supplies the rows that inference computes over.
- Epic E3 [#296](https://github.com/Ontic-Systems/Gradient/issues/296) — capabilities; ADR 0002 (planned) supplies the capability lattice that capability inference operates on.
- Epic E7 [#300](https://github.com/Ontic-Systems/Gradient/issues/300) — stdlib; ADR 0005's tier derivation depends on this engine producing accurate closure rows.
- Epic E12 [#116](https://github.com/Ontic-Systems/Gradient/issues/116) — self-hosting; agent-emit corpus consumes the Query API surface from [#353](https://github.com/Ontic-Systems/Gradient/issues/353).
- ADR 0001 — effect-tier foundation; the row vocabulary inference computes over.
- ADR 0005 — stdlib split; tier derivation consumes the inference output.
- Roadmap: [`docs/roadmap.md` § Vision Roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02).

## Notes

The Q9 reference is to the alignment-session question that locked this decision. The session log is internal-only; this ADR is the canonical public record.
