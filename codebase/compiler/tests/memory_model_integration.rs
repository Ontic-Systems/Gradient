//! Memory Model Integration Tests
//!
//! These tests verify the three-tier memory model:
//! - Tier 1: Arena allocation with defer pattern
//! - Tier 2: Generational references for mutable aliasing
//! - Tier 3: Linear types for unique ownership
//!
//! Each tier builds upon the previous, providing increasing
//! safety guarantees at the cost of some flexibility.

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Compile a C test program with the runtime and run it
fn compile_c_test(c_source: &str) -> (String, i32) {
    let tmp = TempDir::new().expect("failed to create temp dir");

    // Write C test program
    let test_c_path = tmp.path().join("test.c");
    fs::write(&test_c_path, c_source).expect("write test.c");

    // Compile test program with arena runtime
    let runtime_src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("gradient_runtime.c");

    let test_obj = tmp.path().join("test.o");
    let runtime_obj = tmp.path().join("runtime.o");
    let bin_path = tmp.path().join("test");

    // Compile test.c
    let cc_compile_test = Command::new("cc")
        .arg("-c")
        .arg(&test_c_path)
        .arg("-o")
        .arg(&test_obj)
        .status()
        .expect("cc compile test.c");
    assert!(cc_compile_test.success(), "test.c compile failed");

    // Compile runtime
    let cc_compile_runtime = Command::new("cc")
        .arg("-c")
        .arg(&runtime_src)
        .arg("-o")
        .arg(&runtime_obj)
        .status()
        .expect("cc compile runtime");
    assert!(cc_compile_runtime.success(), "runtime compile failed");

    // Link
    let link_status = Command::new("cc")
        .arg(&test_obj)
        .arg(&runtime_obj)
        .arg("-o")
        .arg(&bin_path)
        .arg("-lcurl")
        .status()
        .expect("cc link");
    assert!(link_status.success(), "link failed");

    // Run
    let output = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}

/// Compile and run a Gradient test program
#[allow(dead_code)]
fn compile_gradient_test(source: &str) -> (String, i32) {
    let tmp = TempDir::new().expect("failed to create temp dir");

    // Write Gradient source
    let test_gr_path = tmp.path().join("test.gr");
    fs::write(&test_gr_path, source).expect("write test.gr");

    // Find the gradient compiler
    let compiler = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("debug")
        .join("gradientc");

    let bin_path = tmp.path().join("test");

    // Compile with gradient
    let compile_status = Command::new(&compiler)
        .arg(&test_gr_path)
        .arg("-o")
        .arg(&bin_path)
        .status()
        .expect("gradient compile");

    if !compile_status.success() {
        return ("compile error".to_string(), 1);
    }

    // Run
    let output = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .output()
        .expect("run binary");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}

// ============================================================================
// Tier 1: Arena + Defer Pattern Tests
// ============================================================================

#[test]
fn test_tier1_arena_defer_pattern() {
    // Simulates the pattern:
    //   arena = Arena.new()
    //   defer arena.deinit()
    //   ... use arena ...

    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

// Simulates defer pattern: allocate, use, then cleanup
int process_with_arena() {
    void* arena = __gradient_arena_create();
    if (!arena) return 1;
    
    // Allocate working buffer
    int* buffer = (int*)__gradient_arena_alloc(arena, 100 * sizeof(int));
    if (!buffer) {
        __gradient_arena_destroy(arena);
        return 1;
    }
    
    // Use the buffer
    for (int i = 0; i < 100; i++) {
        buffer[i] = i * 2;
    }
    
    // Verify
    int sum = 0;
    for (int i = 0; i < 100; i++) {
        sum += buffer[i];
    }
    
    // This is the "defer" cleanup
    __gradient_arena_destroy(arena);
    
    // Expected sum: 2 * (0 + 1 + ... + 99) = 2 * 4950 = 9900
    return (sum == 9900) ? 0 : 1;
}

int main() {
    int result = process_with_arena();
    if (result == 0) {
        printf("PASS: arena + defer pattern\n");
    } else {
        printf("FAIL: arena + defer pattern\n");
    }
    return result;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_tier1_arena_multiple_defers() {
    // Tests multiple arenas with nested scope-like defer patterns

    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

// Simulates nested scopes with their own arenas
int nested_arenas() {
    // Outer scope arena
    void* outer = __gradient_arena_create();
    if (!outer) return 1;
    
    int* outer_buf = (int*)__gradient_arena_alloc(outer, 10 * sizeof(int));
    if (!outer_buf) {
        __gradient_arena_destroy(outer);
        return 1;
    }
    
    for (int i = 0; i < 10; i++) {
        outer_buf[i] = i;
    }
    
    // Inner scope arena
    void* inner = __gradient_arena_create();
    if (!inner) {
        __gradient_arena_destroy(outer);
        return 1;
    }
    
    int* inner_buf = (int*)__gradient_arena_alloc(inner, 5 * sizeof(int));
    if (!inner_buf) {
        __gradient_arena_destroy(inner);
        __gradient_arena_destroy(outer);
        return 1;
    }
    
    for (int i = 0; i < 5; i++) {
        inner_buf[i] = outer_buf[i] * 10;
    }
    
    // Verify inner results
    for (int i = 0; i < 5; i++) {
        if (inner_buf[i] != i * 10) {
            __gradient_arena_destroy(inner);
            __gradient_arena_destroy(outer);
            return 1;
        }
    }
    
    // Inner defer (cleanup inner first)
    __gradient_arena_destroy(inner);
    
    // Verify outer still valid
    for (int i = 0; i < 10; i++) {
        if (outer_buf[i] != i) {
            __gradient_arena_destroy(outer);
            return 1;
        }
    }
    
    // Outer defer
    __gradient_arena_destroy(outer);
    
    return 0;
}

int main() {
    int result = nested_arenas();
    if (result == 0) {
        printf("PASS: multiple arenas with nested defers\n");
    } else {
        printf("FAIL: multiple arenas with nested defers\n");
    }
    return result;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_tier1_arena_reset_pattern() {
    // Tests the arena reset pattern for temporary allocations in a loop

    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_dealloc_all(void* arena);
extern void __gradient_arena_destroy(void* arena);

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create\n");
        return 1;
    }
    
    int iteration_results[3] = {0, 0, 0};
    
    // Simulate a loop with temporary allocations
    for (int iter = 0; iter < 3; iter++) {
        // Allocate temporary buffer for this iteration
        int* buf = (int*)__gradient_arena_alloc(arena, 10 * sizeof(int));
        if (!buf) {
            printf("FAIL: allocation in iteration %d\n", iter);
            __gradient_arena_destroy(arena);
            return 1;
        }
        
        // Use the buffer
        for (int i = 0; i < 10; i++) {
            buf[i] = iter * 100 + i;
        }
        
        // Store result
        int sum = 0;
        for (int i = 0; i < 10; i++) {
            sum += buf[i];
        }
        iteration_results[iter] = sum;
        
        // Reset arena for next iteration (like defer in a loop)
        __gradient_arena_dealloc_all(arena);
    }
    
    __gradient_arena_destroy(arena);
    
    // Verify results
    // iter 0: 0+1+2+...+9 = 45
    // iter 1: 100+101+...+109 = 1045
    // iter 2: 200+201+...+209 = 2045
    if (iteration_results[0] != 45 || 
        iteration_results[1] != 1045 || 
        iteration_results[2] != 2045) {
        printf("FAIL: incorrect results\n");
        return 1;
    }
    
    printf("PASS: arena reset pattern\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

// ============================================================================
// Tier 2: Generational References Tests
// ============================================================================

#[test]
fn test_tier2_genref_basic() {
    // Tests basic generational reference operations

    let c_source = r#"
#include <stdio.h>
#include <stdint.h>
#include <string.h>

// GenRef struct (must match runtime definition)
typedef struct GenRef {
    void* ptr;
    uint64_t generation;
} GenRef;

extern void* __gradient_genref_alloc(int64_t size);
extern void __gradient_genref_free(void* ptr);
extern GenRef __gradient_genref_new(void* ptr);
extern void* __gradient_genref_get(GenRef ref);
extern int64_t __gradient_genref_is_valid(GenRef ref);

int main() {
    // Allocate generational memory
    int* data = (int*)__gradient_genref_alloc(sizeof(int) * 5);
    if (!data) {
        printf("FAIL: genref_alloc\n");
        return 1;
    }
    
    // Initialize data
    for (int i = 0; i < 5; i++) {
        data[i] = i * 10;
    }
    
    // Create a generational reference
    GenRef ref = __gradient_genref_new(data);
    
    // Dereference and verify
    int* retrieved = (int*)__gradient_genref_get(ref);
    if (!retrieved) {
        printf("FAIL: genref_get returned NULL\n");
        __gradient_genref_free(data);
        return 1;
    }
    
    // Verify data is correct
    for (int i = 0; i < 5; i++) {
        if (retrieved[i] != i * 10) {
            printf("FAIL: data mismatch at index %d\n", i);
            __gradient_genref_free(data);
            return 1;
        }
    }
    
    // Check reference is valid
    if (!__gradient_genref_is_valid(ref)) {
        printf("FAIL: ref should be valid\n");
        __gradient_genref_free(data);
        return 1;
    }
    
    // Free the allocation - this invalidates the reference
    __gradient_genref_free(data);
    
    // Now the reference should be invalid (generation mismatch)
    if (__gradient_genref_is_valid(ref)) {
        printf("FAIL: ref should be invalid after free\n");
        return 1;
    }
    
    // Dereferencing should return NULL
    void* after_free = __gradient_genref_get(ref);
    if (after_free != NULL) {
        printf("FAIL: genref_get should return NULL after free\n");
        return 1;
    }
    
    printf("PASS: generational references basic\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_tier2_genref_observer_pattern() {
    // Tests the observer pattern with generational references
    // Multiple observers can reference the same data

    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

// GenRef struct (must match runtime definition)
typedef struct GenRef {
    void* ptr;
    uint64_t generation;
} GenRef;

extern void* __gradient_genref_alloc(int64_t size);
extern void __gradient_genref_free(void* ptr);
extern GenRef __gradient_genref_new(void* ptr);
extern void* __gradient_genref_get(GenRef ref);
extern int64_t __gradient_genref_is_valid(GenRef ref);
extern int64_t __gradient_genref_set(GenRef ref, void* new_ptr);

int main() {
    // Allocate shared data
    int* shared = (int*)__gradient_genref_alloc(sizeof(int) * 3);
    if (!shared) {
        printf("FAIL: genref_alloc\n");
        return 1;
    }
    
    shared[0] = 100;
    shared[1] = 200;
    shared[2] = 300;
    
    // Create multiple observer references
    GenRef observer1 = __gradient_genref_new(shared);
    GenRef observer2 = __gradient_genref_new(shared);
    GenRef observer3 = __gradient_genref_new(shared);
    
    // All observers see the same data
    int* data1 = (int*)__gradient_genref_get(observer1);
    int* data2 = (int*)__gradient_genref_get(observer2);
    int* data3 = (int*)__gradient_genref_get(observer3);
    
    if (!data1 || !data2 || !data3) {
        printf("FAIL: observer dereference failed\n");
        __gradient_genref_free(shared);
        return 1;
    }
    
    if (data1[0] != 100 || data2[1] != 200 || data3[2] != 300) {
        printf("FAIL: observer data mismatch\n");
        __gradient_genref_free(shared);
        return 1;
    }
    
    // Allocate new data
    int* new_data = (int*)__gradient_genref_alloc(sizeof(int) * 3);
    if (!new_data) {
        printf("FAIL: alloc new_data\n");
        __gradient_genref_free(shared);
        return 1;
    }
    
    new_data[0] = 1000;
    new_data[1] = 2000;
    new_data[2] = 3000;
    
    // Update via one observer - this invalidates all other references
    // (In real Gradient code, this would be done via genref_set)
    // For this test, we simulate by freeing and noting all become invalid
    
    // Verify all observers were valid before update
    if (!__gradient_genref_is_valid(observer1) || 
        !__gradient_genref_is_valid(observer2) || 
        !__gradient_genref_is_valid(observer3)) {
        printf("FAIL: observers should be valid before free\n");
        __gradient_genref_free(shared);
        __gradient_genref_free(new_data);
        return 1;
    }
    
    // Free the original data - all observers become stale
    __gradient_genref_free(shared);
    
    // All observers should now be invalid
    if (__gradient_genref_is_valid(observer1) || 
        __gradient_genref_is_valid(observer2) || 
        __gradient_genref_is_valid(observer3)) {
        printf("FAIL: observers should be invalid after free\n");
        __gradient_genref_free(new_data);
        return 1;
    }
    
    __gradient_genref_free(new_data);
    
    printf("PASS: generational references observer pattern\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

// ============================================================================
// Tier 3: Linear Types Tests (Placeholder - when implemented)
// ============================================================================

#[test]
fn test_tier3_linear_types_placeholder() {
    // Linear types are not yet fully implemented
    // This test documents the expected behavior

    // When implemented, linear types will provide:
    // - Unique ownership (cannot be duplicated)
    // - Must be consumed exactly once
    // - Drop/defer patterns for cleanup
    // - Move semantics by default

    // Example patterns that will be tested:
    // - File handles that close on drop
    // - Network sockets with guaranteed cleanup
    // - Mutex guards that unlock when moved out of scope

    // For now, we just verify the test framework is ready
    assert!(true, "Linear types tests ready for implementation");
}

// ============================================================================
// Combined Pattern Tests
// ============================================================================

#[test]
fn test_combined_arena_and_genref() {
    // Tests using arena for bulk allocation and genref for shared access

    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

// GenRef struct
typedef struct GenRef {
    void* ptr;
    uint64_t generation;
} GenRef;

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

extern void* __gradient_genref_alloc(int64_t size);
extern void __gradient_genref_free(void* ptr);
extern GenRef __gradient_genref_new(void* ptr);
extern void* __gradient_genref_get(GenRef ref);
extern int64_t __gradient_genref_is_valid(GenRef ref);

typedef struct Node {
    int value;
    GenRef next;  // Generational reference to next node
} Node;

int main() {
    // Use arena for temporary node storage during graph construction
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create\n");
        return 1;
    }
    
    // Use arena for temporary values used while constructing the graph.
    int* scratch = (int*)__gradient_arena_alloc(arena, sizeof(int) * 3);
    if (!scratch) {
        printf("FAIL: arena_alloc\n");
        __gradient_arena_destroy(arena);
        return 1;
    }
    scratch[0] = 1;
    scratch[1] = 2;
    scratch[2] = 3;
    
    // Allocate graph nodes with genref so references have generation headers.
    Node* node0 = (Node*)__gradient_genref_alloc(sizeof(Node));
    Node* node1 = (Node*)__gradient_genref_alloc(sizeof(Node));
    Node* node2 = (Node*)__gradient_genref_alloc(sizeof(Node));
    if (!node0 || !node1 || !node2) {
        printf("FAIL: genref_alloc\n");
        __gradient_genref_free(node0);
        __gradient_genref_free(node1);
        __gradient_genref_free(node2);
        __gradient_arena_destroy(arena);
        return 1;
    }
    
    node0->value = scratch[0];
    node1->value = scratch[1];
    node2->value = scratch[2];
    
    // Create generational references between genref-allocated nodes.
    node0->next = __gradient_genref_new(node1);
    node1->next = __gradient_genref_new(node2);
    
    // Verify chain traversal using genref
    int sum = 0;
    Node* current = node0;
    while (1) {
        sum += current->value;
        
        Node* next = (Node*)__gradient_genref_get(current->next);
        if (!next) break;
        current = next;
    }
    
    if (sum != 6) {  // 1 + 2 + 3
        printf("FAIL: sum should be 6, got %d\n", sum);
        __gradient_arena_destroy(arena);
        return 1;
    }
    
    // Arena cleanup frees scratch construction data; graph nodes are genref-owned.
    __gradient_arena_destroy(arena);

    __gradient_genref_free(node0);
    __gradient_genref_free(node1);
    __gradient_genref_free(node2);
    
    printf("PASS: combined arena and genref\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_combined_all_three_tiers() {
    // Integration test combining all three tiers
    // This is a comprehensive test of the full memory model

    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

// GenRef struct
typedef struct GenRef {
    void* ptr;
    uint64_t generation;
} GenRef;

// Tier 1: Arena
extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

// Tier 2: Generational References
extern void* __gradient_genref_alloc(int64_t size);
extern void __gradient_genref_free(void* ptr);
extern GenRef __gradient_genref_new(void* ptr);
extern void* __gradient_genref_get(GenRef ref);
extern int64_t __gradient_genref_is_valid(GenRef ref);

// Simulated Tier 3: Linear type pattern (unique ownership)
// In real implementation, this would be compiler-enforced
typedef struct LinearHandle {
    int id;
    int consumed;
} LinearHandle;

LinearHandle linear_create(int id) {
    LinearHandle h = {id, 0};
    return h;
}

void linear_consume(LinearHandle* h) {
    if (h->consumed) {
        printf("ERROR: double consume of linear handle\n");
    }
    h->consumed = 1;
}

int main() {
    // Tier 3: Create a linear resource (unique ownership)
    LinearHandle resource = linear_create(42);
    
    // Tier 1: Use arena for temporary working memory
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create\n");
        return 1;
    }
    
    // Process data using arena allocation
    int* work_buffer = (int*)__gradient_arena_alloc(arena, 10 * sizeof(int));
    if (!work_buffer) {
        printf("FAIL: arena_alloc\n");
        __gradient_arena_destroy(arena);
        return 1;
    }
    
    // Initialize work buffer with data derived from linear resource
    for (int i = 0; i < 10; i++) {
        work_buffer[i] = resource.id * i;
    }
    
    // Tier 2: Create generational reference to share results
    int* result = (int*)__gradient_genref_alloc(sizeof(int));
    if (!result) {
        printf("FAIL: genref_alloc\n");
        __gradient_arena_destroy(arena);
        return 1;
    }
    
    // Compute result using arena buffer
    int sum = 0;
    for (int i = 0; i < 10; i++) {
        sum += work_buffer[i];
    }
    *result = sum;
    
    GenRef result_ref = __gradient_genref_new(result);
    
    // Tier 1: Cleanup arena (defer pattern)
    __gradient_arena_destroy(arena);
    
    // Verify result through generational reference
    int* final_result = (int*)__gradient_genref_get(result_ref);
    if (!final_result || *final_result != 1890) {  // 42 * (0+1+...+9) = 42 * 45 = 1890
        printf("FAIL: result verification failed\n");
        __gradient_genref_free(result);
        return 1;
    }
    
    // Tier 2: Cleanup generational allocation
    __gradient_genref_free(result);
    
    // Verify reference is now stale
    if (__gradient_genref_is_valid(result_ref)) {
        printf("FAIL: result_ref should be invalid\n");
        return 1;
    }
    
    // Tier 3: Consume linear resource
    linear_consume(&resource);
    
    printf("PASS: combined all three tiers\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

// ============================================================================
// Performance and Stress Tests
// ============================================================================

#[test]
fn test_arena_vs_malloc_performance() {
    // Compares arena allocation pattern vs individual malloc/free
    // This is more of a benchmark than a correctness test

    let c_source = r#"
#include <stdio.h>
#include <stdint.h>
#include <time.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

#define NUM_ALLOCS 10000
#define ITERATIONS 100

int main() {
    clock_t start, end;
    
    // Test arena allocation pattern
    start = clock();
    for (int iter = 0; iter < ITERATIONS; iter++) {
        void* arena = __gradient_arena_create();
        if (!arena) return 1;
        
        for (int i = 0; i < NUM_ALLOCS; i++) {
            void* p = __gradient_arena_alloc(arena, 64);
            if (!p) {
                __gradient_arena_destroy(arena);
                return 1;
            }
        }
        
        __gradient_arena_destroy(arena);
    }
    end = clock();
    double arena_time = ((double)(end - start)) / CLOCKS_PER_SEC;
    
    // Test malloc/free pattern
    start = clock();
    for (int iter = 0; iter < ITERATIONS; iter++) {
        void* ptrs[NUM_ALLOCS];
        for (int i = 0; i < NUM_ALLOCS; i++) {
            ptrs[i] = malloc(64);
            if (!ptrs[i]) {
                for (int j = 0; j < i; j++) free(ptrs[j]);
                return 1;
            }
        }
        for (int i = 0; i < NUM_ALLOCS; i++) {
            free(ptrs[i]);
        }
    }
    end = clock();
    double malloc_time = ((double)(end - start)) / CLOCKS_PER_SEC;
    
    printf("Arena time: %.4f seconds\n", arena_time);
    printf("Malloc time: %.4f seconds\n", malloc_time);
    
    // Arena should generally be faster for bulk deallocation
    // We don't assert this as it depends on the system
    printf("PASS: performance comparison complete\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}
