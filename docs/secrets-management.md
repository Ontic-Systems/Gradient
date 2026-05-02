# Secrets Management

> **STATUS:** implemented — Inventory and rotation policy reflect current repo state. Sigstore-signed registry workflow (Epic #303) will extend this when shipped.

## Audit Status

**Last audited:** 2026-03-30
**Result:** No plaintext secrets or credentials found in repository history or working tree.

---

## Secrets Inventory

Gradient is a compiler and language runtime — it makes no outbound network calls at runtime. The secret surface is limited to CI/CD and future publishing workflows.

| Secret | Purpose | Store | Rotation |
|--------|---------|-------|----------|
| `GITHUB_TOKEN` | Auto-injected by GitHub Actions for CI operations | GitHub Actions (auto) | Auto-rotated per run |
| `CARGO_REGISTRY_TOKEN` | Publish releases to crates.io (not yet active) | GitHub Actions secret | Annually or on compromise |
| `RELEASE_SIGNING_KEY` | Sign release artifacts (future) | GitHub Actions secret | Annually or on compromise |

No application secrets (database passwords, API keys, third-party tokens) are currently required — the project is a standalone compiler with no external service dependencies.

---

## Rules

1. **Never commit credentials.** `.env`, `.env.*`, `*.pem`, `*.key`, `*.p12` are in `.gitignore`.
2. **Secrets injected at CI runtime only.** Never bake secrets into Docker images or build artifacts.
3. **No plaintext in config files.** All runtime configuration uses environment variables; defaults in committed config files must be non-sensitive.
4. **Principle of least privilege.** CI jobs use the minimal GitHub Actions permissions needed (`contents: read` for most jobs; `contents: write` only for release jobs).

---

## GitHub Actions Permissions Baseline

Add this block to CI workflow jobs that do not need write access:

```yaml
permissions:
  contents: read
```

For future release jobs that publish artifacts:

```yaml
permissions:
  contents: write
  id-token: write  # for OIDC-based registry auth if used
```

---

## Adding a New Secret

1. Create the secret in **GitHub → Settings → Secrets and variables → Actions**.
2. Add it to the inventory table above.
3. Reference it in the workflow as `${{ secrets.SECRET_NAME }}` — never echo or log it.
4. If the secret is needed locally, add it to `.env` (which is gitignored) and document the variable name in `.env.example`.

---

## Rotation Policy

| Trigger | Action |
|---------|--------|
| Annual review | Rotate all long-lived secrets listed in the inventory |
| Suspected compromise | Rotate immediately; audit logs for unauthorized use |
| Team member offboarding | Rotate all secrets that person had access to |
| Dependency with known credential-leak CVE | Rotate same day |

Rotation checklist:
1. Generate new credential in the upstream service.
2. Update the GitHub Actions secret via Settings UI or `gh secret set`.
3. Verify CI still passes with the new value.
4. Revoke the old credential.
5. Note the rotation date in this document.

---

## Secret Scanning

- **GitHub Push Protection** should be enabled on the repository to block commits containing known secret patterns.
- For automated scanning in CI, consider adding [gitleaks](https://github.com/gitleaks/gitleaks) as a workflow step when the project gains more secrets surface.

To enable GitHub Push Protection:
`Settings → Code security → Push protection → Enable`

---

## Local Development

Developers who need runtime credentials for local testing should:

1. Copy `.env.example` to `.env` (gitignored).
2. Fill in the required values from the team's shared vault or request access from the project owner.
3. Never commit `.env`.

Currently no `.env.example` is needed because local builds require no credentials.
