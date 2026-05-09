/*
 * Gradient panic strategy: unwind
 *
 * Selected when a module declares `@panic(unwind)` (the default for `@app`
 * mode). Currently a placeholder that prints + aborts: real stack unwinding
 * with destructor running and `!{Throws(E)}` landing pads is part of the
 * E5 modular-runtime work tracked by the Throws(E) effect (#317) and will
 * land alongside the runtime-rc / runtime-throws crates.
 *
 * Until that lands, `unwind` and `abort` have observably identical behavior
 * on panic: the process terminates with stderr message + non-zero exit.
 * The DISTINCT runtime crate is still important so that:
 *   - Codegen can emit different IR shapes per strategy (landing pads vs.
 *     direct abort) without rewiring the linker.
 *   - The link-time selection is a single switch, not a #ifdef tangle.
 *   - Future unwind work has a clear home (this file).
 *
 * Symbol contract:
 *
 *   void __gradient_panic(const char* msg) -- never returns
 *
 * Linked into the final binary by `gradient build` whenever
 * `Module.panic_strategy == Unwind` (the default).
 */

#include <stdio.h>
#include <stdlib.h>

void __gradient_panic(const char* msg) {
    if (msg != NULL && msg[0] != '\0') {
        fprintf(stderr, "panic (unwind): %s\n", msg);
    } else {
        fprintf(stderr, "panic (unwind)\n");
    }
    fflush(stderr);
    /*
     * TODO(#317/#337): once Throws(E) lands the runtime-throws crate, hand
     * the panic to the unwinder here instead of aborting. For now, behave
     * identically to abort so existing tests stay green.
     */
    abort();
}

const char __gradient_panic_strategy[] = "unwind";
