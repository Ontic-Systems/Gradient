/*
 * Arena Allocator Header
 *
 * Bump-pointer arena allocator for efficient temporary memory management.
 * Used by Gradient's 'defer' and arena-allocation syntax.
 */

#ifndef GRADIENT_ARENA_H
#define GRADIENT_ARENA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Default chunk size: 64KB */
#define ARENA_DEFAULT_CHUNK_SIZE (64 * 1024)

/* Minimum chunk size: 4KB */
#define ARENA_MIN_CHUNK_SIZE (4 * 1024)

/* Chunk node in the arena's linked list of chunks */
typedef struct ArenaChunk {
    struct ArenaChunk* next;   /* Next chunk in list */
    size_t size;               /* Total size of this chunk */
    size_t used;               /* Bytes used in this chunk */
    uint8_t data[];            /* Flexible array member for data */
} ArenaChunk;

/* Arena structure with bump pointer allocation */
typedef struct Arena {
    ArenaChunk* chunks;        /* Linked list of chunks (head = current) */
    uint8_t* bump_ptr;         /* Current bump pointer */
    uint8_t* end_ptr;          /* End of current chunk */
    size_t chunk_size;         /* Default size for new chunks */
    size_t total_allocated;    /* Total bytes allocated across all chunks */
    int num_chunks;            /* Number of chunks allocated */
} Arena;

/* ============================================================================
 * Arena Lifecycle
 * ============================================================================ */

/*
 * arena_create() -> Arena*
 *
 * Create a new arena allocator with default chunk size (64KB).
 * Returns NULL on allocation failure.
 */
Arena* arena_create(void);

/*
 * arena_create_with_size(chunk_size) -> Arena*
 *
 * Create a new arena with a custom initial chunk size.
 * The chunk_size will be rounded up to at least ARENA_MIN_CHUNK_SIZE.
 * Returns NULL on allocation failure.
 */
Arena* arena_create_with_size(size_t chunk_size);

/*
 * arena_destroy(arena) -> void
 *
 * Destroy the arena and free all associated memory including
 * all chunks and the arena structure itself.
 */
void arena_destroy(Arena* arena);

/* ============================================================================
 * Allocation
 * ============================================================================ */

/*
 * arena_alloc(arena, size) -> void*
 *
 * Allocate `size` bytes from the arena with 8-byte alignment.
 * Returns NULL if allocation fails (out of memory).
 * Memory is zero-initialized.
 */
void* arena_alloc(Arena* arena, size_t size);

/*
 * arena_alloc_aligned(arena, size, align) -> void*
 *
 * Allocate `size` bytes with specified alignment (must be power of 2).
 * Alignment must be <= 256 and a power of 2.
 * Returns NULL if allocation fails.
 */
void* arena_alloc_aligned(Arena* arena, size_t size, size_t align);

/* ============================================================================
 * Deallocation
 * ============================================================================ */

/*
 * arena_dealloc_all(arena) -> void
 *
 * Reset the arena, freeing all chunks except keeping one empty chunk
 * for reuse. This effectively clears all allocations but keeps the
 * arena ready for new allocations.
 */
void arena_dealloc_all(Arena* arena);

/*
 * arena_reset(arena) -> void
 *
 * Alias for arena_dealloc_all().
 */
#define arena_reset(arena) arena_dealloc_all(arena)

/* ============================================================================
 * Query
 * ============================================================================ */

/*
 * arena_bytes_used(arena) -> size_t
 *
 * Get total bytes allocated from this arena.
 */
size_t arena_bytes_used(const Arena* arena);

/*
 * arena_num_chunks(arena) -> int
 *
 * Get number of chunks allocated for this arena.
 */
int arena_num_chunks(const Arena* arena);

#ifdef __cplusplus
}
#endif

#endif /* GRADIENT_ARENA_H */
