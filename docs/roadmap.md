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

## Phase 8+ -- Advanced Features (FUTURE)

- Algebraic data types (enum)
- Multi-file module resolution
- Runtime effect enforcement (beyond compile-time)
- LLVM release backend
- Actor runtime with supervision trees
- Package system and dependency resolution
- FFI bridges (C, Rust, Python)
- Canonical formatter (`gradient fmt`)
- REPL (`gradient repl`)
- Documentation generator

---

## Status Key

| Status        | Meaning                               |
|---------------|---------------------------------------|
| COMPLETE      | Shipped and working                   |
| IN PROGRESS   | Actively being built                  |
| PLANNED       | Designed but not started              |
| FUTURE        | On the roadmap but not yet designed   |
