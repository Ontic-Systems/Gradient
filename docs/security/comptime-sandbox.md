# Comptime sandbox

> Issue: [#356](https://github.com/Ontic-Systems/Gradient/issues/356) — tracks an adversarial-review item.
> Epic: [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (threat model).
> Cross-references: [`threat-model.md`](threat-model.md) row S5, [`effect-soundness.md`](effect-soundness.md), [`agent-codegen-guidelines.md`](agent-codegen-guidelines.md) G6.

The Gradient compiler's compile-time evaluator (`comptime`) refuses to call any function that could perform side effects. This tracks an adversarial-review item (compile-time `print(...)` / `read_file(...)` / `extern fn` invocation).

## Threat model

Without a sandbox, `comptime` is equivalent to JavaScript `eval(prompt )` from the perspective of an attacker who controls Gradient source: opening a hostile `.gr` file in an editor that runs `gradient check` (or LSP) becomes RCE.

This is especially dangerous when **compounded with the LSP-untrusted-source gap**: the LSP previously processed untrusted source under the same trust posture as `gradient check`. With LSP `@untrusted` defaults shipped ([#359](https://github.com/Ontic-Systems/Gradient/issues/359)), every LSP buffer is type-checked under `@untrusted` by default; comptime is rejected outright in that mode. The comptime sandbox remains the load-bearing defense for any opt-out workspace (`untrusted = false` in `.gradient/lsp.toml`) or for `gradient check --trusted` runs.

## Defense — three layers

The sandbox is enforced inside the comptime evaluator's `eval_call` (see [`codebase/compiler/src/comptime/evaluator.rs`](../../codebase/compiler/src/comptime/evaluator.rs)). Three checks gate every call site:

### Layer 1: banned-builtin name list

```rust
pub const COMPTIME_BANNED_BUILTINS: &[&str] = &[
 "print", "println", "eprint", "eprintln",
 "read_file", "write_file", "read_line",
 "exec", "spawn", "system", "exit",
 "env", "getenv", "setenv",
 "open", "close", "remove_file", "create_dir", "remove_dir",
 "tcp_connect", "tcp_listen", "udp_send",
 "http_get", "http_post",
];
```

Any call with one of these names is rejected with `SandboxViolation { reason: "banned-builtin" }`, **regardless of the function's declared effect row**. This is defense-in-depth: even if an attacker manages to declare `fn print(s: String) -> !{}`, the name match still trips.

### Layer 2: extern-fn rejection

`extern fn` declarations cannot be evaluated at compile time (their bodies are in a foreign language). Any call to a registered extern produces `SandboxViolation { reason: "extern-fn" }`.

This is structurally enforced — externs go into a separate `extern_functions` set, not the `functions` HashMap, so they cannot accidentally be treated as comptime-evaluable.

### Layer 3: effect-row whitelist

```rust
pub const COMPTIME_ALLOWED_EFFECTS: &[&str] = &["Stack", "Static"];
```

Only **marker effects** that constrain implementation shape but do not perform side effects (per [`effect-soundness.md`](effect-soundness.md) § "Marker vs gating") are permitted. Every other launch-tier effect produces `SandboxViolation { reason: "effect:<name>" }`. The whitelist is intentionally small and named in the negative — adding a new effect to `KNOWN_EFFECTS` without updating the whitelist defaults to "banned in comptime", which is the safe default.

## Acceptance — closes #356

| Acceptance criterion | Status |
|---|---|
| Compile-time `print(...)` → error | ✓ via Layer 1 banned-builtin (`comptime_sandbox_rejects_print_by_name` test) |
| Compile-time `read_file(...)` → error | ✓ via Layer 1 banned-builtin (`comptime_sandbox_rejects_read_file_by_name` test) |
| Compile-time `extern fn` call → error | ✓ via Layer 2 extern-fn rejection (`comptime_sandbox_rejects_extern_fn` test) |
| Whitelist documented | ✓ this doc § "Layer 3: effect-row whitelist" |

## Worked examples

### Rejected: `print` in comptime

```gradient
comptime:
 print("hello") // SandboxViolation: banned-builtin `print`
```

### Rejected: function declaring `!{IO}`

```gradient
fn write_log(msg: String) -> !{IO} Unit:
 ..

comptime:
 write_log("loaded") // SandboxViolation: effect:IO
```

### Rejected: extern fn

```gradient
extern fn c_strlen(s: String) -> Int

comptime:
 c_strlen("abc") // SandboxViolation: extern-fn
```

### Allowed: pure or marker-effect-only fn

```gradient
fn double(n: Int) -> !{Stack} Int:
 n + n

comptime:
 let four = double(2) // permitted; effect row is marker-only
```

## Where it sits in the pipeline

The check fires at the moment the comptime evaluator dispatches a call. By that point:

- The parser has accepted the syntactic form.
- The typechecker has resolved the function name and its declared effect row.
- The evaluator has decided to evaluate this call (i.e. it is genuinely compile-time, not deferred to runtime).

The sandbox is **not** a parse-time or typecheck-time gate — it is a runtime gate inside the comptime VM. Two reasons:

1. **Faithful threat model.** The threat is the evaluator running side-effecting code, so the gate must sit in the evaluator.
2. **Layered defense.** The typechecker already rejects code that calls functions with effects the caller hasn't declared. The sandbox is the second line of defense at the comptime boundary specifically.

## Test coverage

Ten tests in `codebase/compiler/src/comptime/evaluator.rs` § `tests`:

- `comptime_sandbox_rejects_print_by_name`
- `comptime_sandbox_rejects_read_file_by_name`
- `comptime_sandbox_rejects_io_effect`
- `comptime_sandbox_rejects_heap_effect`
- `comptime_sandbox_rejects_extern_fn`
- `comptime_sandbox_rejects_throws_effect`
- `comptime_sandbox_allows_pure_fn`
- `comptime_sandbox_allows_stack_marker_effect`
- `comptime_sandbox_allows_static_marker_effect`
- `comptime_sandbox_disallowed_effect_helper_first_match`

The split between "rejects" and "allows" pins the boundary symmetrically.

## Cross-references

- [`docs/security/threat-model.md`](threat-model.md) — surface row S5 (comptime evaluator).
- [`docs/security/effect-soundness.md`](effect-soundness.md) — formal basis for marker/gating split.
- [`docs/security/agent-codegen-guidelines.md`](agent-codegen-guidelines.md) — G6 (treat `comptime` like `eval`).
- [Epic #302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model.
- [`effects.rs`](../../codebase/compiler/src/typechecker/effects.rs) — `KNOWN_EFFECTS` source-of-truth.
- [`evaluator.rs`](../../codebase/compiler/src/comptime/evaluator.rs) — sandbox implementation + tests.
