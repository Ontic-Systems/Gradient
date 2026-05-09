/*
 * Gradient allocator strategy: pluggable
 *
 * Selected by `gradient build` when the main module declares
 * `@allocator(pluggable)`. The runtime defines NEITHER
 * `__gradient_alloc` NOR `__gradient_free` here — instead, it
 * declares them as `extern` symbols that the embedder MUST resolve
 * at link time. This is the C ABI shape of the `Allocator` trait:
 * a pair of function symbols matching the contract documented in
 * `runtime_allocator_default.c`.
 *
 * Use case: `no_std` builds, embedded targets, or applications that
 * want to plug a bumpalo-style arena, slab allocator, or other
 * custom allocator under the same C ABI vtable. Concrete
 * implementations (bumpalo / slab) ship as separate runtime crates
 * in a future PR (E5 #336 follow-on); today the contract is the
 * trait surface plus the integration test fixture providing a
 * minimal user-space implementation.
 *
 * Symbol contract (matches `runtime_allocator_default.c`):
 *
 *   extern void* __gradient_alloc(size_t size)
 *     -- caller-defined. Must allocate `size` bytes or return NULL.
 *
 *   extern void __gradient_free(void* ptr)
 *     -- caller-defined. Must free a pointer previously returned
 *        by `__gradient_alloc` (or no-op on NULL).
 *
 *   const char __gradient_allocator_strategy[]
 *     -- "pluggable" — same role as the `default` variant's tag.
 *        Linking both this and the default variant produces the
 *        intended multi-definition error.
 *
 * If the embedder fails to provide `__gradient_alloc` /
 * `__gradient_free`, the link will fail with an undefined-symbol
 * error — the intended loud-fail behaviour.
 *
 * See codebase/compiler/runtime/allocator/README.md for the dispatch
 * table and the integration-test fixture under
 * `codebase/build-system/tests/allocator_runtime.rs`.
 */

const char __gradient_allocator_strategy[] = "pluggable";
