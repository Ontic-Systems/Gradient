/*
 * Arena Allocator Implementation
 *
 * Bump-pointer arena allocator for efficient temporary memory management.
 */

#include "arena.h"
#include <stdlib.h>
#include <string.h>
#include <assert.h>

/* Internal: Allocate a new chunk */
static ArenaChunk* arena_chunk_new(size_t chunk_size) {
    /* Allocate chunk with flexible array member */
    size_t total_size = sizeof(ArenaChunk) + chunk_size;
    ArenaChunk* chunk = (ArenaChunk*)malloc(total_size);
    if (!chunk) return NULL;
    
    chunk->next = NULL;
    chunk->size = chunk_size;
    chunk->used = 0;
    /* Don't initialize data - arena_alloc will zero memory */
    
    return chunk;
}

/* Internal: Free all chunks in a linked list */
static void arena_chunks_free(ArenaChunk* chunk) {
    while (chunk) {
        ArenaChunk* next = chunk->next;
        free(chunk);
        chunk = next;
    }
}

/* ============================================================================
 * Arena Lifecycle
 * ============================================================================ */

Arena* arena_create(void) {
    return arena_create_with_size(ARENA_DEFAULT_CHUNK_SIZE);
}

Arena* arena_create_with_size(size_t chunk_size) {
    /* Enforce minimum chunk size */
    if (chunk_size < ARENA_MIN_CHUNK_SIZE) {
        chunk_size = ARENA_MIN_CHUNK_SIZE;
    }
    
    /* Allocate arena structure */
    Arena* arena = (Arena*)malloc(sizeof(Arena));
    if (!arena) return NULL;
    
    /* Allocate initial chunk */
    ArenaChunk* chunk = arena_chunk_new(chunk_size);
    if (!chunk) {
        free(arena);
        return NULL;
    }
    
    /* Initialize arena */
    arena->chunks = chunk;
    arena->bump_ptr = chunk->data;
    arena->end_ptr = chunk->data + chunk_size;
    arena->chunk_size = chunk_size;
    arena->total_allocated = 0;
    arena->num_chunks = 1;
    
    return arena;
}

void arena_destroy(Arena* arena) {
    if (!arena) return;
    
    /* Free all chunks */
    arena_chunks_free(arena->chunks);
    
    /* Free arena structure */
    free(arena);
}

/* ============================================================================
 * Allocation
 * ============================================================================ */

/* Round up to nearest multiple of alignment (must be power of 2) */
static inline size_t align_up(size_t size, size_t align) {
    assert((align & (align - 1)) == 0 && "alignment must be power of 2");
    return (size + align - 1) & ~(align - 1);
}

static inline uintptr_t align_ptr_up(uintptr_t ptr, size_t align) {
    assert((align & (align - 1)) == 0 && "alignment must be power of 2");
    return (ptr + align - 1) & ~(align - 1);
}

void* arena_alloc_aligned(Arena* arena, size_t size, size_t align) {
    if (!arena || size == 0) return NULL;
    
    /* Validate alignment */
    if (align == 0) align = 8;
    if (align > 256 || (align & (align - 1)) != 0) {
        align = 8; /* Fallback to 8-byte alignment */
    }
    
    /* Try to allocate from current chunk */
    uintptr_t current = (uintptr_t)arena->bump_ptr;
    uintptr_t aligned = align_ptr_up(current, align);
    size_t padding = aligned - current;
    
    /* Check if there's enough space in current chunk */
    if (aligned + size <= (uintptr_t)arena->end_ptr) {
        /* Zero the padding bytes for safety */
        if (padding > 0) {
            memset((void*)current, 0, padding);
        }
        
        void* result = (void*)aligned;
        arena->bump_ptr = (uint8_t*)(aligned + size);
        arena->chunks->used += padding + size;
        arena->total_allocated += size;
        
        /* Zero the allocated memory */
        memset(result, 0, size);
        
        return result;
    }
    
    /* Need a new chunk */
    /* Calculate required chunk size (must fit this allocation) */
    size_t required_size = size + align;
    size_t new_chunk_size = arena->chunk_size;
    if (required_size > new_chunk_size) {
        new_chunk_size = required_size;
    }
    
    ArenaChunk* new_chunk = arena_chunk_new(new_chunk_size);
    if (!new_chunk) return NULL;
    
    /* Link new chunk to front of list */
    new_chunk->next = arena->chunks;
    arena->chunks = new_chunk;
    arena->num_chunks++;
    
    /* Set up bump pointer in new chunk */
    arena->bump_ptr = new_chunk->data;
    arena->end_ptr = new_chunk->data + new_chunk_size;
    
    /* Allocate from new chunk */
    current = (uintptr_t)arena->bump_ptr;
    aligned = align_ptr_up(current, align);
    padding = aligned - current;
    
    void* result = (void*)aligned;
    arena->bump_ptr = (uint8_t*)(aligned + size);
    new_chunk->used = padding + size;
    arena->total_allocated += size;
    
    /* Zero the allocated memory */
    memset(result, 0, size);
    
    return result;
}

void* arena_alloc(Arena* arena, size_t size) {
    return arena_alloc_aligned(arena, size, 8);
}

/* ============================================================================
 * Deallocation
 * ============================================================================ */

void arena_dealloc_all(Arena* arena) {
    if (!arena) return;
    
    /* Keep the first chunk for reuse, free the rest */
    ArenaChunk* first = arena->chunks;
    if (!first) return;
    
    ArenaChunk* rest = first->next;
    
    /* Reset first chunk */
    first->next = NULL;
    first->used = 0;
    
    /* Free remaining chunks */
    arena_chunks_free(rest);
    
    /* Reset arena state */
    arena->bump_ptr = first->data;
    arena->end_ptr = first->data + first->size;
    arena->total_allocated = 0;
    arena->num_chunks = 1;
}

/* ============================================================================
 * Query
 * ============================================================================ */

size_t arena_bytes_used(const Arena* arena) {
    if (!arena) return 0;
    return arena->total_allocated;
}

int arena_num_chunks(const Arena* arena) {
    if (!arena) return 0;
    return arena->num_chunks;
}
