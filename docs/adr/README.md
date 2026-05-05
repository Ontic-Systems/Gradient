# Architecture Decision Records

This directory holds Gradient's Architecture Decision Records (ADRs). An ADR captures a single significant architectural decision: the context that forced the decision, the alternatives considered, and the consequences accepted.

ADRs are append-only. When a decision changes, a new ADR is added that supersedes the old one; the old one is left in place with a `Status: Superseded by NNNN` line.

## Index

| # | Title | Status | Epic |
|---|---|---|---|
| [0001](0001-effect-tier-foundation.md) | Effect-tier foundation | Accepted | [#295](https://github.com/Ontic-Systems/Gradient/issues/295) |

## Planned

These ADRs are tracked under their respective epics and will land as the work begins:

| # | Title | Epic |
|---|---|---|
| 0002 | Arenas + capabilities (no lifetime annotations) | [#296](https://github.com/Ontic-Systems/Gradient/issues/296) (sub-issue [#326](https://github.com/Ontic-Systems/Gradient/issues/326)) |
| 0003 | Tiered contract enforcement | [#297](https://github.com/Ontic-Systems/Gradient/issues/297) (sub-issue [#332](https://github.com/Ontic-Systems/Gradient/issues/332)) |
| 0004 | Cranelift dev / LLVM release | [#299](https://github.com/Ontic-Systems/Gradient/issues/299) (sub-issue [#344](https://github.com/Ontic-Systems/Gradient/issues/344)) |
| 0005 | core/alloc/std with effect gating | [#300](https://github.com/Ontic-Systems/Gradient/issues/300) (sub-issue [#349](https://github.com/Ontic-Systems/Gradient/issues/349)) |
| 0006 | Inference engine + @app/@system | [#301](https://github.com/Ontic-Systems/Gradient/issues/301) (sub-issue [#354](https://github.com/Ontic-Systems/Gradient/issues/354)) |
| 0007 | Registry trust model | [#303](https://github.com/Ontic-Systems/Gradient/issues/303) (sub-issue [#370](https://github.com/Ontic-Systems/Gradient/issues/370)) |

## Format

Each ADR follows this skeleton:

```markdown
# ADR NNNN: <Title>

- Status: Accepted | Proposed | Superseded by NNNN | Deprecated
- Deciders: <names or session reference>
- Epic: #<gh-issue>
- Tracking issue: #<gh-issue>

## Context
What is the situation that forces a decision?

## Decision
What is the decision? (Imperative voice — "We will...")

## Consequences
### Positive
### Negative
### Neutral / deferred

## Related
Cross-refs to other ADRs, epics, sub-issues, and roadmap sections.
```

Keep ADRs short, decisive, and dated. The goal is a future agent (or human) reading one ADR and knowing what was decided, why, and what to read next — not a full design document.
