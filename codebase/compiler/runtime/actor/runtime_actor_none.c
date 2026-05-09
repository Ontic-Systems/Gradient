/*
 * Gradient actor strategy: none
 *
 * Selected by `gradient build` when the main module's effect summary does
 * NOT contain `Actor` — i.e. the program is provably free of `spawn`,
 * `send`, `ask`, and any actor-typed value. The canonical
 * `gradient_runtime.o` is still linked (it carries libc helpers like
 * `__gradient_print_int` that don't touch the scheduler), but the
 * `runtime_actor_full.o` object is omitted entirely.
 *
 * Today the actor scheduler still lives as `static` helpers inside the
 * canonical runtime, so the binary-size delta from `full` -> `none` is
 * small (one tag symbol). The next runtime PR will extract the actor
 * scheduler symbols out of the canonical runtime and into
 * `runtime_actor_full.c`, at which point selecting `none` will measurably
 * shrink the binary.
 *
 * Symbol contract:
 *
 *   const char __gradient_actor_strategy[]
 *     -- "none" — visible to `nm`/`strings` and the future
 *        `gradient inspect`. Same name as the `full` variant exports —
 *        linking both produces the intended multi-definition error from
 *        cc, which is the defense-in-depth against accidental double-link.
 *
 * The contract is: if a program with no `Actor` effect ever calls into an
 * actor helper (i.e. the checker missed an actor op), the link will fail
 * because `runtime_actor_full.c`'s exported helpers are not present.
 * This is the intended loud-fail behaviour.
 */

const char __gradient_actor_strategy[] = "none";
