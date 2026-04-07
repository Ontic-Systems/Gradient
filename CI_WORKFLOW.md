# CI PR Workflow

This repository uses a PR-based workflow with live CI monitoring.

## Making Changes

All changes must go through pull requests. Direct commits to main are blocked.

## Tools

- `ci-monitor <pr-number>` - Monitor CI status until complete
- `pr-workflow` - Full PR orchestration

## Quick Start

```bash
# Create branch
pr-workflow branch feature/my-feature

# Make changes, then commit
pr-workflow commit "description" scope type
pr-workflow push

# Create PR and monitor
pr-workflow pr-create "title"
pr-workflow pr-monitor <number>
pr-workflow pr-merge <number>
```
