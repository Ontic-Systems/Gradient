/*
 * Gradient allocator strategy: arena
 *
 * Selected by `gradient build` when the main module declares
 * `@allocator(arena)`. The runtime crate itself supplies
 * `__gradient_alloc(size_t)` / `__gradient_free(void*)` backed by a
 * process-global bump-pointer arena. No embedder vtable is required
 * (contrast `runtime_allocator_pluggable.c` which DECLARES the symbols
 * extern and defers them to the embedder).
 *
 * This is the FIRST CONCRETE `pluggable`-class implementation under
 * the same C ABI vtable established by #336/#542, and the runtime-side
 * beachhead for E3 #320 (arena allocator runtime crate).
 *
 * Symbol contract (matches `runtime_allocator_default.c`):
 *
 *   void* __gradient_alloc(size_t size)
 *     -- allocate `size` bytes from the process-global arena. Returns
 *        NULL on out-of-memory. The arena grows by 64KB chunks on
 *        demand. Allocations are 8-byte aligned and zero-initialised.
 *
 *   void __gradient_free(void* ptr)
 *     -- NO-OP. Arena allocators reclaim memory in bulk at process
 *        exit (via the registered `atexit` hook), not per-allocation.
 *        Forwarding `NULL` is also a no-op (matches libc `free`).
 *
 *   const char __gradient_allocator_strategy[]
 *     -- "arena" — visible to `nm`/`strings` and a future `gradient
 *        inspect`. Same name as the `default` and `pluggable` variants
 *        export — linking any two of them produces the intended
 *        multi-definition error from `cc`, defending against
 *        accidental double-link.
 *
 * Bulk reclamation: the arena and all its chunks are freed by an
 * `atexit`-registered hook. This pairs cleanly with the "frees are
 * no-ops" contract above — programs that allocate millions of small
 * objects and never free them individually pay a single bulk free at
 * exit, not millions of individual `free` calls. The trade-off is that
 * very long-lived programs allocating in a stream pattern will grow
 * monotonically; for those, `@allocator(default)` (libc malloc/free)
 * is the right choice.
 *
 * Implementation: a vendored copy of `codebase/runtime/memory/arena.c`
 * is embedded inline below so this TU is self-contained — the
 * `find_allocator_runtime_source` helper in `build-system/src/commands/build.rs`
 * locates and links exactly ONE `runtime_allocator_<strategy>.c` file,
 * mirroring its sibling axes (panic, alloc, actor, async). Adding a
 * second link input would deviate from that proven 5× pattern. The
 * vendored code below is the same checked-arithmetic-hardened bump
 * allocator (GRA-178 hardening) used by the existing
 * `runtime_arena_tests.rs` suite, so the same security audit applies.
 *
 * See `codebase/compiler/runtime/allocator/README.md` for the dispatch
 * table.
 */

#include <stdlib.h>
#include <string.h>
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <assert.h>

const char __gradient_allocator_strategy[] = "arena";

/* ── Vendored from codebase/runtime/memory/arena.h ──────────────────────── */

#define ARENA_DEFAULT_CHUNK_SIZE (64 * 1024)
#define ARENA_MIN_CHUNK_SIZE     (4 * 1024)

typedef struct ArenaChunk {
    struct ArenaChunk* next;
    size_t             size;
    size_t             used;
    uint8_t            data[];
} ArenaChunk;

typedef struct Arena {
    ArenaChunk* chunks;
    uint8_t*    bump_ptr;
    uint8_t*    end_ptr;
    size_t      chunk_size;
    size_t      total_allocated;
    int         num_chunks;
} Arena;

/* ── Vendored from codebase/runtime/memory/arena.c (subset used here) ──── */

static inline bool gr_arena_checked_add_size(size_t a, size_t b, size_t* out) {
    if (b > SIZE_MAX - a) return false;
    *out = a + b;
    return true;
}

static ArenaChunk* gr_arena_chunk_new(size_t chunk_size) {
    size_t total_size = 0;
    if (!gr_arena_checked_add_size(sizeof(ArenaChunk), chunk_size, &total_size)) {
        return NULL;
    }
    ArenaChunk* chunk = (ArenaChunk*)malloc(total_size);
    if (!chunk) return NULL;
    chunk->next = NULL;
    chunk->size = chunk_size;
    chunk->used = 0;
    return chunk;
}

static void gr_arena_chunks_free(ArenaChunk* chunk) {
    while (chunk) {
        ArenaChunk* next = chunk->next;
        free(chunk);
        chunk = next;
    }
}

static Arena* gr_arena_create_with_size(size_t chunk_size) {
    if (chunk_size < ARENA_MIN_CHUNK_SIZE) chunk_size = ARENA_MIN_CHUNK_SIZE;
    Arena* a = (Arena*)malloc(sizeof(Arena));
    if (!a) return NULL;
    ArenaChunk* chunk = gr_arena_chunk_new(chunk_size);
    if (!chunk) { free(a); return NULL; }
    a->chunks          = chunk;
    a->bump_ptr        = chunk->data;
    a->end_ptr         = chunk->data + chunk_size;
    a->chunk_size      = chunk_size;
    a->total_allocated = 0;
    a->num_chunks      = 1;
    return a;
}

static void gr_arena_destroy(Arena* a) {
    if (!a) return;
    gr_arena_chunks_free(a->chunks);
    free(a);
}

static inline uintptr_t gr_arena_align_ptr_up(uintptr_t ptr, size_t align) {
    assert((align & (align - 1)) == 0);
    if (ptr > UINTPTR_MAX - (align - 1)) return UINTPTR_MAX;
    return (ptr + align - 1) & ~(align - 1);
}

static void* gr_arena_alloc_aligned(Arena* a, size_t size, size_t align) {
    if (!a || size == 0) return NULL;
    if (align == 0) align = 8;
    if (align > 256 || (align & (align - 1)) != 0) align = 8;
    if (size > SIZE_MAX - align) return NULL;

    uintptr_t current = (uintptr_t)a->bump_ptr;
    uintptr_t aligned = gr_arena_align_ptr_up(current, align);
    if (aligned == UINTPTR_MAX) return NULL;
    size_t padding = aligned - current;

    uintptr_t end = (uintptr_t)a->end_ptr;
    if (aligned <= end && size <= (size_t)(end - aligned)) {
        if (padding > 0) memset((void*)current, 0, padding);
        void* result = (void*)aligned;
        a->bump_ptr = (uint8_t*)(aligned + size);
        size_t step = 0;
        if (!gr_arena_checked_add_size(padding, size, &step)) return NULL;
        a->chunks->used += step;
        a->total_allocated += size;
        memset(result, 0, size);
        return result;
    }

    /* Need a new chunk. */
    size_t required = 0;
    if (!gr_arena_checked_add_size(size, align, &required)) return NULL;
    size_t new_chunk_size = a->chunk_size;
    if (required > new_chunk_size) new_chunk_size = required;

    ArenaChunk* nc = gr_arena_chunk_new(new_chunk_size);
    if (!nc) return NULL;
    nc->next = a->chunks;
    a->chunks = nc;
    a->num_chunks++;
    a->bump_ptr = nc->data;
    a->end_ptr  = nc->data + new_chunk_size;

    current = (uintptr_t)a->bump_ptr;
    aligned = gr_arena_align_ptr_up(current, align);
    if (aligned == UINTPTR_MAX) return NULL;
    padding = aligned - current;
    end = (uintptr_t)a->end_ptr;
    if (aligned > end || size > (size_t)(end - aligned)) return NULL;

    void* result = (void*)aligned;
    a->bump_ptr = (uint8_t*)(aligned + size);
    size_t step = 0;
    if (!gr_arena_checked_add_size(padding, size, &step)) return NULL;
    nc->used = step;
    a->total_allocated += size;
    memset(result, 0, size);
    return result;
}

/* ── Process-global arena singleton ────────────────────────────────────── */

static Arena* g_gradient_arena = NULL;
static int    g_gradient_arena_atexit_registered = 0;

static void gr_arena_atexit_hook(void) {
    if (g_gradient_arena) {
        gr_arena_destroy(g_gradient_arena);
        g_gradient_arena = NULL;
    }
}

static Arena* gr_arena_singleton(void) {
    if (!g_gradient_arena) {
        g_gradient_arena = gr_arena_create_with_size(ARENA_DEFAULT_CHUNK_SIZE);
        if (g_gradient_arena && !g_gradient_arena_atexit_registered) {
            /* atexit may fail (returns nonzero) under exotic conditions
             * (no slot available). If it does, the arena will leak at
             * process exit — annoying but not unsafe. */
            (void)atexit(gr_arena_atexit_hook);
            g_gradient_arena_atexit_registered = 1;
        }
    }
    return g_gradient_arena;
}

/* ── The C-ABI Allocator trait ─────────────────────────────────────────── */

void* __gradient_alloc(size_t size) {
    Arena* a = gr_arena_singleton();
    if (!a) return NULL;
    return gr_arena_alloc_aligned(a, size, 8);
}

void __gradient_free(void* ptr) {
    /* Bulk reclamation at process exit; per-allocation free is a no-op.
     * `ptr` is unused — the bump arena does not track per-allocation
     * metadata. */
    (void)ptr;
}
