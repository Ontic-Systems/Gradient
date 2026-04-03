/*
 * Generational References Header
 *
 * Generational references enable mutable aliasing without borrow checker.
 * Each allocation has a generation number; references store (ptr, generation).
 * Dereference checks generation match - a form of lightweight memory safety.
 * 
 * This is Tier 2 of Gradient's memory model for graph structures and
 * observer patterns.
 */

#ifndef GRADIENT_GENREF_H
#define GRADIENT_GENREF_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ============================================================================
 * Generational Reference Types
 * ============================================================================ */

/**
 * A generational reference: pointer + generation number.
 * 
 * The generation is checked against the allocation's current generation
 * on dereference. If they don't match, the reference is stale.
 */
typedef struct GenRef {
    void* ptr;           /* Pointer to the allocation */
    uint64_t generation; /* Generation at time of reference creation */
} GenRef;

/**
 * Header for generational allocations.
 * 
 * This header is stored before the user-visible data in each allocation
 * made via genref_alloc(). It tracks the current generation and allocation
 * size for debugging and safety checks.
 */
typedef struct GenHeader {
    uint64_t generation; /* Current generation, incremented on each update */
    size_t size;         /* Size of user allocation (for debugging) */
    uint32_t magic;      /* Magic number for header validation */
} GenHeader;

/* Magic number for header validation: "GENR" in hex */
#define GENREF_MAGIC 0x47454E52

/* Size of the header that precedes user data */
#define GENREF_HEADER_SIZE sizeof(GenHeader)

/* ============================================================================
 * Allocation
 * ============================================================================ */

/**
 * genref_alloc(size) -> void*
 *
 * Allocate memory with generation tracking. The returned pointer points to
 * the user-visible data (after the internal GenHeader).
 *
 * The allocation starts at generation 1. Use genref_new() to create
 * a GenRef pointing to this allocation.
 *
 * Returns NULL on allocation failure.
 */
void* genref_alloc(size_t size);

/**
 * genref_free(ptr) -> void
 *
 * Free memory allocated with genref_alloc().
 * This also invalidates all existing GenRefs by incrementing the generation.
 */
void genref_free(void* ptr);

/* ============================================================================
 * Generational Reference Operations
 * ============================================================================ */

/**
 * genref_new(ptr) -> GenRef
 *
 * Create a GenRef pointing to an allocation made with genref_alloc().
 * Captures the current generation of the allocation.
 */
GenRef genref_new(void* ptr);

/**
 * genref_get(ref) -> void* | NULL
 *
 * Validate and dereference a GenRef. Checks if the stored generation
 * matches the allocation's current generation.
 *
 * Returns the pointer if valid, NULL if the reference is stale
 * (generation mismatch indicates the allocation was updated/reused).
 */
void* genref_get(GenRef ref);

/**
 * genref_set(ref, new_ptr) -> bool
 *
 * Update the allocation pointed to by ref to point to new_ptr.
 * This operation:
 * 1. Validates ref's generation against the allocation
 * 2. If valid, increments the allocation's generation
 * 3. Updates the allocation to point to new_ptr
 * 4. Returns true on success, false if ref was stale
 *
 * After this call, all existing GenRefs to the allocation become stale.
 */
bool genref_set(GenRef ref, void* new_ptr);

/**
 * genref_get_generation(ptr) -> uint64_t
 *
 * Get the current generation of an allocation.
 * Returns 0 if ptr is not a valid genref allocation.
 */
uint64_t genref_get_generation(void* ptr);

/**
 * genref_is_valid(ref) -> bool
 *
 * Check if a GenRef is still valid (generation matches).
 * This is a predicate version of genref_get() for boolean checks.
 */
bool genref_is_valid(GenRef ref);

/* ============================================================================
 * Internal/Unsafe Operations (for runtime use)
 * ============================================================================ */

/**
 * genref_get_header(ptr) -> GenHeader*
 *
 * Get the GenHeader for an allocation. ptr must be a valid genref allocation.
 * Internal use only - normal code should use the high-level operations.
 */
GenHeader* genref_get_header(void* ptr);

#ifdef __cplusplus
}
#endif

#endif /* GRADIENT_GENREF_H */
