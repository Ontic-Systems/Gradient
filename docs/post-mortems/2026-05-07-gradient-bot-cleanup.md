# Post-mortem: gradient-bot, GH user mentions, and bypassed CI

> **Date**: 2026-05-07
> **Severity**: Repo-integrity / brand-trust issue. No security breach, no leaked credentials, no real third-party access.
> **Detected by**: Maintainer (graydeon).
> **Resolved by**: Force-pushed history rewrite (commit drop + message backtick-wrap), workflow deletion, branch protection install.

## What went wrong

Three independent issues, all introduced in earlier development sessions, surfaced together as a single cleanup task:

### Issue 1: A fictitious `gradient-bot` was pushing directly to `main`

`.github/workflows/loc-dashboard.yml` (originally landed in PR #491) configured git as:

```yaml
git config user.name  "gradient-bot"
git config user.email "gradient-bot@users.noreply.github.com"
```

…and then ran `git push origin HEAD:main` from inside the workflow using the auto-provisioned `GITHUB_TOKEN`.

Effects:

- 8 commits across 2026-05-06 and 2026-05-07 were authored as `gradient-bot`. None was reviewed; none went through a PR.
- The bot identity is **not a real GitHub account**. Anyone who registers the `gradient-bot` login on GitHub afterward would inherit those commits' attribution.
- The maintainer hadn't authorized adding any contributor; this surfaced unexpectedly in the GitHub "Contributors" UI.

### Issue 2: Gradient `@attribute` syntax accidentally pinged real GitHub users

Gradient's surface uses `@`-prefixed attributes (`@verified`, `@trusted`, `@untrusted`, `@cap`, `@extern`, `@export`, `@test`, `@requires`, `@ensures`, `@budget`, `@app`, `@system`, `@runtime_only`). When mentioned unbacticked in a commit subject or PR body, GitHub interprets these as user mentions.

A spot-check confirmed that 11 of the 13 attribute names are **real GitHub accounts** (verified via `GET /users/:login` returning HTTP 200):

```
trusted   200    untrusted 200    verified 200
cap       200    extern    200    export   200
test      200    requires  200    budget   200
app       200    system    200
ensures   404    runtime_only 404
```

39 commit subjects on `main` (and ~30 PR titles / bodies) carried unguarded `@attribute` mentions before this cleanup, each generating an unwanted notification to a stranger.

### Issue 3: PRs merged with a CI lane showing `failure`

Seven recent PRs (#499, #503, #504, #505, #506, #507, #508) were merged with the `reproducible-build` check-run reporting `failure`.

The check-run was *intentionally* non-blocking (`continue-on-error: true` in `.github/workflows/reproducible-build.yml`, marking the lane as advisory while residual cargo/rustc release-build non-determinism is eliminated). The workflow itself reports green; only the per-job check-run reports red.

The deeper problem: **`main` had zero branch protection at the time.** No required status checks, no required PR, no force-push restriction. So even if the lane *had* been blocking, nothing would have stopped a `gh pr merge --squash` from going through, and nothing stopped the `loc-dashboard.yml` workflow from running `git push origin HEAD:main` directly.

## Why it happened

1. **No branch protection on `main`.** The repo was created and operated as a single-maintainer monorepo where every commit is the maintainer's own work, so the natural assumption was that protection isn't needed. That assumption breaks the moment any agent / CI process is given write access.

2. **Convenience push from CI workflow.** The LoC dashboard idea (refresh a markdown table on every push) was a small UX win, but the implementation took the easy path — push back to `main` from inside the workflow — instead of opening a PR. Auto-PRs would have been visible; auto-pushes are invisible.

3. **Gradient's `@attribute` syntax was designed without considering Markdown rendering.** This is a language-design / docs-style issue, not a code bug. Backtick-wrapping is the obvious fix once you notice it, but the `@` collision was never called out as a contribution rule.

## What was done to fix it

### Immediate (this cleanup)

1. **Removed `loc-dashboard.yml` and `scripts/loc-dashboard.sh`.** No more bot identity, no more direct push from CI.

2. **Rewrote `main` history via `git filter-repo`.**
   - **Phase A**: Removed `docs/loc-dashboard.md` from history entirely (`--path docs/loc-dashboard.md --invert-paths`). This made every `gradient-bot` commit empty (each one only touched that file), and filter-repo auto-prunes empty commits, so the 8 bot commits disappeared along with the file.
   - **Phase B**: A `--message-callback` wraps every unbacticked `@<gradient-attribute>` token in commit messages with backticks, so historic commit subjects no longer ping real GH users.

3. **Force-pushed rewritten `main`.** A backup of the pre-rewrite state was tagged as `backup/pre-cleanup-2026-05-07` (pushed to origin) and a working `.git` snapshot was preserved at `/tmp/git-backup-pre-rewrite/` on the maintainer's host.

4. **Added branch protection on `main`** (see "Going forward" below).

5. **Added `# Repository conventions` section to `AGENTS.md`** documenting:
   - The `@attribute` ↔ `@user` collision and the backtick rule.
   - The "no CI auto-push to `main`" rule.
   - The "all commits come from the maintainer's account" rule.

### Going forward

- **Branch protection installed on `main`**:
  - Require pull request before merging (1 approval — auto-satisfied by the maintainer).
  - Require status checks to pass: `check`, `e2e`, `security`, `verified`, `wasm`. (`reproducible-build` and `fuzz_smoke` remain advisory until they stabilize.)
  - Block force-pushes (admin override for cleanup operations like this one).
  - Block deletions.

- **No CI auto-pushes to `main`**. If we ever re-introduce the LoC dashboard, it will be a regenerated artifact (recomputed on demand) or land via a regular PR, not a direct push.

- **Backtick rule for `@attributes`** is now part of `AGENTS.md`. Future agent sessions will see it on context-load.

## What we did NOT do

- **Did not strip `Co-Authored-By: Claude` trailers** from the 24 historical commits that have them. The maintainer chose the "MINIMAL" cleanup tier, which preserves authorship attribution as long as the actual git author is `graydeon`. The trailers are documentary and don't grant access.
- **Did not invalidate the maintainer's git history.** All `graydeon` commits before the cleanup retain their content; only their SHAs changed because of the message rewrite.
- **Did not touch any feature/agent branches on origin.** Those remote branches still exist with their original history. They're either stale (pre-cleanup) or scratch space.

## Verification checklist

- [ ] `git log --all --format='%an' | grep -i gradient-bot` returns nothing on origin/main.
- [ ] `git log --format='%s' origin/main | grep -E '@(verified|trusted|untrusted|cap|extern|export|test|requires|budget|app|system)\b' | grep -v '\`@'` returns nothing.
- [ ] `gh api repos/Ontic-Systems/Gradient/branches/main/protection` returns the protection rule (not 404).
- [ ] `.github/workflows/loc-dashboard.yml` no longer exists.
- [ ] `gh api repos/Ontic-Systems/Gradient/collaborators` lists only `graydeon`.

## Lessons

1. **Branch protection is not optional, even on solo repos**, the moment any automation is given write access.
2. **Domain-specific syntax that overloads platform syntax (`@attr` vs `@user`) needs a contribution-style rule from day one.** Cheap to add early, expensive to retrofit.
3. **"Advisory" CI lanes look identical to "failing" CI lanes in the merge-time UI.** When introducing an advisory lane, make sure it's visually distinct (consider a different lane name, e.g. `reproducible-build-advisory`).
