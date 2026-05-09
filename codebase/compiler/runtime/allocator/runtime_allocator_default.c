/*
 * Gradient allocator strategy: default
 *
 * Selected by `gradient build` when the main module declares
 * `@allocator(default)` (or omits the attribute — `default` is the
 * AST default). Wraps the C standard library's `malloc(3)` and
 * `free(3)` behind the canonical `__gradient_alloc` /
 * `__gradient_free` runtime entry points.
 *
 * Symbol contract (the C-side "Allocator trait"):
 *
 *   void* __gradient_alloc(size_t size)
 *     -- allocate `size` bytes; returns NULL on failure. Caller is
 *        responsible for freeing via `__gradient_free`. Behaviour
 *        when `size == 0` is implementation-defined per C2x; this
 *        wrapper forwards directly to libc.
 *
 *   void __gradient_free(void* ptr)
 *     -- free a pointer previously returned by `__gradient_alloc`.
 *        Forwarding `NULL` is a no-op (libc handles it).
 *
 *   const char __gradient_allocator_strategy[]
 *     -- "default" — visible to `nm`/`strings` and the future
 *        `gradient inspect`. Same name as the `pluggable` variant
 *        exports — linking both produces the intended
 *        multi-definition error from cc, which is the
 *        defense-in-depth against accidental double-link.
 *
 * The default allocator pairs with #538's effect-driven alloc
 * strategy split (`alloc=full`/`alloc=minimal`): the alloc-strategy
 * decides whether the rc/COW machinery is needed, while the
 * allocator-strategy decides which underlying primitive serves any
 * heap requests. Both ride the same `__gradient_alloc` ABI.
 *
 * See codebase/compiler/runtime/allocator/README.md for the dispatch
 * table.
 */

#include <stdlib.h>

const char __gradient_allocator_strategy[] = "default";

void* __gradient_alloc(size_t size) {
    return malloc(size);
}

void __gradient_free(void* ptr) {
    free(ptr);
}
