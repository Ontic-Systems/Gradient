# ADR 0007: Registry trust model

- Status: Accepted (locked 2026-05-02)
- Deciders: Gradient core (alignment session Q13)
- Epic: [#303](https://github.com/Ontic-Systems/Gradient/issues/303)
- Tracking issue: [#370](https://github.com/Ontic-Systems/Gradient/issues/370)
- Related ADRs: [ADR 0001](0001-effect-tier-foundation.md), [ADR 0002](0002-arenas-capabilities.md), [ADR 0003](0003-tiered-contracts.md)
- Related epics: capabilities ([#296](https://github.com/Ontic-Systems/Gradient/issues/296)), threat model ([#302](https://github.com/Ontic-Systems/Gradient/issues/302))

## Context

Gradient targets agents as first-class users (per the [vision roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02)). The package supply chain is therefore an agent-supply-chain question: a malicious or typosquatted dependency the agent picks up while emitting code can compromise an entire program before any human reviews it. The status quo for systems-language registries does not address this:

1. **Cargo / npm — central registry, account-bearer trust.** Anyone can publish under `gradient-foo`. Typosquatting (`gradent-foo`, `gradient_foo`) is unmitigated. There is no machine-readable description of what a package's public API actually does — an agent reads the README and trusts it. Account-takeover compromises every package the account owns. This is the model the 2026-05-02 adversarial review (F9) flagged as inadequate for an agent-emission language.
2. **Deno — URL imports, no central index.** Solves typosquatting (URLs are unique) but loses discovery and the trust UX is "trust this URL." Agents have no fingerprint to verify against; mirrors and CDN hijacks are silent failures.
3. **Go modules — content-addressed by VCS path + version + checksum.** Closer to a workable model — checksums prevent silent tampering, the proxy + sumdb provide a transparency layer — but capabilities and effects are not modeled at the package boundary, so the agent still has to read the source to know what authority a package will demand.

There is a fourth option built into the rest of the language stack: every dependency carries a [capability set](0002-arenas-capabilities.md) and an [effect row](0001-effect-tier-foundation.md) at the function tier. If those propagate cleanly to the package tier, an agent reads a one-line manifest and knows exactly what authority a package needs and what side effects its public API can produce — before any code executes.

We need a registry trust model that:

- Forces every published package to declare the maximal capability set + maximal effect row its public API requires.
- Makes the publication signature non-repudiable and cross-verifiable without the registry as a trust root (so a registry compromise is detectable).
- Prevents typosquatting at install time via reserved-name policy + a fuzzy-match warning.
- Lets `@untrusted` consumers refuse a package whose manifest declares an authority they will not grant.
- Composes with the threat model (Epic E9, [#302](https://github.com/Ontic-Systems/Gradient/issues/302)) so `@untrusted` plugin packages cannot acquire root capabilities even if their manifest declares them.
- Is buildable with off-the-shelf components rather than re-inventing crypto.

## Decision

**Gradient's package registry is sigstore-signed + capability-scoped manifests + transparency log + reserved-name policy.** Every published package carries a machine-readable manifest declaring the maximal effect row + capability set its public API requires; `gradient publish` signs the package via sigstore (keyless OIDC); `gradient install` verifies the signature, the manifest, and a Rekor-style transparency-log inclusion proof before extracting any source. The registry is a CDN over the signed artifacts — a compromise of the registry does not compromise the trust chain because the verification anchors are sigstore + the transparency log, not the registry itself.

The architecture borrows directly from PyPI's sigstore integration, npm's provenance attestations, and Sigstore + Rekor / Cosign for the crypto layer. The novelty is the capability-scoped manifest: nothing else in the systems-language registry space declares per-package authority at the surface today.

### Manifest format (sub-issue [#365](https://github.com/Ontic-Systems/Gradient/issues/365))

Each published package carries `gradient-package.toml` (the lockfile-grade authoritative manifest, distinct from the source-tree's working `gradient.toml`). Required fields:

```toml
[package]
name        = "http-client"
version     = "0.4.2"
edition     = "2026"
description = "Minimal HTTP/1.1 client for Gradient."

[trust]
# The maximal capability set the public API requires. The checker
# rejects compilation of any callsite that fails to provide one of these.
capabilities = ["Network", "Filesystem"]

# The maximal effect row of any function in the public API. Internal
# functions may carry more, but they are not exported.
public_effects = ["IO", "Net", "FS", "Throws(_)"]

# Optional declarations consumed by ADR 0003 (contracts) and the
# threat model (Epic E9).
contract_tier         = "runtime"   # | "verified" | "runtime_only_off_in_release"
predicate_language    = "lia+arrays"
trust_label           = "trusted"   # | "untrusted"
elided_in_release     = []          # contracts elided in release builds
unsafe_extern_count   = 12          # number of `extern fn` callsites under Unsafe
allowed_origins       = ["github.com/example/http-client"]

[provenance]
source_repo    = "https://github.com/example/http-client"
source_commit  = "1a2b3c4d5e6f"
build_workflow = ".github/workflows/release.yml"
build_runner   = "github-hosted-ubuntu-22.04"

[dependencies]
core = "1.0"
tls  = { version = "0.3", capabilities = ["Network"] }
```

The `[trust]` section is the load-bearing piece. Every field there propagates through the dependency graph and is enforced at the consumer's checker:

- `capabilities`: a transitive sum across all dependencies. The consumer's `gradient.toml` declares which capabilities the program is allowed to acquire; if a dependency demands one not in that allowlist, the install fails.
- `public_effects`: lower bound on the consumer's effect row. A function calling into this package picks up these effects as a minimum.
- `trust_label`: `untrusted` packages cannot acquire root capability providers (`core::cap::root_unsafe()` etc.). Composes with Epic E9's `@trusted`/`@untrusted` source-tier annotation.
- `unsafe_extern_count`: surfaces `extern fn` density. The install-time UI reports it; CI policy gates on it (configurable threshold).
- `allowed_origins`: the registry rejects publication if `source_repo` is not on this list. Prevents account-takeover from re-publishing under a stale identity.

### Manifest enforcement at build (sub-issue [#366](https://github.com/Ontic-Systems/Gradient/issues/366))

The checker reads `gradient-package.toml` for every dependency at compile time and enforces:

- **Effects are bounded by `public_effects`.** A public function in package X cannot have a checker-inferred effect row exceeding what its manifest declares. CI rejects publication; the consumer fails fast on a mismatched local install.
- **Capabilities propagate.** Every callsite into package X must hold every capability listed in X's `[trust].capabilities`. The diagnostic points at the manifest line.
- **Trust label propagates.** `@untrusted` consumers can only depend on `untrusted` packages by default; relaxing this requires an explicit `trust_override` in `gradient.toml` per-dependency.
- **Capability override fences.** The consumer's `gradient.toml` may declare `[trust.allow_capabilities] = ["Network"]` to lock the program to a subset; any dependency declaring a non-allowed capability fails install with a structured diagnostic listing which capability was rejected.

This is mechanical: capabilities are typestate from [ADR 0002](0002-arenas-capabilities.md) and effects are rows from [ADR 0001](0001-effect-tier-foundation.md). The registry layer reads the same structures the compiler has already been computing.

### `gradient publish` (sub-issue [#367](https://github.com/Ontic-Systems/Gradient/issues/367))

The publish flow (keyless sigstore via OIDC):

1. `gradient publish` builds the package, computes the manifest's `[trust]` section by re-running the checker over the public API.
2. Source tree + lockfile + manifest are bundled into a signed artifact (`.gradient-pkg`).
3. The publisher authenticates via OIDC (GitHub Actions, Google, GitLab — sigstore's standard providers). Keyless: no long-lived publisher key.
4. Sigstore's Fulcio issues a short-lived certificate bound to the OIDC identity.
5. The artifact + certificate + signature are appended to a Rekor-style transparency log; the inclusion proof is bundled with the package.
6. The package is uploaded to the registry CDN. The CDN is a content-addressed store; the registry trust chain is in the signature + transparency log, not the storage layer.

The CI workflow that runs `gradient publish` is itself part of the `[provenance]` declaration; downstream consumers see "this package was built by `.github/workflows/release.yml` on commit `1a2b3c4d5e6f`" and can verify the workflow on the source repo matches the declared identity.

### `gradient install` (sub-issue [#368](https://github.com/Ontic-Systems/Gradient/issues/368))

The install flow (verify-before-extract):

1. `gradient install <package>@<version>` fetches the signed artifact + signature + transparency log inclusion proof.
2. Verifies the sigstore signature against the bundled certificate.
3. Verifies the certificate's OIDC binding matches an `allowed_origins` entry in the manifest's `[trust]` section.
4. Verifies the transparency log inclusion proof against a public Rekor mirror — at least two of three configured mirrors must agree (so a single-mirror compromise is detectable).
5. Verifies the manifest's declared `[trust]` section against the consumer's `gradient.toml` allowlist; aborts on capability mismatch with a structured diagnostic.
6. Only after all checks pass: extracts the source tree to the local cache.
7. **Typosquat fuzzy-match warning.** If the requested name is within edit distance ≤2 of a higher-downloaded package, the install prints a warning naming both candidates and requires `--accept-typo-risk` to proceed for non-interactive sessions.

The verification anchors are sigstore + Rekor + the manifest, not the registry hostname. A registry compromise (hijacked CDN, DNS takeover) does not bypass the trust chain — the local install would fail at step 2 or 4.

### Backend service MVP (sub-issue [#369](https://github.com/Ontic-Systems/Gradient/issues/369))

The registry backend is intentionally minimal: a content-addressed object store (signed artifacts indexed by `name@version` → blob hash), a metadata index (manifests for browsing), and a publication endpoint that accepts a sigstore-signed upload + transparency-log proof. It does not own a key. It does not own the trust chain. Its job is "host the bytes; serve the metadata; accept signed publications." MVP scope: an S3-style object store + a metadata service + the publication endpoint. Federation across multiple registries is post-MVP.

### Reserved-name policy

A small reserved-name list (`core`, `alloc`, `std`, `gradient`, `gradient-*`, plus a curated set of canonical-package names) is enforced at the registry. Publications using reserved names require explicit allow-listing by the registry operator. Prevents the "official-looking package" failure mode while leaving the namespace open for organic ecosystem growth.

### Package-defined capabilities

Beyond the launch-set capabilities ([ADR 0002](0002-arenas-capabilities.md): `Unsafe`, `Filesystem`, `Network`, `Spawn`, `RawPointer`, `Hardware`), packages may declare additional capabilities in their manifest:

```toml
[trust.defines_capabilities]
HttpClient = { sealed_under = "Network", description = "Issues HTTP requests" }
```

A package-defined capability is sealed under a launch-set capability — it cannot be acquired without holding the launch-set parent. This lets a package express finer-grained authority ("this package needs HTTP, not raw sockets") without diluting the launch set. The manifest format is the canonical declaration; the consumer's checker reads it and tracks the new capability through typestate exactly like a launch-set capability.

## Consequences

### Positive

- **Closes F9.** Adversarial review's "registry must not ship without manifests + sigstore" is addressed at every layer: manifest format ([#365](https://github.com/Ontic-Systems/Gradient/issues/365)), enforcement ([#366](https://github.com/Ontic-Systems/Gradient/issues/366)), publication signing ([#367](https://github.com/Ontic-Systems/Gradient/issues/367)), install verification ([#368](https://github.com/Ontic-Systems/Gradient/issues/368)), backend service ([#369](https://github.com/Ontic-Systems/Gradient/issues/369)).
- **Agent-readable trust boundary.** An agent picks a dependency by reading its manifest's `[trust]` section, not its README. The capability set + effect row + contract tier + unsafe extern count are all machine-readable.
- **Compromise containment.** Registry hijack does not bypass verification. Account takeover is detectable via `allowed_origins`. Single-mirror Rekor compromise is detectable via 2-of-3 mirror agreement. Sigstore key revocation is the standard recovery path.
- **Composes cleanly with the rest of the stack.** No new compiler infrastructure: the manifest's trust fields are precisely the structures [ADR 0001](0001-effect-tier-foundation.md), [ADR 0002](0002-arenas-capabilities.md), and [ADR 0003](0003-tiered-contracts.md) already produce. The registry is a serialization layer over compiler outputs.
- **`@untrusted` source mode becomes enforceable.** Threat model (Epic E9, [#302](https://github.com/Ontic-Systems/Gradient/issues/302)) gets a real lever — a manifest's `trust_label = "untrusted"` actually does something at install time and at compile time.
- **Off-the-shelf crypto.** Sigstore + Rekor are production-grade and operated by the OpenSSF. We do not invent any cryptographic primitives.

### Negative

- **Sigstore is OIDC-bound.** Publishers without a supported OIDC provider cannot publish. Mitigated because the launch set covers the major CI providers (GitHub Actions, GitLab, Google) and self-hosted Fulcio is documented as a fallback, but the friction is real for hobbyist publishers.
- **Manifest computation depends on the checker.** A change to the effect inference engine (Epic E8, [ADR 0006](0006-inference-modes.md)) can shift `public_effects` for an unchanged package — the package would need to re-publish to update the manifest. Mitigated by version pinning the manifest's effect set against a compiler-version field; consumers using a newer compiler get a per-import diagnostic rather than a silent re-inference.
- **Transparency log mirror dependence.** Install requires Rekor mirror reachability. Offline installs need a local mirror cache. We document this in the install flow and ship a `gradient mirror` cache helper post-MVP.
- **Capability set is fixed at publication time.** A package adding a capability requirement after publication requires a new version. This is correct (capability-set changes are semver-major) but it does mean a package cannot quietly broaden its authority footprint.

### Neutral / deferred

- **Registry federation.** Multiple registries trusting each other's signatures, mirror-of-mirror policies, and namespace handoffs are post-MVP. Single-registry MVP is sufficient to close F9.
- **Yanking + revocation.** A published version can be marked yanked in the metadata index, but the signed artifact remains in the transparency log. Revocation semantics (reject install entirely vs warn + allow) are deferred to a follow-on policy ADR.
- **Private registries.** Enterprise / proprietary registries can implement the same protocol with a self-hosted Fulcio + Rekor; the launch design does not require it but does not preclude it.
- **Cross-package verified contracts.** A `@verified` function in package A calling a `@verified` function in package B requires both packages share the predicate language ([ADR 0003](0003-tiered-contracts.md)'s `predicate_language` field). Cross-package verified call composition rules beyond simple equality are deferred.

## Implementation order

Sub-issues land in this order so each adds enforcement + at least one observable check:

1. [#365](https://github.com/Ontic-Systems/Gradient/issues/365) Manifest format spec. The TOML schema + a JSON-Schema mirror for tooling validation. Includes a sample manifest in `docs/registry/manifest.md`.
2. [#366](https://github.com/Ontic-Systems/Gradient/issues/366) Manifest enforcement at build. Reads the manifest from a vendored / locally-cached package; checker rejects effect/capability mismatches. Includes the `[trust.allow_capabilities]` consumer override.
3. [#367](https://github.com/Ontic-Systems/Gradient/issues/367) `gradient publish` — sign + upload via sigstore. MVP supports GitHub Actions OIDC + Google OIDC. Self-hosted Fulcio post-MVP.
4. [#368](https://github.com/Ontic-Systems/Gradient/issues/368) `gradient install` — verify signature + manifest + Rekor inclusion proof. Includes the typosquat fuzzy-match warning.
5. [#369](https://github.com/Ontic-Systems/Gradient/issues/369) Backend service MVP. S3-style object store + metadata index + publication endpoint. Reserved-name policy enforced.

Each sub-issue includes:

- Schema or protocol spec (manifest TOML, OIDC provider list, transparency log endpoint format).
- CLI behavior + structured-diagnostic format on the matching error paths.
- At least one integration test publishing + installing through a local sigstore + Rekor stack.
- Documentation update under `docs/registry/`.

## Comparison

### vs Cargo

| Dimension | Cargo (crates.io) | Gradient (this ADR) |
|---|---|---|
| Authentication | Account-bearer API token | sigstore keyless OIDC + transparency log |
| Tampering detection | Lockfile checksum | sigstore signature + Rekor inclusion proof + manifest |
| Authority declaration | none (any package, any unsafe) | manifest `[trust]` capabilities + `public_effects` |
| Typosquatting mitigation | reactive admin takedown | reserved-name policy + edit-distance install warning |
| Compromise containment | account takeover = full pwnage | `allowed_origins` + sigstore identity binding |
| Agent surface | source-readable | manifest-readable |

### vs Deno

Deno's URL imports cleanly solve typosquatting (URLs are unique) and avoid central-registry compromise. They do not solve discovery, supply-chain provenance, capability declaration, or mirror integrity. This ADR keeps Deno's "no single trust root" property (the trust chain is sigstore + transparency log, not the registry) while adding the capability + effect manifest Deno does not have.

### vs Go modules + sumdb

Go is the closest precedent. Content-addressing + the sumdb transparency layer line up with sigstore + Rekor here. The gap Go does not fill is the per-package authority declaration: the Go ecosystem still relies on source review for "what does this package do." This ADR adds the manifest-level `[trust]` section that makes that question machine-readable.

### vs npm provenance attestations

npm provenance (sigstore-based, GitHub Actions OIDC) is the direct architectural precedent for the publication signing flow. We adopt the same primitives and add the capability manifest.

## Related

- [ADR 0001](0001-effect-tier-foundation.md) — `public_effects` lower bounds the consumer's effect row.
- [ADR 0002](0002-arenas-capabilities.md) — `[trust].capabilities` enumerates the launch-set + package-defined capabilities; capabilities propagate through the dependency graph.
- [ADR 0003](0003-tiered-contracts.md) — `contract_tier` + `predicate_language` declarations enable cross-package verified-call composition.
- Epic E2 [#295](https://github.com/Ontic-Systems/Gradient/issues/295) — effects.
- Epic E3 [#296](https://github.com/Ontic-Systems/Gradient/issues/296) — capabilities; this ADR consumes its launch set.
- Epic E4 [#297](https://github.com/Ontic-Systems/Gradient/issues/297) — contracts; manifest declares the package-tier contract posture.
- Epic E9 [#302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model; `trust_label` propagates `@untrusted` through the dependency graph.
- Epic E10 [#303](https://github.com/Ontic-Systems/Gradient/issues/303) — this ADR's parent.
- Roadmap: [`docs/roadmap.md` § Vision Roadmap](../roadmap.md#vision-roadmap-locked-2026-05-02).
- Adversarial finding F9 — closed at the implementation tier when sub-issues #365–#369 land.

## Notes

The Q13 alignment-session decision locked the sigstore + capability-manifest direction. The session log is internal-only; this ADR is the canonical public record.

Sigstore, Rekor, Fulcio, and Cosign are operated by the Open Source Security Foundation (OpenSSF) and are the same primitives backing PyPI, npm, and Homebrew supply-chain attestations. This ADR does not introduce new cryptographic protocols; it serializes the existing capability + effect surface into the manifest format those primitives sign.
