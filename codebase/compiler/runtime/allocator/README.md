# Allocator strategy runtime crates (E5 #336)

`gradient build` links exactly ONE of these C files alongside the canonical
runtime, selected by the main module's `@allocator(...)` attribute. This
implements the `Allocator` trait at the C ABI level and is the runtime-side
half of E5's modular runtime closure (ADR 0005).

| File | When linked | Symbol body |
|---|---|---|
| `runtime_allocator_default.c` | `@allocator(default)` (default) | `__gradient_alloc` → `malloc(3)`, `__gradient_free` → `free(3)` |
| `runtime_allocator_pluggable.c` | `@allocator(pluggable)` | `__gradient_alloc` / `__gradient_free` declared `extern`; embedder must resolve at link time |
| `runtime_allocator_arena.c` | `@allocator(arena)` (#320 / #336 follow-on) | `__gradient_alloc` → process-global bump-pointer arena (vendored from `runtime/memory/arena.c`); `__gradient_free` is a no-op; bulk reclamation at process exit via `atexit` hook |
| `runtime_allocator_slab.c` | `@allocator(slab)` (#545) | `__gradient_alloc` → fixed-size-class slab (16/32/64/96/128 B classes; ≤128 B served from per-class free list, larger requests fall through to libc `malloc`); `__gradient_free` returns slab blocks to their class free list and forwards large-path blocks to libc `free`; bulk reclamation at process exit via `atexit` hook |
| `runtime_allocator_bumpalo.c` | `@allocator(bumpalo)` (#547) | `__gradient_alloc` → multi-chunk bump-arena allocator (chunks chained via libc `malloc`, size doubles per growth up to a 4 MiB cap; allocations bump downwards from the current chunk's tail; previously-returned pointers are pinned — chunks never relocate); `__gradient_free` is a no-op; bulk reclamation at process exit via `atexit` hook |

## The `Allocator` trait surface

Gradient's allocator surface is intentionally minimal at the C-ABI layer:

```c
void* __gradient_alloc(size_t size);  // returns NULL on OOM
void  __gradient_free(void* ptr);     // ignores NULL
```

Plus the introspectable tag every variant exports:

```c
const char __gradient_allocator_strategy[];  // "default" | "pluggable" | "arena" | "slab" | "bumpalo"
```

This is the C-side `Allocator` trait — every Gradient runtime allocation
goes through these two symbols. The runtime crate selected at link time
decides what bodies they resolve to.

## Selection axis

Attribute-driven (NOT effect-driven). Sibling of `@panic(...)` (#318/#537) in
that respect; distinct from the effect-driven trio
(`alloc_strategy` #333/#538, `actor_strategy` #334/#539,
`async_strategy` #335/#540) which derive their selection from the program's
effect closure.

The reason: whether a deployment is "the embedder provides an allocator" is
a property of how the binary is shipped, not of what the program does. A
`no_std` `+ @allocator(pluggable)` program might use exactly the same
surface code as a host `@allocator(default)` program; only the link-time
contract differs.

## Linker contract

Linking any two of `runtime_allocator_default.o`,
`runtime_allocator_pluggable.o`, `runtime_allocator_arena.o`,
`runtime_allocator_slab.o`, or `runtime_allocator_bumpalo.o` into the
same binary produces a multi-definition error from `cc` on
`__gradient_allocator_strategy`. That's intentional — the build system
selects exactly one variant, and a double-link is a build-system bug we want
to surface loudly.

## Slab / bumpalo / jemalloc follow-ons

The arena variant (`runtime_allocator_arena.c`, #543) was the FIRST
concrete `pluggable`-class implementation. The slab variant
(`runtime_allocator_slab.c`, #545) is the second — a size-class slab
allocator covering 16/32/64/96/128 byte classes with libc malloc
fallthrough for larger requests. The bumpalo variant
(`runtime_allocator_bumpalo.c`, #547) is the third — a multi-chunk
bump-arena inspired by the bumpalo Rust crate, optimised for
allocate-many-then-bulk-reclaim workloads with pointer stability
guarantees the single-region `arena` variant doesn't provide.
Jemalloc-style implementations can ship as further sibling files
(`runtime_allocator_jemalloc.c`, etc.) following the same five-piece
recipe documented in
`software-development/gradient-project-development/references/gradient-runtime-modularization-pattern.md`.

For embedders writing custom allocators (real-time schedulers, GPU
pinned-memory pools, NUMA-aware allocators), `@allocator(pluggable)`
remains the right choice — the embedder supplies the
`__gradient_alloc` / `__gradient_free` bodies at link time and gets a
zero-overhead vtable.

## Companion: size-budget gate

Once arena/slab/bumpalo land, the size-budget gate
(`codebase/build-system/tests/size_budget.rs`, #541 closing #338) is the
regression watchdog that locks any binary-size delta into a CI-enforced
floor.
