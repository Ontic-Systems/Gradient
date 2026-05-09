/*
 * Gradient actor strategy: full
 *
 * Selected by `gradient build` when the main module's effect summary
 * contains `Actor` — i.e. the program uses `spawn`, `send`, `ask`, or any
 * actor-typed value. Linked alongside the canonical `gradient_runtime.o`
 * which still ships the actor scheduler proper (mailbox queues, supervisor
 * trees, etc.) as `static` helpers. This file's job is to (a) document the
 * contract, (b) provide an introspectable tag so `nm <binary>` can confirm
 * the linked variant, and (c) act as the future home of the extracted
 * actor scheduler once it is pulled out of the canonical runtime.
 *
 * Symbol contract:
 *
 *   const char __gradient_actor_strategy[]
 *     -- "full" — visible to `nm`/`strings` and the future
 *        `gradient inspect` subcommand.
 *
 * No callable runtime symbols are exported here yet; the actor scheduler
 * still lives inside the canonical runtime / experimental actor module.
 * The follow-on PR (#334 follow-on) will move that machinery into this
 * file so the canonical runtime shrinks and the link-time omission of
 * the variant runtime under `none` actually drops bytes.
 *
 * This split is the runtime-side commitment to ADR 0005's "modular,
 * effect-driven linker DCE" strategy. See
 * codebase/compiler/runtime/actor/README.md for the dispatch table.
 */

const char __gradient_actor_strategy[] = "full";
