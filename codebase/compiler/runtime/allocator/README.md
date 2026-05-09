# Allocator strategy runtime crates (E5 #336)

`gradient build` links exactly ONE of these C files alongside the canonical
runtime, selected by the main module's `@allocator(...)` attribute. This
implements the `Allocator` trait at the C ABI level and is the runtime-side
half of E5's modular runtime closure (ADR 0005).

| File | When linked | Symbol body |
|---|---|---|
| `runtime_allocator_default.c` | `@allocator(default)` (default) | `__gradient_alloc` → `malloc(3)`, `__gradient_free` → `free(3)` |
| `runtime_allocator_pluggable.c` | `@allocator(pluggable)` | `__gradient_alloc` / `__gradient_free` declared `extern`; embedder must resolve at link time |

## The `Allocator` trait surface

Gradient's allocator surface is intentionally minimal at the C-ABI layer:

```c
void* __gradient_alloc(size_t size);  // returns NULL on OOM
void  __gradient_free(void* ptr);     // ignores NULL
```

Plus the introspectable tag every variant exports:

```c
const char __gradient_allocator_strategy[];  // "default" | "pluggable"
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

Linking both `runtime_allocator_default.o` and `runtime_allocator_pluggable.o`
into the same binary produces a multi-definition error from `cc` on
`__gradient_allocator_strategy`. That's intentional — the build system
selects exactly one variant, and a double-link is a build-system bug we want
to surface loudly.

## Bumpalo / slab follow-on

Concrete arena and slab implementations of the `pluggable` variant ship in
a future PR (E5 #336 follow-on). The current runtime ONLY provides the
trait surface — the build system test
`codebase/build-system/tests/allocator_runtime.rs` includes a minimal
fixture allocator written in C that satisfies the contract for the
integration test, but production users of `@allocator(pluggable)` are
expected to bring their own.

## Companion: size-budget gate

Once arena/slab/bumpalo land, the size-budget gate
(`codebase/build-system/tests/size_budget.rs`, #541 closing #338) is the
regression watchdog that locks any binary-size delta into a CI-enforced
floor.
