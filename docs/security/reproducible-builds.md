# Reproducible builds

> Issue: [#362](https://github.com/Ontic-Systems/Gradient/issues/362) — closes adversarial finding **F8 (MEDIUM)**.
> Epic: [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (threat model).
> Cross-references: [`docs/security/threat-model.md`](threat-model.md) row S8 (self-hosted compiler / DDC).

The Gradient compiler ships with a CI gate that runs two clean builds back-to-back and asserts they produce **bit-identical** binaries. This is a prerequisite for the DDC (diverse double compilation) story tracked under [#361](https://github.com/Ontic-Systems/Gradient/issues/361) — without bit-identical reproducibility, DDC cannot detect a Trojan'd kernel.

## Recipe (local)

```bash
scripts/reproducible-build-check.sh
```

The script:

1. Locks `SOURCE_DATE_EPOCH` to the commit timestamp (`git log -1 --pretty=%ct`). Same convention as [reproducible-builds.org](https://reproducible-builds.org/).
2. Runs `cargo build --release --frozen --locked --bin gradient-compiler` twice into separate `CARGO_TARGET_DIR`s.
3. Hashes each binary with `sha256sum`.
4. Exits 0 if the hashes match, 1 if they differ, 2 on environmental error.

Example output:

```
[1/2] First build (target=/tmp/gradient-build-a-XXXXXXXX) with SOURCE_DATE_EPOCH=1715000000...
      sha256 = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
[2/2] Second build (target=/tmp/gradient-build-b-XXXXXXXX) with SOURCE_DATE_EPOCH=1715000000...
      sha256 = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
REPRODUCIBLE: both builds produced sha256 = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

## Recipe (CI)

The dedicated [`reproducible-build`](../../.github/workflows/reproducible-build.yml) workflow runs the script on every push to `main`, every PR, and on a weekly schedule. The job is timeout-capped at 30 minutes (two release builds of the compiler typically take 4-5 minutes each on `ubuntu-latest`).

When the gate fails, the failure message includes the two SHA-256 hashes and a `diffoscope` invocation hint for local triage.

## Determinism levers in use

| Lever | Where | Why |
|---|---|---|
| `SOURCE_DATE_EPOCH=<commit ts>` | env, both builds | Locks any timestamp Cargo / rustc embeds in the binary. |
| `cargo build --frozen --locked` | both builds | Forbids implicit `Cargo.lock` updates between the pair. |
| `RUSTFLAGS="-C codegen-units=1"` | both builds | Single-threaded codegen; multi-CGU builds are non-deterministic by default. |
| Separate `CARGO_TARGET_DIR` per build | both builds | Prevents warm artifacts from one bleeding into the other. |
| Cranelift backend (default) | both builds | Cranelift is the launch-tier backend; LLVM is gated on E6 (see below). |

## Known limitations

These are deliberately tracked so the gap between "the gate passes" and "the full claim of reproducibility" is visible to anyone reading this doc.

1. **LLVM backend out of scope.** The LLVM backend is gated on Epic [#299](https://github.com/Ontic-Systems/Gradient/issues/299) (backend split, ADR 0004). When LLVM lands, the matching reproducibility levers (`-C link-arg=-Wl,--build-id=none`, `-Cmetadata=...`, `-Clink-arg=-Wl,-no_uuid` on macOS) need adding. Until then this gate covers the Cranelift path only.
2. **Cross-compilation not yet covered.** Once [#342](https://github.com/Ontic-Systems/Gradient/issues/342) (cross-compile via `--target`) lands, the gate should run reproducibility checks per supported triple, not just on the host.
3. **Self-hosted compiler outputs not yet checked.** When the self-hosted `compiler/*.gr` tree starts producing real artifacts ([Epic #116](https://github.com/Ontic-Systems/Gradient/issues/116) — currently bootstrap-stage), reproducibility checks need extending to those artifacts.
4. **Build-id and ELF metadata stripping not yet enforced.** The script relies on `SOURCE_DATE_EPOCH` and `--frozen --locked` to be sufficient. If a future Cargo / rustc default regresses on this, the gate will surface drift but the doc/script don't yet document the post-processing steps. This is the next thing to add if drift recurs.
5. **Toolchain not pinned in this repo.** The `rust-toolchain.toml` (or equivalent) needs to pin the rustc/cargo version exactly so two builds across machines/CI runners use the same toolchain. Tracked as a follow-on under #362 if drift surfaces.

## How DDC will use this

DDC ([#361](https://github.com/Ontic-Systems/Gradient/issues/361)) requires that two independent compilers compiling the same source produce byte-identical artifacts. That property is meaningless if a *single* compiler doesn't even produce identical artifacts twice in a row. So this gate is the prerequisite — it answers the easier "single-compiler reproducibility" question first.

When DDC lands, the workflow becomes: build with reference compiler A → build with reference compiler B → both with reproducible-build levers above → compare. Today's gate covers the first leg.

## Update protocol

When a new determinism lever becomes necessary (e.g. LLVM lands, or a Cargo regression surfaces), update this doc *and* `scripts/reproducible-build-check.sh` *in the same PR*. Both must move together so future contributors don't have to dig through git history to find the rationale for a flag.

When the gate red-lines on `main`, treat as P1: a non-reproducible compiler is a precondition for several attestation claims and downstream tooling.

## Cross-references

- [`docs/security/threat-model.md`](threat-model.md) — surface row S8.
- [`docs/security/README.md`](README.md) — security doc index.
- [Epic #302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model umbrella.
- [#361](https://github.com/Ontic-Systems/Gradient/issues/361) — DDC bootstrap verification (the consumer of this guarantee).
- [reproducible-builds.org](https://reproducible-builds.org/) — community standard for source-date-epoch + determinism levers.
