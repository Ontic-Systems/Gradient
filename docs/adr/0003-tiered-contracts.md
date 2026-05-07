# ADR 0003: Tiered contract enforcement

- Status: Accepted (locked 2026-05-02)
- Deciders: Gradient core (alignment session Q5)
- Epic: [#297](https://github.com/Ontic-Systems/Gradient/issues/297)
- Tracking issue: [#332](https://github.com/Ontic-Systems/Gradient/issues/332)
- Related ADRs: [ADR 0001](0001-effect-tier-foundation.md), [ADR 0002](0002-arenas-capabilities.md), [ADR 0006](0006-inference-modes.md)
- Related epics: effects ([#295](https://github.com/Ontic-Systems/Gradient/issues/295)), capabilities ([#296](https://github.com/Ontic-Systems/Gradient/issues/296)), threat model ([#302](https://github.com/Ontic-Systems/Gradient/issues/302))

## Context

Today's `@requires` / `@ensures` contracts are enforced at runtime: the compiler inserts an assertion at function entry (precondition) and another at function exit (postcondition), each producing a structured contract-violation diagnostic on failure. This is honest for app-tier code and pairs well with the agent generate-verify workflow already documented in [`docs/agent-integration.md`](../agent-integration.md). It is not honest for two boundary cases that the 2026-05-02 alignment session locked into the project's positioning:

1. **The "compiler-VERIFIED" claim.** An earlier review finding flagged that earlier marketing language conflated "runtime-enforced" with "compiler-verified". Sprint 0 closed the language drift (banner, tagline, README reframe), but the underlying capability gap remains: there is no path today by which the compiler statically discharges a contract. Until there is, the verified tier cannot be claimed at all.
2. **Systems and embedded code paths.** A `no_std` firmware module cannot afford the runtime cost of an assertion on every fn entry, and may not have a panic strategy that supports unwind on failure. On hardware-bound paths the only honest options are *prove the contract holds at compile time* or *carry no contract at all*. There is no middle ground that respects the binary-tier constraint.

There is also a third, lower-stakes case that needs an explicit answer:

3. **App-tier release builds where every cycle counts.** A web-server hot path may want to disable runtime contract checks in release builds for measured throughput while keeping them on in debug. Today this is implicit (the compiler always inserts the checks); we need an explicit opt-out with an audit trail so a release build's contract posture is machine-readable rather than guessed at.

We need a single mechanism that:

- Surfaces three distinct enforcement tiers as a function-tier annotation (not a global compiler flag), so the contract posture travels with the code.
- Defaults to runtime enforcement (today's behavior — preserve compatibility).
- Adds a `@verified` tier that emits proof obligations to an SMT backend (Z3) and rejects compilation if any obligation is unmet.
- Adds a `@runtime_only` opt-out with a release-build audit warning, so dropping the runtime check is an explicit decision, not an accident.
- Forbids runtime contracts on `no_std` paths (verified-or-none).
- Composes with [ADR 0001](0001-effect-tier-foundation.md): `!{Throws(E)}` is the effect that propagates a contract violation in `@panic(unwind)` mode; `@panic(none)` modules cannot use runtime contracts at all.

## Decision

**Contracts have three enforcement tiers, chosen per-function (or per-module-default), and visible on the signature.** The default is the current runtime behavior; the other two tiers are explicit opt-ins. The verified tier delivers the static-verification path that addresses the related finding and gives `no_std` paths a way to carry contracts at all. The runtime-only-with-release-opt-out tier gives app-tier release builds a measured, audited way to drop the assertion cost.

| Tier | Annotation | Behavior | Cost | Use case |
|---|---|---|---|---|
| Runtime (default) | `@requires` / `@ensures` (no extra annotation) | Compiler inserts entry/exit assertions; failure raises a contract-violation diagnostic. | Per-call assertion cost. | App-tier code, default. Backwards-compatible with today. |
| Verified | `@verified` + `@requires` / `@ensures` | Compiler emits SMT-LIB proof obligations to Z3. Compilation fails on unmet obligations; counterexamples become structured diagnostics. No runtime check inserted. | Compile-time SMT cost; zero runtime cost. | `no_std` and embedded paths, safety-critical modules, every claim that needs the "compiler-verified" label. |
| Runtime-only opt-out | `@runtime_only(off_in_release)` + `@requires` / `@ensures` | Runtime check inserted in debug builds; elided in release builds. Release elision emits an audit warning at build time + records the elided contracts in the build manifest. | Debug only. Release: zero runtime cost, audit trail. | Hot-path code where the contract is documentation but not a runtime guard in production. |

### `@verified` annotation (sub-issue [#327](https://github.com/Ontic-Systems/Gradient/issues/327))

`@verified` is a function attribute that declares the function's contracts must be statically discharged. The checker rejects a `@verified` function under any of these conditions:

- A `@requires` or `@ensures` predicate references a name the SMT translator cannot model (e.g. an unmodeled `extern fn` call inside the predicate, an unmodeled trait method).
- The function body's effect row exceeds what the verified tier can model (`!{IO}`, `!{Net}`, `!{FS}`, `!{FFI(_)}` are all unmodeled — the contract cannot reason about external state).
- The VC generator emits an obligation Z3 cannot discharge within the configured timeout (default 30s; tunable per-module via `@verified(timeout = "60s")`).

Surface syntax (locked):

```gradient
@verified
@requires(n >= 0)
@ensures(result >= 0 and result <= n)
fn clamp_nonneg(n: Int) -> Int {
 if n >= 0 { n } else { 0 }
}
```

### VC generator (sub-issue [#328](https://github.com/Ontic-Systems/Gradient/issues/328))

The verification-condition generator translates a `@verified` function body to an SMT-LIB problem:

- Function parameters → SMT-LIB free variables.
- `@requires` predicates → assumptions (`(assert ...)`).
- Function body (typechecked AST) → SMT-LIB term in the relevant theory (linear-integer arithmetic for `Int`, bit-vectors for fixed-width ints, arrays for `List` indexing within bounds, uninterpreted functions for opaque user types).
- `@ensures` predicate, with `result` substituted for the body's return term → goal (`(assert (not <ensures>))`).
- Z3 `(check-sat)` must return `unsat` for the obligation to be discharged.

Effect-row constraints carry through: a `@verified` function may only call other `@verified` functions or pure non-allocating leaves. Calls to `@runtime_only` or untyped `extern fn` are rejected at the verifier boundary. This is mechanical: the modular runtime split (Epic E5, [#298](https://github.com/Ontic-Systems/Gradient/issues/298)) already separates allocator/async/IO into linkable units; the verifier consumes the same effect-row metadata to decide what is in-scope.

### Z3 integration + counterexample translation (sub-issue [#329](https://github.com/Ontic-Systems/Gradient/issues/329))

The compiler invokes Z3 as a subprocess (initial implementation; in-process binding is a follow-on optimization). On `unsat`: discharge the obligation, no diagnostic. On `sat`: parse the counterexample model, translate each variable assignment back to source-tier names, and emit a structured `ContractCounterexample` diagnostic with:

- The source span of the failing `@requires` / `@ensures`.
- The minimal binding of free variables that violates the predicate.
- A Gradient-syntax expression evaluating each binding so the agent or human can paste it into a test.

On `unknown` (timeout, theory limit): emit a structured `ContractUnknown` diagnostic distinguishing it from a verified failure. `@verified(strict = true)` rejects on `unknown`; default is to warn-and-pass-through with the contract retained as runtime check.

### `@runtime_only` opt-out + release audit (sub-issue [#330](https://github.com/Ontic-Systems/Gradient/issues/330))

`@runtime_only` is the explicit opt-out from `@verified`. It accepts one option:

- `@runtime_only` — runtime check inserted in all builds (default semantics; equivalent to today's behavior).
- `@runtime_only(off_in_release)` — runtime check inserted in debug; elided in release. Build emits a warning per elided contract, and the elided contracts are listed in the build manifest under `[contracts.elided_in_release]`.

The release-elision audit trail is consumed by:

- The package registry's manifest (Epic E10, [#303](https://github.com/Ontic-Systems/Gradient/issues/303)) — packages declaring elided contracts surface that fact at install time.
- The threat model (Epic E9, [#302](https://github.com/Ontic-Systems/Gradient/issues/302)) — `@untrusted` modules are forbidden from using `off_in_release` (the audit trail itself is the trust gate).

`@runtime_only` without arguments is allowed and is the no-op annotation that explicitly documents "runtime tier, on in all builds." Useful when a module-tier default is `@verified` but a particular function opts back to runtime.

### `no_std` rule: verified-or-none

A module that declares (or is inferred to be) `no_std` — defined per [ADR 0001](0001-effect-tier-foundation.md) as "no `!{Heap}` in the closure of `main`" — has only two valid contract tiers per function:

- `@verified` — discharged at compile time. Zero runtime cost, no panic dependency.
- No contract annotation at all.

Any function carrying `@requires` / `@ensures` without `@verified` in a `no_std` module is a checker error. The diagnostic points the agent at either adding `@verified` (and accepting that the predicate must be SMT-discharged) or removing the contract.

This composes with `@panic(none)` from [ADR 0001](0001-effect-tier-foundation.md): `@panic(none)` modules also cannot carry runtime contracts (a runtime contract violation requires a panic to abort), so the only contract tier they accept is `@verified`. The two rules align.

### Module-tier default

A module may declare a default contract tier via `@contracts(<tier>)`:

```gradient
@contracts(verified) // every fn in this module is @verified by default
mod safety_critical { .. }
```

Per-function annotations override the module default. The module-tier default is itself audited: `@contracts(runtime_only(off_in_release))` records every contract in the module under the registry manifest's elided-contracts list.

### Stdlib pilot (sub-issue [#331](https://github.com/Ontic-Systems/Gradient/issues/331))

At least one stdlib function ships under `@verified` to dogfood the toolchain end-to-end and prove the verified tier is real. Recommended target: a small total function with a tractable predicate — `core::math::abs`, `core::list::index_or_default`, or `core::option::unwrap_or` are good candidates. The pilot's role is to exercise the parser → AST → checker → VC generator → Z3 → counterexample-or-discharge round trip on real stdlib code, not to verify the most ambitious predicate available.

## Consequences

### Positive

- **`compiler-VERIFIED` becomes claimable.** Once `@verified` lands and at least one stdlib pilot is green, the project can honestly say a non-trivial subset is statically discharged. The marketing language stays in lockstep with the implementation — no marketing-language regression.
- **`no_std` paths can carry contracts at all.** Today they cannot (runtime check requires panic + heap). Verified-or-none gives them an honest answer.
- **Audit trail for release-elided contracts.** A package manifest declaring `off_in_release` contracts is machine-readable; the registry (Epic E10) and threat model (Epic E9) consume it directly.
- **Composes with effects + capabilities.** The verifier rejects functions whose effect row exceeds what it can model, which is the same propagation rule [ADR 0001](0001-effect-tier-foundation.md) and [ADR 0002](0002-arenas-capabilities.md) already use. One mental model, three layers.
- **Agent-friendly counterexamples.** Z3's model output, translated back to Gradient syntax, gives an agent a concrete failing input it can paste into a test. Structurally simpler than "contract violation at runtime, here's a stack trace."

### Negative

- **Verified tier is gated on a Z3 subprocess.** Build time grows by the SMT solving cost. Mitigated by per-function caching (the obligation is a function of the body + predicates; cache invalidation is straightforward) and by `unknown`-on-timeout behavior that does not block the build by default.
- **Predicate language is bounded.** Not every honest postcondition is expressible in linear-integer arithmetic + arrays + uninterpreted functions. Functions whose predicates touch unbounded recursion, the heap, or external state cannot be `@verified` — they must stay `@runtime_only`. This is honest (predicate language matches the solver's capability) but it is a real surface limitation.
- **Three tiers is more surface than one.** Mitigated by inference: the default tier is the current behavior, so no existing code changes. `@verified` and `@runtime_only(off_in_release)` are opt-ins; an unannotated function continues to compile exactly as today.
- **`unknown` is a real failure mode.** Z3 can time out or hit a theory limit on a predicate that would have discharged with more time. The default policy is warn-and-pass-through (the contract becomes a runtime check); strict mode is opt-in. Agents need to learn which mode they are in.

### Neutral / deferred

- **Solvers beyond Z3.** CVC5, Yices, and others may be useful for specific theories. Locked out of the launch set; the integration is one-solver-at-a-time.
- **Refinement types as predicates.** A future ADR may add `Int { x | x >= 0 }`-style refinement types that the verifier consumes as built-in `@requires`. Out of scope for the launch tier — `@requires(n >= 0)` is the surface today.
- **Loop invariants and termination.** The launch verifier handles straight-line code and structurally-recursive code with a measure parameter. Free-form loops with invariant annotations are deferred; a function with an unannotated unbounded loop is unverifiable and must be `@runtime_only`.
- **Cross-package verification.** A `@verified` function calling into another package's `@verified` function works only if both packages share the predicate language. The registry manifest (Epic E10) carries the per-package predicate-language declaration; cross-package verified calls are bounded by that declaration.

## Implementation order

Sub-issues land in this order so each adds checker plumbing + at least one observable test:

1. [#327](https://github.com/Ontic-Systems/Gradient/issues/327) `@verified` annotation parses + integrates with the AST/checker. Initially: the annotation is recognized but emits an "unimplemented; falls back to runtime" warning. Establishes the surface syntax + the rejection rules for unmodelable bodies.
2. [#328](https://github.com/Ontic-Systems/Gradient/issues/328) VC generator — body to SMT-LIB. Internal-only output for now; the verifier writes the `.smt2` file but does not yet invoke Z3. Adds golden-file tests over a small body corpus.
3. [#329](https://github.com/Ontic-Systems/Gradient/issues/329) Z3 integration + counterexample → `ContractCounterexample` diagnostic. End-to-end: the `clamp_nonneg` example above must verify; a deliberately wrong version must produce a counterexample mapping back to source-tier names.
4. [#330](https://github.com/Ontic-Systems/Gradient/issues/330) `@runtime_only(off_in_release)` opt-out + audit warning. Includes the build-manifest `[contracts.elided_in_release]` section. The audit trail is the load-bearing piece for E9/E10.
5. [#331](https://github.com/Ontic-Systems/Gradient/issues/331) stdlib pilot — verify one function with `@verified`. Exercises the full round-trip and locks the verified tier as real.

Each sub-issue includes:

- Parser support for new syntax (`@verified`, `@runtime_only`, `@contracts`).
- Checker rule + diagnostic with the canonical "unmodelable predicate" / "verified body exceeds effect row" / "no_std module carries runtime contract" errors.
- Build-manifest format extension where applicable ([#330](https://github.com/Ontic-Systems/Gradient/issues/330)).
- At least one stdlib annotation update or example.
- At least one self-hosted module update (after [#331](https://github.com/Ontic-Systems/Gradient/issues/331) lands, a `compiler/*.gr` function gets a `@verified` annotation as a dogfood).

## Comparison

### vs Rust `assert!` / `debug_assert!`

| Dimension | Rust today | Gradient (this ADR) |
|---|---|---|
| Default | `assert!` always on; `debug_assert!` debug-only | runtime tier always on (= `assert!`); `@runtime_only(off_in_release)` opt-out (= `debug_assert!`) |
| Static verification | none in core; `kani` / `prusti` are external | `@verified` first-class, Z3-discharged, in the compiler |
| Audit trail | none — `debug_assert!` elision is silent | `[contracts.elided_in_release]` build-manifest section + per-elision warning |
| `no_std` | both `assert!` variants supported (no panic-strategy interaction) | runtime tier forbidden in `no_std`; `@verified` is the only option |

### vs Dafny / F* / Lean

These tools deliver static verification today and inspire the verified tier here. The trade vs them is scope: Dafny / F* / Lean ask the developer to think in the verifier's predicate language from day one. Gradient ships runtime tier as the default, opt-in to verified, so app-tier code does not pay the cognitive cost. The verified tier is intentionally narrower than Dafny — linear arithmetic + arrays + UFs at launch, not the full higher-order logic Dafny supports — because the goal is "agents can emit verified code", not "researchers can express arbitrary specifications".

### vs runtime-only-everywhere

Pure runtime enforcement is the current behavior and the path of least resistance. It does not address the related finding (no static verification claim), it does not fit `no_std` (panic + heap dependency), and it does not give release-elided contracts an audit trail. We keep it as the default because backward compatibility and pedagogical simplicity matter; we add the other two tiers because the gaps are real.

## Related

- [ADR 0001](0001-effect-tier-foundation.md) — `!{Throws(E)}` and `@panic(strategy)` interact directly with the runtime tier; `@panic(none)` allows only `@verified`.
- [ADR 0002](0002-arenas-capabilities.md) — capability-bound calls inside a `@verified` body must hold their capabilities through the predicate; the verifier treats capability use as an effect-row obligation.
- [ADR 0006](0006-inference-modes.md) — `@app` defaults to runtime tier; `@system` modules infer toward `@verified`-or-none for any function in a `no_std` closure.
- Epic E2 [#295](https://github.com/Ontic-Systems/Gradient/issues/295) — effects, blocks this ADR's effect-row rejection rules.
- Epic E4 [#297](https://github.com/Ontic-Systems/Gradient/issues/297) — this ADR's parent.
- Epic E5 [#298](https://github.com/Ontic-Systems/Gradient/issues/298) — modular runtime; the verifier consumes the same effect-row metadata used to gate runtime DCE.
- Epic E9 [#302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model; `@untrusted` modules are forbidden from `off_in_release` elision.
- Epic E10 [#303](https://github.com/Ontic-Systems/Gradient/issues/303) — registry manifest carries the predicate-language declaration for cross-package verified calls and the elided-contract audit trail.
- [`docs/agent-integration.md`](../agent-integration.md) — `## Design-by-Contract for Agents` section; this ADR is the canonical record for the verified tier surfaced there.
- An earlier review finding (the "compiler-VERIFIED" claim) — closed by Sprint 0 marketing fixes; the implementation gap is closed by this ADR + sub-issues #327–#331.
- An earlier review finding (runtime contracts skippable in release with no audit trail) — closed by [#330](https://github.com/Ontic-Systems/Gradient/issues/330).
- Roadmap: [`docs/roadmap.md` § Vision Roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02).

## Notes

The Q5 alignment-session decision locked tiered enforcement (runtime + verified + runtime-only opt-out). The session log is internal-only; this ADR is the canonical public record.

These adversarial-review findings are addressed by this ADR + its tracked sub-issues (#327–#331). the related finding is closed at the implementation tier when [#331](https://github.com/Ontic-Systems/Gradient/issues/331) lands a verified stdlib function; the related finding is closed when [#330](https://github.com/Ontic-Systems/Gradient/issues/330) lands the audit warning + build-manifest section.
