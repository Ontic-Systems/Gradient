# Package manifest format (`gradient.toml`)

> Status: **launch-tier spec** — covers the fields landed in the build-system
> manifest parser via #365. Capability-token specifics (post-#321 typestate
> work) will get their own follow-on section once the typestate engine
> lands.

A Gradient project's root directory carries a `gradient.toml` file. It is
the canonical declaration of what the package is, what it depends on,
and what surface area it claims (effects + capabilities). The same file
is consumed by:

- `gradient build` / `gradient run` / `gradient test` (resolution + build).
- `gradient publish` (registry upload — #367).
- `gradient install` (registry-side signature + manifest verification — #368).
- `gradient build` (manifest-effect enforcement at build time — #366).

This document is the **format specification**. For installation flow,
signing semantics, and registry endpoints, see the sibling docs once
those issues land (linked from each section below).

## TL;DR — minimal manifest

```toml
[package]
name = "my-project"
version = "0.1.0"
edition = "2026"

[dependencies]
```

That's it. Existing projects without `[dependencies]` continue to parse —
the table is optional. Effects + capabilities are optional too; they default
to "the package declares no manifest-level ceiling".

## TL;DR — agent-emitted manifest with declared surface area

```toml
[package]
name = "agent-runner"
version = "0.1.0"
edition = "2026"
effects      = ["Heap", "IO", "Net"]
capabilities = ["FS", "Time"]

[dependencies]
json = "1.0"
utils = { path = "../utils" }
```

The agent-readable surface is locked in two new arrays:

| Field | What it declares | Validated against |
|---|---|---|
| `effects` | The maximum effect set any function in this package may use. | `gradient_compiler::typechecker::effects::KNOWN_EFFECTS` plus parameterized shapes (`Throws(<T>)`, `FFI(<abi>)`, `Arena(<name>)`). |
| `capabilities` | Capabilities the package requests at install/build time. | Same vocabulary as `effects` today; will diverge post-#321. |

## Sections

### `[package]`

| Key | Type | Required | Notes |
|---|---|---|---|
| `name` | `string` | yes | `^[a-zA-Z][a-zA-Z0-9_-]{0,63}$`. M-1 rule rejects flag-shaped names. |
| `version` | `string` | yes | SemVer string. The build-system uses the `semver` crate for parsing. |
| `edition` | `string` | no | The Gradient edition. `"2026"` for current. |
| `effects` | `array<string>` | no | **#365/#366.** Declared maximum effect set. Empty list = pure package. Absent = no manifest-level ceiling. `gradient build` rejects functions using effects outside the declared union of `effects` + `capabilities`. |
| `capabilities` | `array<string>` | no | **#365/#366.** Declared capability requests. Absent = no requests. Until #321, build enforcement treats this as the same launch-tier vocabulary as `effects`. |

### `[dependencies]`

Three dependency shapes are supported. They have not changed in #365 —
this is documented here for reference.

#### Simple version dependency

```toml
[dependencies]
math = "1.0"
```

#### Path dependency

```toml
[dependencies]
math-utils = { path = "../math-utils" }
```

#### Git dependency (commit SHA required by H-1)

```toml
[dependencies]
utils = { git = "https://github.com/example/utils.git", rev = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0" }
```

The `rev` field is **required** for git dependencies — it must be a
40-character hex SHA. H-1 in the build-system rejects any git dep
without it. Branches and tags are not accepted at the manifest level.

#### Registry dependency

```toml
[dependencies]
math = { version = "1.2.0", registry = "github" }
```

Today `"github"` is wired for fetch/add flows. `gradient publish` supports the
launch-tier signed upload path via a `file://` registry target: it writes a
`.gradient-pkg` bundle, derives the authoritative `gradient-package.toml`, signs
the artifact with `cosign sign-blob --bundle`, and uploads the artifact,
sigstore bundle, manifest, and publish metadata under `<registry>/<name>/<version>/`.

## Effect-name vocabulary

The bare effect names accepted in `[package].effects` and
`[package].capabilities` come from `KNOWN_EFFECTS` in
`codebase/compiler/src/typechecker/effects.rs`. As of #365 the set is:

```
IO, Net, FS, Mut, Time, Actor, Async, Send, Atomic, Volatile,
Heap, Stack, Static
```

In addition, the manifest accepts these **parameterized** effect shapes
(the same shapes the typechecker recognizes at function-signature scope):

| Shape | Example | Spec |
|---|---|---|
| `Throws(<TypeName>)` | `Throws(ParseError)` | ADR-locked exception channel. |
| `FFI(<abi>)` | `FFI(C)` | ADR-locked FFI channel. ABI = `C` today. |
| `Arena(<name>)` | `Arena(scratch)` | Parameterized arena scope (#544). |

Anything else is rejected at `parse(toml)` time, with the offending
name + field surfaced in the error message:

```
[package].effects contains unknown effect 'NotAnEffect'. Allowed:
["IO", "Net", "FS", "Mut", "Time", "Actor", "Async", "Send",
 "Atomic", "Volatile", "Heap", "Stack", "Static"] plus parameterized
forms like Throws(<Type>), FFI(<abi>), Arena(<name>).
```

This validator is the single point of agreement between agent-emitted
manifests and what the typechecker actually accepts as effects at
function-signature scope. If a future PR changes `KNOWN_EFFECTS`, the
manifest parser will accept the new vocabulary automatically — no
duplication.

## Effects vs. capabilities — current state

In Gradient today, capabilities are modeled as effect-name lists (see
`@cap(IO, Net)` in `ast/item.rs::CapDecl`). The manifest deliberately
splits them into two fields so that:

1. **Forward-compatibility.** When E3 (#296 / #321) lands typestate
   capability tokens distinct from effects, the `capabilities` field
   already has its own slot and can pick up a new validator without a
   manifest-schema breaking change.

2. **Agent affordance.** A reader (registry, IDE plugin, agent
   pre-flight) can distinguish "this package needs `Heap` to do its
   internal work" from "this package wants the caller to hand it the
   `FS` capability". Today both validate against the same set of names;
   tomorrow they will diverge.

Until #321 lands, the two fields use the same validator. Documented
behavior change:

| Today (#365) | After #321 |
|---|---|
| `capabilities = ["FS"]` validated by `is_valid_effect_name`. | `capabilities = ["FS"]` validated against the capability typestate model — a strict subset of effect names AND/OR capability-only names. |

## Round-trip guarantees

The build-system's `manifest::parse` and `toml::to_string` round-trip
preserves:

- `name`, `version`, `edition` (string fields).
- `effects` (`Option<Vec<String>>` — order-preserving).
- `capabilities` (`Option<Vec<String>>` — order-preserving).
- `dependencies` (`BTreeMap<String, Dependency>` — name-ordered).

Round-trip is covered by
`codebase/build-system/src/manifest.rs::tests::manifest_round_trip_preserves_effects_and_capabilities`.

## Validation rules (post-#365)

In order:

1. **M-1**: `[package].name` matches `^[a-zA-Z][a-zA-Z0-9_-]{0,63}$`,
   doesn't start with `-`.
2. **H-1**: every git dependency carries `rev = "<40-char hex SHA>"`.
3. **#365**: every entry in `[package].effects` is recognized by
   `is_valid_effect_name`.
4. **#365**: every entry in `[package].capabilities` is recognized by
   `is_valid_effect_name`.
5. **#365**: neither array contains an empty-string entry.

An empty `effects = []` array is **valid** — it means "this package
performs no effects" (pure package). An absent `effects` field is also
valid and means "no manifest-level ceiling".

## Build-time ceiling enforcement (#366)

When either `[package].effects` or `[package].capabilities` is present,
`gradient build` enforces the manifest surface after the compiler succeeds
and before runtime helpers are compiled/linked.

The build-system uses the compiler Query API to inspect every function in
`src/main.gr`. For each function, it unions declared effects and inferred
effects; any effect missing from the manifest's `effects + capabilities`
union is a build error. Module-level capability ceilings surfaced by the
Query API are checked the same way. Error messages include `gradient.toml`,
the offending function or module name, the missing effect/capability, and
the declared ceiling list.

Important distinction:

- absent `effects` and absent `capabilities` = no manifest-level ceiling;
- present empty array, e.g. `effects = []`, = explicit empty ceiling;
- until #321, `capabilities = ["IO"]` can satisfy a function that uses
  `IO` because capabilities share the launch-tier effect vocabulary.

## Compatibility notes

- **Old manifests parse unchanged.** All pre-#365 manifests omit
  `effects` and `capabilities`; both fields default to `None`.
- **Untrusted manifests.** `gradient build` enforces the declared
  manifest ceiling whenever `effects` or `capabilities` is present. Future
  registry/install hooks (#367/#368/#369) will reuse the same metadata for
  upload, signature, and install-time policy.
- **TOML version.** The manifest is parsed with `toml = "0.8"`. Any
  TOML-1.0 feature is fair game.

## Examples

### Pure stdlib-style package

```toml
[package]
name = "core-math"
version = "0.1.0"
effects = []
```

### Application package with full effect set

```toml
[package]
name = "scratch-pad"
version = "0.2.0"
effects = ["Heap", "IO", "FS", "Time"]

[dependencies]
serde = "1.0"
```

### Library that requests caller-provided capabilities

```toml
[package]
name = "log-writer"
version = "0.1.0"
capabilities = ["FS"]

[dependencies]
```

### Mixed parameterized effects

```toml
[package]
name = "ffi-wrapper"
version = "0.1.0"
effects = ["FFI(C)", "Throws(WrapError)", "Heap"]
```

## Cross-references

| Topic | Where |
|---|---|
| Effect vocabulary | `codebase/compiler/src/typechecker/effects.rs` (`KNOWN_EFFECTS`, `is_valid_effect_name`). |
| Manifest parser | `codebase/build-system/src/manifest.rs`. |
| Threat model — manifest-effect enforcement | `docs/security/threat-model.md` (post-#366 update). |
| Acceptance issue | #365 (this PR). |
| Parent epic | #303 (E10 — Package registry). |
| Follow-on issues | #366 (manifest effect enforcement at build), #367 (publish), #368 (install verification), #369 (registry backend MVP). |
