# Gradient — Threat Model

> Issue: [#355](https://github.com/Ontic-Systems/Gradient/issues/355) — Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302).
> Cross-references adversarial-review findings F2–F8 (see [`internal qa/2026-05-02-adversarial-review.md`](https://github.com/Ontic-Systems/Gradient/issues/302) — referenced indirectly through the public sub-issues that close each finding).

This document enumerates Gradient's attack surfaces, the threat actors against each, the current mitigation status, and the issue thread that owns each remaining gap. It is a **public, living document** — every PR that lands a sub-issue under Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (or that materially changes one of the surfaces below) is expected to update the row in §[Surfaces](#surfaces).

The threat model deliberately predates a public push. Findings F1–F8 from the 2026-05-02 adversarial review are wired into the roadmap so that no row is left implicitly closed.

## Threat actors

We model four actors. The mitigations below name which actor each surface defends against.

| Actor | Capability assumed | Notes |
|---|---|---|
| **A1 — Untrusted source author** | Submits arbitrary `.gr` source to a Gradient toolchain (compiler, LSP, registry). | Includes LLM agents emitting code from prompt injection. |
| **A2 — Untrusted package author** | Publishes a package to the (planned) registry that downstreams may install. | Capability-scoped manifest checks happen at build time. |
| **A3 — Compromised dev machine** | Has local FS access. Can poison a build by editing source or `~/.gradient`. | Out of scope for compiler-side mitigations; only the registry signature path defends here. |
| **A4 — Network attacker on registry / fetch path** | Can intercept or modify package downloads. | Mitigated by sigstore + manifest verification (planned). |

## Status legend

| Status | Meaning |
|---|---|
| `mitigated` | Fix shipped, anchored by tests + at least one issue link. |
| `partial` | Fix scoped down at the launch tier; a follow-on issue covers the residual gap. |
| `open` | No code-level mitigation today. The threat is named, the owning issue is filed, and the public roadmap reflects the gap. |
| `n/a` | Surface intentionally not defended at this layer (e.g. A3 against compiler-internal threats). |

Severity follows the adversarial-review convention: `LOW`, `MEDIUM`, `HIGH`, `CRITICAL`.

## Surfaces

The 10 surfaces below cover everything an attacker can poke at when interacting with a Gradient toolchain or program.

### S1. Agent-emitted code

> **Threat actor**: A1.
> **Severity**: HIGH.
> **Status**: `partial`.

LLM-emitted code reaches the compiler with no prior trust signal. Without effect/capability gating, an agent can land a function that silently calls `print` from inside a `!{}` declaration if the type system permits effect erasure.

**Mitigations in place**:

- Effect rows on every signature ([ADR 0001](../adr/0001-effect-tier-foundation.md)).
- Effect propagation enforced at every call site (see [`docs/security/effect-soundness.md`](effect-soundness.md) for the rules and soundness sketch).
- Heap allocation effect-gated at the allocation site ([#313 / #455](https://github.com/Ontic-Systems/Gradient/issues/313)).
- Marker effects (`Stack`, `Static`, `Volatile`) preserved across composition ([#314 / #456 / #458](https://github.com/Ontic-Systems/Gradient/issues/314)).
- `Throws(E)` effect propagation ([#317 / #487](https://github.com/Ontic-Systems/Gradient/issues/317)).

**Remaining gap**:

- Bidirectional effect inference ([#350](https://github.com/Ontic-Systems/Gradient/issues/350)) so callers can rely on the absence of an effect *without* every callee declaring it explicitly. Currently elision is not allowed; once inference lands the soundness sketch must be extended to the inferred form.
- `@trusted` / `@untrusted` source mode ([#360](https://github.com/Ontic-Systems/Gradient/issues/360)) — the LSP and comptime defaults need to assume A1 by default (see S5).

### S2. FFI (`extern fn`)

> **Threat actor**: A1, A2.
> **Severity**: HIGH.
> **Status**: `partial` — gating is conservative-by-default; capability gate is open.

`extern fn` declarations cross the language boundary. A poisoned extern can do anything its host process is permitted to do.

**Mitigations in place**:

- Extern declarations without an explicit effect row default to `EXTERN_DEFAULT_EFFECTS = { IO, Net, FS, Mut, Time }` ([`codebase/compiler/src/typechecker/effects.rs`](../../codebase/compiler/src/typechecker/effects.rs)). This forces every caller to declare those effects, which means extern usage is always effect-visible from the call graph.
- Mod-block extern declarations enforce no-overwrite over kernel-pre-registered surfaces ([#262](https://github.com/Ontic-Systems/Gradient/issues/262)) so a malicious mod cannot redeclare a kernel function with a narrower effect row.

**Remaining gap (F5 — HIGH)**:

- `Unsafe` capability gate on `extern fn` ([#322 / ADR 0002](https://github.com/Ontic-Systems/Gradient/issues/322)). Today, any module can declare `extern fn`. Post-#322, declaring an `extern fn` will require the module to hold the `Unsafe` capability *and* an `!{FFI(C)}` effect on the resulting call. ADR 0002 already locks this design.
- `@repr(C)` struct layout ([#323](https://github.com/Ontic-Systems/Gradient/issues/323)) — required so FFI types have a stable, audit-able layout.
- `gradient bindgen` MVP ([#324 / #374](https://github.com/Ontic-Systems/Gradient/issues/324)) — generates externs from C headers under controlled effect/capability rows.

### S3. Package registry

> **Threat actor**: A2, A4.
> **Severity**: HIGH.
> **Status**: `open` (registry not yet implemented).

A planned registry will distribute packages whose code runs on user machines. Any registry that ships without signing + capability-scoped manifests is a credential-laundering vector.

**Mitigations planned (Epic [#303](https://github.com/Ontic-Systems/Gradient/issues/303))**:

- Manifest format spec ([#365](https://github.com/Ontic-Systems/Gradient/issues/365)) — capability-scoped (effects + capabilities + dependencies).
- Build-time enforcement of manifest effects/capabilities ([#366](https://github.com/Ontic-Systems/Gradient/issues/366)).
- `gradient publish --sign` ([#367](https://github.com/Ontic-Systems/Gradient/issues/367)) — sigstore-backed.
- `gradient install --verify` ([#368](https://github.com/Ontic-Systems/Gradient/issues/368)) — refuses unsigned or manifest-mismatched packages.
- Registry backend MVP ([#369](https://github.com/Ontic-Systems/Gradient/issues/369)).

ADR 0007 ([`adr/0007-registry-trust.md`](../adr/0007-registry-trust.md)) locks the design. **The registry will not ship until all of [#365–#369] land.** That sequence is the F9 mitigation.

### S4. Capability tokens

> **Threat actor**: A1, A2.
> **Severity**: MEDIUM.
> **Status**: `open` — capability typestate engine not yet implemented.

Capabilities are first-class tokens that gate access to side-effecting operations beyond what effects alone express (e.g. arenas, `Unsafe`). Without a typestate engine, capability tokens can be dropped, duplicated, or laundered through generic combinators.

**Mitigations planned (Epic [#296](https://github.com/Ontic-Systems/Gradient/issues/296))**:

- Capability typestate engine ([#321](https://github.com/Ontic-Systems/Gradient/issues/321)) — checks linear / affine usage.
- Capability inference ([#351](https://github.com/Ontic-Systems/Gradient/issues/351)).
- Migrate one self-hosted module to capability-passing ([#325](https://github.com/Ontic-Systems/Gradient/issues/325)) — dogfood gate before the engine is declared production-ready.

ADR 0002 ([`adr/0002-arenas-capabilities.md`](../adr/0002-arenas-capabilities.md)) locks the design.

### S5. Comptime evaluator

> **Threat actor**: A1.
> **Severity**: HIGH (F2 / MEDIUM in adversarial review, escalated by F4 to HIGH when LSP is in scope).
> **Status**: `partial` — comptime sandbox shipped (closes F2); LSP `@untrusted` default still open (#359).

The comptime evaluator runs Gradient code at compile time. If an editor plugin (LSP) processes untrusted source and the comptime evaluator is unsandboxed, opening a hostile `.gr` file is RCE on the developer's machine.

**Mitigations in place**:

- **Comptime sandbox shipped** ([#356](https://github.com/Ontic-Systems/Gradient/issues/356), see [`comptime-sandbox.md`](comptime-sandbox.md)). Three-layer defense in `eval_call`: banned-builtin name list, extern-fn rejection, effect-row whitelist (`Stack`/`Static` only). Closes F2.

**Mitigations planned (Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302))**:

- LSP defaults to `@untrusted` mode ([#359](https://github.com/Ontic-Systems/Gradient/issues/359)) — closes F4. Until both [#356] and [#359] ship, **the LSP must not be exposed to untrusted source.** **#356 has now shipped**; #359 is the remaining gap.
- `@untrusted` source mode ([#360](https://github.com/Ontic-Systems/Gradient/issues/360)) — adds the source-tier marker that LSP and comptime check against.

Until S5 is fully mitigated (i.e. [#359] also ships), the README and getting-started docs must not encourage running the LSP against arbitrary `.gr` files.

### S6. Contracts (runtime + verified)

> **Threat actor**: A1.
> **Severity**: MEDIUM.
> **Status**: `partial`.

`@requires` / `@ensures` contracts can be made permissive (always-true preconditions) by an attacker, defeating the runtime-asserts safety net. The `@verified` tier mitigates by discharging obligations statically; `@runtime_only(off_in_release)` is itself an attack surface (release builds skip the assertion).

**Mitigations in place**:

- Runtime contracts enforced on entry/exit ([ADR 0003](../adr/0003-tiered-contracts.md)).
- `@verified` opt-in via `GRADIENT_VC_VERIFY=1` discharges obligations through Z3 ([#329 / #437](https://github.com/Ontic-Systems/Gradient/issues/329)). Counterexamples surface as structured diagnostics.
- Stdlib pilot's `@verified` modules are continuously discharged on every CI green (13 modules, 126 fns / 170 obligations as of [#490](https://github.com/Ontic-Systems/Gradient/pull/490)).
- `@runtime_only(off_in_release)` opt-out is gated by an audit JSON written to `target/release/audit.json` ([#330 / #438](https://github.com/Ontic-Systems/Gradient/issues/330)). Release builds may not strip contracts under `core/` or `alloc/` paths.

**Remaining gap (F11)**:

- `result` keyword shadowing — current parser may accept `let result = ...` as a local binding inside a `@ensures` body. Tracked under [`gradient-reserved-keywords-trap`](../../#) skill; not a separate sub-issue today, but listed here so the gap is visible.

### S7. Effect system

> **Threat actor**: A1.
> **Severity**: MEDIUM (F7).
> **Status**: `mitigated` at the launch tier — soundness sketch published [#363 / #492](https://github.com/Ontic-Systems/Gradient/pull/492).

If the effect system is unsound, every other security claim that depends on it (S1, S2, S5) is brittle.

**Mitigations in place**:

- Effect rows + propagation rules formalized in [`docs/security/effect-soundness.md`](effect-soundness.md).
- Subject reduction + progress sketch covers the launch-tier vocabulary.
- Effect-security corollary anchors the load-bearing claim ("`IO` not in row → no `IO` at runtime").

**Remaining gap**:

- Mechanization (Coq formalization) is tracked as an open question in the soundness doc itself.
- Inference soundness extension ([#350](https://github.com/Ontic-Systems/Gradient/issues/350)) needed before the inferred form ships.

### S8. Self-hosted compiler / DDC

> **Threat actor**: A1, A3.
> **Severity**: MEDIUM (F6).
> **Status**: `open`.

A self-hosted compiler (`compiler/*.gr`) is itself code that compiles other code. Without diverse double compilation (DDC) the bootstrap chain is unverified — a Trojan'd kernel could persist across rebuilds.

**Mitigations planned**:

- DDC bootstrap verification ([#361](https://github.com/Ontic-Systems/Gradient/issues/361)) — closes F6 deliverable. Plan: build the self-hosted compiler with two independent reference compilers and verify the artifacts are byte-identical. **Status: published [`ddc.md`](ddc.md) with full procedure + obstacles + mitigations + release-checklist hook. Run gated on Epic #116 (self-hosted compiler reaching execution).**
- Reproducible builds ([#362](https://github.com/Ontic-Systems/Gradient/issues/362)) — closes F8 deliverable. Required so DDC verification is meaningful. **Status: published [`reproducible-builds.md`](reproducible-builds.md), CI gate live in [`.github/workflows/reproducible-build.yml`](../../.github/workflows/reproducible-build.yml). Gate is currently advisory (`continue-on-error`) — first runs detect real residual drift; tightening levers PR-by-PR until two consecutive runs match. Cranelift backend covered; LLVM out of scope (E6).**

The self-hosted tree is presently bootstrap-stage only (`SelfHostedDefault`/`SelfHostedGated`/`Hybrid` rows in [`kernel_boundary.rs`](../../codebase/compiler/src/kernel_boundary.rs)). The DDC requirement does not yet bind because the self-hosted compiler does not yet execute end-to-end (see [`docs/SELF_HOSTING.md`](../SELF_HOSTING.md) for the honest split). It will bind before any "true self-hosted compiler alpha" claim.

### S9. Query API / LSP

> **Threat actor**: A1.
> **Severity**: HIGH (compounds F2 → F4).
> **Status**: `partial`.

The Query API and LSP both consume source files and produce structured output for tooling consumers. Either may be exposed to untrusted source (an editor opening a `.gr` file from a downloaded package).

**Mitigations in place**:

- Query API is read-only and pure (no comptime execution unless explicitly invoked).
- LSP handlers all delegate to the same Rust kernel as the Query API, with no side-effecting paths.

**Remaining gap (F4 — HIGH)**:

- LSP `@untrusted` default ([#359](https://github.com/Ontic-Systems/Gradient/issues/359)) — the LSP currently has the same trust posture as `gradient check`. Until [#359] ships, an editor that opens a hostile `.gr` file with comptime-execution paths active is RCE-equivalent.

### S10. WASM sandbox / target

> **Threat actor**: A2.
> **Severity**: LOW (current scope) → MEDIUM if the WASM target is recommended for production.
> **Status**: `partial` — WASM target is experimental.

The WASM backend produces sandboxed code suitable for embedding. The sandbox itself is the WASM runtime's responsibility, not Gradient's, but Gradient must not generate WASM that escapes the sandbox via FFI.

**Mitigations in place**:

- WASM is documented as experimental ([`docs/WASM.md`](../WASM.md)).
- `extern fn` calls are not currently emitted into WASM (would require host-import declarations); the codegen is conservative.

**Remaining gap**:

- Enforce capability/effect rows in the WASM emitter so `extern fn` declarations cannot silently call into the host. Tracked indirectly under [#322](https://github.com/Ontic-Systems/Gradient/issues/322) and [#374](https://github.com/Ontic-Systems/Gradient/issues/374); add a dedicated WASM sub-issue if the WASM target moves out of experimental.

## Tooling-related findings

These are not "surfaces" in the system-architecture sense but are tracked for completeness.

### TF1. Fuzz harness on parser/checker/IR (F3 — HIGH)

> **Status**: `partial` — lexer + parser harness shipped (#357); checker + IR harness still planned (#358).

A parser/checker without fuzzing is brittle against malformed agent-emitted input. This is *not* a security issue per se (the typechecker is total / does not crash on invalid input), but a fuzz harness is the standard external check for that property.

**Mitigations in place**:

- cargo-fuzz harness for lexer + parser shipped ([#357](https://github.com/Ontic-Systems/Gradient/issues/357), see [`fuzz-harness.md`](fuzz-harness.md)). Two targets (`lex_random_bytes`, `parse_random_text`) run nightly via `.github/workflows/fuzz.yml` cron `0 2 * * *` for 4h each; PR smoke runs each for 30s when `codebase/fuzz/**` changes.

**Mitigations planned**:

- cargo-fuzz harness for checker + IR builder ([#358](https://github.com/Ontic-Systems/Gradient/issues/358)) — extends the same pattern to `check_random_module` and `lower_random_module` targets.

### TF2. Prompt-injection-resistant codegen guidelines (F2 / F4-adjacent)

> **Status**: `mitigated` (documentation-tier — published guidelines).

LLM-emitted code is by definition affected by prompt injection. We need a public document codifying patterns that are robust against this — e.g. effect-row checks, capability minimization, deterministic comptime, no shell-out by default.

**Mitigations in place**:

- [`docs/security/agent-codegen-guidelines.md`](agent-codegen-guidelines.md) — G1–G10 codifying explicit effect rows, smallest-effect-row principle, capability whitelisting, threat-tier markers, and refusal-surfacing rules ([#364](https://github.com/Ontic-Systems/Gradient/issues/364)).
- Cross-linked from [`docs/agent-integration.md`](../agent-integration.md) header.

## Summary table

| # | Surface | Severity | Status | Owning issues |
|---|---|---|---|---|
| S1 | Agent-emitted code | HIGH | partial | #350, #360 |
| S2 | FFI (`extern fn`) | HIGH | partial | #322, #323, #324, #374 |
| S3 | Package registry | HIGH | open | #365–#369 |
| S4 | Capability tokens | MEDIUM | open | #321, #351, #325 |
| S5 | Comptime evaluator | HIGH | partial | #359 (S5 #356 closed) |
| S6 | Contracts | MEDIUM | partial | (F11 known; no separate issue today) |
| S7 | Effect system | MEDIUM | mitigated (sketch) | #363 (closed) — mechanization deferred |
| S8 | Self-hosted compiler / DDC | MEDIUM | open | #361, #362 |
| S9 | Query API / LSP | HIGH | partial | #359 |
| S10 | WASM target | LOW (today) | partial | #322 (indirectly) |
| TF1 | Fuzz harness | HIGH | partial | #358 (TF1 #357 closed) |
| TF2 | Prompt-injection-resistant codegen | MEDIUM | mitigated (docs) | #364 (closed) |

## Update protocol

When you land a sub-issue under Epic [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (or under a referenced sibling epic):

1. Update the relevant row's **Status** column above.
2. Add a one-line entry under the surface's "Mitigations in place" / "Mitigations planned" section.
3. If the issue closes an adversarial finding (F2 … F8), strike through the finding number in the row.
4. Cross-link the closing PR.

Drift between this doc and the underlying surface is itself a security issue — keep it tight.

## Cross-references

- [`docs/security/effect-soundness.md`](effect-soundness.md) — soundness sketch for S7.
- [`docs/security/README.md`](README.md) — index of security docs.
- [Epic #302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model + sigstore-prep + sandbox + fuzz + DDC + reproducible builds.
- [ADR 0001](../adr/0001-effect-tier-foundation.md), [ADR 0002](../adr/0002-arenas-capabilities.md), [ADR 0003](../adr/0003-tiered-contracts.md), [ADR 0007](../adr/0007-registry-trust.md) — locked design anchors for the mitigations above.
