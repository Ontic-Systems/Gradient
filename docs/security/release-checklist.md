# Gradient Release Checklist

> Cross-references: [`docs/SELF_HOSTING.md`](../SELF_HOSTING.md), [`docs/security/threat-model.md`](threat-model.md), [`docs/security/ddc.md`](ddc.md), [`docs/security/reproducible-builds.md`](reproducible-builds.md), [`docs/loc-dashboard.md`](../loc-dashboard.md).

This checklist gates each release tier against an honest set of requirements. **Tier escalation is one-way** — once we ship a release at a tier, we do not retreat from its requirements.

The three tiers (per [`SELF_HOSTING.md`](../SELF_HOSTING.md) / handoff alpha-readiness assessment) are:

1. **Rust-hosted Gradient alpha** — the Rust kernel + Cranelift backend + Query API + build CLI + contracts + stdlib pilot.
2. **Self-hosting bootstrap alpha** — `compiler/*.gr` self-hosted modules execute through `bootstrap_*` kernels for at least one full pipeline phase end-to-end.
3. **True self-hosted compiler alpha** — the self-hosted compiler compiles itself, with DDC.

## Tier 1 — Rust-hosted Gradient alpha

### Build / test gates

- [ ] `cargo test --workspace` green on `main` for at least three consecutive commits.
- [ ] CI lanes green: `check`, `security`, `e2e`, `wasm`, `verified`.
- [ ] Trust corpus ≥ 150 happy + 10 sad fixtures (current: 144 + 10 + 22 extended = 166 tests via #497).
- [ ] `@verified` stdlib pilot ≥ 12 modules with all obligations discharged on every CI green (current: **14** modules / 136 fns / 180 obligations via #498).
- [ ] `cargo clippy --workspace -- -D warnings` clean (or pre-existing drift documented in handoff).

### Documentation gates

- [ ] `README.md` § "What Works Today" matches kernel reality.
- [ ] `docs/agent-integration.md` STATUS line accurate.
- [ ] `docs/roadmap.md` epic status table accurate.
- [ ] `docs/loc-dashboard.md` auto-refresh action green within last 7 days.
- [ ] `docs/security/threat-model.md` rows updated for any sub-issue closed.
- [ ] All 7 ADRs in `docs/adr/` accepted (current: 0001–0007 ✓).

### Security gates

- [ ] `docs/security/effect-soundness.md` published (✓ via #492).
- [ ] `docs/security/threat-model.md` published (✓ via #493).
- [ ] `docs/security/agent-codegen-guidelines.md` published (✓ via #500).
- [ ] `docs/security/reproducible-builds.md` + CI gate live (✓ advisory via #499).
- [ ] DDC: **not required** at this tier; dry-run procedure documented (✓ via [`ddc.md`](ddc.md)).

### Release artifacts

- [ ] Release notes draft enumerating closed sub-issues + adversarial findings.
- [ ] Self-hosting honesty banner: "Rust kernel is the trusted center; self-hosting is bootstrap-stage."
- [ ] No claim of "compiler-VERIFIED" beyond the discharged stdlib pilot scope.

## Tier 2 — Self-hosting bootstrap alpha

In addition to all Tier 1 gates:

### Build / test gates

- [ ] At least one Hybrid phase has flipped to `SelfHostedDefault` (current Hybrid: `lex`, `parse`, `check`, `lower`).
- [ ] `compiler/main.gr` parses cleanly against the host parser (today: ~147 parse errors; tracked under [#379](https://github.com/Ontic-Systems/Gradient/issues/379)).
- [ ] Direct-execution readiness probe passes (`self-hosted-compiler-direct-execution-readiness` skill checklist).

### Security gates

- [ ] **Reproducible-build CI gate flipped from advisory to mandatory** (`continue-on-error: false`) on the Cranelift backend.
- [ ] DDC: **dry-run must pass** — single-compiler reproducibility green; DDC procedure walkthrough completed at least once even if no second toolchain is pinned yet.
- [ ] Comptime sandbox shipped ([#356](https://github.com/Ontic-Systems/Gradient/issues/356)) — banning `!{IO}` at compile time.
- [ ] `Unsafe` capability gate on `extern fn` ([#322](https://github.com/Ontic-Systems/Gradient/issues/322)) — tracks an adversarial-review item.
- [ ] LSP defaults to `@untrusted` mode ([#359](https://github.com/Ontic-Systems/Gradient/issues/359)) — tracks an adversarial-review item.

### Documentation gates

- [ ] Self-hosting share (`docs/loc-dashboard.md`) ≥ 25% of compiler LoC in `compiler/*.gr`.
- [ ] Public bootstrap walkthrough doc explaining how to build the self-hosted compiler from scratch.
- [ ] Release notes explicitly call out the self-hosting bootstrap claim and what it does/does not mean.

## Tier 3 — True self-hosted compiler alpha

In addition to all Tier 1 + Tier 2 gates:

### Build / test gates

- [ ] All four currently-Hybrid phases (`lex`, `parse`, `check`, `lower`) flipped to `SelfHostedDefault`.
- [ ] Self-hosted compiler compiles itself end-to-end (`compiler/main.gr` → bootstrap-stage → next-generation `gradient-compiler` binary).
- [ ] Trust corpus ≥ 250 fixtures including direct-execution-validated paths.
- [ ] Self-hosting share ≥ 75% of compiler LoC in `compiler/*.gr` (current: 8.9%).

### Security gates

- [ ] **DDC must be passing as a hard gate.** See [`ddc.md`](ddc.md) for procedure.
- [ ] Two diverse reference compilers selected, pinned, and documented (per `ddc.md` Step 0).
- [ ] DDC run has produced bit-identical artifacts at least once.
- [ ] Reproducible-build gate must remain mandatory; any regression to advisory disqualifies a Tier 3 release.
- [ ] All adversarial-review findings the related findings closed (current: the related findings-deliverable closed; the related findings open).
- [ ] Effect-system soundness mechanized at least informally (Coq sketch acceptable; current: informal sketch only).

### Documentation gates

- [ ] Self-hosting walkthrough updated to show the self-hosted compiler is canonical.
- [ ] Bootstrap chain diagram in `docs/SELF_HOSTING.md` shows the chain ends at the self-hosted compiler.
- [ ] Public DDC report shipped with the release artifact, listing the two reference compilers + sha256 matches.

## Per-release update protocol

When cutting a release at any tier:

1. Run this checklist top-to-bottom against the proposed release branch.
2. For every unchecked box that is **out of scope for the tier**, mark explicitly with a justification.
3. For every unchecked box that is **in scope but failing**, do not cut the release.
4. Update [`docs/security/threat-model.md`](threat-model.md) for any surface row whose status changed.
5. Run [`scripts/loc-dashboard.sh`](../../scripts/loc-dashboard.sh) and re-commit if it drifted.
6. Tag the release with `vTIER.X.Y-PATCH` (e.g. `v1.0.0-alpha-rust-hosted`).
7. After the tag, re-confirm the GitNexus index is up-to-date.

## Cross-references

- [`docs/SELF_HOSTING.md`](../SELF_HOSTING.md) — honest split of the three tiers.
- [`docs/security/ddc.md`](ddc.md) — DDC procedure and current obstacles.
- [`docs/security/reproducible-builds.md`](reproducible-builds.md) — DDC's precondition.
- [`docs/security/threat-model.md`](threat-model.md) — security surface registry.
- [`docs/loc-dashboard.md`](../loc-dashboard.md) — self-hosting share dashboard.
