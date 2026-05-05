# ADR 0004: Cranelift dev / LLVM release backend split

- Status: Accepted (locked 2026-05-02)
- Deciders: Gradient core (alignment session Q7)
- Epic: [#299](https://github.com/Ontic-Systems/Gradient/issues/299)
- Tracking issue: [#344](https://github.com/Ontic-Systems/Gradient/issues/344)
- Related: [ADR 0001 — effect-tier foundation](0001-effect-tier-foundation.md)

## Context

Gradient targets the **agent-native + systems-first generalist** position. "Systems-first" requires backends that emit code for the embedded and exotic-CPU tiers (Cortex-M, RISC-V, hypervisors, eventually GPU); "agent-native" requires a dev loop fast enough that an LLM agent's iterate-and-test cycle is dominated by reasoning, not codegen.

The Rust ecosystem has shown that **one backend cannot serve both ends** of this spectrum cleanly:

- **Cranelift** is fast (sub-second cold builds, milliseconds warm), self-contained (no system dependency), and tracks rustc as a first-class target. It has weak/no support for: ARM Cortex-M, RISC-V embedded targets, custom calling conventions, exotic atomics, GPU. Optimization quality is intentionally one tier below LLVM (designed for compile speed first, runtime perf second).
- **LLVM** has industry-grade backends for every realistic CPU target including embedded, ships DWARF emit, integrates with platform tools (lld, libcxx, debuggers). It is heavyweight: multi-second link times, system-library dependency, large memory footprint, slow to embed.
- **MLIR** would unify both stories long-term but is a multi-year integration cost. Deferred.
- **GPU codegen** (SPIR-V / PTX / Metal) is post-1.0 per Q7 — not on this ADR's path.

Today, Gradient ships **Cranelift only**. The CLI advertises `--release` and `--backend cranelift|llvm` but the LLVM path is unimplemented. This ADR locks the policy that closes that gap.

## Decision

Gradient ships **two production backends**: Cranelift for development, LLVM for release and systems-tier targets. The split is policy, not preference — each backend covers a distinct part of the agent-native+systems-first matrix.

### Backend selection matrix

| Mode | Default backend | Override | Why |
|---|---|---|---|
| `gradient build` (debug) | Cranelift | `--backend llvm` | Fast cold/warm builds preserve the agent iteration loop. |
| `gradient run` | Cranelift | `--backend llvm` | Same as `build`; agents test changes in-loop. |
| `gradient test` | Cranelift | `--backend llvm` | Test runs are part of the agent loop. |
| `gradient build --release` | **LLVM** | `--backend cranelift` | Release builds prioritize runtime performance and broad-target support. |
| `gradient build --target <triple>` (cross-compile) | LLVM | none — Cranelift cannot serve embedded targets | Cortex-M, RISC-V embedded, exotic CPUs only ship through LLVM. |
| `gradient bench` (Epic E11 [#371](https://github.com/Ontic-Systems/Gradient/issues/371)) | LLVM | `--backend cranelift` | Numbers must reflect what users will actually run; LLVM is the release path. |

The `--backend` flag (sub-issue [#341](https://github.com/Ontic-Systems/Gradient/issues/341)) is the explicit override. The default rules above reflect agent-loop ergonomics + release-build pragmatism; an embedded-firmware project may want to flip its default to LLVM via project-tier configuration (handled when [#341](https://github.com/Ontic-Systems/Gradient/issues/341) lands).

### Cross-compilation policy (sub-issue [#342](https://github.com/Ontic-Systems/Gradient/issues/342))

`gradient build --target <triple>` selects LLVM unconditionally. Triple syntax follows Rust's: `<arch>-<vendor>-<sys>-<env>`. The first wave covers:

- `x86_64-unknown-linux-gnu` (host default)
- `aarch64-apple-darwin`
- `aarch64-unknown-linux-gnu`
- `thumbv7em-none-eabi` (Cortex-M4F embedded)
- `riscv32imac-unknown-none-elf` (RISC-V embedded)

`x86_64-pc-windows-msvc` and `wasm32-unknown-unknown` (separate from the existing experimental WASM backend per Epic E2) follow once the first wave lands.

Cranelift cross to non-host triples is not supported and produces a clear error pointing at `--backend llvm`.

### DWARF emit (sub-issue [#343](https://github.com/Ontic-Systems/Gradient/issues/343))

Both backends emit DWARF in debug builds. LLVM uses its native pipeline; Cranelift uses `cranelift-debug` / `gimli` integration. Acceptance: `gdb` / `lldb` can step Gradient source line-by-line on at least `x86_64-unknown-linux-gnu` (Cranelift + LLVM) and `thumbv7em-none-eabi` (LLVM only, via a probe-rs-style debugger).

### Maintenance cost

We accept the cost of two backends because the alternatives are worse:

- **Cranelift only**: forecloses systems-tier per Q7. Disqualifies the language for the embedded/exotic half of the target market.
- **LLVM only**: kills the agent loop. Multi-second link times turn 30-call agent sessions into 5-minute waits.
- **MLIR**: 12–24 months of integration cost before the first release. Q7 deferred this for at least one major version.
- **Custom backend**: not seriously considered. We are not in the codegen-research business.

The cost is paid in:

- **CI matrix.** Every PR runs both backends on the host triple; cross-compile sanity runs on a smaller cadence. Pre-merge time stays bounded by parallelizing.
- **Test fixture replication.** Each codegen test runs against both backends. Differential output is allowed (e.g. instruction selection differs); semantic output (program exit codes, observable behavior) must be identical.
- **DWARF parity.** Bugs must be fixed on both paths.
- **Bug-report triage.** "Reproduces only on LLVM 17" / "regression in Cranelift 0.110" tickets become routine.

The maintenance cost is bounded because **the backend split is at the IR-emit boundary**, not threaded through the language. The frontend (parser, checker, IR builder) is single-source; only the IR-to-machine-code translation forks.

## Consequences

### Positive

- **Agent loop preserved.** Cranelift keeps cold-build latency in the sub-second range, where iteration cost is dominated by reasoning, not codegen.
- **Systems tier unlocked.** LLVM brings every realistic embedded target into reach without a custom-backend project.
- **Release perf credible.** `gradient bench` runs against LLVM-optimized binaries; published numbers reflect what users actually deploy.
- **DWARF means real debuggability.** Both backends emit standard DWARF; existing toolchains (gdb, lldb, probe-rs, perf, valgrind) work without Gradient-specific shims.

### Negative

- **Two-backend CI cost.** Every PR pays for both pipelines. We mitigate by sharing the IR layer and parallelizing the codegen jobs.
- **Two-backend bug surface.** Backend-specific regressions become a routine class of issue. We mitigate by enforcing semantic-equivalence assertions in the codegen test corpus.
- **LLVM dependency in release.** A release build now depends on the system LLVM toolchain. We document the supported LLVM version range and pin in CI; failures pre-merge are easier to triage than failures in the field.
- **Two paths means two skill sets.** Contributors must learn enough of both backends to land codegen changes without breaking the other. Mitigated by Cranelift's smaller surface (most changes will be Cranelift-only and gated on the LLVM impl catching up later).

### Neutral / deferred

- **GPU backend.** Q7 explicitly deferred SPIR-V / PTX / Metal post-1.0. A future ADR will extend this matrix.
- **MLIR.** A future major version may consolidate on MLIR if the integration cost drops. Today's split is forward-compatible: MLIR can subsume both backends without the frontend changing.
- **WASM.** The experimental WASM backend lives outside this ADR and follows Epic E2's separate path. A future ADR may unify it under the same matrix once it stabilizes.

## Implementation order

Sub-issues land in this order so each step ships value independently:

1. [#339](https://github.com/Ontic-Systems/Gradient/issues/339) — LLVM IR emitter scaffold (`gradient_compiler::codegen::llvm`). Establishes the module structure, links against `inkwell`, emits a minimal `add(a, b)` test.
2. [#341](https://github.com/Ontic-Systems/Gradient/issues/341) — `--backend cranelift|llvm` flag. Wires the selector through `main.rs` and the build-system CLI (`commands/build.rs`). Default rules from this ADR's matrix.
3. [#340](https://github.com/Ontic-Systems/Gradient/issues/340) — closures, generics, pattern match in the LLVM emitter. Brings LLVM to feature parity with Cranelift on the host triple.
4. [#342](https://github.com/Ontic-Systems/Gradient/issues/342) — `--target <triple>` cross-compile. First wave: aarch64, ARM Cortex-M, RISC-V embedded.
5. [#343](https://github.com/Ontic-Systems/Gradient/issues/343) — DWARF emit on both backends. Verified against gdb/lldb on x86-64-linux and probe-rs on Cortex-M.

Each sub-issue includes:

- Codegen test additions that run on both backends (semantic equivalence asserted).
- CI matrix entry for the new backend / target.
- README / `docs/` update if user-visible behavior changes.

## Related

- Epic E6 [#299](https://github.com/Ontic-Systems/Gradient/issues/299) — this ADR's parent.
- Sub-issues [#339](https://github.com/Ontic-Systems/Gradient/issues/339) – [#343](https://github.com/Ontic-Systems/Gradient/issues/343).
- Epic E11 [#304](https://github.com/Ontic-Systems/Gradient/issues/304) — tooling suite; `gradient bench` ([#371](https://github.com/Ontic-Systems/Gradient/issues/371)) consumes this matrix; `gradient cross` ([#375](https://github.com/Ontic-Systems/Gradient/issues/375)) is a thin wrapper over `--target`.
- ADR 0001 — effect-tier foundation; effects are codegen-agnostic but Epic E5's runtime-DCE depends on LLVM-grade DCE for release builds.
- Existing CLIF dump path: `gradient --asm` (PR #395, closes [#373](https://github.com/Ontic-Systems/Gradient/issues/373)) is Cranelift-only; an LLVM IR dump path lands alongside [#339](https://github.com/Ontic-Systems/Gradient/issues/339).
- Roadmap: [`docs/roadmap.md` § Vision Roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02).

## Notes

The Q7 reference is to the alignment-session question that locked this decision. The session log is internal-only; this ADR is the canonical public record.
