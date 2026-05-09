# Gradient runtime — alloc-strategy split (E5 #333)

ADR 0005 commits Gradient to a modular, effect-driven runtime. This
directory is the runtime-side implementation of the **alloc strategy**
axis — the second runtime axis split out from `gradient_runtime.c`,
following the panic-strategy split that landed in #537.

## Selection

`gradient build` chooses ONE of these objects to link based on whether
the main module's **effect summary** (computed by the typechecker)
contains `Heap`:

| Effect summary contains `Heap`? | Object linked | Tag value |
|---|---|---|
| Yes (default for any program using lists, strings, maps, etc.) | `runtime_alloc_full.o` | `"full"` |
| No (heap-free program — `core` tier, `@no_std` modules, pure-arithmetic mains) | `runtime_alloc_minimal.o` | `"minimal"` |

The canonical `gradient_runtime.o` is ALWAYS linked alongside whichever
alloc-strategy object is chosen — it provides libc helpers
(`__gradient_print_int`, `__gradient_string_from_int`, etc.) that don't
themselves allocate.

The selection is **automatic**, NOT a user-facing module attribute. This
mirrors ADR 0005's commitment to effect-driven DCE: the runtime closure
is determined by the program's effect surface, not by an explicit
opt-in. (Compare to `@panic(abort|unwind|none)` from #537, which IS
user-facing because the strategy choice can't be derived from
effects alone.)

## Symbol contract

Both files export exactly one symbol:

```c
const char __gradient_alloc_strategy[];   /* "full" | "minimal" */
```

The shared name is intentional — linking both objects produces a
multiple-definition error from `cc`, acting as defense-in-depth against
accidental double-link.

The future of this directory is to be the home of the rc/COW machinery
currently sitting as `static` helpers inside `gradient_runtime.c`
(`map_retain`/`map_release`/`set_retain`/`set_release`/`map_deep_copy`/etc.).
A follow-on PR will (a) externalise those helpers as `__gradient_rc_*`
symbols, (b) move them out of `gradient_runtime.c` and into
`runtime_alloc_full.c`, and (c) update the existing `gradient_runtime.c`
call sites to use the externalised entry points. Once that lands, a
heap-free program selecting `minimal` will see a measurable binary-size
delta.

For now: the split exists, the dispatch is wired, the tag is
introspectable via `nm <binary> | grep __gradient_alloc_strategy`, and
the future extraction can be a small, mechanical PR.

## Per-variant status

| File | Status | Notes |
|---|---|---|
| `runtime_alloc_full.c` | tag-only | rc/COW machinery still lives in `gradient_runtime.c`; extract in a follow-on PR |
| `runtime_alloc_minimal.c` | tag-only | proves the link-time omission of `runtime_alloc_full.o` works; the missing-symbol guard against accidental heap calls is the intended loud-fail behaviour |

## Why three crates instead of `#ifdef` macros

Same rationale as the panic-strategy split: `cc -c` of three
single-purpose files produces deterministic objects independent of
preprocessor state, the link command is the single source of truth for
which strategy is in effect, and the tag symbol survives strip without
LTO surprises.
