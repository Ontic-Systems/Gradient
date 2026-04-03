//! Integration tests for Arena Allocator Runtime
//!
//! These tests verify the bump-pointer arena allocator functionality
//! by compiling a small C test program that exercises the arena functions.

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Compile the arena runtime and a test program, then run it
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

#[test]
fn test_arena_basic_allocation() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    void* ptr1 = __gradient_arena_alloc(arena, 64);
    if (!ptr1) {
        printf("FAIL: arena_alloc returned NULL\n");
        return 1;
    }
    
    // Check zero initialization
    unsigned char* data = (unsigned char*)ptr1;
    if (*data != 0) {
        printf("FAIL: memory not zero-initialized\n");
        return 1;
    }
    
    // Write and read back
    *data = 0x42;
    if (*data != 0x42) {
        printf("FAIL: write/read failed\n");
        return 1;
    }
    
    void* ptr2 = __gradient_arena_alloc(arena, 128);
    if (!ptr2) {
        printf("FAIL: second alloc returned NULL\n");
        return 1;
    }
    
    if (ptr1 == ptr2) {
        printf("FAIL: allocations returned same pointer\n");
        return 1;
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: basic allocation\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_multiple_allocations() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    void* ptrs[10];
    for (int i = 0; i < 10; i++) {
        ptrs[i] = __gradient_arena_alloc(arena, (i + 1) * 16);
        if (!ptrs[i]) {
            printf("FAIL: allocation %d returned NULL\n", i);
            return 1;
        }
        // Write marker
        uint64_t* data = (uint64_t*)ptrs[i];
        *data = i;
    }
    
    // Verify markers
    for (int i = 0; i < 10; i++) {
        uint64_t* data = (uint64_t*)ptrs[i];
        if (*data != i) {
            printf("FAIL: marker %d corrupted, got %lu\n", i, *data);
            return 1;
        }
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: multiple allocations\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_multiple_chunks() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    // Allocate 50KB blocks - should trigger multiple chunks (64KB default chunk size)
    int large_size = 50 * 1024;
    void* ptrs[3];
    
    for (int i = 0; i < 3; i++) {
        ptrs[i] = __gradient_arena_alloc(arena, large_size);
        if (!ptrs[i]) {
            printf("FAIL: large allocation %d returned NULL\n", i);
            return 1;
        }
        uint64_t* data = (uint64_t*)ptrs[i];
        *data = i + 100;
    }
    
    // Verify data integrity
    for (int i = 0; i < 3; i++) {
        uint64_t* data = (uint64_t*)ptrs[i];
        if (*data != (uint64_t)(i + 100)) {
            printf("FAIL: data %d corrupted\n", i);
            return 1;
        }
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: multiple chunks\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_alignment() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    int sizes[] = {1, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65};
    int num_sizes = sizeof(sizes) / sizeof(sizes[0]);
    
    for (int i = 0; i < num_sizes; i++) {
        void* ptr = __gradient_arena_alloc(arena, sizes[i]);
        if (!ptr) {
            printf("FAIL: allocation of size %d returned NULL\n", sizes[i]);
            return 1;
        }
        
        uintptr_t addr = (uintptr_t)ptr;
        if (addr % 8 != 0) {
            printf("FAIL: allocation of size %d not 8-byte aligned (addr=%p)\n", sizes[i], ptr);
            return 1;
        }
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: alignment\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_dealloc_all_clears_everything() {
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
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    // First allocations
    void* ptr1 = __gradient_arena_alloc(arena, 100);
    void* ptr2 = __gradient_arena_alloc(arena, 200);
    if (!ptr1 || !ptr2) {
        printf("FAIL: initial allocations failed\n");
        return 1;
    }
    
    uint64_t* data1 = (uint64_t*)ptr1;
    uint64_t* data2 = (uint64_t*)ptr2;
    *data1 = 0xDEADBEEF;
    *data2 = 0xCAFEBABE;
    
    if (*data1 != 0xDEADBEEF || *data2 != 0xCAFEBABE) {
        printf("FAIL: write failed\n");
        return 1;
    }
    
    // Reset arena
    __gradient_arena_dealloc_all(arena);
    
    // New allocation after reset
    void* ptr3 = __gradient_arena_alloc(arena, 50);
    if (!ptr3) {
        printf("FAIL: alloc after reset failed\n");
        return 1;
    }
    
    // Allocate more
    void* ptr4 = __gradient_arena_alloc(arena, 100);
    if (!ptr4) {
        printf("FAIL: second alloc after reset failed\n");
        return 1;
    }
    
    if (ptr3 == ptr4) {
        printf("FAIL: allocations after reset are same\n");
        return 1;
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: dealloc_all clears everything\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_null_handling() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_dealloc_all(void* arena);
extern void __gradient_arena_destroy(void* arena);

int main() {
    // Test null arena with alloc
    void* ptr = __gradient_arena_alloc(NULL, 100);
    if (ptr != NULL) {
        printf("FAIL: alloc with null arena should return NULL\n");
        return 1;
    }
    
    // Test null arena with dealloc_all (should not crash)
    __gradient_arena_dealloc_all(NULL);
    
    // Test null arena with destroy (should not crash)
    __gradient_arena_destroy(NULL);
    
    printf("PASS: null handling\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_zero_size() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    // Test zero size allocation
    void* ptr = __gradient_arena_alloc(arena, 0);
    if (ptr != NULL) {
        printf("FAIL: zero-size allocation should return NULL\n");
        return 1;
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: zero size handling\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_negative_size() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    // Test negative size allocation
    void* ptr = __gradient_arena_alloc(arena, -1);
    if (ptr != NULL) {
        printf("FAIL: negative-size allocation should return NULL\n");
        return 1;
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: negative size handling\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_alloc_after_dealloc() {
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
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    // First allocation phase
    void* ptrs1[5];
    for (int i = 0; i < 5; i++) {
        ptrs1[i] = __gradient_arena_alloc(arena, 1000);
        if (!ptrs1[i]) {
            printf("FAIL: first phase alloc %d failed\n", i);
            return 1;
        }
        uint64_t* data = (uint64_t*)ptrs1[i];
        *data = i;
    }
    
    // Dealloc all
    __gradient_arena_dealloc_all(arena);
    
    // Second allocation phase
    void* ptrs2[5];
    for (int i = 0; i < 5; i++) {
        ptrs2[i] = __gradient_arena_alloc(arena, 1000);
        if (!ptrs2[i]) {
            printf("FAIL: second phase alloc %d failed\n", i);
            return 1;
        }
        uint64_t* data = (uint64_t*)ptrs2[i];
        *data = i + 100;
    }
    
    // Verify second phase data
    for (int i = 0; i < 5; i++) {
        uint64_t* data = (uint64_t*)ptrs2[i];
        if (*data != (uint64_t)(i + 100)) {
            printf("FAIL: second phase data %d incorrect, got %lu\n", i, *data);
            return 1;
        }
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: alloc after dealloc\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_consecutive_allocations_bump_pointer() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    // Allocate small blocks - should be contiguous (bump pointer style)
    int size = 16;
    void* ptr1 = __gradient_arena_alloc(arena, size);
    void* ptr2 = __gradient_arena_alloc(arena, size);
    void* ptr3 = __gradient_arena_alloc(arena, size);
    
    if (!ptr1 || !ptr2 || !ptr3) {
        printf("FAIL: allocation failed\n");
        return 1;
    }
    
    uintptr_t addr1 = (uintptr_t)ptr1;
    uintptr_t addr2 = (uintptr_t)ptr2;
    uintptr_t addr3 = (uintptr_t)ptr3;
    
    // Consecutive allocations should be adjacent (with 8-byte alignment)
    // addr2 should be >= addr1 + 16 (rounded up to multiple of 8 = 16)
    if (addr2 < addr1 + size) {
        printf("FAIL: bump pointer did not advance properly\n");
        return 1;
    }
    if (addr3 < addr2 + size) {
        printf("FAIL: bump pointer did not advance properly (2)\n");
        return 1;
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: bump pointer allocation\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}

#[test]
fn test_arena_stress_many_small_allocations() {
    let c_source = r#"
#include <stdio.h>
#include <stdint.h>

extern void* __gradient_arena_create(void);
extern void* __gradient_arena_alloc(void* arena, int64_t size);
extern void __gradient_arena_destroy(void* arena);

#define NUM_ALLOCS 10000

int main() {
    void* arena = __gradient_arena_create();
    if (!arena) {
        printf("FAIL: arena_create returned NULL\n");
        return 1;
    }
    
    void* ptrs[NUM_ALLOCS];
    
    for (int i = 0; i < NUM_ALLOCS; i++) {
        ptrs[i] = __gradient_arena_alloc(arena, 32);
        if (!ptrs[i]) {
            printf("FAIL: allocation %d failed\n", i);
            return 1;
        }
        
        // Write marker
        uint32_t* data = (uint32_t*)ptrs[i];
        *data = i;
    }
    
    // Verify all markers
    for (int i = 0; i < NUM_ALLOCS; i++) {
        uint32_t* data = (uint32_t*)ptrs[i];
        if (*data != (uint32_t)i) {
            printf("FAIL: marker %d corrupted, got %u\n", i, *data);
            return 1;
        }
    }
    
    __gradient_arena_destroy(arena);
    printf("PASS: stress test\n");
    return 0;
}
"#;

    let (out, code) = compile_c_test(c_source);
    assert_eq!(code, 0, "test failed: {}", out);
    assert!(out.contains("PASS"), "expected PASS, got: {}", out);
}
