/*
 * Generational References Implementation
 *
 * Tier 2 memory model: mutable aliasing with generation tracking.
 * Each allocation has a monotonically increasing generation counter.
 * References store (ptr, generation) and check on dereference.
 */

#include "genref.h"
#include <stdlib.h>
#include <string.h>
#include <assert.h>

/* ============================================================================
 * Internal Helpers
 * ============================================================================ */

/**
 * Get the header pointer from user data pointer.
 * The header is stored immediately before the user data.
 */
GenHeader* genref_get_header(void* ptr) {
    if (!ptr) return NULL;
    return (GenHeader*)((uint8_t*)ptr - GENREF_HEADER_SIZE);
}

/**
 * Validate that a pointer is a valid genref allocation.
 */
static bool is_valid_genref(void* ptr) {
    if (!ptr) return false;
    GenHeader* header = genref_get_header(ptr);
    return header->magic == GENREF_MAGIC;
}

/* ============================================================================
 * Allocation
 * ============================================================================ */

void* genref_alloc(size_t size) {
    if (size == 0) return NULL;
    
    /* Allocate space for header + user data */
    size_t total_size = GENREF_HEADER_SIZE + size;
    void* mem = malloc(total_size);
    if (!mem) return NULL;
    
    /* Initialize header */
    GenHeader* header = (GenHeader*)mem;
    header->generation = 1;  /* Start at generation 1 */
    header->size = size;
    header->magic = GENREF_MAGIC;
    
    /* Return pointer to user data (after header) */
    void* user_ptr = (uint8_t*)mem + GENREF_HEADER_SIZE;
    
    /* Zero-initialize user data */
    memset(user_ptr, 0, size);
    
    return user_ptr;
}

void genref_free(void* ptr) {
    if (!ptr) return;
    
    /* Validate this is actually a genref allocation */
    GenHeader* header = genref_get_header(ptr);
    if (header->magic != GENREF_MAGIC) {
        /* Not a valid genref - ignore or could assert in debug builds */
        return;
    }
    
    /* Increment generation to invalidate all existing references */
    header->generation++;
    
    /* Clear magic to mark as freed */
    header->magic = 0;
    
    /* Free the entire block including header */
    free(header);
}

/* ============================================================================
 * Generational Reference Operations
 * ============================================================================ */

GenRef genref_new(void* ptr) {
    GenRef ref = { NULL, 0 };
    
    if (!ptr) return ref;
    
    GenHeader* header = genref_get_header(ptr);
    if (header->magic != GENREF_MAGIC) {
        /* Not a valid genref allocation */
        return ref;
    }
    
    ref.ptr = ptr;
    ref.generation = header->generation;
    return ref;
}

void* genref_get(GenRef ref) {
    if (!ref.ptr) return NULL;
    
    GenHeader* header = genref_get_header(ref.ptr);
    if (header->magic != GENREF_MAGIC) {
        /* Allocation was freed or corrupted */
        return NULL;
    }
    
    /* Check generation match */
    if (header->generation != ref.generation) {
        /* Reference is stale - allocation was updated */
        return NULL;
    }
    
    return ref.ptr;
}

bool genref_set(GenRef ref, void* new_ptr) {
    if (!ref.ptr || !new_ptr) return false;
    
    GenHeader* header = genref_get_header(ref.ptr);
    if (header->magic != GENREF_MAGIC) {
        /* Not a valid genref or already freed */
        return false;
    }
    
    /* Validate ref's generation matches current */
    if (header->generation != ref.generation) {
        /* Reference is stale */
        return false;
    }
    
    /* Increment generation - invalidates all existing references */
    header->generation++;
    
    /* Update the allocation to point to new data */
    /* Note: This copies the content from new_ptr to the allocation */
    memcpy(ref.ptr, new_ptr, header->size);
    
    return true;
}

uint64_t genref_get_generation(void* ptr) {
    if (!ptr) return 0;
    
    GenHeader* header = genref_get_header(ptr);
    if (header->magic != GENREF_MAGIC) {
        return 0;
    }
    
    return header->generation;
}

bool genref_is_valid(GenRef ref) {
    return genref_get(ref) != NULL;
}

/* ============================================================================
 * Gradient Runtime Interface (for compiler integration)
 * ============================================================================ */

/**
 * __gradient_genref_alloc(size) -> void*
 *
 * Gradient runtime interface for generational allocation.
 * Called by compiled Gradient code.
 */
void* __gradient_genref_alloc(int64_t size) {
    if (size <= 0) return NULL;
    return genref_alloc((size_t)size);
}

/**
 * __gradient_genref_new(ptr) -> GenRef
 *
 * Gradient runtime interface to create a GenRef.
 * Returns the GenRef as a value (passed by value on most ABIs).
 */
GenRef __gradient_genref_new(void* ptr) {
    return genref_new(ptr);
}

/**
 * __gradient_genref_get(ref) -> void*
 *
 * Gradient runtime interface to dereference a GenRef.
 * Returns NULL if the reference is stale.
 */
void* __gradient_genref_get(GenRef ref) {
    return genref_get(ref);
}

/**
 * __gradient_genref_set(ref, new_ptr) -> int64_t
 *
 * Gradient runtime interface to update via GenRef.
 * Returns 1 on success, 0 on failure (stale reference).
 */
int64_t __gradient_genref_set(GenRef ref, void* new_ptr) {
    return genref_set(ref, new_ptr) ? 1 : 0;
}

/**
 * __gradient_genref_free(ptr) -> void
 *
 * Gradient runtime interface to free a generational allocation.
 */
void __gradient_genref_free(void* ptr) {
    genref_free(ptr);
}

/**
 * __gradient_genref_is_valid(ref) -> int64_t
 *
 * Gradient runtime interface to check GenRef validity.
 * Returns 1 if valid, 0 if stale.
 */
int64_t __gradient_genref_is_valid(GenRef ref) {
    return genref_is_valid(ref) ? 1 : 0;
}
