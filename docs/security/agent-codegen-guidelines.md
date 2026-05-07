# Agent codegen guidelines — prompt-injection resistance

> Issue: [#364](https://github.com/Ontic-Systems/Gradient/issues/364) — closes adversarial-review tooling finding **TF2**.
> Epic: [#302](https://github.com/Ontic-Systems/Gradient/issues/302) (threat model).
> Cross-references: [`docs/agent-integration.md`](../agent-integration.md), [`docs/security/threat-model.md`](threat-model.md) row S1, [`docs/security/effect-soundness.md`](effect-soundness.md).

This document codifies the recommended agent codegen practices for emitting Gradient code. Following these guidelines makes generated code resistant to prompt-injection attacks where a hostile data source convinces an LLM to emit code with effects, capabilities, or contracts the user did not authorize.

The guidelines are deliberately **prescriptive** — agents should treat each item as a hard rule, and tooling that wraps Gradient codegen should enforce them mechanically where possible.

## Threat model

The relevant threat actor is **A1 — untrusted source author** from [`threat-model.md`](threat-model.md). The attacker is upstream of the LLM: they may be the source of training data, retrieved documents, tool outputs, or user-pasted code that the LLM is summarizing/translating/extending.

The attacker's goal is to convince the LLM to emit Gradient code that:

- Uses an effect the user did not authorize (e.g. `!{IO}` smuggled into a "pure" function).
- Calls into an `extern fn` the user did not authorize (e.g. arbitrary FFI).
- Skips contract enforcement (e.g. `@runtime_only(off_in_release)` on a security-critical assertion).
- Exfiltrates data via a side channel (e.g. logging, network call, deliberately-leaky panic message).

The attacker does **not** control the Gradient compiler. So the compiler's effect/capability/contract checks are the last line of defense. **The guidelines below ensure the LLM's output reaches the compiler in a form the compiler can actually check.**

## G1. Use grammar-constrained decoding

Gradient ships a JSON-schema-style grammar for structured emission (see [`docs/agent-integration.md`](../agent-integration.md) § "Structured Output Formats"). Agents that emit Gradient code via free-form generation are vulnerable to:

- malformed syntax that masks a side-channel (e.g. emitting a comment that the parser accepts but the LLM smuggles a payload into);
- out-of-grammar effect tokens that the parser rejects (forcing the agent to retry, potentially leaking the prompt back).

**Guideline G1.** Emit Gradient code through grammar-constrained decoding whenever the agent's frontend supports it. Free-form generation is acceptable only when followed by a strict parse-and-reject step.

## G2. Declare effects explicitly; never omit and rely on inference

The launch-tier compiler enforces explicit effect rows on every signature. Bidirectional effect inference ([#350](https://github.com/Ontic-Systems/Gradient/issues/350)) is planned but unshipped; once it lands, an agent that elides effect rows will let the compiler infer them, *and an attacker can choose effects the user did not intend*.

**Guideline G2.** Emit explicit effect rows on every `fn` signature. For pure functions, emit `!{}` (the empty row), not nothing. Treat an effect row as a security-relevant declaration; never let the LLM leave it implicit.

## G3. Use the smallest effect row that compiles

The temptation is to emit a permissive row (e.g. `!{IO, FS, Net, Mut, Time}`) "just in case." That widens the attack surface — every function with `!{IO}` is a potential exfil channel.

**Guideline G3.** Start with `!{}`. Run the compiler. Add exactly the effects the compiler reports as missing. Never widen further. If the user asks for a "function that reads a file," add `!{FS}` and `!{Throws(IoError)}`, not a blanket `!{IO}`.

This is the agent-codegen analog of the principle of least privilege.

## G4. Capability tokens are not optional decorations

When a function uses an `Unsafe` capability (e.g. an `extern fn` declaration; see [#322](https://github.com/Ontic-Systems/Gradient/issues/322) and ADR 0002), the capability token must be threaded through the call site explicitly. An agent that elides the capability token is asking the compiler to fail; an agent that smuggles the token from a hostile source is exactly the prompt-injection vector this section names.

**Guideline G4.** When a generated function requires a capability, it must:

1. Take the capability token as an explicit parameter.
2. Use it exactly once (or with the capability's documented multiplicity).
3. Never pass it through a generic combinator that an attacker controls.

If the LLM is unable to thread the capability through, fail loud — emit a diagnostic comment in the source, not a `// TODO` that an attacker can exploit later.

## G5. Contracts encode intent — never skip them under release pressure

`@requires` / `@ensures` contracts are part of the function's specification, not its implementation. An agent that generates code matching a spec must generate the matching contract.

**Guideline G5.** When a function has a documented precondition or postcondition, emit the corresponding `@requires` / `@ensures`. Do not use `@runtime_only(off_in_release)` on contracts the user did not explicitly mark as performance-critical. Use `@verified` when the contract is provable inside the launch-tier predicate fragment ([`reproducible-builds.md`'s sister doc `effect-soundness.md` § 4.3](effect-soundness.md) and the stdlib pilot are the reference shapes).

## G6. Treat `comptime` like `eval` — only on trusted input

Gradient's `comptime` evaluator runs Gradient code at compile time. Until [#356](https://github.com/Ontic-Systems/Gradient/issues/356) (comptime sandbox) lands, an agent that generates a `comptime` block from untrusted input is the same vulnerability class as JavaScript `eval(prompt())`.

**Guideline G6.** Do not emit `comptime` blocks from agent-generated code unless the agent has verified its input came from a trusted source (e.g. a developer prompt, not a retrieved document). When in doubt, emit a runtime computation instead. Treat `comptime` the way you would treat `unsafe` in a memory-safe language.

## G7. Pin the threat tier of the generated code

When emitting code, the agent should declare which threat tier the code targets:

| Tier | Marker | Properties |
|---|---|---|
| trusted | `@trusted` (planned, [#360](https://github.com/Ontic-Systems/Gradient/issues/360)) | Authored by a human developer or pinned-source agent. Comptime allowed; LSP runs full features. |
| untrusted | `@untrusted` (planned, [#360](https://github.com/Ontic-Systems/Gradient/issues/360)) | From an unverified source (e.g. user input, retrieved docs). Comptime banned at compile time; LSP runs in restricted mode. |

**Guideline G7.** Tag generated code with the appropriate marker. Conservative default: `@untrusted` unless the agent has cryptographic evidence the source is trusted.

Until [#360] ships, treat *all* agent-generated code as if it carried `@untrusted` for the purpose of choosing what features to use. This means: no `comptime`, narrow effect rows, narrow capability use, contract enforcement on.

## G8. Surface refusals; never silently broaden

When the compiler rejects code generated under a narrow effect row or capability, the agent has two options:

1. **Surface the refusal to the user** — "I tried to write this with `!{}` but it needs `!{FS}`. Authorize?" This is the secure path.
2. **Silently broaden the effect row** to make the code compile. **This is the prompt-injection vector** — an attacker engineers a prompt that forces the agent to widen the effect row beyond what the user asked for.

**Guideline G8.** Tooling that wraps Gradient codegen MUST distinguish "compiler accepted" from "compiler refused, agent retried with wider permissions." The retry path requires explicit user confirmation; the accept path does not.

## G9. Emit checkable diagnostics, not silent fallbacks

If the agent cannot satisfy a constraint (e.g. cannot find a way to express the algorithm without `!{Heap}`), the output should be a diagnostic, not silent code that compiles via fallback.

**Guideline G9.** When the agent fails, fail with a Gradient-syntax comment block:

```gradient
// AGENT_REFUSAL: cannot implement `parse_packet` within `!{}` — needs `!{Heap}` for the parse table.
// User must explicitly authorize broader effects.
fn parse_packet(input: Bytes) -> Result[Packet, ParseError]:
    panic("agent refused; see comment above")
```

This makes refusals visible in the source rather than buried in chat history.

## G10. Capability whitelist for agent-driven workflows

When an agent is wrapped in a tool (e.g. an editor plugin, a CI bot, a code-review assistant), the wrapper should declare a **capability whitelist**:

- Which effects may the agent emit unprompted? (e.g. an editor plugin emitting an autocomplete might allow `!{}` only.)
- Which capabilities may it thread through call sites?
- Which contract markers may it use?

**Guideline G10.** Wrappers MUST publish their capability whitelist as part of the tool's documentation, and the tool's prompts MUST reference the whitelist so a prompt-injection attempt to widen permissions is visible to the user when the prompt is logged.

## Enforcement

These guidelines are intentionally guidance-level — Gradient cannot enforce them at the compiler layer because they describe agent behavior outside the compiler. Two things are enforceable today:

- **The compiler's effect/capability/contract checks.** Agents that violate G2–G6 produce code the compiler rejects. The agent then has to either retry (the secure path) or smuggle the violation past the compiler (which is the attack — and is what G8/G9/G10 are designed to surface).
- **Grammar-constrained decoding (G1).** The agent's frontend can pin this mechanically.

Further enforcement requires the planned features called out per-guideline (#322 Unsafe gate, #350 effect inference, #356 comptime sandbox, #360 `@trusted`/`@untrusted`).

## Summary table

| # | Guideline | Enforcement today | Enforcement after planned features |
|---|---|---|---|
| G1 | Grammar-constrained decoding | Frontend choice | Frontend choice (unchanged) |
| G2 | Explicit effect rows | Today: required (no inference yet) | Post-#350: still required even after inference lands |
| G3 | Smallest effect row | Compiler checks subset | Same |
| G4 | Capability tokens explicit | Manual today | Post-#322: typestate-checked |
| G5 | Contracts encode intent | Runtime (manual to enforce shape) | Post-`@verified` expansion: SMT-discharged |
| G6 | `comptime` only on trusted | Manual today | Post-#356: compile-time `!{IO}` ban |
| G7 | Pin threat tier | Documentation only | Post-#360: source-tier marker enforced |
| G8 | Surface refusals | Tool wrapper | Tool wrapper |
| G9 | Diagnostic-not-fallback | Tool wrapper | Tool wrapper |
| G10 | Capability whitelist | Tool wrapper | Tool wrapper |

## Cross-references

- [`docs/agent-integration.md`](../agent-integration.md) — primary agent integration reference.
- [`docs/security/threat-model.md`](threat-model.md) — surface row S1 (agent-emitted code).
- [`docs/security/effect-soundness.md`](effect-soundness.md) — soundness sketch for the effect system that backs G2/G3.
- [`docs/security/reproducible-builds.md`](reproducible-builds.md) — sibling security doc.
- [Epic #302](https://github.com/Ontic-Systems/Gradient/issues/302) — threat model umbrella.
- ADR 0001 (effect tier), ADR 0002 (capabilities), ADR 0003 (tiered contracts) — locked design anchors.
