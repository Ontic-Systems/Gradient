/*
 * Gradient allocator strategy: slab
 *
 * Selected by `gradient build` when the main module declares
 * `@allocator(slab)`. The runtime crate itself supplies
 * `__gradient_alloc(size_t)` / `__gradient_free(void*)` backed by a
 * fixed-size-class slab allocator. No embedder vtable required.
 *
 * This is a sibling concrete `pluggable`-class implementation joining
 * `runtime_allocator_arena.c` (#543) under the same C ABI vtable
 * established by #336/#542. Closes #545.
 *
 * Symbol contract (matches `runtime_allocator_default.c`):
 *
 *   void* __gradient_alloc(size_t size)
 *     -- if size <= SLAB_MAX_CLASS_BYTES, serve from the per-class
 *        free list. Otherwise fall through to libc `malloc(size)`.
 *        Returns NULL on out-of-memory. Allocations are at least
 *        SLAB_ALIGN-aligned (16 B today).
 *
 *   void __gradient_free(void* ptr)
 *     -- if `ptr` is in a slab arena (recognised via the slab tag
 *        stored just before each block), return it to its class's
 *        free list. Otherwise hand it to libc `free()`. Forwarding
 *        `NULL` is a no-op.
 *
 *   const char __gradient_allocator_strategy[]
 *     -- "slab" — visible to `nm`/`strings` and a future `gradient
 *        inspect`. Same name as the `default`, `pluggable`, and
 *        `arena` variants export — linking any two of them produces
 *        the intended multi-definition error from `cc`, defending
 *        against accidental double-link.
 *
 * Design summary (size-class slab):
 *
 *   * Five fixed size classes: 16, 32, 64, 96, 128 bytes. Allocations
 *     <= a class's body size are rounded up to that class. Allocations
 *     > 128 B fall through to libc `malloc` (the "large" path).
 *   * Each block carries an 8-byte header storing a slab tag (so
 *     `__gradient_free` can distinguish slab blocks from libc blocks)
 *     and a class index. The pointer returned to the caller points
 *     just past the header.
 *   * Each class owns a singly-linked free list and a chunk list.
 *     New chunks are obtained via libc `malloc` in 16 KB increments
 *     and carved into class-sized blocks on first touch.
 *   * Frees push the block onto its class's free list. No coalescing
 *     happens — same-class frees are reused but cannot satisfy a
 *     different class's request. Worst-case fragmentation is bounded
 *     by the class set.
 *   * The slab is process-local and NOT thread-safe; matches the
 *     thread-safety posture of the canonical runtime today. A future
 *     PR can add a per-class mutex if/when concurrent allocation
 *     becomes a goal.
 *
 * Trade-offs vs. siblings:
 *
 *   * vs. `default` (libc malloc): slab amortises the small-object
 *     hot path to O(1) per alloc/free with locality of reference;
 *     downside is per-class fragmentation and a fixed 8 B header
 *     overhead per allocation.
 *   * vs. `arena` (bump-pointer, no per-allocation free): slab
 *     reclaims memory at free-time so it suits long-running programs
 *     allocating in patterns where the live set stays bounded but the
 *     total cumulative allocation grows; arena is better for short
 *     burst-and-exit patterns.
 *   * vs. `pluggable` (embedder supplies bodies): slab is a turn-key
 *     opt-in for programs that just want a faster small-object path
 *     without writing custom alloc code.
 *
 * See `codebase/compiler/runtime/allocator/README.md` for the
 * dispatch table and integration tests in
 * `codebase/build-system/tests/allocator_runtime.rs`.
 */

#include <stdlib.h>
#include <string.h>
#include <stddef.h>
#include <stdint.h>

const char __gradient_allocator_strategy[] = "slab";

/* ── Size classes ──────────────────────────────────────────────────────── */

#define SLAB_NUM_CLASSES   5
#define SLAB_MAX_CLASS_BYTES 128
#define SLAB_ALIGN         16
#define SLAB_CHUNK_BYTES   (16 * 1024)
#define SLAB_TAG           0x534C4142u   /* 'SLAB' little-endian */

static const size_t SLAB_CLASS_BYTES[SLAB_NUM_CLASSES] = {
    16, 32, 64, 96, 128
};

/* Header stored just before every slab-served block. The pointer
 * returned to the caller is `((uint8_t*)header) + sizeof(SlabHeader)`.
 * The header lets `__gradient_free` recognise slab blocks (tag) and
 * route them to the correct free list (class index). */
typedef struct SlabHeader {
    uint32_t tag;        /* SLAB_TAG for slab blocks */
    uint32_t class_idx;  /* 0..SLAB_NUM_CLASSES-1 */
} SlabHeader;

/* Free-list node overlays the body of a free block. Only valid when
 * the block sits on a class's free list. */
typedef struct SlabFreeNode {
    struct SlabFreeNode* next;
} SlabFreeNode;

/* Chunk list — kept so the atexit hook can free the entire slab
 * arena. A chunk is a libc-malloc'd region carved into class-sized
 * blocks on first touch. */
typedef struct SlabChunk {
    struct SlabChunk* next;
    /* The carved blocks immediately follow this header in memory.
     * No additional bookkeeping needed once a block is on the free
     * list — `next` carry over the SlabFreeNode overlay. */
} SlabChunk;

/* Per-class state. The free list is the hot path; the chunk list is
 * cold and used only by the atexit hook. */
typedef struct SlabClass {
    SlabFreeNode* free_list;
    SlabChunk*    chunks;
} SlabClass;

static SlabClass g_classes[SLAB_NUM_CLASSES];
static int       g_atexit_registered = 0;

/* ── Helpers ───────────────────────────────────────────────────────────── */

static size_t slab_block_stride(size_t class_bytes) {
    /* Total bytes per block including header, rounded up to SLAB_ALIGN
     * so consecutive blocks stay aligned. */
    size_t raw = sizeof(SlabHeader) + class_bytes;
    size_t rem = raw % SLAB_ALIGN;
    return rem == 0 ? raw : raw + (SLAB_ALIGN - rem);
}

static int slab_pick_class(size_t size) {
    /* Returns the smallest class index whose body size >= `size`,
     * or -1 if `size` is too large for any slab class. */
    for (int i = 0; i < SLAB_NUM_CLASSES; ++i) {
        if (size <= SLAB_CLASS_BYTES[i]) {
            return i;
        }
    }
    return -1;
}

static void slab_atexit(void);

static int slab_grow_class(int class_idx) {
    /* Allocate a 16 KB chunk from libc and carve it into class-sized
     * blocks linked into the class's free list. Returns 0 on success,
     * -1 on libc OOM. */
    SlabChunk* chunk = (SlabChunk*)malloc(sizeof(SlabChunk) + SLAB_CHUNK_BYTES);
    if (!chunk) {
        return -1;
    }
    chunk->next = g_classes[class_idx].chunks;
    g_classes[class_idx].chunks = chunk;

    size_t stride = slab_block_stride(SLAB_CLASS_BYTES[class_idx]);
    size_t n = SLAB_CHUNK_BYTES / stride;
    uint8_t* base = (uint8_t*)(chunk + 1);

    /* Carve from high address downwards so the lowest-address block
     * ends up at the head of the free list (mildly nicer for
     * locality on first allocation). */
    for (size_t i = 0; i < n; ++i) {
        uint8_t* slot = base + i * stride;
        SlabHeader* hdr = (SlabHeader*)slot;
        hdr->tag = SLAB_TAG;
        hdr->class_idx = (uint32_t)class_idx;
        SlabFreeNode* node = (SlabFreeNode*)(slot + sizeof(SlabHeader));
        node->next = g_classes[class_idx].free_list;
        g_classes[class_idx].free_list = node;
    }

    if (!g_atexit_registered) {
        g_atexit_registered = 1;
        atexit(slab_atexit);
    }
    return 0;
}

static void slab_atexit(void) {
    /* Bulk-free every chunk acquired by every class. The free lists
     * become invalid but the process is exiting so we don't bother
     * scrubbing them. */
    for (int i = 0; i < SLAB_NUM_CLASSES; ++i) {
        SlabChunk* c = g_classes[i].chunks;
        while (c) {
            SlabChunk* next = c->next;
            free(c);
            c = next;
        }
        g_classes[i].chunks = NULL;
        g_classes[i].free_list = NULL;
    }
}

/* ── Public ABI ────────────────────────────────────────────────────────── */

void* __gradient_alloc(size_t size) {
    if (size == 0) {
        /* C2x leaves the behaviour implementation-defined; mirror
         * `runtime_allocator_default.c` which forwards directly to
         * libc by treating zero as a 1-byte request on the slab path
         * (cheaper than special-casing). */
        size = 1;
    }

    int class_idx = slab_pick_class(size);
    if (class_idx < 0) {
        /* Large path: fall through to libc malloc. We need
         * `__gradient_free` to be able to distinguish a large-path
         * pointer from a slab-path pointer by reading
         * `ptr - sizeof(SlabHeader)`, so we write a SlabHeader at
         * exactly that offset. The full layout is:
         *
         *     [ raw .. raw+8 )           : pad (unused)
         *     [ raw+8 .. raw+16 )        : SlabHeader{ tag=0, class_idx=LARGE }
         *     [ raw+16 .. raw+16+size )  : caller body
         *
         * The 16-byte preamble keeps the body at least SLAB_ALIGN-
         * aligned, and lets `__gradient_free` recover the original
         * libc-malloc'd base via `(uint8_t*)ptr - SLAB_ALIGN`. */
        size_t need = SLAB_ALIGN + size;
        uint8_t* raw = (uint8_t*)malloc(need);
        if (!raw) {
            return NULL;
        }
        SlabHeader* hdr = (SlabHeader*)(raw + SLAB_ALIGN - sizeof(SlabHeader));
        hdr->tag = 0;             /* not SLAB_TAG */
        hdr->class_idx = 0xFFFFFFFFu;  /* large-path sentinel */
        return raw + SLAB_ALIGN;
    }

    if (!g_classes[class_idx].free_list) {
        if (slab_grow_class(class_idx) != 0) {
            return NULL;
        }
    }
    SlabFreeNode* node = g_classes[class_idx].free_list;
    g_classes[class_idx].free_list = node->next;

    /* Zero the body so callers see deterministic memory (matches
     * `runtime_allocator_arena.c`'s contract). */
    memset(node, 0, SLAB_CLASS_BYTES[class_idx]);
    return (void*)node;
}

void __gradient_free(void* ptr) {
    if (!ptr) {
        return;
    }
    SlabHeader* hdr = (SlabHeader*)((uint8_t*)ptr - sizeof(SlabHeader));
    /* For large-path allocations the same offset reaches a header
     * whose tag is 0; for slab-path allocations the tag is SLAB_TAG.
     * The 16-byte large-path stride keeps the header read in-bounds
     * either way (slab blocks have at least 8 bytes of header, large
     * blocks have a 16-byte header preamble). */
    if (hdr->tag == SLAB_TAG && hdr->class_idx < (uint32_t)SLAB_NUM_CLASSES) {
        SlabFreeNode* node = (SlabFreeNode*)ptr;
        node->next = g_classes[hdr->class_idx].free_list;
        g_classes[hdr->class_idx].free_list = node;
        return;
    }
    /* Large path. The original libc-malloc'd base sits SLAB_ALIGN
     * bytes before `ptr`. */
    free((uint8_t*)ptr - SLAB_ALIGN);
}
