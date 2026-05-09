/*
 * Gradient panic strategy: none
 *
 * Selected when a module declares `@panic(none)`. The checker statically
 * rejects every panic-able operation (integer division, modulo, array/list
 * indexing — see `PanicStrategy::forbids_panicking_ops` in
 * codebase/compiler/src/ast/module.rs) BEFORE codegen, so a well-formed
 * `@panic(none)` program never emits a call to `__gradient_panic`.
 *
 * If codegen does emit a call (because future panic-able operations are
 * added without a corresponding checker rejection rule), this stub still
 * exists so the link succeeds — it just terminates the process with a
 * distinctive stderr line so the bug is loud rather than silent.
 *
 * In other words: this file's existence is a defense-in-depth backstop, not
 * a real runtime body. The contract is that `@panic(none)` programs do not
 * call into it. The embedded fixture under
 * codebase/compiler/tests/embedded_no_panic.gr proves the contract on each
 * CI run by linking with this object and not calling __gradient_panic.
 *
 * Symbol contract:
 *
 *   void __gradient_panic(const char* msg) -- never returns
 *
 * Linked into the final binary by `gradient build` whenever
 * `Module.panic_strategy == None`.
 */

#include <stdio.h>
#include <stdlib.h>

void __gradient_panic(const char* msg) {
    fprintf(stderr,
            "internal error: __gradient_panic invoked under @panic(none) "
            "(compiler bug — checker should have rejected the panicking op). "
            "Message: %s\n",
            (msg != NULL && msg[0] != '\0') ? msg : "<no message>");
    fflush(stderr);
    abort();
}

const char __gradient_panic_strategy[] = "none";
