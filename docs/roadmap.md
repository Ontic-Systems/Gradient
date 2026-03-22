# Gradient Development Roadmap

## Vision

Gradient is the world's first programming language designed for autonomous AI agents. Every decision -- from syntax to error messages to the build system -- is optimized for LLM consumption and agentic workflows.

---

## Phase 0 -- Foundation (COMPLETE)

- **Formal PEG grammar** (`resources/grammar.peg`) -- specification covering modules, functions, let bindings, control flow, types, and expressions
- **Language reference** (`resources/language-reference.md`) -- complete v0.1 language documentation
- **`gradient` CLI scaffold** (`codebase/build-system/`) -- unified toolchain entry point with subcommands (build, run, test, check, fmt, new, init, repl) built with clap
- **Cranelift codegen PoC** (`codebase/compiler/`) -- proof that IR to Cranelift to native binary works; produces a working "Hello from Gradient!" binary
- **IR type system** (`codebase/compiler/src/ir/`) -- SSA-form IR with instruction variants, type definitions, value references
- **Test framework** (`codebase/test-framework/`) -- golden test runner, test harness, multi-tier test strategy
- **Example programs** (`resources/v0.1-examples/`) -- hello.gr and factorial.gr

## Phase 1 -- Compiler Frontend (COMPLETE)

- **Hand-written lexer** (`codebase/compiler/src/lexer/`) with INDENT/DEDENT token injection, position tracking, and error tokens for recovery -- **61 tests**
- **Recursive descent parser** (`codebase/compiler/src/parser/`) implementing the PEG grammar with error recovery; produces a typed AST where every node carries source location spans -- **46 tests**
- **Typed AST** (`codebase/compiler/src/ast/`) with modules for block, expression, item, module, span, statement, and type nodes
- Machine-readable parse errors with expected/found information
- Support for: module declarations, use imports (including selective `use mod.{A, B}`), function definitions, let bindings, if/else if/else expressions, for loops, annotations, typed holes, field access, and all arithmetic/comparison/logical operators

## Phase 2 -- Type System (COMPLETE)

- **Static type checker** (`codebase/compiler/src/typechecker/`) with type inference for let bindings -- **52 tests**
- Five built-in types: `Int`, `Float`, `String`, `Bool`, `()`
- Function signature checking with parameter type validation and return type checking
- Effect system: `!{IO}` annotations verified at call sites; calling an effectful function from a pure function is a type error
- Scope management with lexical scoping (push/pop scope stack)
- Forward references: all function signatures registered before bodies are checked
- Error recovery via `Ty::Error` sentinel that suppresses cascading diagnostics
- String concatenation via `+` operator type-checked as `String + String -> String`

## Phase 3 -- IR Generation (COMPLETE)

- **AST to SSA IR translation** (`codebase/compiler/src/ir/builder/`) -- **27 tests**
- IR instruction set: Const, Call, Ret, Add, Sub, Mul, Div, Mod, Cmp, Branch, Jump, Phi, Alloca, Load, Store, Neg, Not, StringConcat
- Scope resolution and name binding during IR lowering
- Recursive function support
- Conditional branching (if/else) lowered to IR blocks with phi nodes

## Phase 4 -- Full Pipeline (COMPLETE)

- **End-to-end compilation**: Source (.gr) -> Lexer -> Parser -> Type Checker -> IR Builder -> Cranelift Codegen -> Object File -> System Linker -> Native Binary
- `gradient build` produces a real binary from `.gr` source
- `gradient run` compiles and executes
- **HARD CHECKPOINT PASSED**: Real Gradient programs compiled from source to native binaries

## Phase 5 -- Working CLI and Recursion (COMPLETE)

- `gradient new <name>` creates real projects with `gradient.toml` and `src/main.gr`
- `gradient build` invokes the compiler, then links with `cc` to produce an executable in `target/debug/`
- `gradient run` builds and executes the binary, forwarding the exit code
- `gradient check` type-checks without emitting a final binary
- Project discovery: walks up the directory tree to find `gradient.toml`
- Compiler discovery: locates the compiler binary relative to the build system
- Recursive functions (factorial, fibonacci) compile and run correctly
- Full arithmetic: addition, subtraction, multiplication, division, modulo, unary negation

## Phase 6 -- Core Standard Library (COMPLETE)

- **I/O builtins**: `print(String)`, `println(String)`, `print_int(Int)`, `print_float(Float)`, `print_bool(Bool)`
- **Math builtins**: `abs(Int) -> Int`, `min(Int, Int) -> Int`, `max(Int, Int) -> Int`
- **Conversion builtins**: `to_string(Int) -> String`, `int_to_string(Int) -> String`
- **Other builtins**: `range(Int)`, `mod_int(Int, Int) -> Int`
- **String concatenation** via the `+` operator
- **Modulo operator** (`%`) for integer modulo
- All builtins registered in both the type checker and codegen layers
- End-to-end test programs: hello world, arithmetic, factorial, fibonacci, string concat, math builtins, modulo

## Phase 7 -- LSP Server (COMPLETE)

- **LSP server** (`codebase/devtools/lsp/`) communicating over stdio via JSON-RPC
- **Diagnostics**: real-time lex, parse, and type-check errors published on file open, change, and save
- **Hover**: type and signature information for builtins, keywords, and user-defined functions (parsed from AST)
- **Completions**: keyword and builtin function name suggestions with full signatures
- **Custom `gradient/batchDiagnostics` notification**: sends all diagnostics for a file in one message with per-phase error counts, designed for AI agent consumption
- In-memory document store for open files
- Built with `tower-lsp` and `tokio`
- **11 tests** (5 unit + 6 integration)

## Phase A -- Compiler-as-Library (COMPLETE)

- **Structured query API**: `Session::from_source()`, `check()`, `symbols()`, `module_contract()`, `type_at()`
- **JSON-serializable results** for all compiler outputs (serde)
- **CLI flags**: `--check --json`, `--inspect --json`
- **13 new tests**

## Phase B -- Enforced Effect System (COMPLETE)

- **5 canonical effects**: IO, Net, FS, Mut, Time
- **Effect inference**: compiler tracks which effects each function body actually uses
- **Unknown effect validation**: reject `!{Foo}` with helpful messages
- **Purity guarantees**: `is_pure` means compiler-proven no side effects
- **CLI flag**: `--effects --json`
- **7 new tests**

## Phase C -- Module Capability Constraints (COMPLETE)

- **`@cap(IO, Net)` annotation** limits module's maximum effects
- **`@cap()` = module must be entirely pure**
- Compiler rejects functions that exceed the capability ceiling
- Capability ceiling shown in module contracts
- **6 new tests**

## Phase D+E -- Dependency Analysis and Code Transforms (COMPLETE)

- **Call graph**: which functions call which
- **`session.callees(fn_name)`**: dependency query
- **`session.rename(old, new)`**: compiler-verified rename with re-checking
- **RenameResult** with locations, verification status
- Call graph included in module contracts
- **7 new tests**

## Phase F -- Mutable Bindings and While Loops (COMPLETE)

- **Mutable bindings** (`let mut`) with reassignment support
- **Assignment statements** (`name = expr`) for mutable bindings
- **While loops** (`while condition: body`)
- Compiler enforces that only `let mut` bindings can be reassigned
- **19 new tests**

## Phase G -- Pattern Matching (Basic) (COMPLETE)

- **`match` expression** with integer literal, boolean literal, and wildcard (`_`) patterns
- Match arms use colon-delimited indented blocks, consistent with the rest of the language
- `match` is an expression: all arms must agree on type when used in a `let` binding
- **8 new tests**

## Phase H -- Enum Types / ADTs (COMPLETE)

- **Enum type declarations** (`type Color = Red | Green | Blue`)
- **Unit variants** fully supported end-to-end (parsing, type checking, codegen)
- **Tuple variants** (`type Option = Some(Int) | None`) parsed and type-checked; codegen for payloads is deferred
- **Match on enum variants** -- `match` extended to support variant patterns
- Enum types integrated into the type system alongside built-in types

## Phase I -- Multi-File Module Resolution (COMPLETE)

- **`use` declarations resolved to source files**: `use math` resolves to `math.gr`, `use a.b` resolves to `a/b.gr`
- **Qualified calls**: imported functions called via `module.function(args)` syntax
- **`Session::from_file(path)`**: compiler session entry point for multi-file projects
- **Cross-file type checking and codegen**: the full pipeline works across module boundaries

## Phase J -- Canonical Formatter (COMPLETE)

- **`gradient fmt`** canonical formatter via `--fmt` flag on `gradient-compiler`
- **`--fmt --write`** mode for in-place file updates
- **`--fmt`** (without `--write`) prints formatted output to stdout for diff/check workflows
- One canonical form per construct -- the formatter is a normalization function, not a style guide
- Agents can pipe source through `--fmt` to get deterministic, canonical output

## Phase K -- Interactive REPL (COMPLETE)

- **`gradient repl`** interactive session via `--repl` flag on `gradient-compiler`
- Cranelift-backed evaluation of expressions and statements
- Type inference feedback: the REPL reports the inferred type of each expression
- **Non-interactive mode**: agents can pipe expressions to `--repl` via stdin for programmatic type inference and evaluation
- Supports all language constructs available in the compiler pipeline

---

# Research-Driven Roadmap (2026-03-22)

The following phases are prioritized by empirical evidence from 60+ academic papers
on LLM code generation, agent workflows, and formal verification. See
`resources/research-synthesis.md` for the full literature review.

---

## Phase L -- Grammar for Constrained Decoding (COMPLETE)

**Evidence:** SynCode (2024) eliminates all syntax errors at near-zero overhead.
XGrammar (NeurIPS 2024) achieves 100x speedup and ships in vLLM, SGLang,
TensorRT-LLM. Grammar-Aligned Decoding (NeurIPS 2024) preserves output quality
under grammar constraints.

**Deliverables:**
- `resources/gradient.ebnf` -- formal EBNF grammar compatible with XGrammar/llguidance/Outlines
- `resources/constrained-decoding-guide.md` -- integration guide for vLLM, llguidance, and Outlines
- PEG grammar updated with match/enum rules
- Published as a standalone artifact alongside the compiler
- Any agents using Gradient through an inference engine can guarantee syntactically
  valid output at ~50us/token overhead

**Impact:** Eliminates all syntax errors in AI-generated Gradient code. Zero compiler
changes needed -- this is purely a grammar artifact.

## Phase M -- Design-by-Contract (COMPLETE)

**Evidence:** Clover (Stanford 2024) achieves 87% correctness with generate+verify.
DafnyBench shows LLMs went from 68% to 96% on formal specs in one year. AutoSpec
(2025) achieves 79% on spec synthesis. LLMs achieve 82-96% success on Dafny-style
pre/postconditions -- vastly higher than on informal coding tasks (Lean 27%, Verus
44%, Dafny 82%).

**Deliverables:**
- `@requires(condition)` annotation on functions -- preconditions
- `@ensures(condition)` annotation on functions -- postconditions
- `result` keyword in postconditions to refer to the return value
- Runtime contract checking (assert on entry/exit)
- Contracts visible in query API (`symbols()`, `module_contract()`) with JSON serialization
- Contract violations produce structured error messages
- **24 new tests** (parser, type checker, IR builder, query API)

**Example:**
```
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)
```

**Impact:** Agents generate code that satisfies a formal contract. The compiler verifies
the contract holds at runtime. This enables the "vericoding" workflow: generate, verify, trust.
No human review needed for contract-verified functions.

---

## Phase N -- Type-Directed Completion Context (COMPLETE)

**Evidence:** Blinn et al. (OOPSLA 2024) show typed holes provide exactly the
information LLMs need. ETH type-constrained decoding (PLDI 2025) reduces compilation
errors by 75%. Type information is the single most effective form of context for
guiding LLM code generation.

**Deliverables:**
- `session.completion_context(line, col)` -- returns expected type at cursor,
  all bindings in scope with their types, matching functions, enum variants, and builtins
- Enhanced typed hole (`?`) diagnostics with matching bindings and functions
- `--complete line col --json` CLI flag for programmatic access
- **13 new tests**

**Impact:** Agents receive a ranked list of type-valid completions at any cursor position.
Combined with grammar-constrained decoding (Phase L), this means AI-generated code
is both syntactically valid AND type-directed.

## Phase O -- Generics and Bidirectional Type Inference (COMPLETE)

**Evidence:** Type-constrained decoding research (PLDI 2025) shows richer types =
tighter constraints = better generation. MoonBit (ICSE 2024) emphasizes mandatory type
signatures at boundaries. Parametric polymorphism is required for a usable standard
library and for expressing common patterns.

**Deliverables:**
- `fn identity[T](x: T) -> T` -- type parameters on functions
- `type Option[T] = Some(T) | None` -- type parameters on enums
- Bidirectional type inference at call sites (unification)
- `[` and `]` tokens added to lexer
- **21 new tests**

## Phase P -- Effect Polymorphism (COMPLETE)

**Evidence:** Koka's row-polymorphic effects enable composable effect management.
GPCE/SPLASH 2024 demonstrates type-safe code generation with algebraic effects.
Effect handlers are the composable answer to dependency injection.

**Deliverables:**
- Lowercase effect variables (e.g., `!{e}`) in function signatures
- Effect variables resolve at call sites (pure -> empty, effectful -> concrete effects)
- `is_effect_polymorphic` in query API
- **12 new tests**

## Phase Q -- Context Budget Tooling (COMPLETE)

**Evidence:** Chroma "Context Rot" (2025) shows performance degrades with input length.
Factory.ai: "context quality > context size." Aider RepoMap proves ~1K tokens of
structural overview outperforms raw source. Greptile shows code needs NL translation
for effective semantic search.

**Deliverables:**
- `session.context_budget(fn_name, budget)` with relevance-ranked items
- `session.project_index()` for structural overview
- `--context --budget N --function name` and `--inspect --index` CLI flags
- **18 new tests**

## Phase R -- Runtime Capability Budgets (COMPLETE)

**Evidence:** E2B uses Firecracker microVMs (<125ms boot). NVIDIA recommends WASM for
agent sandboxing. Google Cloud uses resource caps (8.2s max execution). Capability-based
security (Dennis & Van Horn, 1966) is the correct model for agent sandboxing.

**Deliverables:**
- `@budget(cpu: 5s, mem: 100mb)` annotations on functions
- Budget containment checking (callee cannot exceed caller)
- Budgets visible in query API and module contracts
- **16 new tests**

---

## Phase S -- LLVM Release Backend (COMPLETE)

**Deliverables:**
- `CodegenBackend` trait abstracting codegen backends
- Cranelift implements trait as default debug backend
- LLVM backend behind `llvm` cargo feature flag (stub until LLVM available)
- `--release` CLI flag selects LLVM when compiled in
- **8 new tests**

## Phase T -- Package System (COMPLETE)

**Deliverables:**
- Enhanced `gradient.toml` with `[dependencies]` section (path-based deps)
- `gradient.lock` lockfile with SHA-256 content-addressed checksums
- Dependency resolver: cycle detection, diamond dedup, topological ordering
- `gradient add <path>` and `gradient update` CLI commands
- Build integrates dependency resolution
- **19 new tests**

## Phase U -- FFI Bridges (COMPLETE)

**Deliverables:**
- `@extern` with optional library name: `@extern("libm")`
- `@export` annotation for C-compatible function exports
- FFI type validation (Int, Float, Bool, String, Unit)
- `Linkage::Import` for extern, `Linkage::Export` for export
- Extern/export visible in query API
- **18 new tests**

---

## Tier 4 -- Full Platform (FUTURE)

### Phase V -- Actor Runtime and Supervision Trees

- Actor-based concurrency model with message passing
- Supervision trees for fault tolerance
- Maps naturally to agent-spawning patterns
- Resource isolation per actor

### Phase W -- Documentation Generator

- `gradient doc` produces machine-readable API documentation
- Module contracts as the primary format (already implemented)
- Human-readable HTML output from contracts
- Cross-referenced with call graph and effect analysis

---

## Status Key

| Status        | Meaning                               |
|---------------|---------------------------------------|
| COMPLETE      | Shipped and working                   |
| IN PROGRESS   | Actively being built                  |
| PLANNED       | Designed, evidence-backed, not started|
| FUTURE        | On the roadmap but not yet designed   |
