/*
 * Gradient panic strategy: abort
 *
 * Selected when a module declares `@panic(abort)` (or omits the attribute and
 * the compiler default resolves to `abort` for `@system` / `no_std` builds).
 *
 * Semantics:
 *   - On panic, terminate the process immediately via abort(3).
 *   - No landing pads, no destructors, no stack unwinding.
 *   - Smallest binaries; no recovery surface.
 *
 * Symbol contract (used by codegen-emitted panic call sites; future #321
 * capability typestate engine will route capability-violation panics through
 * here too):
 *
 *   void __gradient_panic(const char* msg) -- never returns
 *
 * Linked into the final binary by `gradient build` whenever
 * `Module.panic_strategy == Abort`. See codebase/build-system/src/commands/build.rs
 * `select_panic_runtime`.
 */

#include <stdio.h>
#include <stdlib.h>

void __gradient_panic(const char* msg) {
    if (msg != NULL && msg[0] != '\0') {
        fprintf(stderr, "panic: %s\n", msg);
    } else {
        fprintf(stderr, "panic\n");
    }
    fflush(stderr);
    abort();
}

/*
 * Strategy tag for runtime introspection / debug builds.
 * Exported so the linker fails fast if two strategy objects accidentally
 * end up in the link line (multiple-definition error from the linker).
 */
const char __gradient_panic_strategy[] = "abort";
