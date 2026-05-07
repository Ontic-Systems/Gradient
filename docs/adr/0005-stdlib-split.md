# ADR 0005: Stdlib core/alloc/std split with effect gating

- Status: Accepted (locked 2026-05-02)
- Deciders: Gradient core (alignment session Q8)
- Epic: [#300](https://github.com/Ontic-Systems/Gradient/issues/300)
- Tracking issue: [#349](https://github.com/Ontic-Systems/Gradient/issues/349)
- Depends on: [ADR 0001 — effect-tier foundation](0001-effect-tier-foundation.md)
- Related: [ADR 0004 — Cranelift dev / LLVM release backend split](0004-cranelift-llvm-split.md)

## Context

Gradient's standard library today is a **flat surface of builtins** registered directly in the typechecker (`codebase/compiler/src/typechecker/env.rs`). Every program gets the same surface — there is no way for an embedded firmware target to ask for "just the parts that don't allocate," and no way for the compiler to reject a `file_read` call inside a module that's supposed to be heap-free.

The Rust ecosystem has shown that this split matters in practice. Rust's `core` / `alloc` / `std` crate hierarchy — and the `#![no_std]` / `extern crate alloc` ergonomics around it — is the canonical model for serving the embedded-to-app spectrum from a single language. Gradient targets the same spectrum (per the agent-native + systems-first generalist position) and needs the equivalent split.

Two implementation paths were considered:

1. **Cargo-style feature flags.** Annotate each builtin with `#[cfg(feature = "alloc")]` / `#[cfg(feature = "std")]`; users set features in `gradient.toml`. Familiar from Rust, but feature flags are an out-of-band channel that doesn't appear in the type system — the checker can't tell you "this fn uses an alloc-only builtin" without re-implementing feature resolution.
2. **Effect-gated split.** Annotate each builtin with the effect row it implies (`!{Heap}` for allocators, `!{IO, FS}` for `file_read`, etc.). The "tier" of stdlib a module needs is then a derived property of its effect closure: a module whose closure includes `!{Heap}` needs `alloc`; a module whose closure includes `!{IO}`/`!{FS}`/`!{Net}`/`!{Time}` needs `std`.

ADR 0001 locked "everything is an effect" as the pattern across memory, concurrency, and errors. Splitting stdlib by effect is the application of that same lock to the library surface — and it removes the need for a parallel feature-flag mechanism that the checker would have to reason about separately.

Q8 of the alignment session locked **effect gating, not feature flags**.

## Decision

The Gradient standard library is partitioned into three tiers — `core`, `alloc`, `std` — and the boundary between tiers is defined by **the effect rows their builtins carry**. There is no separate `--features` mechanism for the stdlib. A module's tier is the smallest tier whose effect surface covers its inferred effect closure.

### Tier definitions

| Tier | Effect contract | Examples (illustrative — concrete list lands with [#346](https://github.com/Ontic-Systems/Gradient/issues/346)) |
|---|---|---|
| `core` | Zero `!{Heap}`, zero `!{IO}`, zero `!{FS}`, zero `!{Net}`, zero `!{Time}`. Pure data manipulation, integer/float math, Option/Result combinators, pure string ops on slices, atomic primitives, comparison + ordering helpers. | `int_add`, `bool_not`, `option_map`, `slice_len`, `result_or_else`, `atomic_load`/`atomic_store`, `mem_compare`, `int_to_string` (write to caller-provided buffer). |
| `alloc` | `core` + `!{Heap}`. Owned containers, heap-backed string formatting, list/map/set, refcounted handles, COW. | `list_new`, `list_push`, `string_concat`, `map_insert`, `format` (returns owned `String`), `box_new`. |
| `std` | `alloc` + `!{IO}` + `!{FS}` + `!{Net}` + `!{Time}` + `!{Mut}` (process-tier mutable state). Anything that touches the operating system, the network, or the wall clock. | `print`, `file_read`, `file_write`, `tcp_connect`, `time_now`, `env_get`, `process_exit`. |

### Module tier inference

A module's tier is **derived**, not declared. The checker:

1. Computes the effect closure of every public symbol in the module (Epic E8 inference, [#350](https://github.com/Ontic-Systems/Gradient/issues/350)).
2. Picks the smallest tier whose effect contract covers that closure.
3. Records the tier on the module's signature.

A `no_std` module is **defined** as a module whose effect closure contains no `!{Heap}` — there is no `#![no_std]`-style attribute. The closure-checking rule does the work. Similarly, an `alloc`-but-no-`std` module is one whose closure contains `!{Heap}` but none of `{IO, FS, Net, Time}`.

### Compiler rejections

The checker rejects, with a structured diagnostic, the following patterns (sub-issue [#348](https://github.com/Ontic-Systems/Gradient/issues/348)):

- A symbol declared `import std::<x>` where `<x>` is in the `std` tier but the importing module's effect closure does not contain a `std`-tier effect. The fix is to (a) actually use the symbol in a way that surfaces the effect, or (b) drop the import.
- A `core`-declared module (a module with an explicit `@core` attribute or one whose declared signature claims `!{}` empty effect row) calling any `alloc`-tier or `std`-tier builtin.
- An `alloc`-declared module calling a `std`-tier builtin.

The diagnostic for each rejection points at the offending call site and names the missing effect — e.g. `error: this call to file_read requires !{IO, FS}; module is declared @core`.

### Migration path

The current flat builtin surface migrates in three sub-issues:

1. **[#346](https://github.com/Ontic-Systems/Gradient/issues/346) — annotate every builtin with effect row.** Every entry in `env.rs::TypeEnv::new ` gets an explicit `effects: vec![...]`. Today, most kernel surfaces are pre-registered with `effects: vec![]` (the ModBlock-ExternFn unblocker from #259); the migration replaces those empty rows with accurate ones. Stdlib builtins (as distinct from kernel surfaces) get the same treatment.
2. **[#345](https://github.com/Ontic-Systems/Gradient/issues/345) — scaffold core/alloc/std crate split.** The Rust-side workspace gains three feature-namespaced modules (or three crates, TBD during scaffold) so that consumers can link against `gradient-core` alone if they choose. The .gr-side surface remains a single import root for now; the tier is checked via the effect contract.
3. **[#347](https://github.com/Ontic-Systems/Gradient/issues/347) — `no_std` test matrix.** CI runs a `no_std` smoke build that compiles a known-pure module with the `core`-only surface and asserts zero `!{Heap}` in the closure. Adds a regression target so future stdlib additions can't silently drag `!{Heap}` into the `core` tier.
4. **[#348](https://github.com/Ontic-Systems/Gradient/issues/348) — `import std` in `no_std` module is a compile error.** Wires the rejection above with a structured diagnostic.

### Self-hosted compiler

The self-hosted compiler (`compiler/*.gr`) lives at the `alloc` tier today (it allocates lists, strings, AST nodes) and likely will land at the `alloc` tier or low `std` tier when the surface is finalized — `compiler/main.gr` reads files, which is `!{FS, IO}`. Sub-issue [#382](https://github.com/Ontic-Systems/Gradient/issues/382) (E2 dogfood) will surface concrete effect rows on the .gr-side modules and make the tier self-evident.

The bootstrap kernel surface (`bootstrap_*` externs) is a separate concern — those are FFI-shaped functions registered in env.rs with `effects: vec![]` and not part of the user-facing stdlib tier system.

## Consequences

### Positive

- **Single mental model.** Tier membership is derived from effect rows, which the checker already tracks per ADR 0001. No new annotation grammar, no parallel feature-flag system.
- **Embedded story works.** A firmware module that never touches `!{Heap}` is automatically `core`-tier and links against the `core` surface only. Linker DCE per Epic E5 can drop the alloc and std crates entirely from the final binary.
- **Diagnostic clarity.** When a `no_std` module accidentally calls `format(...)`, the error names the effect (`!{Heap}`) and the offending call. LLM agents acting on these diagnostics get a structurally simple message.
- **Composes with `@app`/`@system` modes.** Epic E8's `@app` default permits `!{Heap}` and lands at `alloc`-tier minimum; `@system` denies `!{Heap}` and lands at `core`-tier unless explicitly granted. The mode attribute is policy, the effect row is mechanism.
- **Tier is computed, not declared.** No one writes `#![no_std]` and then forgets to update it when they add a `format(...)` call. The closure check catches the drift on the next build.

### Negative

- **Effect annotations on every builtin** are a one-time migration cost (sub-issue [#346](https://github.com/Ontic-Systems/Gradient/issues/346)). Today most stdlib builtins carry the conservative `EXTERN_DEFAULT_EFFECTS = [IO, Net, FS, Mut, Time]` row, which over-reports — the migration reduces noise but requires touching every entry.
- **No explicit `#![no_std]` attribute.** Engineers familiar with Rust may expect to declare intent; instead they look at the inferred tier in the build output. We will surface tier prominently in `gradient build --verbose` and `gradient doc` output to compensate.
- **Tier inference depends on inference engine.** This ADR can land before Epic E8's bidirectional inference is complete, but the `import std in no_std` rejection ([#348](https://github.com/Ontic-Systems/Gradient/issues/348)) needs at least the closure computation working. We accept that [#348](https://github.com/Ontic-Systems/Gradient/issues/348) implementation is ordered after the closure-side of E8.
- **Three tiers may not be enough long-term.** Rust has been wrestling with `core` vs `alloc` boundary cases (formatting, error trait) for years. We accept the same cost; future ADRs may refine the split.

### Neutral / deferred

- **Feature flags for non-stdlib code.** This ADR governs only the standard library surface. Application crates may still use feature flags for compile-time configuration; that's outside the stdlib effect-gating scope.
- **`alloc`-but-no-`std` linker-time DCE.** Epic E5 ([#298](https://github.com/Ontic-Systems/Gradient/issues/298)) handles the actual DCE; this ADR only specifies the effect-tier contract that drives it.
- **`gradient bindgen`-generated externs.** When [#324](https://github.com/Ontic-Systems/Gradient/issues/324) / [#375](https://github.com/Ontic-Systems/Gradient/issues/375) generate Gradient externs from C headers, those externs default to `!{FFI(C), Unsafe}` per Epic E3 — they don't enter the stdlib tier system. A C-binding consumer at `core` tier still works, since `!{FFI(C)}` is orthogonal to the `core`/`alloc`/`std` axis.

## Implementation order

Sub-issues land in this order so each step ships value independently:

1. [#346](https://github.com/Ontic-Systems/Gradient/issues/346) — annotate every builtin with effect row. Foundational; everything below depends on it.
2. [#345](https://github.com/Ontic-Systems/Gradient/issues/345) — scaffold `core`/`alloc`/`std` crate split on the Rust side.
3. [#347](https://github.com/Ontic-Systems/Gradient/issues/347) — `no_std` test matrix: CI gate that builds a known-pure module against `core`-only and asserts no `!{Heap}`.
4. [#348](https://github.com/Ontic-Systems/Gradient/issues/348) — `import std` in `no_std` module = compile error. Final consumer-facing rejection rule.

Each sub-issue includes:

- Update to `env.rs` or the new tier modules.
- At least one self-hosted module dogfood under [#382](https://github.com/Ontic-Systems/Gradient/issues/382).
- README / `docs/` update if user-visible behavior changes (especially [#347](https://github.com/Ontic-Systems/Gradient/issues/347) and [#348](https://github.com/Ontic-Systems/Gradient/issues/348)).

## Related

- Epic E7 [#300](https://github.com/Ontic-Systems/Gradient/issues/300) — this ADR's parent.
- Sub-issues [#345](https://github.com/Ontic-Systems/Gradient/issues/345) – [#348](https://github.com/Ontic-Systems/Gradient/issues/348).
- Epic E2 [#295](https://github.com/Ontic-Systems/Gradient/issues/295) — effects; this ADR depends on `!{Heap}`/`!{IO}`/`!{FS}`/`!{Net}`/`!{Time}` semantics from ADR 0001.
- Epic E5 [#298](https://github.com/Ontic-Systems/Gradient/issues/298) — modular runtime; tier-driven linker DCE consumes the effect rows from this ADR.
- Epic E8 [#301](https://github.com/Ontic-Systems/Gradient/issues/301) — inference engine; the closure computation drives tier derivation.
- Epic E12 [#116](https://github.com/Ontic-Systems/Gradient/issues/116) — self-hosting; `compiler/*.gr` is a real-world tier consumer.
- ADR 0001 — effect-tier foundation; pattern lock that makes this split coherent.
- Roadmap: [`docs/roadmap.md` § Vision Roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02).

## Notes

The Q8 reference is to the alignment-session question that locked this decision. The session log is internal-only; this ADR is the canonical public record.
