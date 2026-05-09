/*
 * Gradient async strategy: full
 *
 * Selected by `gradient build` when the main module's effect summary
 * contains `Async` — i.e. the program uses async/await, futures, or
 * any value typed by the experimental async runtime. Linked alongside
 * the canonical `gradient_runtime.o` which still ships the async
 * executor proper (task queue, poll loop, waker registry) as `static`
 * helpers. This file's job is to (a) document the contract,
 * (b) provide an introspectable tag so `nm <binary>` can confirm the
 * linked variant, and (c) act as the future home of the extracted
 * async executor once it is pulled out of the canonical runtime.
 *
 * Symbol contract:
 *
 *   const char __gradient_async_strategy[]
 *     -- "full" — visible to `nm`/`strings` and the future
 *        `gradient inspect` subcommand.
 *
 * No callable runtime symbols are exported here yet; the async
 * executor still lives inside the canonical runtime / experimental
 * async module. The follow-on PR (#335 follow-on) will move that
 * machinery into this file so the canonical runtime shrinks and the
 * link-time omission of the variant runtime under `none` actually
 * drops bytes.
 *
 * This split is the runtime-side commitment to ADR 0005's "modular,
 * effect-driven linker DCE" strategy. Sibling of the actor-strategy
 * split (#539): same recipe, different trigger effect. See
 * codebase/compiler/runtime/async/README.md for the dispatch table.
 */

const char __gradient_async_strategy[] = "full";
