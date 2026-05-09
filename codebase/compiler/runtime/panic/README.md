# Gradient panic strategy runtimes (#337, E5 ADR 0005)

This directory holds the three concrete runtime implementations of the
`@panic(...)` module attribute (parsed/checked since #521, AST type
`PanicStrategy { Abort, Unwind, None }`).

## Files

| File | Strategy | Selected when |
|---|---|---|
| `runtime_panic_abort.c`  | `Abort`  | `@panic(abort)` |
| `runtime_panic_unwind.c` | `Unwind` | `@panic(unwind)` (default) |
| `runtime_panic_none.c`   | `None`   | `@panic(none)` |

All three define the same C symbol:

```c
void __gradient_panic(const char* msg);  // never returns
```

The build system picks exactly **one** of these three objects per binary
based on `Module.panic_strategy` (see
`codebase/build-system/src/commands/build.rs::select_panic_runtime`). Linking
two would produce a multiple-definition error from `cc` — that's intentional
and acts as a defense against accidental double-link.

A `const char __gradient_panic_strategy[]` tag is also exported by each
runtime so curious consumers (e.g. a future `gradient inspect`) can identify
the linked strategy at runtime.

## Why three crates instead of one `#ifdef` tangle?

ADR 0005 (and Q14 of the locked vision stack) commits to a "modular,
effect-driven linker DCE" design where the runtime is composed of small
single-responsibility pieces selected by effects/attributes. Putting all
three strategies behind preprocessor flags in one file would:

1. Hide the strategy boundaries from `gradient build` (each strategy
   lives in its own object).
2. Force the codegen layer to know about C preprocessor macros instead of
   just emitting a call to `__gradient_panic` and letting the linker pick.
3. Make future per-strategy work (e.g. real unwinding via libunwind in
   `runtime_panic_unwind.c`) cascade across compile-time configuration.

Three files keeps each strategy independently maintainable and gives
future work (`runtime-throws`, `runtime-rc`, `runtime-arena`) a precedent
to follow.

## Status

- `Abort` — final shape. `abort(3)` after a stderr line.
- `Unwind` — placeholder. Today behaves like `Abort` (prints + aborts).
  Real stack unwinding with destructor / `!{Throws(E)}` landing pads is
  tracked by the Throws(E) effect (#317) and the future `runtime-throws`
  crate.
- `None` — defense-in-depth backstop. Checker statically rejects every
  panic-able op under `@panic(none)`, so the body should never be reached
  in well-formed programs. If it is, the process terminates with a
  distinctive "internal error" message so the bug is loud not silent. See
  `tests/embedded_no_panic.gr` for the contract proof.
