# Stdlib Migration Guide — `core` / `alloc` / `std` Tier Split

> **Status**: Scaffold landed in [#345](https://github.com/Ontic-Systems/Gradient/issues/345).
> Locked decision: [ADR 0005 — Stdlib core/alloc/std split with effect gating](adr/0005-stdlib-split.md).
> Parent epic: [#300](https://github.com/Ontic-Systems/Gradient/issues/300).

This guide explains how the Gradient standard library is partitioned into
three tiers — `core`, `alloc`, `std` — and what existing code needs to do
to opt into the partition. **Existing code keeps compiling unchanged.** The
scaffold landed by #345 introduces the **classification machinery**; the
user-facing rejection rules land in follow-on sub-issues.

## TL;DR

- The stdlib is split into three tiers. Tier membership is **derived from
  the function's effect row**, not from a `--features` flag, not from a
  `#![no_std]` attribute, and not from a per-module declaration.
- Today (post-#345): every registered builtin can be classified into a
  tier via `TypeEnv::lookup_fn_tier` or
  `typechecker::stdlib_tier::classify_effects`. **Nothing rejects yet.**
- After [#347](https://github.com/Ontic-Systems/Gradient/issues/347) (**LANDED**):
  CI runs a `no_std` smoke target (`cargo test -p gradient-compiler --test
  no_std_smoke`) that lex+parse+type-checks every `.gr` fixture under
  `codebase/compiler/tests/no_std_corpus/` and asserts every function's
  effect closure classifies at `core`. Today's fixtures cover arithmetic,
  control flow, and basic no-alloc data structures (tuples + Option/Result
  pure decomposers). The cross-compile target-triple matrix the issue body
  also names (`x86_64-unknown-none`, `arm-none-eabi`, `riscv32imac-unknown-none-elf`)
  is parked behind E5 (modular runtime split) and E6 (cross-compile backend
  split) per the issue's "Blocked by" line; when E5/E6 land, this smoke
  test grows a parallel cross-compile matrix.
- After [#348](https://github.com/Ontic-Systems/Gradient/issues/348) (**LANDED**):
  the parser accepts a top-of-file `@no_std` module attribute (no args,
  declared next to `@trusted` / `@untrusted` / `@panic(...)`). When set,
  the checker rejects any call that requires `!{Heap}`, `!{IO}`, `!{FS}`,
  `!{Net}`, `!{Time}`, or `!{Mut}` with a structured diagnostic naming
  the in-scope function, the call site, the offending effect, the
  resulting tier, and the declared ceiling. Out-of-axis effects
  (`!{Stack}` / `!{Atomic}` / `!{Volatile}` / `!{Async}` / `!{Send}` /
  `!{Throws(_)}` / `!{FFI(_)}`) stay orthogonal and never trip the
  ceiling.

## The three tiers

| Tier | Effect contract | Examples |
|---|---|---|
| `core` | No `!{Heap}`, `!{IO}`, `!{FS}`, `!{Net}`, `!{Time}`, `!{Mut}` | `int_add`, `bool_not`, `option_is_some`, `iter_has_next`, `pi`, `sin`, `string_compare`, `datetime_year`, `hashmap_len`, atomic primitives. |
| `alloc` | `core` + `!{Heap}` | `string_to_int`, `string_to_float`, `string_find`, `range_iter`, `int_to_string`, list/map/set/queue allocators, `format`-shaped builders. |
| `std` | `alloc` + `!{IO}` + `!{FS}` + `!{Net}` + `!{Time}` + `!{Mut}` | `print`, `file_read`, `file_write`, `tcp_connect`, `time_now`, `env_get`, `process_exit`. |

## How classification works

Every builtin in `codebase/compiler/src/typechecker/env.rs` carries an
explicit `effects: vec![...]` row. ([#346](https://github.com/Ontic-Systems/Gradient/issues/346)
closed the audit pass that made the rows explicit across five waves
[#523](https://github.com/Ontic-Systems/Gradient/pull/523)
[#524](https://github.com/Ontic-Systems/Gradient/pull/524)
[#525](https://github.com/Ontic-Systems/Gradient/pull/525)
[#526](https://github.com/Ontic-Systems/Gradient/pull/526)
[#527](https://github.com/Ontic-Systems/Gradient/pull/527).)

The classifier — `typechecker::stdlib_tier::classify_effects` — walks the
effect row once and applies the smallest-tier rule:

1. If any effect is in `STD_TIER_EFFECTS` (`IO`, `FS`, `Net`, `Time`,
   `Mut`), the tier is `Std`.
2. Else if any effect is in `ALLOC_TIER_EFFECTS` (`Heap`), the tier is
   `Alloc`.
3. Else (including the empty effect row), the tier is `Core`.

**Out-of-axis effects are deliberately ignored.** Effects that classify
along an orthogonal axis — `Async`, `Send`, `Atomic`, `Volatile`,
`Stack`, `Static`, `Throws(_)`, `FFI(_)`, `Actor`, and any effect variable
— never promote a function past its memory/IO tier. A `core` consumer can
still call a `bindgen`-generated `extern fn` whose effect row is
`!{FFI(C)}`.

### Programmatic classification

Three public APIs are exposed under `gradient_compiler::typechecker`:

```rust
use gradient_compiler::typechecker::{
    classify_effects,            // pure fn over &[String] -> StdlibTier
    permitted_under,             // (callee_tier, module_tier) -> bool
    StdlibTier,                  // Core | Alloc | Std (Ord by inclusion)
};
use gradient_compiler::typechecker::env::TypeEnv;

let env = TypeEnv::new();
assert_eq!(env.lookup_fn_tier("string_to_int"), Some(StdlibTier::Alloc));
assert_eq!(env.lookup_fn_tier("abs"),           Some(StdlibTier::Core));
```

`StdlibTier` is `Ord`: `Core < Alloc < Std`. `permitted_under(callee,
module)` is `callee <= module` — the rule the future #348 rejection will
key off.

## What changes for existing code

**Nothing breaks.** All current code compiles unchanged. The scaffold is
purely additive:

- New module `codebase/compiler/src/typechecker/stdlib_tier.rs`.
- New method `TypeEnv::lookup_fn_tier(name)`.
- New integration test `codebase/compiler/tests/stdlib_tier_classification.rs`
  pins representative builtins to their expected tier.

Application code does not need to do anything. `gradient build` does not
gate on tier today; the linker-DCE consumer of these tiers (Epic E5
[#298](https://github.com/Ontic-Systems/Gradient/issues/298)) is a future
PR.

## What changes when [#347](https://github.com/Ontic-Systems/Gradient/issues/347) lands (LANDED)

CI gains a `no_std` smoke target: every `.gr` fixture under
`codebase/compiler/tests/no_std_corpus/` is lex+parse+type-checked, and
every function's inferred-plus-declared effect closure must classify at
`core` (zero `!{Heap}` / `!{IO}` / `!{FS}` / `!{Net}` / `!{Time}` /
`!{Mut}`). Out-of-axis effects like `!{Stack}` are allowed (and most
fixtures carry `!{Stack}` for clarity).

Authoring a module that accidentally introduces `!{Heap}` (e.g. by
calling `int_to_string`) will fail this smoke target with a structured
error naming the offending function and effect closure.

The classifier in this PR can spot-check your candidate module before
authoring a fixture:

```rust
use gradient_compiler::typechecker::env::TypeEnv;

let env = TypeEnv::new();
for callee in your_module.callees() {
    if env.lookup_fn_tier(callee) > Some(StdlibTier::Core) {
        panic!("{callee} would break no_std");
    }
}
```

## What changes when [#348](https://github.com/Ontic-Systems/Gradient/issues/348) lands (LANDED)

The parser accepts a top-of-file `@no_std` module attribute (no
arguments, declared alongside `@trusted` / `@untrusted` /
`@panic(...)`). When set, the checker rejects any call whose required
effect promotes the call past `core` with a structured diagnostic:

```
error: call to `string_to_int` requires effect `Heap` (tier `alloc`); module is declared `@no_std` (ceiling `core`)
  --> src/parser.gr:42:5
   |
42 |     ret string_to_int(s)
   |         ^^^^^^^^^^^^^
note: in `parse` → call to `string_to_int` → effect `!{Heap}` exceeds the declared `core` ceiling
note: either drop the call to `string_to_int` call (and any dependency that requires `!{Heap}`) or remove the `@no_std` module attribute
```

The diagnostic surfaces:

- The **in-scope function** (`parse` here) — the half of the chain
  visible to the user without a transitive callgraph walk.
- The **call site** the offending effect propagates from.
- The **effect** that promotes the tier (`Heap` here).
- The **classified tier** of the call (`alloc`).
- The **declared ceiling** of the module (`core`).
- A **fix-it** offering both directions (drop the call vs. remove the
  attribute).

Concretely: an `import std::file::read` in a `no_std`-declaring module
becomes a compile error rather than a runtime tier mismatch.

## What does NOT change

The user-visible `.gr` import root remains a single namespace. There is
no `import core::option` vs `import std::option` distinction in source
today; a function call's tier is checked by its effect, not by its
import path. ADR 0005 explicitly defers the question of whether the Rust
workspace eventually grows three crates (`gradient-core`,
`gradient-alloc`, `gradient-std`) versus three feature-namespaced modules
in a single crate. The scaffold landed by #345 chose the
**single-module, three-namespaced-tiers** path; future PRs may split into
three crates if Epic E5's linker-DCE story benefits.

## Reference

- ADR 0005 — `docs/adr/0005-stdlib-split.md`.
- Classifier — `codebase/compiler/src/typechecker/stdlib_tier.rs`.
- Integration tests — `codebase/compiler/tests/stdlib_tier_classification.rs`.
- `no_std` smoke target (#347) — `codebase/compiler/tests/no_std_smoke.rs`
  + fixture corpus under `codebase/compiler/tests/no_std_corpus/`.
- Effect-row audit pass that made classification possible — [#346](https://github.com/Ontic-Systems/Gradient/issues/346).
- Follow-on sub-issues — [#347](https://github.com/Ontic-Systems/Gradient/issues/347), [#348](https://github.com/Ontic-Systems/Gradient/issues/348).
- Epic E7 parent — [#300](https://github.com/Ontic-Systems/Gradient/issues/300).
- Epic E5 (modular runtime DCE) — [#298](https://github.com/Ontic-Systems/Gradient/issues/298).

## Follow-on: string concatenation `+` propagates `Heap` (#531)

Post-#348 audit found that the `String + String` binary operator was
returning `Ty::String` without propagating `Heap` to the caller, even
though the runtime allocates a fresh `String` on the heap (see
`__gradient_string_concat`). This was the audit-trail leak called out in
the post-#530 handoff (pitfall #66) and in `gradient-stdlib-effect-row-annotation-pattern.md`
pitfall #10.

`#531` closes the gap. After this PR:

```gradient
fn build(prefix: String, suffix: String) -> String:
    ret prefix + suffix    // ← compile error
```

```text
error: string concatenation `+` requires effect `Heap`
note: add `!{Heap}` to the enclosing function's signature
```

Same diagnostic + ceiling rejection composes with `@no_std`:

```gradient
@no_std
fn build(prefix: String, suffix: String) -> !{Heap} String:
    ret prefix + suffix    // ← rejected: Heap exceeds @no_std (Core)
```

Numeric `+` (`Int + Int`, `Float + Float`) stays pure — pinned by
`int_addition_stays_pure_after_heap_propagation` and
`float_addition_stays_pure_after_heap_propagation`.
