# Contributing to Gradient

## Commit Message Standards

All commits must follow the **Conventional Commits** format:

```
<type>(<scope>): <description>
```

### Types

| Type | Description |
|------|-------------|
| `feat` | A new feature |
| `fix` | A bug fix |
| `docs` | Documentation only changes |
| `style` | Code style changes (formatting, semicolons, etc.) |
| `refactor` | Code changes that neither fix bugs nor add features |
| `perf` | Performance improvements |
| `test` | Adding or correcting tests |
| `build` | Changes to build system or dependencies |
| `ci` | Changes to CI configuration |
| `chore` | Other changes that don't modify src or test files |
| `revert` | Reverts a previous commit |
| `merge` | Merge commit |

### Scope (Optional)

Common scopes in this repo:
- `parser` - Parser changes
- `lexer` - Lexer changes
- `typechecker` - Type checker changes
- `codegen` - Code generation changes
- `runtime` - Runtime changes
- `ast` - AST changes
- `ir` - Intermediate representation
- `contracts` - Contract verification
- `docs` - Documentation

### Subject Line Rules

1. Use **imperative mood** ("add" not "added")
2. Don't capitalize the first letter
3. Don't end with a period
4. Keep it under 72 characters
5. **Backtick-wrap any Gradient `@attribute`** to avoid pinging unrelated GitHub users — see "Gradient `@attribute` syntax in Markdown" below.

### Examples

```
feat(parser): implement error recovery strategies

fix(codegen): resolve memory leak in closure capture
test(typechecker): add exhaustiveness check tests
ci: update GitHub Actions to v4
merge: PR #42 (feat/new-parser)
```

### Setup

The repo includes:
- `.gitmessage` - Commit message template
- `.git/hooks/commit-msg` - Validation hook

Configure the template:
```bash
git config commit.template .gitmessage
```

The hook validates automatically on commit.

## Pull Request Standards

### PR Title Format

Same as commits:
```
<type>(<scope>): <description>
```

Examples:
- `feat: implement actor message passing`
- `fix(codegen): correct float register allocation`
- `docs: add API reference for context budget`

### PR Description Template

```markdown
## Summary
Brief description of changes

## Changes
- List of specific changes

## Testing
- How was this tested?

## Related Issues
Fixes #XXX
```

## Code Style

- Rust: Follow `cargo fmt` and `cargo clippy`
- C: Follow existing style in runtime files
- Use 4 spaces for indentation
- Maximum line length: 100 characters

## Issue-First Workflow

All bug fixes and substantial changes must follow the issue-first workflow:

### For Bug Fixes
1. **Create an issue first** describing the bug
2. **Reference the issue** in your PR description with `Fixes #XXX`
3. **Batch small fixes** - combine related small bugs into a single issue/PR
4. Only merge after CI passes

### For Features
1. **Reference the roadmap** in your PR description
2. Ensure alignment with project direction
3. Update documentation with the feature

### Issue Templates

Use the appropriate template when creating issues:
- **Bug Report**: For bugs and unexpected behavior
- **Feature Request**: For new features and enhancements

## Testing

All changes must include tests:
- Unit tests for new functionality
- Integration tests for end-to-end features
- Update existing tests if behavior changes

Run tests before submitting:
```bash
cargo test --workspace
```

## Documentation

Update documentation with every change:
- README.md (if user-facing changes)
- API documentation (if public API changes)
- Changelog (if applicable)
- Any relevant guides or tutorials

Never reference internal documents (agent handoffs, private notes) in public-facing PRs or issues.

## Gradient `@attribute` syntax in Markdown

Gradient uses `@`-prefixed attributes — `@trusted`, `@untrusted`, `@verified`, `@cap`, `@extern`, `@export`, `@test`, `@requires`, `@ensures`, `@budget`, `@app`, `@system`, `@runtime_only`, plus future ones. Many of these names are real GitHub accounts, so writing them unguarded in a commit subject, PR title, PR body, issue title, or issue body **pings strangers**.

**Rule:** any time a Gradient `@attribute` appears in any GitHub-rendered Markdown context (commit message, PR/issue title or body, README, docs/), wrap it in backticks: `` `@verified` ``, `` `@untrusted` ``, etc. Code blocks (triple-backtick fenced) are also fine.

| ❌ Don't | ✅ Do |
|---|---|
| `feat(stdlib): @verified pilot module` | `` feat(stdlib): `@verified` pilot module `` |
| `closes #N via @untrusted source mode` | `` closes #N via `@untrusted` source mode `` |
| `@cap whitelist enforcement` | `` `@cap` whitelist enforcement `` |

This includes when an `@attribute` is part of a longer sentence in the PR body. The backticks are the minimum; consider also using fenced code blocks for example snippets.

CI enforces this on every pull request via the `attribute-mention-guard` lane in `.github/workflows/ci.yml`, which runs `scripts/check-attribute-mentions.py` against the PR title, PR body, and every commit message in the PR. To check a message locally before pushing:

```bash
python3 scripts/check-attribute-mentions.py --text "feat(stdlib): @verified pilot module"
python3 scripts/check-attribute-mentions.py --file /tmp/commit_msg.txt
git log -1 --format='%B' | python3 scripts/check-attribute-mentions.py --stdin
python3 scripts/check-attribute-mentions.py --self-test       # script regression tests
python3 scripts/check-attribute-mentions.py --list-attributes # show the known set
```

## CI workflow identity

CI workflows must not be configured with a non-author git identity (e.g. a fictitious `project-bot`) that pushes back to `main` from inside the workflow. Pull requests should come from individual contributor accounts. Any auto-generated artifact that would otherwise need to land on `main` must do so via a regular PR opened from a real contributor account, or be regenerated on-demand and not committed at all.

## Branch protection on `main`

`main` is protected. All changes go through pull requests. Force-pushes and deletions are restricted to repository admins.

Required status checks: `check`, `e2e`, `security`, `verified`, `wasm`. Advisory lanes (`reproducible-build`, `fuzz_smoke`) are visible but not blocking.
