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

## Tier 4 -- Full Platform

## Phase V -- Actor Runtime Language Foundation (COMPLETE)

**Deliverables:**
- `actor` declarations with `state` fields and `on` message handlers
- `spawn`, `send`, `ask` expressions
- `Ty::Actor` type and `!{Actor}` effect
- Actor info in query API
- **23 new tests**

## Phase W -- Documentation Generator (COMPLETE)

**Deliverables:**
- `///` doc comment syntax (lexer `DocComment` token)
- Doc comments attached to functions, types, enums, actors
- `session.documentation()` returning `ModuleDocumentation` with `FunctionDoc`, `TypeDoc`
- `session.documentation_text()` for plain-text output
- `--doc` and `--doc --json` CLI flags
- **14 new tests**

---

## Tier 5 -- Language Maturity

### Phase X -- Closures and First-Class Functions (COMPLETE)

**Deliverables:**
- Closure syntax: `|x: Int, y: Int| x + y`, `|x: Int| -> Int: x + 1`, `|| expr`
- `ClosureParam` AST node with optional type annotations
- Closures type-checked as `Ty::Fn { params, ret, effects: [] }`
- IR lowering as generated `__closure_N` functions
- Closures as arguments to higher-order functions
- **18 new tests**

### Phase Y -- Expanded Standard Builtins (COMPLETE)

**Deliverables:**
- 12 string builtins: `string_length`, `string_contains`, `string_starts_with`, `string_ends_with`, `string_substring`, `string_trim`, `string_to_upper`, `string_to_lower`, `string_replace`, `string_index_of`, `string_char_at`, `string_split`
- 6 numeric builtins: `float_to_int`, `int_to_float`, `pow`, `float_abs`, `float_sqrt`, `float_to_string`
- Cranelift codegen for all 18 builtins using C library FFI
- **25 new tests**

### Phase Z -- Test Framework (COMPLETE)

**Deliverables:**
- `@test` annotation on functions (validated: no params, returns `()` or `Bool`)
- `is_test` flag on `FnDef` and `SymbolInfo`
- `gradient test` command with test discovery, harness generation, execution, and reporting
- Test filtering via `--filter` flag
- **23 new tests**

### Phase AA -- Tuple Types (COMPLETE)

**Deliverables:**
- `(Int, String)` tuple type expressions and `Ty::Tuple` internal type
- `(1, "hello")` tuple literal expressions
- `pair.0`, `pair.1` numeric field access
- `let (a, b) = pair` destructuring in let bindings
- `StmtKind::LetTupleDestructure` and `ItemKind::LetTupleDestructure`
- **20 new tests**

### Phase BB -- Traits and Interfaces (COMPLETE)

**Deliverables:**
- `trait` declarations with method signatures (`trait Display: fn display(self) -> String`)
- `impl` blocks (`impl Display for Int: ...`)
- Trait bounds on generics (`[T: Display]`)
- `Self` type in trait methods resolving to the implementing type
- `TypeParam` struct replacing `Vec<String>` for type parameters
- TraitInfo/ImplInfo in TypeEnv with registration and lookup
- Trait and Impl in query API SymbolKind
- **20 new tests**

### Phase CC -- Result Type and Error Handling (COMPLETE)

**Deliverables:**
- Built-in `Result[T, E] = Ok(T) | Err(E)` and `Option[T] = Some(T) | None`
- `?` operator for error propagation (postfix, same precedence as field access)
- `is_ok(Result) -> Bool` and `is_err(Result) -> Bool` convenience functions
- Ok/Err/Some/None constructors in type environment
- **19 new tests**

### Phase DD -- List Type and Literals (COMPLETE)

**Deliverables:**
- `List[T]` type (`Ty::List`) with `[1, 2, 3]` literal syntax
- 8 core builtins: `list_length`, `list_get`, `list_push`, `list_concat`, `list_is_empty`, `list_head`, `list_tail`, `list_contains`
- Heap-allocated runtime representation `[length, capacity, data...]`
- Generic type checking via `check_list_builtin`
- **26 new tests**

### Phase EE -- String Interpolation (COMPLETE)

**Deliverables:**
- `f"hello {name}"` syntax with `InterpolatedString` lexer token
- `StringInterp` AST node with `StringInterpPart::Literal` and `StringInterpPart::Expr`
- Expression parsing inside `{}` via nested lexer/parser
- Desugared to `string_concat` chains with auto-conversion (`int_to_string`, `float_to_string`, `bool_to_string`)
- `bool_to_string(Bool) -> String` new builtin
- **19 new tests**

### Phase FF -- Higher-Order List Functions (COMPLETE)

**Deliverables:**
- 9 higher-order builtins: `list_map`, `list_filter`, `list_fold`, `list_foreach`, `list_any`, `list_all`, `list_find`, `list_sort`, `list_reverse`
- Full generic type inference (e.g., `list_map(List[Int], (Int) -> String) -> List[String]`)
- Closure parameter type validation against list element type
- **27 new tests**

### Phase GG -- Method Call Syntax (COMPLETE)

**Deliverables:**
- `obj.method(args)` dispatches to free functions or trait methods
- 20 builtin methods: `"s".length()`, `"s".contains()`, `[1,2].push(3)`, `[1,2].get(0)`, etc.
- Trait method dispatch: `x.display()` resolves through impl blocks
- Chained method calls: `"hello".trim().length()`
- **26 new tests**

### Phase HH -- Pipe Operator (COMPLETE)

**Deliverables:**
- `x |> f |> g` pipe syntax with `|>` token
- Lowest precedence operator, left-associative
- Desugars to nested function calls: `g(f(x))`
- Works with named functions and closures
- **14 new tests**

### Phase II -- For-In Loops and Range Expressions (COMPLETE)

**Deliverables:**
- `for x in list:` iteration over `List[T]` values
- `for x in 0..10:` range expression syntax (`start..end`)
- Range expressions produce iterable integer sequences
- **Test count updated below**

### Phase JJ -- Pattern Matching Guards (COMPLETE)

**Deliverables:**
- `match` arm guards with `if condition` (e.g., `n if n > 0:`)
- Variable binding patterns in match arms
- String literal patterns in match expressions
- **Test count updated below**

### Phase KK -- Match Exhaustiveness Checking (COMPLETE)

**Deliverables:**
- Exhaustiveness checking for `Bool` matches (must cover `true` and `false`)
- Exhaustiveness checking for enum matches (must cover all variants)
- Non-exhaustive match warnings with helpful diagnostics
- **Test count updated below**

### Phase LL -- Tuple Variant Codegen (COMPLETE)

**Deliverables:**
- Heap-allocated tagged union representation for all enum variants: `[tag: i64, field_0: i64, ...]`
- Uniform representation: both unit variants and tuple variants use the same heap-allocated layout
- Three new IR instructions: `ConstructVariant { result, tag, payload }`, `GetVariantTag { result, ptr }`, `GetVariantField { result, ptr, index }`
- Cranelift codegen for all three instructions: `ConstructVariant` uses `malloc` and stores bitcast fields, `GetVariantTag` loads the tag slot, `GetVariantField` loads payload slots with type-correct loads (F64 fields loaded directly as `f64` to avoid clobbering the `al` register for variadic calls)
- Match arms with payload bindings (`Circle(r): r`) extract the field via `GetVariantField` and bind it in scope
- Enum demo programs with `Circle(Float) | Box(Float) | Point` compile and produce correct output
- **13 new tests** (823 total: 5 IR builder tests, 8 codegen tests)

**Example:**
```
type Shape = Circle(Float) | Box(Float) | Point

fn area(s: Shape) -> Float:
    match s:
        Circle(r):
            r
        Box(side):
            side
        Point:
            0.0
```

**Impact:** Enum types with payloads are now fully usable end-to-end, from source through to native binary execution. This unlocks `Option`-style and `Result`-style patterns in Gradient programs.

---

## Phase MM -- Standard I/O Expansion (COMPLETE)

**Deliverables:**
- `read_line() -> !{IO} String` — reads one line from stdin (via `__gradient_read_line` helper in `runtime/gradient_runtime.c`)
- `parse_int(s: String) -> Int` — parses a string to integer using C `atoi`; returns `0` on failure
- `parse_float(s: String) -> Float` — parses a string to float using C `atof`; returns `0.0` on failure
- `exit(code: Int) -> !{IO} ()` — calls C `exit()` to terminate the process immediately
- `args() -> !{IO} List[String]` — returns real command-line arguments via C runtime argc/argv integration
- C runtime helper: `codebase/compiler/runtime/gradient_runtime.c` — provides `__gradient_read_line`
- 7 type checker tests covering IO effect constraints and type correctness
- 5 codegen unit tests verifying object-file generation
- 7 integration tests that compile, link, and run real binaries
- **Test count: 831 total (up from 810)**

---

### Phase NN -- File I/O Builtins (COMPLETE)

**Deliverables:**
- 4 new builtins under the `FS` effect: `file_read`, `file_write`, `file_exists`, `file_append`
- Type checker registration with full `!{FS}` effect enforcement
- Cranelift codegen via external C helpers (`runtime/gradient_runtime.c`) declared as `Linkage::Import`
- `runtime/gradient_runtime.c` extended with `__gradient_file_read`, `__gradient_file_write`, `__gradient_file_exists`, `__gradient_file_append` implementations
- FS effect enforcement: calling any file builtin from a pure or `!{IO}` function is a type error
- `@cap(IO)` modules cannot declare `!{FS}` functions; `@cap(IO, FS)` modules can
- Integration test fixture `tests/file_io.gr` covering all four builtins end-to-end
- Build system auto-links `runtime/gradient_runtime.c` — no manual linking step required
- **15 new tests**

---

## Phase OO -- HashMap Type (COMPLETE)

**Deliverables:**
- `Ty::Map(Box<Ty>, Box<Ty>)` variant added to the type system with `Display` impl (`Map[K, V]`)
- `Map[K, V]` type annotations resolved in `resolve_type_expr` (`Generic { name: "Map", args }`)
- 7 map builtins registered in `TypeEnv::preload_builtins()`:
  - `map_new() -> Map[String, V]`
  - `map_set(Map[String,V], String, V) -> Map[String,V]`
  - `map_get(Map[String,V], String) -> Option`
  - `map_contains(Map[String,V], String) -> Bool`
  - `map_remove(Map[String,V], String) -> Map[String,V]`
  - `map_size(Map[String,V]) -> Int`
  - `map_keys(Map[String,V]) -> List[String]`
- `check_map_builtin` method in checker.rs for type-aware dispatch of all 7 builtins
- `types_compatible_with_typevars` helper allows generic `map_new()` to satisfy typed annotations like `Map[String, Int]`
- Map method syntax: `m.set(k,v)`, `m.get(k)`, `m.contains(k)`, `m.remove(k)`, `m.size()`, `m.keys()`
- IR builder registers 7 map function references (`map_new` through `map_keys`)
- Cranelift codegen: 9 C FFI declarations + 7 codegen cases (including inline `Some`/`None` construction for `map_get`)
- `runtime/gradient_runtime.c`: `GradientMap` struct + 9 C helper functions (`__gradient_map_new`, `_set_str`, `_set_int`, `_get_str`, `_get_int`, `_contains`, `_remove`, `_size`, `_keys`)
- Persistent-by-copy map semantics: `map_set` and `map_remove` return new map instances
- **12 new type-checker unit tests**

---

## Phase PP -- Standard Library Expansion (COMPLETE)

**Goal:** Expand Gradient's stdlib from 62+ to ~134 builtins across math, string, data structures, date/time, env/process, and JSON.

### Phase PP.1 -- Math Builtins (25 functions)
- Trigonometric: `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`
- Logarithmic/Exp: `log`, `log10`, `log2`, `exp`, `exp2`
- Rounding: `ceil`, `floor`, `round`, `trunc`
- Constants: `pi()`, `e()`
- Additional: `gcd(Int,Int)`, `float_mod(Float,Float)`, `clamp(T,T,T)`

### Phase PP.2 -- Random Builtins (4 functions)
- `random() -> Float`
- `random_int(Int, Int) -> Int`
- `random_float() -> Float`
- `seed_random(Int) -> ()`

### Phase PP.3 -- String Utilities Batch 1 (9 functions)
- `string_join`, `string_repeat`, `string_pad_left`, `string_pad_right`
- `string_strip`, `string_strip_prefix`, `string_strip_suffix`
- `string_to_int`, `string_to_float` (with Option return types)

### Phase PP.4 -- String Utilities Batch 2 (6 functions)
- `string_format`, `string_is_empty`, `string_reverse`
- `string_compare`, `string_find`, `string_slice`

### Phase PP.5 -- Set Container Type (8 functions)
- `Set[T]` type variant
- `set_new`, `set_add`, `set_remove`, `set_contains`
- `set_size`, `set_union`, `set_intersection`, `set_to_list`

### Phase PP.6 -- Queue Container Type (5 functions)
- `Queue[T]` type variant
- `queue_new`, `queue_enqueue`, `queue_dequeue`, `queue_peek`, `queue_size`

### Phase PP.7 -- Stack Container Type (5 functions)
- `Stack[T]` type variant
- `stack_new`, `stack_push`, `stack_pop`, `stack_peek`, `stack_size`

### Phase PP.8 -- Date/Time Builtins (8 functions)
- `now() -> Int` (timestamp in seconds, !{Time})
- `now_ms() -> Int` (timestamp in milliseconds, !{Time})
- `sleep(Int) -> ()` (milliseconds, !{Time})
- `time_string() -> String` (RFC3339, !{Time})
- `date_string() -> String` (YYYY-MM-DD, !{Time})
- `datetime_year(Int) -> Int` (pure)
- `datetime_month(Int) -> Int` (pure)
- `datetime_day(Int) -> Int` (pure)

### Phase PP.9 -- Environment/Process Builtins (7 functions)
- `get_env(String) -> Option[String]` (!{IO})
- `set_env(String, String) -> ()` (!{IO})
- `current_dir() -> String` (!{IO})
- `change_dir(String) -> ()` (!{IO})
- `process_id() -> Int` (pure)
- `system(String) -> Int` (!{IO})
- `sleep_seconds(Int) -> ()` (!{Time})

### Phase PP.10 -- JSON Builtins (8+ functions)
- JSON parsing and serialization
- Typed extractors: `json_get_string`, `json_get_int`, etc.
- Inspection utilities: `json_type`, `json_has_key`, `json_keys`
- **Already implemented in previous commits**

**Total: ~81 new builtins added (from 62 to ~143)**
**Status: All sub-tasks completed via parallel subagent implementation**

**Deliverables:**
- `Ty::Map(Box<Ty>, Box<Ty>)` variant added to the type system with `Display` impl (`Map[K, V]`)
- `Map[K, V]` type annotations resolved in `resolve_type_expr` (`Generic { name: "Map", args }`)
- 7 map builtins registered in `TypeEnv::preload_builtins()`:
  - `map_new() -> Map[String, V]`
  - `map_set(Map[String,V], String, V) -> Map[String,V]`
  - `map_get(Map[String,V], String) -> Option`
  - `map_contains(Map[String,V], String) -> Bool`
  - `map_remove(Map[String,V], String) -> Map[String,V]`
  - `map_size(Map[String,V]) -> Int`
  - `map_keys(Map[String,V]) -> List[String]`
- `check_map_builtin` method in checker.rs for type-aware dispatch of all 7 builtins
- `types_compatible_with_typevars` helper allows generic `map_new()` to satisfy typed annotations like `Map[String, Int]`
- Map method syntax: `m.set(k,v)`, `m.get(k)`, `m.contains(k)`, `m.remove(k)`, `m.size()`, `m.keys()`
- IR builder registers 7 map function references (`map_new` through `map_keys`)
- Cranelift codegen: 9 C FFI declarations + 7 codegen cases (including inline `Some`/`None` construction for `map_get`)
- `runtime/gradient_runtime.c`: `GradientMap` struct + 9 C helper functions (`__gradient_map_new`, `_set_str`, `_set_int`, `_get_str`, `_get_int`, `_contains`, `_remove`, `_size`, `_keys`)
- Persistent-by-copy map semantics: `map_set` and `map_remove` return new map instances
- **12 new type-checker unit tests**

---

## Status Key

| Status        | Meaning                               |
|---------------|---------------------------------------|
| COMPLETE      | Shipped and working                   |
| IN PROGRESS   | Actively being built                  |
| PLANNED       | Designed, evidence-backed, not started|
| FUTURE        | On the roadmap but not yet designed   |
