# Agent Integration Guide

> **STATUS:** partial — Grammar-constrained decoding, runtime contracts, effects, and the Query API are implemented. Static `@verified` SMT discharge ships as **opt-in via `GRADIENT_VC_VERIFY=1`** with one-hundred-thirty-six stdlib functions across fourteen modules (`core_math.gr`, `core_bool.gr`, `core_compare.gr`, `core_int_ops.gr`, `core_arith_ops.gr`, `core_order_ops.gr`, `core_pair_ops.gr`, `core_select_ops.gr`, `core_chain_ops.gr`, `core_witness_ops.gr`, `core_parity_ops.gr`, `core_interval_ops.gr`, `core_neg_ops.gr`, `core_inequality_chain_ops.gr`) discharged on every CI run (see [ADR 0003 step 5 / sub-issue #331](https://github.com/Ontic-Systems/Gradient/issues/331)); broader stdlib coverage, `@untrusted` source mode, and capability-scoped manifests remain planned (Epics #297, #302, #303).

This document is the primary reference for AI agents integrating Gradient into their tool chains. It covers structured output formats, typed holes, the LSP server with agent-specific extensions, and recommended integration patterns.

For Gradient's design principles and research foundation, see [README.md](../README.md). This guide focuses on practical integration details.

## Why Gradient for Agents

Gradient is purpose-built for agent code generation:

- **Grammar-constrained decoding ready** — LL(1) grammar compatible with XGrammar, llguidance, Outlines for token-level syntax enforcement
- **Compiler-enforced effects** — `!{IO}` annotations are verified, not just convention; pure functions are compiler-proven
- **Structured compiler API** — JSON-serializable diagnostics via `session.check()`, `session.symbols()`, `session.module_contract()`
- **Typed holes** — `?hole` placeholders provide type-directed completion context
- **Contracts** — `@requires`/`@ensures` annotations enable generate-verify workflows with runtime checking

See [README.md](../README.md) for the full research foundation and design principles.

## The Generate-Verify Workflow

Agent-generated code is compiler-checked and runtime-enforced today, with a roadmap toward static verification. Gradient delivers this workflow today:

1. **Agent generates Gradient code with grammar-constrained decoding.** The formal EBNF grammar (`resources/gradient.ebnf`) is compatible with XGrammar, llguidance, and Outlines. Constrained decoding engines enforce it token-by-token. Result: zero syntax errors (SynCode, 2024).
2. **Compiler provides type-directed completion context.** Typed holes and structured diagnostics give the agent precise type information at every decision point. Result: 75% fewer type errors (Blinn et al., OOPSLA 2024).
3. **Functions have `@requires`/`@ensures` contracts.** Agents generate not just implementations but specifications. The contracts are machine-checkable declarations of intent. The `result` keyword in postconditions references the return value.
4. **Compiler enforces contracts at runtime.** The compiler inserts assertion checks on function entry (preconditions) and exit (postconditions). Contract violations produce structured error messages.
   - **STATUS: implemented** — runtime contract enforcement.
   - **STATUS: opt-in (launch tier)** — static SMT-discharged contract verification via the `@verified` annotation. Set `GRADIENT_VC_VERIFY=1` to route every `@verified fn` through the Z3 subprocess discharger; counterexamples surface as structured diagnostics with Gradient-syntax bindings (see [ADR 0003: Tiered contract enforcement](adr/0003-tiered-contracts.md), [#329](https://github.com/Ontic-Systems/Gradient/issues/329)). One-hundred-thirty-six stdlib functions across fourteen modules ([`compiler/stdlib/core_math.gr`](../codebase/compiler/stdlib/core_math.gr) — 10 fns / 17 obligations covering Int arithmetic, [`compiler/stdlib/core_bool.gr`](../codebase/compiler/stdlib/core_bool.gr) — 6 fns / 6 obligations covering Bool algebra, [`compiler/stdlib/core_compare.gr`](../codebase/compiler/stdlib/core_compare.gr) — 10 fns / 11 obligations covering comparison reflection and inclusive-range membership, [`compiler/stdlib/core_int_ops.gr`](../codebase/compiler/stdlib/core_int_ops.gr) — 10 fns / 11 obligations covering Int-arithmetic identities, [`compiler/stdlib/core_arith_ops.gr`](../codebase/compiler/stdlib/core_arith_ops.gr) — 10 fns / 10 obligations covering Int-arithmetic axioms (additive identities/inverses/commutativity/associativity), [`compiler/stdlib/core_order_ops.gr`](../codebase/compiler/stdlib/core_order_ops.gr) — 10 fns / 14 obligations covering Int-ordering branch reflection, one-sided clamps, inclusive-range predicates, and total-order negation identities, [`compiler/stdlib/core_pair_ops.gr`](../codebase/compiler/stdlib/core_pair_ops.gr) — 10 fns / 17 obligations covering pair/triple-symmetric ops including 3-way min, sorted-mid extraction, abs-diff, pair/triple equality, 4-way max, additive commutativity witness, and ordered-pair difference, [`compiler/stdlib/core_select_ops.gr`](../codebase/compiler/stdlib/core_select_ops.gr) — 10 fns / 11 obligations covering conditional-selection identities including ternary-style branch reflection, branch-arm equivalences, idempotent selection, `bool→Int` mapping, and disjunctive postcondition shapes, [`compiler/stdlib/core_chain_ops.gr`](../codebase/compiler/stdlib/core_chain_ops.gr) — 10 fns / 10 obligations covering transitive equality / strict-order / non-strict-order chains, multi-step additive identity chains, constant-step increment/decrement chains, and conjunctive sign predicates by transitivity, [`compiler/stdlib/core_witness_ops.gr`](../codebase/compiler/stdlib/core_witness_ops.gr) — 10 fns / 15 obligations covering trivial existential-style witnesses including successor/predecessor strict-order, between-witness for strict-order intervals, doubled-non-negative, even-step, triple-additive, sorted-mid, and conjunctive lower-bound on positive sums, and [`compiler/stdlib/core_parity_ops.gr`](../codebase/compiler/stdlib/core_parity_ops.gr) — 10 fns / 17 obligations covering parity / multiple / step witnesses including doubling/tripling/quadrupling via repeated addition, additive step-by-2 / step-by-4 chains, even-step monotonicity, non-negative double, double-difference non-negativity, and positive-tripling) ship under `@verified` and are discharged on every green CI run as a continuous honesty check ([#331](https://github.com/Ontic-Systems/Gradient/issues/331)). Research target: 82–96% first-pass success on Dafny-style specs; the Dafny figure is an aspirational benchmark for the verified tier, not a current Gradient measurement.
5. **Effect system guarantees no undeclared side effects.** A function without `!{IO}` in its signature is compiler-proven pure. No runtime surprises.
6. **Result: agent-generated code that is compiler-checked, runtime-enforced, and on a path to compiler-verified.** The combination of grammar constraints, type checking, runtime contract enforcement, and effect typing means the compiler can vouch for well-formedness and runtime correctness today; the `@verified` tier is roadmapped for static verification.

## Design-by-Contract for Agents

Design-by-contract is the single highest-leverage feature for agent code generation. Research shows LLMs achieve 82-96% first-pass success rates when generating code against formal specifications and discharging them with an SMT solver (e.g. Dafny). Gradient targets the same pattern via the planned `@verified` tier (Epic #297). The figures cited here are research benchmarks for that pattern, not measurements of Gradient's current runtime-enforcement implementation.

### Contract Annotations

Gradient supports two contract annotations on functions:

- **`@requires(condition)`** -- precondition checked on function entry
- **`@ensures(condition)`** -- postcondition checked on function exit

The `result` keyword in `@ensures` refers to the function's return value.

```
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)
```

Multiple contracts can be stacked:

```
@requires(a > 0)
@requires(b > 0)
@ensures(result > 0)
fn multiply_positive(a: Int, b: Int) -> Int:
    ret a * b
```

### Runtime Checking

Contracts are enforced at runtime via assertion checks:

- **Preconditions** are checked on function entry. If a `@requires` condition is false, the program halts with a structured contract violation error.
- **Postconditions** are checked on function exit. If an `@ensures` condition is false after the function body executes, the program halts with a structured error.

Contract violations produce structured error messages that agents can parse and act on.

### Static Verification: the `@verified` Tier (opt-in)

Annotate a function with `@verified` to statically discharge its `@requires`/`@ensures` predicates against Z3 instead of inserting runtime checks. The launch tier handles linear-integer arithmetic, booleans, equality, and straight-line / `if`-branch bodies; predicates outside that fragment surface as a structured diagnostic so the agent learns the verifier's bounds rather than silently dropping the contract.

```
@verified
@requires(true)
@ensures(result >= a)
@ensures(result >= b)
fn max_int(a: Int, b: Int) -> Int:
    if a >= b:
        a
    else:
        b
```

Discharge end-to-end:

```
$ GRADIENT_VC_VERIFY=1 gradient-compiler --check --json max.gr
{
  "ok": true,
  "diagnostics": [
    { "severity": "warning",
      "message": "@verified function `max_int`: all 2 contract obligation(s) discharged by Z3" }
  ]
}
```

A buggy version produces a counterexample diagnostic with the failing inputs in Gradient syntax — directly paste-able as a regression test:

```
@verified
@requires(true)
@ensures(result >= 0)
fn bad_clamp(n: Int) -> Int:
    if n >= 0:
        n
    else:
        0 - 1
# error: @verified function `bad_clamp` violates @ensures #0:
#        counterexample: n = -1, result = -1
```

Set `GRADIENT_Z3_BIN=/path/to/z3` to pick a specific binary; otherwise the discharger looks for `z3` on `PATH`. `GRADIENT_DUMP_VC=1` writes the SMT-LIB queries under `target/vc/` for inspection.

#### Stdlib pilot (sub-issue [#331](https://github.com/Ontic-Systems/Gradient/issues/331))

The stdlib `@verified` corpus now spans fourteen modules:

- [`compiler/stdlib/core_math.gr`](../codebase/compiler/stdlib/core_math.gr) — Int-arithmetic slice. Ten functions (`clamp_nonneg`, `max_int`, `abs_int_nonneg`, `add_one`, `min_int`, `add_two`, `max3_int`, `is_nonneg`, `eq_int`, `clamp_in_range`) covering linear-integer arithmetic, multi-arg / nested-`if` shapes, `Bool`-returning predicates, and non-trivial `@requires` clauses (17 obligations).
- [`compiler/stdlib/core_bool.gr`](../codebase/compiler/stdlib/core_bool.gr) — Bool-algebra slice. Six functions (`and_bool`, `or_bool`, `not_bool`, `eq_bool`, `xor_bool`, `implies_bool`) covering short-circuit conjunction/disjunction, negation, equality / inequality reflection, and material implication (6 obligations).
- [`compiler/stdlib/core_compare.gr`](../codebase/compiler/stdlib/core_compare.gr) — comparison-reflection slice. Ten functions (`is_zero`, `is_pos`, `is_neg`, `lt_int`, `leq_int`, `gt_int`, `geq_int`, `neq_int`, `sign_int`, `between_inclusive`) covering sign predicates, ordering reflection, three-way sign with bounded range, and inclusive-range membership with non-trivial `@requires` (11 obligations).
- [`compiler/stdlib/core_int_ops.gr`](../codebase/compiler/stdlib/core_int_ops.gr) — Int-arithmetic-identity slice. Ten functions (`double_int`, `triple_int`, `succ_int`, `pred_int`, `incr_by`, `decr_by`, `add_three`, `double_nonneg`, `add_nonneg_grows`, `neg_int`) covering doubling/tripling identities, successor/predecessor, generalized increment/decrement, three-argument sums, branching monotonicity over `@requires(n >= 0)`, and additive negation (11 obligations).
- [`compiler/stdlib/core_arith_ops.gr`](../codebase/compiler/stdlib/core_arith_ops.gr) — Int-arithmetic-axiom slice. Ten functions (`add_zero`, `zero_add`, `sub_zero`, `sub_self_zero`, `add_comm`, `add_assoc`, `sub_add_inverse`, `add_sub_inverse`, `double_plus_one`, `quad_int`) covering additive identities, additive inverses, commutativity, associativity reflection, and small linear-combination identities (10 obligations).
- [`compiler/stdlib/core_order_ops.gr`](../codebase/compiler/stdlib/core_order_ops.gr) — Int-ordering slice. Ten functions (`min_left_when_le`, `min_right_when_ge`, `max_left_when_ge`, `max_right_when_le`, `clamp_lower_bound`, `clamp_upper_bound`, `in_closed_range`, `not_less_is_ge`, `not_greater_is_le`, `le_or_gt_total`) covering branch reflection for min/max, one-sided clamps, inclusive-range predicates, and total-order negation identities (14 obligations).
- [`compiler/stdlib/core_pair_ops.gr`](../codebase/compiler/stdlib/core_pair_ops.gr) — pair/triple-symmetric slice. Ten functions (`min3_int`, `mid3_sorted`, `abs_diff`, `pair_eq`, `pair_neq`, `triple_all_eq`, `triple_any_neq`, `max4_int`, `add_pair_comm_witness`, `ordered_diff`) covering 3-way min, sorted-mid extraction, absolute-difference, pair/triple equality predicates, 4-way max, additive commutativity witness, and ordered-pair difference (17 obligations).
- [`compiler/stdlib/core_select_ops.gr`](../codebase/compiler/stdlib/core_select_ops.gr) — conditional-selection slice. Ten functions (`select_true_left`, `select_false_right`, `select_idempotent`, `select_eq_args`, `bool_to_int`, `select_neg_branch_returns_n`, `nonneg_or_zero`, `nonpos_or_zero`, `select_add_or_sub`, `step_either_way`) covering ternary-style selection identities, branch-arm equivalences, idempotent selection, `bool→Int` mapping, and disjunctive postcondition shapes (11 obligations).
- [`compiler/stdlib/core_chain_ops.gr`](../codebase/compiler/stdlib/core_chain_ops.gr) — transitive-chain slice. Ten functions (`chain_eq_three`, `chain_lt_three`, `chain_le_three`, `add_zero_chain`, `sub_zero_chain`, `triple_inc`, `triple_dec`, `chain_pos_sum`, `chain_nonneg_sum`, `chain_eq_sum_three`) covering chained equality / strict-order / non-strict-order transitivity, multi-step additive identity chains (`n + 0 + 0 + 0`, `n - 0 - 0 - 0`), constant-step increment/decrement chains (`n + 1 + 1 + 1`, `n - 1 - 1 - 1`), and conjunctive sign predicates whose conclusions follow by transitivity (10 obligations).
- [`compiler/stdlib/core_witness_ops.gr`](../codebase/compiler/stdlib/core_witness_ops.gr) — witness/existence slice. Ten functions (`add_one_witness`, `sub_one_witness`, `successor_increases`, `predecessor_decreases`, `between_witness`, `nonneg_double_witness`, `even_step_witness`, `triple_witness`, `mid_witness`, `pos_sum_witness`) covering trivial existential-style witnesses showing that for any input, an output exists satisfying a linear-integer postcondition — successor/predecessor strict-order witnesses, between-witness using `a + 1` for `a < b`, doubled-non-negative witness, even-step `n + 2` witness, triple-additive witness, sorted-mid witness using lower bound, and conjunctive lower-bound witness on positive sums (15 obligations).
- [`compiler/stdlib/core_parity_ops.gr`](../codebase/compiler/stdlib/core_parity_ops.gr) — parity / multiple / step slice. Ten functions (`double_int`, `triple_int`, `quad_int`, `step_by_two`, `step_by_four`, `next_even_step`, `nonneg_double`, `double_diff_nonneg`, `triple_minus_one`, `pos_triple`) covering doubling/tripling/quadrupling expressed via repeated addition (no `*` operator) so the SMT lowering stays inside linear-integer arithmetic, additive step-by-2 / step-by-4 chains, even-step monotonicity (`n + n + 2 > n + n`), non-negative double under `@requires(n >= 0)`, double-difference non-negativity under `@requires(a >= b)`, the `n + n + n - n == n + n` cancellation identity, and positive-tripling under `@requires(n > 0)` (17 obligations).
- [`compiler/stdlib/core_interval_ops.gr`](../codebase/compiler/stdlib/core_interval_ops.gr) — interval / bound-preservation slice. Ten functions (`keep_lower_bound`, `keep_upper_bound`, `keep_interval`, `bump_above_lower`, `drop_below_upper`, `clamp_lower_one_sided`, `clamp_upper_one_sided`, `interval_width`, `add_nonneg_preserves_lower`, `reconstruct_upper_from_width`) covering lower/upper-bound preservation, inclusive intervals, one-sided clamps, span-width non-negativity, non-negative-addition lower-bound preservation, and width-based upper reconstruction inside linear-integer arithmetic (21 obligations).
- [`compiler/stdlib/core_neg_ops.gr`](../codebase/compiler/stdlib/core_neg_ops.gr) — additive-negation slice. Ten functions (`neg_via_zero_sub`, `neg_cancels_input`, `double_neg_returns_input`, `neg_of_nonneg_is_nonpos`, `neg_of_nonpos_is_nonneg`, `sub_as_add_neg`, `add_neg_self_is_zero`, `neg_swap_diff`, `neg_of_positive_is_negative`, `neg_sum_distributes`) covering negation expressed via `0 - n`, additive-inverse cancellation (`n + (0 - n) == 0`), double-negation (`0 - (0 - n) == n`), sign-flip predicates over `n >= 0` / `n <= 0` / `n > 0`, subtraction-as-add-negation (`a - b == a + (0 - b)`), difference-swap (`0 - (a - b) == b - a`), and negation-distribution over sums (`0 - (a + b) == 0 - a - b`) — all inside linear-integer arithmetic (10 obligations).
- [`compiler/stdlib/core_inequality_chain_ops.gr`](../codebase/compiler/stdlib/core_inequality_chain_ops.gr) — k-step inequality / strict-order chain slice. Ten functions (`lt_chain_four`, `le_chain_four`, `lt_chain_five`, `le_chain_five`, `mixed_chain_a`, `mixed_chain_b`, `gt_chain_four`, `ge_chain_four`, `pos_chain_sum_four`, `nonneg_chain_sum_five`) extending the 3-step transitivity from `core_chain_ops.gr` to 4- and 5-step chains, covering strict (`<`/`>`) and non-strict (`<=`/`>=`) ordering chains, mixed strict/non-strict chains where strict propagates, and conjunctive sign predicates over 4-/5-argument sums (10 obligations).

Every function exercises the parser → AST → checker → VC encoder → Z3 path on every CI run. The dedicated `verified` lane in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) installs Z3 and pins `GRADIENT_Z3_REQUIRED=1`, so a regression that breaks the discharge path turns the lane red rather than silently skipping. This anchors the "compiler-verified" claim to a continuously-checked artifact and closes adversarial finding F1's request that the marketing language match a real verified subset (see [ADR 0003 step 5](adr/0003-tiered-contracts.md)).

### Contracts in the Query API

Contracts are visible through the structured query API:

- **`session.symbols()`** -- each function's symbol entry includes its contracts
- **`session.module_contract()`** -- the module contract includes contracts for all public functions

This means agents can read a module's contracts without parsing source code -- they get structured JSON describing every function's preconditions and postconditions.

### Agent Workflow: Generate with Contracts

The recommended workflow for agents generating Gradient code with contracts:

1. **Specify contracts first.** Write the `@requires`/`@ensures` annotations before the function body. This declares intent.
2. **Generate the implementation.** Fill in the function body to satisfy the contracts.
3. **Type-check.** Run `gradient-compiler --check --json file.gr` to verify the code is well-typed with structured diagnostics.
4. **Run.** Execute the program. If a contract is violated at runtime, the structured error message tells the agent exactly which contract failed and why.
5. **Iterate.** Fix the implementation until all contracts pass.

This workflow maps toward the "vericoding" pattern from the research literature: generate code against a formal specification, verify it holds, trust the result. Today Gradient verifies contracts at runtime; static verification (Dafny/F* tier) is tracked under Epic #297.

### Contracts for Code Review

When an agent reviews Gradient code, contracts provide a machine-readable summary of expected behavior. Instead of reading and reasoning about an entire function body, the agent can:

1. Read the contracts via `session.module_contract()`.
2. Verify that the contracts capture the intended behavior.
3. Trust that the runtime will enforce compliance.

This reduces the review task from "understand what this code does" to "verify that the contracts match the specification."

---

## Type-Directed Completion Context

The compiler provides rich completion context at any cursor position, giving agents exactly the information needed to generate type-correct code. This is backed by research showing type-directed generation reduces compile errors by 75% (ETH Zurich, PLDI '25).

### API

```rust
let ctx = session.completion_context(line, col);
// Returns:
//   expected_type: the type expected at the cursor position
//   bindings: all bindings in scope with their types
//   matching_functions: functions whose return type matches the expected type
//   matching_variants: enum variants that match the expected type
//   matching_builtins: builtin functions that match the expected type
```

### CLI

```bash
gradient-compiler file.gr --complete 5 12 --json
```

Returns a JSON object with all completion candidates ranked by relevance.

### Enhanced Typed Hole Diagnostics

When the compiler encounters a typed hole (`?`), it now reports not just the expected type but also all in-scope bindings, functions, and enum variants that would satisfy the hole. This turns every `?` into a type-directed menu of valid completions.

### Agent Workflow: Type-Directed Generation

1. **Write a skeleton with typed holes.** Place `?` at every decision point.
2. **Query completion context.** Run `--complete line col --json` at each hole.
3. **Select from candidates.** The compiler provides ranked candidates -- bindings in scope, functions with matching return types, and enum variants.
4. **Fill and re-check.** Replace the hole with the selected candidate and re-run `gradient check`.

This workflow is strictly more powerful than untyped hole-filling because the agent receives a curated set of type-valid options rather than generating from scratch.

## Context Budget Tooling

Agents operate under context window budgets. Sending too much code degrades performance ("context rot"). Sending too little misses critical information. Context budget tooling solves this by letting agents request exactly the right amount of context for a given task.

### API

```rust
// Get relevance-ranked context for editing a function within a token budget
let ctx = session.context_budget("process_data", 1000);
// Returns: function signature, called functions' contracts, relevant type
// definitions, capability ceiling -- ranked by relevance, trimmed to budget

// Get a structural overview of the entire project
let index = session.project_index();
// Returns: all modules, all public functions with signatures, type definitions,
// capability ceilings -- a RepoMap-style index for navigation
```

### CLI

```bash
# Get optimal context for editing `main` within a 1000-token budget
gradient-compiler --context --budget 1000 --function main file.gr

# Get a structural index of the project
gradient-compiler --inspect --index file.gr
```

### What Context Includes

The context budget API returns items ranked by relevance to the target function:

1. **The function's own signature and contracts** (highest priority)
2. **Signatures of functions it calls** (direct dependencies)
3. **Type definitions used in the function** (parameter and return types)
4. **Module capability ceiling** (what effects are allowed)
5. **Contracts of called functions** (pre/postconditions)

Items are trimmed to fit the requested token budget. The most relevant items are always included first.

### Agent Workflow: Budget-Aware Editing

1. **Request context.** Call `session.context_budget("target_fn", budget)` with a token budget appropriate for your model's context window.
2. **Include in prompt.** Insert the returned context into the LLM prompt. It is already ranked and trimmed -- no further processing needed.
3. **Generate code.** The LLM generates with exactly the right amount of context.
4. **Verify.** Run `gradient check` to confirm correctness.

### Project Index for Navigation

`session.project_index()` (or `--inspect --index` via CLI) returns a structural overview of the entire codebase -- module names, public function signatures, type definitions, and capability ceilings. This is the Gradient equivalent of Aider's RepoMap: a compact, high-signal summary that helps agents navigate unfamiliar codebases without reading every source file.

## Structured Query API (Primary Agent Interface)

The compiler-as-library API is the most powerful way for agents to interact with Gradient. Instead of parsing CLI output, agents call structured Rust methods and receive JSON-serializable results.

- **`Session::from_source(code)`** -- one-call setup; creates a compiler session from a source string.
- **`Session::from_file(path)`** -- creates a compiler session from a file path; resolves `use` imports relative to the file's location for multi-file projects.
- **`session.check()`** -- returns `CheckResult` with JSON diagnostics (errors, warnings, per-phase counts).
- **`session.symbols()`** -- returns the symbol table with `is_pure`, `effects`, types, and signatures for every symbol.
- **`session.module_contract()`** -- returns a compact API summary including function signatures, effects, purity, capability ceiling, and call graph.
- **`session.effect_summary()`** -- returns per-function effect inference showing which effects each function body actually uses.
- **`session.rename(old, new)`** -- compiler-verified rename: updates all references, re-checks the program, and returns a `RenameResult` with locations and verification status.
- **`session.callees(fn_name)`** -- dependency query returning which functions a given function calls.
- **`session.completion_context(line, col)`** -- type-directed completion context at a cursor position: expected type, in-scope bindings, matching functions, enum variants, and builtins.
- **`session.context_budget(fn_name, budget)`** -- relevance-ranked context for editing a function, trimmed to the given token budget.
- **`session.project_index()`** -- structural overview of the project (modules, public signatures, types, capabilities).

All results are serde-serializable and can be converted to JSON with a single call.

## Compilation Workflow

The simplest integration workflow is:

```
1. gradient new my-project     # Create a project
2. Write src/main.gr           # Generate Gradient source
3. gradient check              # Type-check (fast feedback)
4. gradient build              # Compile to native binary
5. gradient run                # Execute
```

**Multi-file support:** The compiler resolves `use` declarations to source files
on disk (`use math` resolves to `math.gr`, `use a.b` resolves to `a/b.gr`).
Agents can split code across multiple `.gr` files and the compiler will resolve
and compile them together. For multi-file projects, use `Session::from_file(path)`
instead of `Session::from_source(code)` -- it resolves imports relative to the
file's location and compiles all referenced modules.

Or in a tight loop:

```
Agent -> write .gr files -> gradient check -> fix errors -> gradient run -> check output -> done
```

## Machine-Readable Output

The compiler supports structured JSON output via CLI flags:
- `gradient-compiler --check --json file.gr` -- structured diagnostics with per-phase error counts
- `--inspect --json` -- module contract (signatures, effects, purity, call graph)
- `--effects --json` -- per-function effect analysis
- `--complete <line> <col> --json` -- type-directed completion candidates at a cursor position (file must be the first positional arg)
- `--context --budget N --function name` -- relevance-ranked context within a token budget
- `--inspect --index` -- structural project index (modules, signatures, types)

Compiler errors are also printed to stderr in human-readable form. Each error includes:
- The compiler phase that produced it (parse error, type error, IR error)
- A description of the problem (expected type, found type, etc.)

The LSP server provides structured diagnostics via standard LSP protocol and the custom `gradient/batchDiagnostics` notification (see below).

## Typed Holes for Agent-Assisted Coding

Typed holes are Gradient's mechanism for incremental, type-directed code generation. An agent writes a skeleton with `?hole_name` placeholders, compiles, reads the compiler's response, and fills the holes.

### Example

Source file:

```
fn process(data: Int) -> Int:
    let filtered = ?filter_step
    let result = ?aggregate_step
    ret result
```

Running `gradient check` on this file will report the expected types for each hole, helping the agent determine what expressions to fill in.

### How Agents Should Use Holes

1. Generate a function skeleton with holes at every decision point.
2. Run `gradient check` and read the error output.
3. Use the expected type information to constrain generation.
4. Fill holes one at a time, re-checking after each fill to propagate type information.
5. When `gradient check` returns zero errors, the function is complete.

This workflow is more reliable than generating an entire function body in one pass because each step is validated by the type checker.

## Language Server Protocol (LSP)

Gradient ships a working LSP server (`codebase/devtools/lsp/`). It communicates over stdio via JSON-RPC.

### Implemented Features

- **Diagnostics** -- errors from the lexer, parser, and type checker are published on file open, change, and save.
- **Hover** -- type and signature information for builtins, keywords, and user-defined functions.
- **Completion** -- context-aware suggestions: keywords and builtin function names with signatures.
- **`gradient/batchDiagnostics`** -- custom notification (see below).

### Agent-Specific: Batch Diagnostics

The custom `gradient/batchDiagnostics` notification sends all diagnostics for a file in a single message. The payload includes:

```json
{
  "uri": "file:///path/to/main.gr",
  "diagnostics": [ ... ],
  "lex_errors": 0,
  "parse_errors": 0,
  "type_errors": 1
}
```

This is preferred for agents that process files atomically -- no streaming, no incremental updates. The per-phase error counts let agents decide how to respond (e.g., if `lex_errors > 0`, the source is fundamentally malformed; if only `type_errors > 0`, the syntax is valid but types are wrong).

### Planned Extensions

| Method | Description | Status |
|---|---|---|
| `gradient/holeFill` | Request fill suggestions for a typed hole. | Planned |
| `gradient/effectQuery` | Query what effects a function or module requires. | **Implemented** via `session.effect_summary()` and `--effects --json` |
| `gradient/capabilityCheck` | Verify whether a code block stays within a capability budget. | **Implemented** via `@cap` annotations and `session.module_contract()` |
| `gradient/astDump` | Return the full AST as structured JSON. | Planned |
| `gradient/irDump` | Return the IR for a specific function as structured JSON. | Planned |

## Canonical Formatter

The `--fmt` flag on `gradient-compiler` normalizes Gradient source into canonical form. This is useful for agents that generate code and want to ensure consistent formatting before committing, diffing, or feeding code back into a context window.

```bash
# Print formatted output to stdout (no file modification)
gradient-compiler --fmt file.gr

# Format and overwrite the file in place
gradient-compiler --fmt --write file.gr
```

**Agent use cases:**

- **Normalize before diffing.** Format both files before comparing to eliminate spurious whitespace differences.
- **Post-generation cleanup.** After generating `.gr` code, pipe it through `--fmt` to guarantee canonical output without manually tracking indentation rules.
- **CI checks.** Run `--fmt` and compare against the original to enforce consistent style across a codebase.

The formatter produces one canonical form per construct. There are no configuration options or style knobs -- the output is deterministic.

## Interactive REPL

The `--repl` flag on `gradient-compiler` starts an interactive evaluation session backed by Cranelift. Agents can use the REPL to quickly evaluate expressions and inspect inferred types without creating a full project.

```bash
# Interactive mode (for human or agent terminal sessions)
gradient-compiler --repl

# Non-interactive mode: pipe expressions via stdin
echo "1 + 2" | gradient-compiler --repl
```

**Non-interactive mode for agent piping.** When stdin is not a TTY (i.e., input is piped), the REPL reads from stdin, evaluates each line, and prints results to stdout. This allows agents to programmatically query the type checker and evaluator without maintaining an interactive session.

**Agent use cases:**

- **Type inference queries.** Pipe an expression to `--repl` to get its inferred type without writing a full program.
- **Quick evaluation.** Verify the result of an arithmetic or string expression before embedding it in generated code.
- **Exploratory prototyping.** Test small code fragments in isolation before assembling them into a module.

## CLI for Agents

The compiler supports JSON output flags for all major operations, making it easy for agents to parse results without scraping human-readable text:

```
gradient-compiler file.gr --check --json                        # structured diagnostics
gradient-compiler file.gr --inspect --json                      # module contract
gradient-compiler file.gr --effects --json                      # effect analysis
gradient-compiler file.gr --complete 5 12 --json                # type-directed completion at line 5, col 12
gradient-compiler file.gr --context --budget 1000 --function main  # context budget for editing main
gradient-compiler file.gr --inspect --index                     # structural project index
gradient-compiler file.gr --fmt --experimental                  # canonical formatting to stdout
gradient-compiler file.gr --fmt --write --experimental          # canonical formatting in place
echo "expr" | gradient-compiler --repl --experimental           # evaluate expression, get type + value
```

All JSON output is serde-serialized and follows stable schemas. Agents should prefer these flags over parsing stderr text.

## Integration Patterns

### Pattern 1: Write, Check, Fix Loop

The simplest integration. The agent writes `.gr` files, runs the compiler, reads the diagnostics, fixes errors, and repeats.

```
Agent -> write .gr file -> gradient check -> read errors -> fix errors -> repeat
```

This pattern requires no LSP server and works with any agent framework that can invoke shell commands and read stderr.

### Pattern 2: Typed Holes for Scaffolding

The agent generates a skeleton with `?hole` placeholders, then uses the compiler's feedback to complete the implementation incrementally.

```
Agent -> write skeleton with ?holes -> gradient check -> read hole errors -> fill holes -> gradient check -> done
```

This pattern is best when the agent knows the high-level structure but is uncertain about specific expressions.

### Pattern 3: LSP for Interactive Development

The agent connects to the LSP server over stdio and uses it for real-time feedback during an extended coding session.

```
Agent <-> LSP server (JSON-RPC/stdio) -> real-time diagnostics, completions, hover
```

This pattern is best for agents that maintain a long-running session and make many small edits. The LSP server re-runs the compiler pipeline on every change.

### Pattern 4: Build and Execute

The agent compiles the program and inspects the output to verify correctness.

```
Agent -> gradient build -> gradient run -> capture stdout -> verify output -> done
```

This pattern is useful for agents that can validate program behavior by checking the output against expected results.

### Pattern 5: Structured API Integration

For Rust-based agents, the compiler-as-library API provides the richest integration. The agent creates a `Session` directly and calls structured query methods:

```rust
use gradient_compiler::Session;

let session = Session::from_source(r#"
    mod example
    fn add(a: Int, b: Int) -> Int:
        ret a + b
    fn main() -> !{IO} ():
        print_int(add(3, 4))
"#);

// Type-check and get structured diagnostics
let result = session.check();
assert!(result.errors.is_empty());

// Get the module contract (signatures, effects, purity, call graph)
let contract = session.module_contract();

// Query per-function effects
let effects = session.effect_summary();

// Query call graph
let callees = session.callees("main");

// Compiler-verified rename
let rename_result = session.rename("add", "sum");
```

This pattern gives agents full access to the compiler's analysis without any serialization overhead, and is the recommended approach for agents built in Rust.

### Pattern 6: Generate-Verify Loop

The full generate-verify workflow combines grammar-constrained decoding, type checking, and contract verification into a single pipeline. Each stage eliminates a class of errors, so the agent never wastes tokens on code that fails a later stage.

```
Agent -> generate with grammar constraint -> type-check -> run with contracts -> trust result
```

Step by step:

1. **Generate.** The agent generates Gradient source using a grammar-constrained decoding engine (XGrammar, vLLM, Outlines) with the formal EBNF grammar (`resources/gradient.ebnf`). The LL(1) grammar guarantees the output is syntactically valid. Zero parse errors.
2. **Type-check.** The agent runs `gradient-compiler --check --json file.gr` and reads structured diagnostics. Typed holes provide completion context for any remaining gaps. The agent fixes type errors using the compiler's feedback.
3. **Verify contracts.** The agent writes `@requires`/`@ensures` annotations on functions and runs the program. Runtime contract checking asserts preconditions on entry and postconditions on exit. If a contract is violated, a structured error message identifies exactly which contract failed.
4. **Trust result.** If all three stages pass, the code is compiler-verified: syntactically valid, well-typed, contract-compliant, and effect-safe. The agent (or a downstream system) can trust the result without additional testing for the properties covered by the contracts.

All stages of this pattern are working today. Grammar-constrained decoding eliminates syntax errors. Type checking catches type mismatches. Runtime contract checking enforces `@requires`/`@ensures` annotations. The effect system guarantees no undeclared side effects.

## Built-in Functions Available to Agents

All builtins are available without imports:

| Function | Signature |
|---|---|
| `print` | `print(value: String) -> !{IO} ()` |
| `println` | `println(value: String) -> !{IO} ()` |
| `print_int` | `print_int(value: Int) -> !{IO} ()` |
| `print_float` | `print_float(value: Float) -> !{IO} ()` |
| `print_bool` | `print_bool(value: Bool) -> !{IO} ()` |
| `abs` | `abs(n: Int) -> Int` |
| `min` | `min(a: Int, b: Int) -> Int` |
| `max` | `max(a: Int, b: Int) -> Int` |
| `mod_int` | `mod_int(a: Int, b: Int) -> Int` |
| `to_string` | `to_string(value: Int) -> String` |
| `int_to_string` | `int_to_string(value: Int) -> String` |
| `range` | `range(n: Int) -> Iterable` |

String concatenation uses the `+` operator. Integer modulo uses the `%` operator.

## Getting Started as an Agent

1. Read `docs/language-guide.md` for syntax and semantics.
2. Read `resources/grammar.peg` for the formal grammar definition.
3. Study the test programs in `codebase/compiler/tests/` for working examples.
4. Use `gradient check` to validate generated code.
5. Use typed holes (`?name`) when unsure about specific expressions -- let the type checker guide generation.
6. Remember: every block-opening line (`fn`, `if`, `else`, `for`, `while`, `match`) must end with `:`.
