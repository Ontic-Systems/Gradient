# Self-Hosting Roadmap

Gradient is moving compiler ownership from the Rust host compiler into `compiler/*.gr` while keeping a small, explicit Rust kernel for runtime/platform primitives and backend machinery.

Status: active bootstrap-stage work, not fully self-hosted.

## Target State

Rust should keep:

- platform and runtime primitives
- file/process integration
- low-level bootstrap storage and FFI-shaped host services
- backend engines where host integration is still required
- the trusted fallback/compiler kernel until self-hosted parity is proven

Gradient should increasingly own:

- lexer and parser logic
- semantic analysis
- IR construction
- codegen orchestration and emission policy
- compiler driver flow
- query/LSP services useful to agents

## Current State

Current active self-hosted source tree: `compiler/*.gr`.

| Area | Current status |
|---|---|
| Self-hosted source inventory | 14 modules, over 7,000 lines under `compiler/*.gr` |
| Bootstrap collections | runtime-backed typed handles landed in #236 |
| Lexer | `lexer.gr::tokenize` accumulates real runtime-backed `TokenList` values via #237 |
| Parser token access | `parser.gr::current_token` and `peek_token` read runtime-backed token kind/span data via #238 |
| Parser AST storage | parser node/list storage and normalized export walk runtime-backed AST stores via #239 |
| Checker env/dispatch | `checker.gr` uses runtime-backed env/fn/var storage and AST dispatch via #240 |
| Parser differential gate | bridged Rust-vs-parser-shaped comparison exists for the current bootstrap corpus |
| Lexer parity gate | bridge-mirrored token shape parity exists for the current single-line bootstrap corpus |
| Checker parity gate | not implemented; tracked by #226 |
| IR/codegen/pipeline | structural/bootstrap-stage; tracked by #227-#230 |
| Driver/query/LSP | structural/bootstrap-stage; tracked by #231-#233 |
| Trust/kernel boundary | not measured/enforced yet; tracked by #234/#235 |

## What Is Proven

The repository currently proves:

- the self-hosted compiler module set exists and typechecks as source
- bootstrap collection handles are real runtime-backed values, not dummy fields
- self-hosted lexer code can append real tokens into a host-backed `TokenList`
- self-hosted parser token access can read token kind/span values from that list
- parser AST nodes/lists can round-trip through host-backed stores
- checker env storage supports let-bound locals, function params, shadowing, parent chains, function signatures, safe defaults, and primitive type round-trips
- source-text gates reject regression toward legacy dummy/placeholder collection shapes

## Known Gaps

These are expected blockers, not regressions:

1. Token payloads are incomplete.
   - Ident names, literal values, string payloads, and error messages are not fully recoverable through the bootstrap token FFI.
   - This blocks stronger direct parser execution claims.

2. Lexer parity is narrow.
   - Current parity is single-line bootstrap token-shape coverage.
   - `lexer.gr::next_token` still needs newline handling plus `INDENT`/`DEDENT` coverage.
   - Numeric literal parity still ignores literal payload values.

3. Parser direct execution is not complete.
   - Current differential coverage is bridge-shaped.
   - #223 tracks invoking `parser.gr` directly through the Gradient runtime/comptime path.

4. Parser corpus is too small.
   - #224 tracks expanding coverage across syntax used by `compiler/*.gr`.

5. Checker needs differential parity.
   - #240 provides runtime-backed env/AST dispatch substrate.
   - #226 should now focus on normalized diagnostics/type-result parity against the Rust checker.

6. IR/codegen/pipeline are not executable compiler phases yet.
   - #227/#228 track AST-to-IR lowering and IR parity.
   - #229/#230 track executable codegen and real phase pipeline execution.

7. Driver/query/LSP are not production self-hosted services yet.
   - #231 tracks `main.gr` driver usability.
   - #232/#233 track query/session and LSP backing data.

8. Trust and kernel boundary are not quantified.
   - #234 tracks end-to-end bootstrap trust checks.
   - #235 tracks measuring and shrinking the Rust kernel boundary.

## Active Issue Map

| Issue | Role |
|---|---|
| #116 | full self-hosting umbrella |
| #223 | invoke `parser.gr` directly in the differential gate |
| #224 | expand parser parity corpus beyond bootstrap basics |
| #226 | add checker differential parity gate |
| #227 | make `ir_builder.gr` lower real AST to IR |
| #228 | add IR differential/golden parity tests |
| #229 | implement executable codegen/emission slice |
| #230 | make `compiler.gr` pipeline execute real phases |
| #231 | make `main.gr` a usable bootstrap compiler driver |
| #232 | back `query.gr` with real sessions and diagnostics |
| #233 | back `lsp.gr` with query/session data |
| #234 | add end-to-end bootstrap trust checks |
| #235 | define and shrink the Rust kernel boundary |

## Execution Order

Recommended near-term order:

1. #223 parser direct-execution prerequisites and direct invocation.
2. #224 parser corpus expansion.
3. #226 checker differential parity gate.
4. #227/#228 IR execution and parity.
5. #229/#230 codegen and full compiler pipeline execution.
6. #231/#232/#233 driver, query, and LSP backing.
7. #234/#235 trust checks and Rust-kernel boundary metrics.

## Execution Principles

1. Keep the bootstrap parser intentionally narrow.
2. Prefer differential/golden tests before broadening syntax.
3. Keep temporary host-backed stores behind explicit boundaries.
4. Do not claim direct execution until the harness proves self-hosted code ran.
5. Keep the Rust compiler usable while self-hosting evolves.
6. Keep public claims narrower than internal aspirations.

## Definition Of Done

Self-hosting is not complete until:

- `compiler/*.gr` performs real lexer/parser/checker/IR/codegen/pipeline work for a documented source subset
- parity gates compare self-hosted results against Rust host results for that subset
- bootstrap trust checks prove the path does not silently fall back to Rust implementations
- remaining Rust kernel responsibilities are listed, measured, and intentionally retained
- public docs, CI jobs, and issue tracker state all describe the same boundary
