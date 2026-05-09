/*
 * Gradient async strategy: none
 *
 * Selected by `gradient build` when the main module's effect summary
 * does NOT contain `Async` — i.e. the program is provably free of
 * async/await, futures, and any async-typed value. The canonical
 * `gradient_runtime.o` is still linked (it carries libc helpers like
 * `__gradient_print_int` that don't touch the async executor), but
 * the `runtime_async_full.o` object is omitted entirely.
 *
 * Today the async executor still lives as `static` helpers inside the
 * canonical runtime, so the binary-size delta from `full` -> `none`
 * is small (one tag symbol). The next runtime PR will extract the
 * async executor symbols out of the canonical runtime and into
 * `runtime_async_full.c`, at which point selecting `none` will
 * measurably shrink the binary.
 *
 * Symbol contract:
 *
 *   const char __gradient_async_strategy[]
 *     -- "none" — visible to `nm`/`strings` and the future
 *        `gradient inspect`. Same name as the `full` variant exports —
 *        linking both produces the intended multi-definition error
 *        from cc, which is the defense-in-depth against accidental
 *        double-link.
 *
 * The contract is: if a program with no `Async` effect ever calls
 * into an async helper (i.e. the checker missed an async op), the
 * link will fail because `runtime_async_full.c`'s exported helpers
 * are not present. This is the intended loud-fail behaviour.
 */

const char __gradient_async_strategy[] = "none";
