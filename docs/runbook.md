# Gradient On-Call Runbook

## Purpose

This runbook covers the most common failure scenarios for the Gradient project. It is scoped to the CI pipeline and local development toolchain for the pre-production phase. Add a web-service section when a hosted component is deployed.

---

## Triage Levels

| Level | Description | Response |
|-------|-------------|----------|
| **P0** | `main` branch broken — no commits building | Drop everything; fix or revert within 1 hour |
| **P1** | Flaky test on `main`; CI unreliable | Fix within 24 hours; do not merge new work until resolved |
| **P2** | CI slow (job durations exceed SLO target) | Investigate within 48 hours |
| **P3** | Documentation or tooling nit | Next sprint |

---

## Runbook Entries

### 1. CI is failing on `main`

**Symptoms:** GitHub Actions shows red on the latest commit to `main`.

**Steps:**
1. Open the failing workflow run in GitHub Actions.
2. Identify which job failed (`check`, `e2e`, or both).
3. Expand the failing step to read the error.
4. If the failure is a **test regression**:
   - Find the commit that introduced it with `git bisect`.
   - Revert that commit: `git revert <sha>` — push to `main` immediately.
   - Open a fix issue and link to the reverted commit.
5. If the failure is a **dependency/toolchain issue** (e.g., `rust-toolchain` version change):
   - Pin the toolchain version in `rust-toolchain.toml` if not already.
   - Open a compatibility fix PR.
6. If the failure is a **cache poisoning issue** (stale `Swatinem/rust-cache`):
   - Delete the cache via GitHub Actions → Caches → select and delete.
   - Re-run the workflow.

---

### 2. Rust cache is stale / build is slow

**Symptoms:** `check` job takes > 5 minutes; no incremental compilation benefit.

**Steps:**
1. Check the `Swatinem/rust-cache` step output — confirm it restored a cache hit.
2. If no cache hit: check that the `workspaces: codebase` setting is correct in `.github/workflows/ci.yml`.
3. If cache hit but still slow: a dependency may have changed. This is expected after adding new crates; will normalise on next run.
4. If the cache is consistently poisoned, delete all caches from: Repository → Actions → Caches.

---

### 3. Clippy is reporting warnings as errors

**Symptoms:** The `Clippy` step fails with `error[clippy::...]`.

**Steps:**
1. Run locally: `cd codebase && cargo clippy --workspace -- -D warnings`
2. Read each error; fix the lint or add a justified `#[allow(clippy::...)]` with a comment.
3. Do not use `#[allow(clippy::all)]` — fix per-lint.
4. Push the fix. CI should go green.

---

### 4. End-to-end tests failing (`e2e` job)

**Symptoms:** The `e2e` job fails on a `.gr` test file; compile or link error.

**Steps:**
1. Identify the failing `.gr` file from the job log.
2. Reproduce locally:
   ```bash
   cd codebase
   cargo build -p gradient-compiler
   cargo run --quiet -p gradient-compiler -- compiler/tests/<failing>.gr /tmp/test.o
   cc /tmp/test.o -o /tmp/test_bin
   /tmp/test_bin
   ```
3. If the compile step fails: a recent compiler change broke code generation. Use `git bisect` to find the regression.
4. If the link step fails: the runtime C helper may be missing or mislinked. Check `codebase/compiler/runtime/gradient_runtime.c` is being compiled and linked.
5. Fix, push, verify CI green.

---

### 5. `gradient test` framework hangs or races

**Symptoms:** The "Test framework (sequential)" step times out or produces inconsistent results.

**Steps:**
1. This step runs with `--test-threads=1` to avoid temp-dir races. Ensure the step in CI still has that flag.
2. If hanging: a test may be blocking on stdin. Check for tests that use `read_line()` without piped input.
3. Run locally to reproduce: `cd codebase && cargo test -p gradient-test-framework -- --test-threads=1`
4. Isolate with `--filter <test-name>`.

---

### 6. Dependency vulnerability found (cargo audit)

**Symptoms:** `cargo audit` reports a `RUSTSEC` advisory (this step is tracked in [ONT-33](/ONT/issues/ONT-33)).

**Steps:**
1. Read the advisory details: `cargo audit --json | jq '.vulnerabilities'`
2. Check if an updated version of the affected crate is available: `cargo update <crate-name>`
3. If an update is available and compatible: update `Cargo.lock`, run tests, merge.
4. If no fix is available: assess exploitability in context (many advisories are not exploitable in a CLI compiler). Document the decision in a comment in `Cargo.toml`.
5. If actively exploitable: treat as P0.

---

## First Production Deploy Checklist

Before the first GitHub Release:

- [ ] All CI jobs green on `main` for the last 10 pushes
- [ ] `cargo audit` shows no critical vulnerabilities
- [ ] Branch protection enabled on `main` (see [ONT-35](/ONT/issues/ONT-35))
- [ ] `rustfmt` check passing (see [ONT-32](/ONT/issues/ONT-32))
- [ ] Release workflow exists and has been tested on a dry-run tag
- [ ] SLOs defined and alert validation steps completed (see `docs/monitoring.md`)
- [ ] This runbook reviewed and up to date

---

## Contacts & Escalation

| Role | Name | When to contact |
|------|------|-----------------|
| Infrastructure Lead | Ops | CI/CD, deployment, toolchain issues |
| CTO | CTO | Escalation for P0 incidents not resolved in 1 hour |

---

*Owner: Ops (Infrastructure Lead) — update when new failure modes are discovered.*
