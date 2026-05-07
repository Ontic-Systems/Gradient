# Diverse Double-Compile (DDC) — Procedure + Current Obstacles

> Issue: [#361](https://github.com/Ontic-Systems/Gradient/issues/361) — tracks an adversarial-review item.
> Epic: [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (threat model).
> Cross-references: [`reproducible-builds.md`](reproducible-builds.md), [`threat-model.md`](threat-model.md) row S8, [Epic #116](https://github.com/Ontic-Systems/Gradient/issues/116) (full self-hosting).

## Background — what DDC defends against

David A. Wheeler's *Diverse Double-Compile* (DDC) defends against the "Trusting Trust" attack (Thompson, 1984): a compromised compiler that inserts a backdoor into every program it compiles, *including itself*, persisting across rebuilds even when the source is clean.

The defense: compile your trusted source `S` with two **diverse** trusted compilers `T₁` and `T₂`. If both produce **bit-identical** artifacts, the source `S` is what it claims to be — neither compiler could have inserted the backdoor without the other producing a different artifact.

For Gradient, the threat is:

- The Rust kernel compiler (`codebase/compiler/src/*.rs` built by rustc) is itself the trust root.
- The self-hosted compiler (`compiler/*.gr`) is being progressively built. Eventually it will compile itself.
- Without DDC, a Trojan'd `rustc` could plant a backdoor in the Rust kernel that survives every self-hosted bootstrap.

## Current status — **procedure documented, run not yet possible**

DDC requires that the self-hosted compiler **actually execute end-to-end** so that two independent reference compilers can compile the same Gradient source. As of [`SELF_HOSTING.md`](../SELF_HOSTING.md) and the kernel-boundary catalog (`codebase/compiler/src/kernel_boundary.rs`):

- 4 phases run as `SelfHostedDefault` (`emit`, `pipeline`, `query`, `lsp`).
- 2 phases run as `SelfHostedGated` (`driver`, `trust`).
- 4 phases remain `Hybrid` (`lex`, `parse`, `check`, `lower`).

The self-hosted compiler does **not yet compile a non-trivial Gradient program from `.gr` source to a runnable binary**. Until it does, DDC has no artifact to compare. Honest acceptance for [#361](https://github.com/Ontic-Systems/Gradient/issues/361):

- [x] Procedure documented (this file).
- [ ] Run at least once successfully — **gated on Epic [#116](https://github.com/Ontic-Systems/Gradient/issues/116) reaching direct execution**. Documented as obstacle below.
- [x] Mitigations documented (this file).
- [x] Recorded as part of release checklist (see [`release-checklist.md`](release-checklist.md)).

## Procedure (when self-hosting reaches execution)

Once the self-hosted compiler can compile itself, the run looks like this. Until then, the procedure is dry-run only — every step has a working command shape, but the artifacts at the end are placeholders.

### Step 0. Pre-conditions

1. **Reproducibility**. Single-compiler reproducibility must already be green. See [`reproducible-builds.md`](reproducible-builds.md) — when that gate is no longer advisory, this precondition is met.
2. **Two diverse trusted compilers**. Today, there is exactly one Rust kernel compiler. We need a second, diverse compiler. Plausible candidates (in increasing diversity-distance order):
 - **Build the Rust kernel with two different rustc toolchains** (e.g. stable vs nightly, or two different stable point-releases). Weakest form of DDC because the same backdoor in `rustc` itself would propagate to both.
 - **Cross-compile the Rust kernel via two different LLVM versions**. Stronger.
 - **Bootstrap a *minimal* second compiler in a different language family** (e.g. a hand-rolled OCaml interpreter that runs `compiler/*.gr` directly). Strongest. Significant effort.

 For the launch tier we plan option 1 (two-rustc) and document the attack surface that remains: a Trojan'd rustc would still defeat DDC.

3. **A canonical "trust source" `S`**. The self-hosted compiler's `compiler/*.gr` tree at a specific commit, built from a clean checkout, with `Cargo.lock` and `compiler/*.gr` pinned by hash.

### Step 1. Build the self-hosted compiler with the first reference compiler

```bash
# Reference compiler 1: rustc 1.X stable
rustup default 1.X-stable
SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct) \
RUSTFLAGS="-C codegen-units=1 -C link-arg=-Wl,--build-id=none --remap-path-prefix=$PWD=. --remap-path-prefix=$HOME/.cargo=/cargo" \
CARGO_TARGET_DIR=/tmp/ddc-stage1 \
 cargo build --manifest-path codebase/Cargo.toml \
 -p gradient-compiler --release --bin gradient-compiler --locked

# Use stage 1 to compile the canonical .gr trust source.
/tmp/ddc-stage1/release/gradient-compiler \
 --emit-binary --out /tmp/ddc-self1 compiler/main.gr
sha256sum /tmp/ddc-self1
```

### Step 2. Build the self-hosted compiler with the second reference compiler

```bash
# Reference compiler 2: rustc 1.Y stable (diverse from Y in step 1).
rustup default 1.Y-stable
# (same env vars + RUSTFLAGS as step 1)
CARGO_TARGET_DIR=/tmp/ddc-stage2 \
 cargo build .. # identical to step 1 except CARGO_TARGET_DIR

/tmp/ddc-stage2/release/gradient-compiler \
 --emit-binary --out /tmp/ddc-self2 compiler/main.gr
sha256sum /tmp/ddc-self2
```

### Step 3. Compare

```bash
if [ "$(sha256sum /tmp/ddc-self1 | awk '{print $1}')" = "$(sha256sum /tmp/ddc-self2 | awk '{print $1}')" ]; then
 echo "DDC PASS: bit-identical"
else
 echo "DDC FAIL: artifacts differ"
 diffoscope /tmp/ddc-self1 /tmp/ddc-self2
 exit 1
fi
```

### Step 4. Cross-bootstrap (full Wheeler form)

The previous three steps verify that **the Rust kernel** is reproducible across two rustc toolchains. The full Wheeler form additionally verifies that **the self-hosted compiler binary** (when built by the Rust kernel) compiles the next-stage self-hosted compiler bit-identically. This requires the self-hosted compiler to be capable of emitting a binary equivalent to `gradient-compiler`.

When that capability exists:

```bash
/tmp/ddc-self1 --emit-binary --out /tmp/ddc-stage3a compiler/main.gr
/tmp/ddc-self2 --emit-binary --out /tmp/ddc-stage3b compiler/main.gr
sha256sum /tmp/ddc-stage3a /tmp/ddc-stage3b # must match
```

If steps 1–4 all pass, the DDC claim holds: the self-hosted compiler is what its source claims to be, modulo the trust posture of the two reference compilers.

## Obstacles (today)

1. **Self-hosted compiler does not yet execute end-to-end.** The dominant obstacle. Tracked under Epic [#116](https://github.com/Ontic-Systems/Gradient/issues/116). Until at least the `lower` phase is no longer Hybrid and `compiler/main.gr` is runnable, steps 2 and 3 cannot run against a real artifact. Mitigation: document the procedure now (this file) so the run is a small step once execution lands.
2. **Single-compiler reproducibility is not yet green.** See [`reproducible-builds.md`](reproducible-builds.md) § "Current status — advisory". DDC requires reproducibility as a precondition; until the gate flips to mandatory, DDC's "bit-identical" comparison is meaningless. Mitigation: tighten determinism levers PR-by-PR; flip the gate when two consecutive runs match.
3. **`compiler/main.gr` modernization needed.** Pre-flight against the host parser shows ~147 parse errors against current canonical syntax (outdated `type Name:` + `case X` enum forms; `:` instead of `->` for return-type separators). See [skill `gradient-main-gr-modernization`](../../#) and [#379](https://github.com/Ontic-Systems/Gradient/issues/379). Mitigation: PR-sized cleanup before mod-wrap.
4. **Second diverse compiler not chosen.** Today we have exactly one rustc lineage; selecting a second toolchain choice is itself a security decision. Mitigation: document the threat surface of each candidate (see Step 0 above) and pick the strongest plausible option that does not delay the launch.
5. **CI-level cost.** Each DDC run involves two release builds + two self-hosted bootstraps, each ~5+ minutes. Running on every PR is prohibitive. Mitigation: run DDC weekly + on release tags + on workflow_dispatch, not per-commit.

## Mitigations summary

| Obstacle | Mitigation | Tracking |
|---|---|---|
| Self-host not executing | Continue Hybrid phase flips | [#116](https://github.com/Ontic-Systems/Gradient/issues/116) |
| Reproducibility advisory | Tighten determinism levers | [#362](https://github.com/Ontic-Systems/Gradient/issues/362) |
| `main.gr` not parseable | Modernize syntax then mod-wrap | [#379](https://github.com/Ontic-Systems/Gradient/issues/379) |
| No second reference compiler | Plan to pin two rustc toolchains; consider OCaml minimal compiler later | This document |
| CI cost | Schedule, not on-every-PR | This document |

## Release checklist hook

The release checklist ([`release-checklist.md`](release-checklist.md)) carries one DDC item per release:

- For Rust-hosted alpha: **DDC not required** (no self-hosting claim).
- For self-hosting bootstrap alpha: **DDC dry-run must pass** — that is, single-compiler reproducibility green + procedure dry-runnable.
- For true self-hosted compiler alpha: **DDC must be passing** as a hard gate.

## What this doc does and does not claim

It **does** claim:

- The DDC procedure for Gradient is documented end-to-end.
- The obstacles preventing a real DDC run today are explicitly named and tied to tracking issues.
- The release checklist ties DDC to the right release tier.

It **does not** claim:

- A DDC run has been executed. (It hasn't; the precondition isn't met.)
- The Rust kernel is verified Trojan-free. (DDC defends against this; until DDC runs, the kernel is in the same trust posture as any Rust application.)

## Cross-references

- David A. Wheeler, *Fully Countering Trusting Trust through Diverse Double-Compiling*, 2009. [PDF](https://dwheeler.com/trusting-trust/).
- Ken Thompson, *Reflections on Trusting Trust*, 1984.
- [`reproducible-builds.md`](reproducible-builds.md) — DDC's prerequisite.
- [`threat-model.md`](threat-model.md) — surface row S8.
- [`release-checklist.md`](release-checklist.md) — DDC hook per release tier.
- [Epic #116](https://github.com/Ontic-Systems/Gradient/issues/116) — full self-hosting.
- [Epic #302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model.
