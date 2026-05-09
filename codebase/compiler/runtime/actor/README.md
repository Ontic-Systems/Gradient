# Gradient runtime — actor-strategy split (E5 #334)

ADR 0005 commits Gradient to a modular, effect-driven runtime. This
directory is the runtime-side implementation of the **actor strategy**
axis — the third runtime axis split out of `gradient_runtime.c`,
following the panic-strategy split (#537) and the alloc-strategy split
(#538).

## Selection

`gradient build` chooses ONE of these objects to link based on whether
the main module's **effect summary** (computed by the typechecker)
contains `Actor`:

| Effect summary contains `Actor`? | Object linked | Tag value |
|---|---|---|
| Yes (program uses `spawn`/`send`/`ask`/actor types) | `runtime_actor_full.o` | `"full"` |
| No (no `Actor` effect anywhere — most non-concurrent programs) | `runtime_actor_none.o` | `"none"` |

The canonical `gradient_runtime.o` is ALWAYS linked alongside whichever
actor-strategy object is chosen — it provides libc helpers
(`__gradient_print_int`, `__gradient_string_from_int`, etc.) that don't
themselves spawn actors.

The selection is **automatic**, NOT a user-facing module attribute.
Sibling pattern to the alloc-strategy split (#538): ADR 0005's
commitment to effect-driven DCE means the runtime closure is
determined by the program's effect surface, not by an explicit opt-in.
(Compare to `@panic(abort|unwind|none)` from #537, which IS user-facing
because the strategy choice can't be derived from effects alone.)

## Symbol contract

Both files export exactly one symbol:

```c
const char __gradient_actor_strategy[];   /* "full" | "none" */
```

The shared name is intentional — linking both objects produces a
multiple-definition error from `cc`, acting as defense-in-depth against
accidental double-link.

The future of this directory is to be the home of the actor scheduler
currently sitting as `static` helpers inside the canonical runtime
(mailbox queues, supervisor trees, `spawn`/`send`/`ask` lowering
helpers). A follow-on PR will (a) externalise those helpers as
`__gradient_actor_*` symbols, (b) move them out of `gradient_runtime.c`
and into `runtime_actor_full.c`, and (c) update the existing
`gradient_runtime.c` call sites to use the externalised entry points.
Once that lands, an actor-free program selecting `none` will see a
measurable binary-size delta.

For now: the split exists, the dispatch is wired, the tag is
introspectable via `nm <binary> | grep __gradient_actor_strategy`, and
the future extraction can be a small, mechanical PR.

## Per-variant status

| File | Status | Notes |
|---|---|---|
| `runtime_actor_full.c` | tag-only | actor scheduler still lives in `gradient_runtime.c` (and the experimental actor module); extract in a follow-on PR |
| `runtime_actor_none.c` | tag-only | proves the link-time omission of `runtime_actor_full.o` works; the missing-symbol guard against accidental actor calls is the intended loud-fail behaviour |

## Why two crates instead of `#ifdef` macros

Same rationale as the panic-strategy and alloc-strategy splits: `cc -c`
of two single-purpose files produces deterministic objects independent
of preprocessor state, the link command is the single source of truth
for which strategy is in effect, and the tag symbol survives strip
without LTO surprises.
