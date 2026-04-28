//! GRA-178 regression tests: arena allocator overflow safety.
//!
//! These tests link `runtime/memory/arena.c` directly (not via the
//! `__gradient_arena_*` wrappers) so we can pass attacker-shaped
//! `size_t` values that would not survive the wrapper's `int64_t`
//! signature.  They verify that the checked arithmetic in
//! `arena_alloc_aligned` and `arena_chunk_new` returns NULL on overflow
//! instead of wrapping and producing an undersized buffer.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn arena_c_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler -> codebase parent")
        .join("runtime")
        .join("memory")
        .join("arena.c")
}

fn arena_h_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler -> codebase parent")
        .join("runtime")
        .join("memory")
}

fn build_and_run(test_name: &str, source: &str) {
    let tmp = TempDir::new().expect("tempdir");
    let src = tmp.path().join(format!("{test_name}.c"));
    let bin = tmp.path().join(test_name);
    fs::write(&src, source).expect("write source");

    let arena_c = arena_c_path();
    let inc = arena_h_dir();

    let status = Command::new("cc")
        .arg("-I")
        .arg(&inc)
        .arg(&src)
        .arg(&arena_c)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("compile");
    assert!(status.success(), "compile failed for {test_name}");

    let output = Command::new(&bin)
        .env("ASAN_OPTIONS", "halt_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1")
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{test_name} failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn arena_chunk_new_rejects_size_max() {
    // arena_create_with_size(SIZE_MAX) would compute `sizeof(ArenaChunk) + SIZE_MAX`
    // which wraps to a tiny value; malloc would succeed and any subsequent write
    // into ->data would corrupt the heap.  The fix returns NULL on overflow.
    let source = r#"
#include <stddef.h>
#include <stdint.h>
#include "arena.h"

int main(void) {
    Arena* a = arena_create_with_size((size_t)-1);
    /* Must fail cleanly: NULL, not a corrupt arena pointing at a 1-byte buffer. */
    return a == NULL ? 0 : 1;
}
"#;
    build_and_run("arena_chunk_size_overflow", source);
}

#[test]
fn arena_alloc_aligned_rejects_size_max() {
    // arena_alloc_aligned(arena, SIZE_MAX, align) must reject before computing
    // size+align (which would overflow) and before computing aligned+size (which
    // would wrap the bump pointer past end_ptr).
    let source = r#"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include "arena.h"

int main(void) {
    Arena* a = arena_create();
    if (!a) return 1;

    void* p1 = arena_alloc_aligned(a, (size_t)-1, 8);
    if (p1 != NULL) {
        arena_destroy(a);
        return 2; /* must reject SIZE_MAX */
    }

    /* Just shy of SIZE_MAX is still impossible to satisfy and must be NULL. */
    void* p2 = arena_alloc_aligned(a, (size_t)-1 - 8, 8);
    if (p2 != NULL) {
        arena_destroy(a);
        return 3;
    }

    /* A reasonable allocation must still succeed after the rejected ones. */
    void* p3 = arena_alloc_aligned(a, 64, 8);
    if (p3 == NULL) {
        arena_destroy(a);
        return 4;
    }

    arena_destroy(a);
    return 0;
}
"#;
    build_and_run("arena_alloc_size_overflow", source);
}

#[test]
fn arena_alloc_aligned_zero_size_returns_null() {
    // Pre-existing contract: zero-size allocations return NULL.  Re-asserted
    // here so future overflow-rejection logic doesn't accidentally treat 0 as
    // a valid allocation.
    let source = r#"
#include <stddef.h>
#include <stdint.h>
#include "arena.h"

int main(void) {
    Arena* a = arena_create();
    if (!a) return 1;
    void* p = arena_alloc_aligned(a, 0, 8);
    int ok = (p == NULL);
    arena_destroy(a);
    return ok ? 0 : 2;
}
"#;
    build_and_run("arena_alloc_zero_size", source);
}

#[test]
fn arena_repeated_alloc_does_not_wrap_bump_pointer() {
    // Sanity: many allocations that together approach but never exceed the
    // chunk size must succeed; the next one that would push past end_ptr
    // must trigger a new chunk rather than wrapping.
    let source = r#"
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include "arena.h"

int main(void) {
    Arena* a = arena_create_with_size(4096);
    if (!a) return 1;

    /* Fill the first chunk with small allocations. */
    for (int i = 0; i < 100; i++) {
        void* p = arena_alloc_aligned(a, 32, 8);
        if (p == NULL) {
            arena_destroy(a);
            return 2;
        }
    }

    /* This allocation is larger than what's left and must trigger a new
     * chunk via the checked size+align path; must succeed, not wrap. */
    void* big = arena_alloc_aligned(a, 8192, 8);
    int ok = (big != NULL) && (arena_num_chunks(a) >= 2);
    arena_destroy(a);
    return ok ? 0 : 3;
}
"#;
    build_and_run("arena_repeated_alloc", source);
}
