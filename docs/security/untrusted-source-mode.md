# `@untrusted` source mode

> Issue: [#360](https://github.com/Ontic-Systems/Gradient/issues/360) — closes adversarial finding **F4 (HIGH)** input-surface portion.
> Sibling issue: [#359](https://github.com/Ontic-Systems/Gradient/issues/359) — flips the LSP default to `@untrusted` for unsaved buffers.
> Epic: [#302](https://github.com/Ontic-Systems/Gradient/issues/302).

Gradient draws a hard line between code humans wrote (`@trusted`, the default) and code an AI agent emitted from an external prompt (`@untrusted`). Untrusted modules run inside a restricted language subset that strips the four most dangerous superpowers of the language away from agent-emitted source.

## TL;DR

```gradient
@untrusted

fn safe(x: Int, y: Int) -> !{IO} Int:
    x + y
```

A file with `@untrusted` at the very top has these four restrictions:

| # | Restriction | Why |
|---|---|---|
| 1 | No `comptime` parameters | Comptime is full-power Rust during compilation; an attacker-controlled prompt could execute arbitrary code at build time. The three-layer comptime sandbox (#356) further restricts comptime *itself*; `@untrusted` simply forbids it outright. |
| 2 | No `@extern` (FFI) | FFI calls leave the type system entirely. An untrusted module that calls `dlopen("libc.so")` defeats every other guarantee. |
| 3 | Effects must be explicit | Effect inference makes it easy to accidentally pick up an effect from a callee. Untrusted code must spell out exactly what it does — `!{IO}`, `!{FS}`, etc. — so the effect-cap (`@cap`) machinery and reviewers can audit it. |
| 4 | Return types must be explicit | Type inference at module boundaries hides the public-API surface from review. Untrusted modules must declare every fn signature in full so the contract is visible. |

These four together close the F4 input surface: an agent emitting Gradient text into a build cannot run code at compile time, can't escape into the host process via FFI, and can't sneak an effect past the review.

## Surface syntax

```text
@untrusted    // file-scope: applies to whole module
              // takes no arguments
```

The attribute appears **before** any `mod`, `use`, or item declaration. It's a file-scope marker; you cannot mix `@trusted` and `@untrusted` items inside the same module — that would defeat the auditing purpose.

For symmetry / documentation, `@trusted` is also accepted but is the implicit default.

## Default behaviour

| Source | Trust posture |
|---|---|
| File on disk, no annotation | `Trusted` |
| File on disk, `@trusted` | `Trusted` |
| File on disk, `@untrusted` | `Untrusted` |
| Unsaved LSP buffer (#359) | `Untrusted` (planned) |

The LSP default flip lives in #359 — the rationale is that a buffer the user is mid-pasting from an LLM hasn't been reviewed yet; the worst that happens is some red squiggles until the user explicitly marks it `@trusted`.

## Diagnostics

When the typechecker hits a violation, it surfaces the message at the offending site with a hint pointing at the workaround:

```
error: extern function `sqrt` is not allowed in @untrusted module
  note: FFI is banned in @untrusted modules — agent-emitted code
        may not call into native libraries. Move FFI declarations
        to a @trusted module.
```

```
error: comptime parameter `T` is not allowed in @untrusted module
  note: comptime evaluation is disabled in @untrusted modules —
        agent-emitted code may not run at compile time.
```

```
error: function `foo` must declare its effects in @untrusted module
  note: effect inference is disabled in @untrusted modules; add
        an explicit effect annotation, e.g. `-> !{IO} Int` or
        `-> !{} Int` for a pure function.
```

```
error: function `foo` must declare its return type in @untrusted module
  note: return-type inference is disabled in @untrusted modules;
        add an explicit `-> T` clause.
```

## Workspace pattern

Mixing trust postures within a workspace is the supported pattern:

```
my-app/
├── trusted/             # human-authored core, no annotation needed
│   ├── ffi.gr           # @extern declarations live here
│   └── core.gr
└── agent-output/
    ├── feature_a.gr     # @untrusted at top of file
    └── feature_b.gr     # @untrusted at top of file
```

Build the trusted modules first, expose their public API through `use`, and let the untrusted modules call into them. The untrusted code can use any function the trusted side exports; it just can't introduce new `@extern` declarations or comptime parameters of its own.

## Related work

- [`comptime-sandbox.md`](comptime-sandbox.md) — the three-layer sandbox that protects comptime evaluation *inside* trusted code (closes F2). `@untrusted` is the orthogonal lever for code an agent emitted; the sandbox is for code humans authored that uses comptime.
- [`fuzz-harness.md`](fuzz-harness.md) — fuzzes the parser/checker against arbitrary text including `@untrusted` annotations (closes F3).
- [`threat-model.md`](threat-model.md) row S9 / TF2.

## Test fixtures

The acceptance suite lives in `compiler/src/typechecker/tests.rs`:

- `untrusted_module_rejects_extern_fn` — no FFI.
- `untrusted_module_rejects_comptime_param` — no comptime.
- `untrusted_module_requires_explicit_effects` — explicit effects.
- `untrusted_module_requires_explicit_return_type` — explicit return type.
- `untrusted_module_accepts_well_formed_function` — happy path.
- `trusted_module_unrestricted_by_default` — default posture has full language.
- `explicit_trusted_annotation_unrestricted` — `@trusted` is a no-op marker.
- `untrusted_attribute_rejects_arguments` — `@untrusted("loose")` is a parse error.

## Acceptance — closes #360

- [x] Attribute parses (`@trusted` / `@untrusted` recognized at file scope; no-arg form only).
- [x] All four restrictions enforced (typechecker check_untrusted_restrictions).
- [x] Test fixtures: `@trusted` and `@untrusted` modules in same workspace (8 tests in `typechecker/tests.rs`).
