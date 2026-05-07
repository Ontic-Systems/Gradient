# Effect-system soundness — informal proof sketch

> Issue: [#363](https://github.com/Ontic-Systems/Gradient/issues/363) — closes adversarial finding **F7 (MEDIUM)**.
> Epic: [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (threat model).
> Locked design: [ADR 0001 — Effect-tier foundation](../adr/0001-effect-tier-foundation.md).

This document is an **informal soundness sketch** for Gradient's effect system: the typing rules, the propagation discipline, the soundness theorem (subject reduction + progress), and the open questions whose resolution would lift the sketch to a mechanized proof. It is intentionally human-readable rather than Coq/TLA+; mechanization is tracked as future work in §[Open questions](#open-questions).

The sketch grounds the load-bearing security claim that **a function whose declared effect row excludes `IO` cannot perform IO at runtime**, and the corresponding compositional claims for every other effect in the launch-tier vocabulary.

## 0. Notation

We write:

- `Γ` for the type+effect environment (variables ↦ types, function names ↦ signatures).
- `e` for an expression and `s` for a statement.
- `t` for a value type (`Int`, `Bool`, record/enum, function, etc.).
- `ε` for an effect row — a finite multiset of effect names. We treat it set-theoretically when checking subsumption (`ε ⊆ ε'`) but multiset-theoretically when reasoning about runtime obligations (allocations consume `Heap` once per site).
- `Γ ⊢ e : t ! ε` for "in environment `Γ`, expression `e` has type `t` and may incur effects in `ε`."
- `s ⟶ s'` for one step of the operational semantics; `→*` for the reflexive-transitive closure.

The launch-tier effect vocabulary is fixed by [`KNOWN_EFFECTS`](../../codebase/compiler/src/typechecker/effects.rs):

```
{ IO, Net, FS, Mut, Time, Actor, Async, Send, Atomic, Volatile, Heap, Stack, Static }
```

plus the parameterized family `Throws(E)` where `E` ranges over user-defined error types ([ADR 0001 §"Decision" and #317](../adr/0001-effect-tier-foundation.md)). Effect *variables* (lowercase identifiers like `e`, `eff`) range over multisets of the above.

## 1. Effect rows on signatures

Every function signature carries an explicit effect row:

```gradient
fn read_file(path: String) -> !{IO, FS, Throws(IoError)} String:
    ...
```

Externs default to a conservative ceiling when no row is declared (`EXTERN_DEFAULT_EFFECTS = { IO, Net, FS, Mut, Time }`) — this is the **safe-by-default** axis: extern bodies are unverifiable so the checker treats them as if they may do everything in that ceiling.

Effects can be:

| Class | Examples | Behavior |
|---|---|---|
| **Gating** | `IO`, `Net`, `FS`, `Mut`, `Time`, `Actor`, `Async`, `Send`, `Atomic`, `Heap` | Caller must declare/cap the effect or the call is a type error. |
| **Marker / informational** | `Stack`, `Static`, `Volatile` | Documented and surfaced via the Query API but do not gate (callers are not forced to re-declare). They constrain *implementation* shape (e.g. `Volatile` forbids reordering by codegen) rather than *callability*. |
| **Parameterized** | `Throws(E)` | Acts as gating; matched by exact name (`Throws(ParseError)` ≠ `Throws(IoError)`). The checker validates `E` is a well-formed type identifier ([#317](https://github.com/Ontic-Systems/Gradient/issues/317)). |

The marker/gating split is important: it means effect rows are *not* a single uniform monoid, and a future formalization will partition the row monoid accordingly.

## 2. Typing rules

We give the rules in syntax-directed form. Numbered subscripts (`T-Var`, `T-App`, …) are stable so external references (e.g. issue threads) can name a single rule.

### 2.1 Values

```
                  ─────────────────────  T-Lit
                  Γ ⊢ literal : t ! ∅

         x ↦ t ∈ Γ
         ─────────────────  T-Var
         Γ ⊢ x : t ! ∅
```

Variables and literals incur no effects.

### 2.2 Function abstraction

```
        Γ, x₁:t₁, …, xₙ:tₙ ⊢ body : t ! εᵢ        εᵢ ⊆ ε
        ──────────────────────────────────────────────────  T-Fn
        Γ ⊢ fn(x₁:t₁, …, xₙ:tₙ) -> !ε t : (t₁,…,tₙ) -> !ε t ! ∅
```

Defining a function does not itself perform its effects — the body's inferred effects must be a *subset* of the declared row, but the abstraction is pure. This is the standard "effect rows are latent" property and is the foundation of compositional reasoning.

### 2.3 Function call

```
        Γ ⊢ f : (t₁,…,tₙ) -> !ε t ! ε_f
        Γ ⊢ a₁ : t₁ ! ε₁    …    Γ ⊢ aₙ : tₙ ! εₙ
        ──────────────────────────────────────────────  T-App
        Γ ⊢ f(a₁,…,aₙ) : t ! ε ∪ ε_f ∪ ε₁ ∪ … ∪ εₙ
```

A call's incurred effects are the union of:
- the *declared* effect row of the callee (`ε`),
- the effects required to *produce* the callee value (`ε_f`, almost always `∅` when `f` is a top-level name), and
- the effects required to evaluate each argument (`εᵢ`).

This is the **compositional propagation** axis. There is no per-effect propagation rule — every effect propagates uniformly because it is a row entry.

### 2.4 Allocation

```
        Γ ⊢ e : t ! ε
        ─────────────────────  T-AllocHeap
        Γ ⊢ box(e) : Box[t] ! ε ∪ {Heap}
```

`Heap` is added at the allocation *site*, not at the call site of a constructor. The implementation enforces this in the checker rather than at parse time so that compile-time-folded values do not pay the effect ([#313 / #455](https://github.com/Ontic-Systems/Gradient/issues/313)). `Stack` and `Static` are added analogously by their construction forms but are markers, not gating.

### 2.5 Throw

```
        Γ ⊢ e : E ! ε
        ─────────────────────────  T-Throw
        Γ ⊢ throw e : ⊥ ! ε ∪ {Throws(E)}
```

`throw e` has the bottom type and adds `Throws(E)` to its row, where `E` is the static type of the thrown value. `try`/`catch` removes the matching `Throws(E)` from the row of the protected expression ([ADR 0001 §"Errors" / #317](../adr/0001-effect-tier-foundation.md)).

### 2.6 Sequencing and let

```
        Γ ⊢ e₁ : t₁ ! ε₁    Γ, x:t₁ ⊢ e₂ : t₂ ! ε₂
        ───────────────────────────────────────────  T-Let
        Γ ⊢ let x = e₁ in e₂ : t₂ ! ε₁ ∪ ε₂
```

Nothing is hidden. Every sub-expression's row is unioned into the surrounding context's row.

### 2.7 Subsumption (effect widening)

```
        Γ ⊢ e : t ! ε      ε ⊆ ε'
        ─────────────────────────────  T-Sub
        Γ ⊢ e : t ! ε'
```

A function declared `!{IO, FS}` may be invoked from a caller declared `!{IO, FS, Time}` even though `Time` is unused. **There is no effect *narrowing* admissible** — you cannot type-check a body as `!{IO, FS}` if its inferred effects are `!{IO, FS, Time}`. This is the property that makes effects security-meaningful.

### 2.8 Module-tier ceiling (`@cap`)

A module declared `@cap(IO, Time)` constrains every function in the module to draw from the listed effects only ([#321](https://github.com/Ontic-Systems/Gradient/issues/321), capability typestate engine, will refine this further). The checker verifies that no function declares an effect outside the module ceiling.

## 3. Propagation discipline

Three properties define how effects propagate, and the soundness argument depends on each:

1. **Closed propagation under composition.** If `f` calls `g`, every effect declared by `g` appears in `f`'s declared row (or is consumed by a `try`/`catch`-style construct that explicitly *handles* it). This is enforced by `T-App` plus `T-Sub`. Proof: by induction on the structure of `f`'s body.
2. **No silent extension by externs.** Extern declarations either carry an explicit row or default to `EXTERN_DEFAULT_EFFECTS`. The checker rejects callers that would otherwise pick up implicit effects. Re-confirmed by inspection of `bootstrap_*` registration (`env.rs` pre-registers each kernel surface with its declared row; mod-block redeclarations follow the "first-pass / no-overwrite" discipline of [#262](https://github.com/Ontic-Systems/Gradient/issues/262)).
3. **No effect erasure under generics.** A generic `apply<T, U>(f: T -> !ε U, x: T) -> !ε U` propagates `ε` to its caller by row variable substitution. There is no implicit erasure, so a generic combinator cannot launder effects. (The current implementation makes effect variables explicit in source; effect inference [#350](https://github.com/Ontic-Systems/Gradient/issues/350) will make them inferable but not implicit.)

## 4. Soundness theorem (sketch)

We use the standard Wright–Felleisen formulation: **soundness ≡ subject reduction + progress**.

### 4.1 Subject reduction (preservation)

> **Theorem (Subject Reduction).** If `Γ ⊢ e : t ! ε` and `e ⟶ e'`, then `Γ ⊢ e' : t ! ε'` with `ε' ⊆ ε`.

I.e. evaluation never *grows* the effect row. The new row may be smaller (an `IO`-effecting call has finished and its successor expression no longer carries `IO`), but it cannot include any effect not in `ε`.

Sketch (induction on the derivation of `e ⟶ e'`):

- **β-reduction** (T-App with a known function value): the body's effect row is `≤ ε` by `T-Fn` and `T-App`; argument substitution preserves typing by the standard substitution lemma.
- **Allocation step** (T-AllocHeap reduces `box(v)` to a pointer value): the resulting pointer expression has `ε' = ε \ {Heap}` since the allocation-site obligation has been discharged. `Heap` is in `ε` by `T-AllocHeap` so `ε' ⊆ ε`.
- **Throw / catch step**: `try { … throw e … } catch E => h(e)` reduces to `h(e)`; the `Throws(E)` effect on the `try`-protected sub-expression is not in the resulting expression's row by the standard `try`-removes-`Throws(E)` rule.
- **Sequencing / let**: trivial — by IH the sub-expression's row only shrinks, so the surrounding union shrinks.

### 4.2 Progress

> **Theorem (Progress).** If `∅ ⊢ e : t ! ε` and `e` is closed, then either `e` is a value, or there exists some `e'` with `e ⟶ e'`, *or* `e` is a stuck-at-handler form (a runtime panic, an unhandled `Throws(E)`, or an effect-capability violation that the runtime aborts on).

The `or` clause is necessary because Gradient deliberately allows runtime panics and unhandled-throw aborts as terminal outcomes — they are *runtime* failures, not type-system soundness failures. They preserve the property "every reduction of a well-typed expression either makes progress or terminates with a *typed* failure."

Sketch:

- A well-typed value form is one of literal, function value, allocated pointer, etc.
- A well-typed redex matches one of the operational rules in §4.1; progress on the redex is guaranteed by case analysis.
- A well-typed expression that is not a value and not a redex must be a context with a redex inside (canonical-forms lemma).

### 4.3 Security corollary

> **Corollary (Effect security).** If `∅ ⊢ p : t ! ε` and `IO ∉ ε`, then no execution trace of `p` performs an IO operation.

Proof: by subject reduction, every reachable expression also has effect row `⊆ ε`. By progress, every reduction step either makes progress (and the new row remains `⊆ ε`) or terminates with a typed failure. Therefore no reduction can introduce an `IO` reduction step, since such a step would witness `IO ∈ ε` for some intermediate expression — contradiction.

The same corollary holds, mutatis mutandis, for every gating effect in `KNOWN_EFFECTS`.

The corollary does **not** hold for marker effects (`Stack`, `Static`, `Volatile`) — they do not gate behavior so a function omitting them is not protected from being inlined into a context that uses them. This is by design: markers convey *implementation properties* the optimizer must respect, not *security claims*.

## 5. Worked examples

### 5.1 IO containment

```gradient
fn pure_double(n: Int) -> !{} Int:
    n + n

fn main() -> !{IO} Unit:
    print(pure_double(21))   // OK — Int → Int, no IO from pure_double
    pure_double(print_count) // OK — print_count is from `main`'s row
```

Suppose someone tries:

```gradient
fn pure_double(n: Int) -> !{} Int:
    print(n)                 // type error: caller declared !{} but body needs !{IO}
    n + n
```

The checker rejects the body because the inferred row `!{IO}` is *not* a subset of the declared `!{}` (T-Sub goes one direction only).

### 5.2 Throws containment

```gradient
fn parse_or_default(s: String) -> !{} Int:
    try:
        parse_int(s)         // !{Throws(ParseError)}
    catch ParseError:
        0
```

`parse_or_default`'s declared row is `!{}`. `parse_int` has `!{Throws(ParseError)}`. The `try`/`catch` removes the matching `Throws(ParseError)` from the row of the protected expression, leaving `!{}` — which is `⊆ {}`. Type-checks.

If we forget the `catch`:

```gradient
fn parse_or_default(s: String) -> !{} Int:
    parse_int(s)             // type error: !{Throws(ParseError)} ⊄ !{}
```

The checker rejects.

### 5.3 No-std preservation

```
@cap(Static, Volatile)
mod kernel:
    fn write_register(addr: Int, value: Int) -> !{Static, Volatile} Unit:
        ...
```

A function that calls `box(...)` inside this module raises a checker error because the *inferred* `Heap` is not in the module's `@cap` ceiling. This is the property that makes `no_std` checkable.

## 6. Existing test coverage anchoring this sketch

The trust corpus and the typechecker test suite collectively pin every load-bearing rule:

| Rule | Pinned by |
|---|---|
| T-Sub correctness for `IO` (no narrowing) | `typechecker::effects::tests::*` (general) |
| T-AllocHeap site enforcement | `gradient-heap-effect-allocation-sites` skill anchor + checker effect tests |
| `Throws(E)` validation + propagation | `throws_effect_*` tests added by [#487](https://github.com/Ontic-Systems/Gradient/issues/487) |
| Marker effects do not gate | `volatile_and_atomic_compose` test ([#456 / #458](https://github.com/Ontic-Systems/Gradient/issues/456)) |
| Extern effect-default ceiling | `bootstrap_*` env-registration tests + checker arg-type tests |
| Module `@cap` ceiling | tracked under [#321](https://github.com/Ontic-Systems/Gradient/issues/321) — capability typestate engine |

The trust corpus's bootstrap-subset gate (`bootstrap_trust_checks.rs`) does not exercise effects directly but locks the parser/checker shape that the rules above operate on. Future expansions may add an effect-aware lane.

## 7. Open questions

These are deliberately listed so the sketch's known gaps are visible to anyone consuming this document:

1. **Mechanization.** The sketch is informal. A Coq formalization of the row monoid + subject reduction would be a significant credibility upgrade. TLA+ would be a poor fit (state-machine focus).
2. **Effect inference soundness ([#350](https://github.com/Ontic-Systems/Gradient/issues/350)).** Bidirectional inference will permit elision of effect rows. The inference rules are not in scope here — this sketch only covers the elaborated form.
3. **Capability typestate ([#321](https://github.com/Ontic-Systems/Gradient/issues/321)).** Once typestate caps land, effect rows are no longer the sole gating mechanism. The rules will need to be lifted to a row-monoid × typestate-lattice product.
4. **Generic effect rows.** Effect variables are admitted (lowercase identifiers); the substitution lemma is implicit. A formal proof should make the substitution lemma explicit and verify that every internal use is a sound row substitution.
5. **`Throws(E)` and exception flow.** The current rule treats `Throws(E)` like any other named effect. A more precise treatment would distinguish `Throws(E)` from non-throwable effects (e.g. `Throws(E)` interacts with control flow via `try`/`catch`; `IO` does not).
6. **Async + Send composition.** `Async` and `Send` are gating but compose with each other and with `Atomic`. The composition rules are documented in [ADR 0001 §"Async / Send"](../adr/0001-effect-tier-foundation.md) but the soundness sketch has not yet been extended to `async` reduction (which involves continuations).
7. **Linker DCE invariants ([#298](https://github.com/Ontic-Systems/Gradient/issues/298)).** The modular runtime claims that effects not declared in the program are stripped at link time. This is a *codegen* invariant that depends on effect-system soundness but is not implied by it. Track separately under E5.
8. **Verified-tier interaction ([#297](https://github.com/Ontic-Systems/Gradient/issues/297) / ADR 0003).** `@verified` discharges contract obligations via Z3. Static SMT discharge does not currently reason about effects. A future amendment may add effect-row discharge to the verified tier.

## 8. Status & change-log

| Date | Change |
|---|---|
| 2026-05-07 | Initial sketch, anchoring [F7](https://github.com/Ontic-Systems/Gradient/issues/363). Closes adversarial-finding F7. |

When a rule changes, update this doc *in the same PR* as the implementation change so the soundness story does not drift from the checker.

## 9. Cross-references

- [ADR 0001 — Effect-tier foundation](../adr/0001-effect-tier-foundation.md) — locked design decisions.
- [`codebase/compiler/src/typechecker/effects.rs`](../../codebase/compiler/src/typechecker/effects.rs) — `KNOWN_EFFECTS`, `EXTERN_DEFAULT_EFFECTS`, validation predicates.
- [`codebase/compiler/src/typechecker/checker.rs`](../../codebase/compiler/src/typechecker/checker.rs) — propagation enforcement at use sites.
- Adversarial review 2026-05-02 — finding F7 (this doc closes it).
- Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model umbrella.
