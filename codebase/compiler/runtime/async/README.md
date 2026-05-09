# Gradient runtime — async-strategy split (E5 #335)

ADR 0005 commits Gradient to a modular, effect-driven runtime. This
directory is the runtime-side implementation of the **async strategy**
axis — the fourth runtime axis split out of `gradient_runtime.c`,
following the panic-strategy split (#537), the alloc-strategy split
(#538), and the actor-strategy split (#539).

## Selection

`gradient build` chooses ONE of these objects to link based on whether
the main module's **effect summary** (computed by the typechecker)
contains `Async`:

| Effect summary contains `Async`? | Object linked | Tag value |
|---|---|---|
| Yes (program uses async/await, futures, or async-typed values) | `runtime_async_full.o` | `"full"` |
| No (no `Async` effect anywhere — most synchronous programs) | `runtime_async_none.o` | `"none"` |

The canonical `gradient_runtime.o` is ALWAYS linked alongside whichever
async-strategy object is chosen — it provides libc helpers
(`__gradient_print_int`, `__gradient_string_from_int`, etc.) that don't
themselves spawn async tasks.

The selection is **automatic**, NOT a user-facing module attribute.
Sibling pattern to the actor-strategy split (#539): ADR 0005's
commitment to effect-driven DCE means the runtime closure is
determined by the program's effect surface, not by an explicit opt-in.
(Compare to `@panic(abort|unwind|none)` from #537, which IS user-facing
because the strategy choice can't be derived from effects alone.)

## Symbol contract

Both files export exactly one symbol:

```c
const char __gradient_async_strategy[];   /* "full" | "none" */
```

The shared name is intentional — linking both objects produces a
multiple-definition error from `cc`, acting as defense-in-depth against
accidental double-link.

The future of this directory is to be the home of the async executor
currently sitting as `static` helpers inside the canonical runtime
(task queue, poll loop, waker registry, future-of-T lowering helpers).
A follow-on PR will (a) externalise those helpers as
`__gradient_async_*` symbols, (b) move them out of `gradient_runtime.c`
and into `runtime_async_full.c`, and (c) update the existing
`gradient_runtime.c` call sites to use the externalised entry points.
Once that lands, an async-free program selecting `none` will see a
measurable binary-size delta.

For now: the split exists, the dispatch is wired, the tag is
introspectable via `nm <binary> | grep __gradient_async_strategy`, and
the future extraction can be a small, mechanical PR.

## Per-variant status

| File | Status | Notes |
|---|---|---|
| `runtime_async_full.c` | tag-only | async executor still lives in `gradient_runtime.c` (and the experimental async module); extract in a follow-on PR |
| `runtime_async_none.c` | tag-only | proves the link-time omission of `runtime_async_full.o` works; the missing-symbol guard against accidental async calls is the intended loud-fail behaviour |

## Why two crates instead of `#ifdef` macros

Same rationale as the panic-strategy, alloc-strategy, and actor-strategy
splits: `cc -c` of two single-purpose files produces deterministic
objects independent of preprocessor state, the link command is the
single source of truth for which strategy is in effect, and the tag
symbol survives strip without LTO surprises.
