/*
 * Gradient alloc strategy: full
 *
 * Selected by `gradient build` when the main module's effect summary
 * contains `Heap` — i.e. the program uses heap-allocating builtins (string
 * concat, list/map/set construction, etc.). Linked alongside the canonical
 * `gradient_runtime.o` which still ships the actual refcount + COW
 * machinery as `static` helpers. This file's job is to (a) document the
 * contract, (b) provide an introspectable tag so `nm <binary>` can confirm
 * the linked variant, and (c) act as the future home of the extracted
 * refcount/COW machinery once it is pulled out of `gradient_runtime.c`.
 *
 * Symbol contract:
 *
 *   const char __gradient_alloc_strategy[]
 *     -- "full" — visible to `nm`/`strings` and the future `gradient inspect`
 *
 * No callable runtime symbols are exported here yet; the rc/COW work
 * (`map_retain`/`map_release`/`set_retain`/`set_release`/`map_deep_copy`
 * etc.) still lives inside `gradient_runtime.c`. The next runtime PR
 * (issue #333 follow-on) will move that machinery into this file so the
 * canonical runtime shrinks and the link-time omission of the variant
 * runtime under `minimal` actually drops bytes.
 *
 * This split is the runtime-side commitment to ADR 0005's "modular,
 * effect-driven linker DCE" strategy. See
 * codebase/compiler/runtime/alloc/README.md for the dispatch table.
 */

const char __gradient_alloc_strategy[] = "full";
