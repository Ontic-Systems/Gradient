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

## Testing

All changes must include tests:
- Unit tests for new functionality
- Integration tests for end-to-end features
- Update existing tests if behavior changes

Run tests before submitting:
```bash
cargo test --workspace
```
