/*
 * Gradient allocator strategy: bumpalo
 *
 * Selected by `gradient build` when the main module declares
 * `@allocator(bumpalo)`. The runtime crate itself supplies
 * `__gradient_alloc(size_t)` / `__gradient_free(void*)` backed by a
 * multi-chunk bump-arena allocator inspired by the `bumpalo` Rust
 * crate. No embedder vtable required.
 *
 * This is a sibling concrete `pluggable`-class implementation joining
 * `runtime_allocator_arena.c` (#543) and `runtime_allocator_slab.c`
 * (#546) under the same C ABI vtable established by #336/#542.
 * Closes #547.
 *
 * Symbol contract (matches `runtime_allocator_default.c`):
 *
 *   void* __gradient_alloc(size_t size)
 *     -- bump from the tail of the current chunk. If the current chunk
 *        has insufficient space, allocate a new (larger) chunk via
 *        libc `malloc` and chain it. Returns NULL only on libc OOM.
 *        Allocations are at least BUMPALO_ALIGN-aligned (16 B today).
 *        Previously-returned pointers are NEVER moved — chunks never
 *        relocate.
 *
 *   void __gradient_free(void* ptr)
 *     -- no-op. The entire chain is reclaimed at process exit via an
 *        `atexit` hook. Forwarding `NULL` is also a no-op.
 *
 *   const char __gradient_allocator_strategy[]
 *     -- "bumpalo" — visible to `nm`/`strings` and a future `gradient
 *        inspect`. Same name as the `default`, `pluggable`, `arena`,
 *        and `slab` variants export — linking any two of them
 *        produces the intended multi-definition error from `cc`,
 *        defending against accidental double-link.
 *
 * Design summary (multi-chunk bump arena):
 *
 *   * The arena is a singly-linked list of chunks. Each chunk is a
 *     libc-malloc'd region carrying its own bump cursor and end
 *     pointer. The chain head is the "current" chunk that satisfies
 *     allocations until exhausted.
 *   * On exhaustion, a new chunk is allocated with size doubled from
 *     the previous chunk (capped at BUMPALO_MAX_CHUNK_BYTES) or sized
 *     to fit the request (whichever is larger). The new chunk
 *     becomes the head; the old chunk is retained in the chain so
 *     its previously-returned pointers stay valid.
 *   * Each allocation rounds size up to BUMPALO_ALIGN and bumps the
 *     cursor backwards from the chunk's end. Sub-allocations are
 *     therefore returned in descending-address order within a chunk
 *     (mirrors the bumpalo crate; cheaper alignment math).
 *   * The arena is process-local and NOT thread-safe; matches the
 *     thread-safety posture of the canonical runtime today and the
 *     sibling `arena` / `slab` allocators. A future PR can add a
 *     mutex if/when concurrent allocation becomes a goal.
 *   * Frees are no-ops. Bulk reclamation of every chunk happens at
 *     process exit via `atexit`.
 *
 * Trade-offs vs. siblings:
 *
 *   * vs. `default` (libc malloc): bumpalo amortises allocation to
 *     a few-instruction bump on the hot path with no free-time cost,
 *     at the price of monotonic memory growth (no per-allocation
 *     reclamation).
 *   * vs. `arena` (single growing region): bumpalo guarantees
 *     pointer stability across allocations because chunks never
 *     relocate. The arena variant grows a single region and may
 *     relocate on growth — fine for code that doesn't hold raw
 *     pointers across allocations, problematic when it does.
 *   * vs. `slab` (per-class free lists, individual frees): bumpalo
 *     trades free-time reclamation for a much simpler hot path and
 *     stronger pointer-stability guarantees. Slab is better for
 *     long-running programs allocating in patterns where the live
 *     set stays bounded; bumpalo is better for short burst-and-exit
 *     patterns OR for programs that allocate a static graph and
 *     then read it.
 *   * vs. `pluggable` (embedder supplies bodies): bumpalo is a
 *     turn-key opt-in for programs that just want fast allocation
 *     with bulk-reclaim semantics without writing custom alloc code.
 *
 * See `codebase/compiler/runtime/allocator/README.md` for the
 * dispatch table and integration tests in
 * `codebase/build-system/tests/allocator_runtime.rs`.
 */

#include <stdlib.h>
#include <string.h>
#include <stddef.h>
#include <stdint.h>

const char __gradient_allocator_strategy[] = "bumpalo";

/* ── Tuning constants ──────────────────────────────────────────────────── */

#define BUMPALO_ALIGN              16
#define BUMPALO_INITIAL_CHUNK_BYTES (16 * 1024)
/* Cap chunk growth at 4 MiB so a long-running allocator-heavy program
 * doesn't overshoot when satisfying a brief allocation spike. Beyond
 * this cap the chunk size stays at BUMPALO_MAX_CHUNK_BYTES; very
 * large single allocations get a custom-sized chunk that exactly
 * fits the request (plus header). */
#define BUMPALO_MAX_CHUNK_BYTES    (4 * 1024 * 1024)

/* ── Chunk record ──────────────────────────────────────────────────────── */

/* One chunk in the chain. The body of the chunk follows the header in
 * memory; `cursor` bumps DOWNWARDS from `end` so a body region of
 * size `end - body_base` is available initially. */
typedef struct BumpaloChunk {
    struct BumpaloChunk* next;   /* older chunks (NULL terminates the chain) */
    uint8_t*             cursor; /* next-allocation upper bound — bumps down */
    uint8_t*             body_base; /* lower bound of usable body */
    size_t               size;   /* total body bytes (for diagnostics + atexit) */
} BumpaloChunk;

static BumpaloChunk* g_head = NULL;        /* current (newest) chunk */
static size_t        g_last_chunk_bytes = 0;
static int           g_atexit_registered = 0;

/* ── Helpers ───────────────────────────────────────────────────────────── */

static size_t bumpalo_round_up(size_t n, size_t align) {
    size_t rem = n % align;
    return rem == 0 ? n : n + (align - rem);
}

static void bumpalo_atexit(void);

static int bumpalo_grow(size_t needed_body_bytes) {
    /* Choose the next chunk size: at least double the previous, capped
     * at BUMPALO_MAX_CHUNK_BYTES, but never less than the request. */
    size_t next_bytes = g_last_chunk_bytes == 0
        ? BUMPALO_INITIAL_CHUNK_BYTES
        : g_last_chunk_bytes * 2;
    if (next_bytes > BUMPALO_MAX_CHUNK_BYTES) {
        next_bytes = BUMPALO_MAX_CHUNK_BYTES;
    }
    if (next_bytes < needed_body_bytes) {
        /* Single allocation larger than our growth schedule — size
         * the chunk to exactly fit it (rounded up to alignment so
         * the cursor math stays clean). */
        next_bytes = bumpalo_round_up(needed_body_bytes, BUMPALO_ALIGN);
    }

    BumpaloChunk* chunk = (BumpaloChunk*)malloc(sizeof(BumpaloChunk) + next_bytes);
    if (!chunk) {
        return -1;
    }
    chunk->next = g_head;
    chunk->body_base = (uint8_t*)(chunk + 1);
    chunk->cursor = chunk->body_base + next_bytes; /* one-past-end; bump down */
    chunk->size = next_bytes;
    g_head = chunk;
    g_last_chunk_bytes = next_bytes;

    if (!g_atexit_registered) {
        g_atexit_registered = 1;
        atexit(bumpalo_atexit);
    }
    return 0;
}

static void bumpalo_atexit(void) {
    /* Bulk-free every chunk in the chain. The cursor bookkeeping
     * becomes invalid but the process is exiting so we don't bother
     * scrubbing it. */
    BumpaloChunk* c = g_head;
    while (c) {
        BumpaloChunk* next = c->next;
        free(c);
        c = next;
    }
    g_head = NULL;
    g_last_chunk_bytes = 0;
}

/* ── Public ABI ────────────────────────────────────────────────────────── */

void* __gradient_alloc(size_t size) {
    if (size == 0) {
        /* Mirror the slab variant: treat zero as a 1-byte request so
         * the bump path stays uniform. The caller gets a unique
         * non-NULL pointer. */
        size = 1;
    }
    size_t need = bumpalo_round_up(size, BUMPALO_ALIGN);

    /* Fast path: current chunk has room. */
    if (g_head) {
        size_t avail = (size_t)(g_head->cursor - g_head->body_base);
        if (avail >= need) {
            g_head->cursor -= need;
            uint8_t* out = g_head->cursor;
            /* Zero the body so callers see deterministic memory
             * (matches the arena and slab variants' contract). */
            memset(out, 0, need);
            return (void*)out;
        }
    }

    /* Slow path: grow and retry. */
    if (bumpalo_grow(need) != 0) {
        return NULL;
    }
    /* The new head has at least `need` bytes available by construction. */
    g_head->cursor -= need;
    uint8_t* out = g_head->cursor;
    memset(out, 0, need);
    return (void*)out;
}

void __gradient_free(void* ptr) {
    /* No-op. The bumpalo arena reclaims memory in bulk at process
     * exit via the `atexit` hook. Forwarding `NULL` matches libc
     * `free(NULL)` semantics. */
    (void)ptr;
}
