# Gradient Security Reference

**Last updated:** 2026-04-23
**Status:** Active — reflects security posture as of the Wave 1–5 remediation initiative.

---

## 1. Effect Rows

Gradient tracks side effects in the type system. Every function that performs a side effect must declare it in its signature via `!{EffectName}`. The compiler enforces this — a function without an `!{...}` annotation is compiler-proven pure.

### 1.1 Canonical Effect Set

| Effect | Meaning | Example builtins |
|--------|---------|-----------------|
| `!{IO}` | stdio, process control | `print`, `read_line`, `exit` |
| `!{FS}` | filesystem read/write | `file_read`, `file_write`, `list_dir` |
| `!{Net}` | outbound network | `http_get`, `http_post` |
| `!{Env}` | process environment access | `get_env`, `set_env` |
| `!{Crypto}` | cryptographic randomness | `secure_random_bytes` |
| `!{Mut}` | global mutable state | mutable globals |
| `!{Time}` | clock / sleep | `now`, `sleep` |
| `!{Actor}` | actor spawn / message passing | `spawn`, `send`, `ask` |

Unknown effects (e.g. `!{Foo}`) are a compile error with a diagnostic listing valid names.

### 1.2 `!{Env}` — Environment Effect

`!{Env}` gates read and write access to the OS process environment.

**Why a separate effect?** Before Wave 4, `get_env`/`set_env` were typed as `!{IO}`. This meant any function that printed to stdout could also silently exfiltrate or overwrite environment variables without the caller knowing. The `!{Env}` row makes that capability explicit and auditable in type signatures.

```gradient
# Requires !{Env}
fn read_api_key() -> !{Env} String:
    get_env("API_KEY")

# Compile error — missing !{Env}
fn bad() -> !{IO} String:
    get_env("SECRET")   # error: get_env requires !{Env}, declare it
```

**Migration from `!{IO}`:** The compiler emits a migration diagnostic when `get_env`/`set_env` appear in a function declared only `!{IO}`. The fix is to add `!{Env}` to the effect set.

**Security implication:** Any function calling `get_env` or `set_env` is now visibly branded at the type level. Code review and `@cap` module annotations can prohibit environment access in untrusted modules.

### 1.3 `!{Crypto}` — Cryptographic Randomness Effect

`!{Crypto}` gates access to the CSPRNG.

**Why a separate effect?** `random_int`/`random_float` use a seeded PRNG — not cryptographically secure, subject to prediction, and not suitable for secrets, tokens, or nonces. `secure_random_bytes` calls `getrandom(2)` / `arc4random_buf` / `BCryptGenRandom` depending on platform. The two must not be confused.

```gradient
# Non-crypto randomness — does NOT require !{Crypto}
fn roll_die() -> !{IO} Int:
    random_int(1, 6)

# Cryptographic randomness — requires !{Crypto}
fn gen_token(n: Int) -> !{Crypto} ByteString:
    secure_random_bytes(n)
```

`random_int` and `random_float` remain available for non-security purposes (simulations, games, tests). Their signatures do **not** carry `!{Crypto}` — using them for secret generation is a logic error visible in the type.

**Migration note:** `random_int` previously used modulo bias (`rand() % range`). As of Wave 4, it uses rejection sampling to eliminate non-uniform distribution for non-power-of-2 ranges. This is not a breaking change — same signature, corrected implementation.

### 1.4 Using `@cap` to Limit Module Effects

Modules that should not perform environment or crypto operations can enforce that at the module level:

```gradient
@cap(!{IO})   // This module may only use IO; Env, Crypto, Net, FS are forbidden
module MyPureLib:
    ...
```

Any function in a `@cap`-restricted module that tries to use a forbidden effect is a compile error.

---

## 2. The `spawn` vs `shell` Boundary

### 2.1 The Hazard

The deprecated `system` builtin (internally `shell`) executed its argument via `/bin/sh -c`. Any caller-controlled string passed to `system` was a shell injection vector:

```gradient
# UNSAFE — do not use
let result = system("process " + user_input)  # shell metacharacters in user_input → RCE
```

The shell interpreter expands `$VAR`, backticks, semicolons, pipes, redirection operators, and glob patterns before the target process ever sees the argument.

### 2.2 The Safe Replacement: `spawn`

`spawn` executes a program directly via `posix_spawnp` / `execvp`. No shell is invoked. Arguments are passed as a `List[String]` and reach the child process verbatim — the OS does not interpret them.

```gradient
# Safe
let status = spawn("process", ["--flag", user_input])
```

No string in `user_input` can escape the argument vector. Semicolons, `$`, backticks, and redirection operators are literal characters from the child process's perspective.

**Signature:**
```gradient
spawn : (String, List[String]) -> !{IO} Int
```

The first argument is the executable name (resolved via `PATH`). The second is the argument list. The return value is the child process exit status.

### 2.3 Migration from `system`

`system` is deprecated and emits a compiler warning. It will be removed in the next major release. Migration:

```gradient
# Before
let _ = system("git commit -m " + msg)

# After
let _ = spawn("git", ["commit", "-m", msg])
```

For cases where shell features (pipelines, redirections) are genuinely required, spawn a shell explicitly and pass arguments safely:

```gradient
# Explicit shell invocation — clear and auditable
let _ = spawn("sh", ["-c", "cat /etc/hosts | wc -l"])
# WARNING: only safe when the shell script is a hard-coded literal, not user input
```

### 2.4 WASM Boundary

In WASM builds, the spawn boundary is enforced by capability isolation at the WASI layer. The WASM runtime must be invoked with explicit capability grants:

```sh
wasmtime --dir=./data --env=HOME=/tmp output.wasm
```

Running with `--dir=/` or without `--dir`/`--env` grants ambient authority and undermines the capability model. The WASM backend tracks `!{FS}` and `!{IO}` effects to emit only the WASI imports actually required — a pure-function module emits zero WASI imports (see `docs/WASM.md`).

---

## 3. Dependency Pinning Policy

### 3.1 Gradient Package Dependencies (`gradient.toml`)

All Gradient package dependencies must be pinned to a specific commit SHA. Tag references are a hard error:

```toml
# FORBIDDEN — tags are mutable and can be force-pushed
foo = { git = "https://github.com/org/foo", tag = "v1.0.0" }

# REQUIRED — commit SHAs are immutable
foo = { git = "https://github.com/org/foo", rev = "a1b2c3d4e5f6..." }
```

The lockfile (`gradient.lock`) records an `archive_sha256` field alongside the resolved SHA:

```
[package.foo]
rev = "a1b2c3d4e5f6..."
archive_sha256 = "<hex>"   # sha256 of downloaded archive bytes, before extraction
```

The resolver verifies this hash on every subsequent fetch. A mismatch is a hard build error.

**Why:** A mutable tag pointing at a different commit is an undetected supply chain substitution. A SHA + archive hash provides two independent checks: the VCS history cannot be altered (SHA), and the downloaded artifact matches what was originally locked (archive hash).

### 3.2 Rust/Cargo Dependencies

- All crates are pinned in `Cargo.lock` (committed to the repository).
- `cargo build` and CI always run with `--locked` to prevent silent lockfile drift.
- `Cargo.toml` specifies minimum compatible versions; `Cargo.lock` pins exact versions.
- OpenSSL transitive dependencies are eliminated — the project uses `rustls` for TLS (`reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }`). Verify with `cargo tree -i openssl` (must return nothing).

### 3.3 CI Enforcement

Two automated checks run on every PR and on a weekly schedule:

**`cargo deny check`** (configured in `deny.toml`):
- `[advisories]`: fails on crates with known CVEs.
- `[licenses]`: rejects crates with incompatible licenses.
- `[bans]`: blocks known-problematic crates.

**`cargo audit`** (weekly scheduled workflow):
- Queries the RustSec advisory database.
- Fails CI if any crate in `Cargo.lock` has a published advisory.

Cranelift is pinned to the current minor in `Cargo.toml` to prevent silent major-version upgrades introducing new codegen surface.

### 3.4 Release Artifact Verification

Release archives ship with a `SHA256SUMS` file. The install script (`scripts/install.sh`) verifies the downloaded binary against this file before executing it. The installed binary's SHA256 is printed to stdout post-install for independent verification.

Future: a `RELEASE_SIGNING_KEY` slot is reserved in GitHub Actions secrets for signing release artifacts.

---

## 4. Reporting Vulnerabilities

To report a security issue privately, email the project owner at the address in `CONTRIBUTING.md` or open a GitHub Security Advisory (Settings → Security → Advisories → New draft advisory). Do not file public issues for unpatched vulnerabilities.

---

## 5. References

| Document | Topic |
|----------|-------|
| `docs/WASM.md` | WASM security properties, capability isolation |
| `docs/secrets-management.md` | CI/CD secrets, rotation policy, scanning |
| `docs/RUNTIME_AUTHORITY.md` | Which C runtime is linked; runtime symbol inventory |
| `docs/language-guide.md` | Full effect system reference, `@cap` annotation |
| `CHANGELOG.md` | Per-release security entries under `### Security` |
