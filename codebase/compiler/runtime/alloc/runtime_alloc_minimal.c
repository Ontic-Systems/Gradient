/*
 * Gradient alloc strategy: minimal
 *
 * Selected by `gradient build` when the main module's effect summary does
 * NOT contain `Heap` — i.e. the program is provably free of heap-allocating
 * builtins. The canonical `gradient_runtime.o` is still linked (it carries
 * libc helpers like `__gradient_print_int` that don't allocate), but the
 * `runtime_alloc_full.o` object is omitted entirely.
 *
 * Today the rc/COW machinery still lives as `static` helpers inside
 * `gradient_runtime.c`, so the binary-size delta from `full` -> `minimal`
 * is small (one tag symbol). The next runtime PR will extract the rc/COW
 * symbols out of `gradient_runtime.c` and into `runtime_alloc_full.c`,
 * at which point selecting `minimal` will measurably shrink the binary.
 *
 * Symbol contract:
 *
 *   const char __gradient_alloc_strategy[]
 *     -- "minimal" — visible to `nm`/`strings` and the future
 *        `gradient inspect`. Same name as the `full` variant exports —
 *        linking both produces the intended multi-definition error from
 *        cc, which is the defense-in-depth against accidental double-link.
 *
 * The contract is: if a `@no_std` / heap-free program ever calls into a
 * heap-allocating helper (i.e. the checker missed an allocation site),
 * the link will fail because `runtime_alloc_full.c`'s exported helpers
 * are not present. This is the intended loud-fail behaviour.
 */

const char __gradient_alloc_strategy[] = "minimal";
