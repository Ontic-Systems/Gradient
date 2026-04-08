# Gradient Roadmap

**Status Key:** ✅ Stable | 🟢 Beta | 🧪 Experimental | 🚧 Planned | ❌ Broken

## Current Status: Alpha (Phase 2 — Self-Hosting)

**1,058 tests passing locally.** The compiler works. Programs compile to native binaries.

**Phase 2 Progress:** Self-hosting compiler partially complete. `token.gr`, `lexer.gr`, and `parser.gr` parse and typecheck successfully.

**Note:** Public CI shows failures due to environment differences. Local builds pass. See [CI Status](../STATUS_LOCAL_TRUTH.md).

---

## Completed Phases

### Phase 0 — Foundation
- PEG grammar, CLI scaffold, Cranelift codegen PoC

### Phase 1 — Host Compiler Frontend
- Hand-written lexer (94 tests)
- Recursive descent parser (128 tests)
- Error recovery

### Phase 2 — Type System
- Static type checker with inference (371 tests)
- Five built-in types: `Int`, `Float`, `String`, `Bool`, `()`
- Effect system: `!{IO}`, `!{Net}`, `!{FS}`, `!{Mut}`, `!{Time}`

### Phase 3 — IR Generation
- AST to SSA IR translation
- Instruction set: Const, Call, Ret, arithmetic, branching, phi nodes

### Phase 4 — Full Pipeline
- End-to-end: `.gr` → native binary via Cranelift
- `gradient build`, `gradient run`

### Phase 5 — Working CLI
- `gradient new`, `gradient check`
- Project discovery, compiler discovery

### Phase 6 — Standard Library
- I/O: `print`, `println`, `print_int`, `print_float`, `print_bool`, `read_line`
- Math: `abs`, `min`, `max`, `mod_int`, `pow`, `sqrt`
- Conversions: `int_to_string`, `parse_int`, `parse_float`

### Phase A — Compiler-as-Library
- `Session::from_source()`, `check()`, `symbols()`
- JSON-serializable outputs

### Phase B — Effect System Polish
- Effect inference and validation
- Purity guarantees

### Phase C — Module Capabilities
- `@cap(IO, Net)` annotations

### Phase D+E — Analysis & Transforms
- Call graph, dependency queries
- Compiler-verified rename

### Phase F — Control Flow
- Mutable bindings (`let mut`)
- While loops, assignment

### Phase G — Pattern Matching
- `match` with int/bool/wildcard patterns

### Phase H — Algebraic Data Types
- Enum types: `type Color = Red | Green | Blue`
- Tuple variants: `type Shape = Circle(Float) | Point`

### Phase I — Modules
- Multi-file resolution: `use math` → `math.gr`
- Qualified calls: `math.add(a, b)`

### Phase L — Grammar for Constrained Decoding
- `resources/gradient.ebnf` for XGrammar/llguidance/Outlines

### Phase M — Design-by-Contract
- `@requires`, `@ensures` with runtime checking
- `result` keyword in postconditions

### Phase N — Generics
- Type parameters: `fn identity[T](x: T) -> T`
- Bidirectional inference

### Phase O — Effect Polymorphism
- Lowercase effect variables: `!{e}`

### Phase P — Expanded Language
- Tuples: `(Int, String)` with destructuring
- Closures: `|x| x + 1`
- Traits: `trait Display`, `impl Display for MyType`
- Result/Option types with `?` operator
- Lists: `List[T]` with `[1, 2, 3]` literals
- String interpolation: `f"hello {name}"`
- Pipe operator: `x |> f |> g`
- For-in loops: `for x in list:`
- Match guards and exhaustiveness checking

### Phase Q — Method Syntax
- `obj.method(args)` dispatch
- Chained calls: `"hello".trim().length()`

### Phase R — I/O Expansion
- `read_line()`, `parse_int`, `parse_float`, `exit(code)`, `args()`
- File I/O: `file_read`, `file_write`, `file_exists` under `!{FS}` effect

### Phase S — Data Structures
- `Map[String, V]` with 7 builtins
- Persistent-by-copy semantics

### Phase T — Test Framework
- `@test` annotation
- `gradient test` with discovery, harness, reporting

### Phase U — LSP Server
- Diagnostics, hover, completions
- `gradient/batchDiagnostics` for agents

### Phase V — Actors
- `actor` declarations with `state` and `on` handlers
- `spawn`, `send`, `ask` expressions
- `!{Actor}` effect

---

## In Progress / Experimental

| Feature | Status | Notes |
|---------|--------|-------|
| Self-hosting compiler | 🚧 **In Progress** | 3/7 core files parse & typecheck (see below) |
| Canonical formatter (`gradient fmt`) | 🧪 Experimental | Code exists (1,297 lines), CLI not wired |
| Interactive REPL (`gradient repl`) | 🧪 Experimental | Code exists (960 lines), not functional |
| WebAssembly backend | 🧪 Experimental | Compile with `--features wasm` |
| Git dependencies | 🧪 Experimental | CLI support exists, unverified end-to-end |
| LLVM backend | ❌ Broken | Disabled in CI (Polly linking issue) |
| SMT verification | 🚧 Planned | Feature flag only, not functional |

### Self-Hosting Progress Detail

Writing the Gradient compiler in Gradient itself (~6,800 lines across 7 files).

| File | Lines | Status | Notes |
|------|-------|--------|-------|
| `token.gr` | ~750 | ✅ Complete | Token types parse & typecheck |
| `lexer.gr` | ~490 | ✅ Complete | Lexical analysis with `LexState` record |
| `parser.gr` | ~990 | ✅ Complete | AST nodes & recursive descent parser |
| `types.gr` | ~600 | 🚧 In Progress | Type system definitions |
| `checker.gr` | ~800 | 🚧 In Progress | Type inference & checking |
| `ir.gr` | ~700 | 🚧 In Progress | IR instruction definitions |
| `ir_builder.gr` | ~400 | 🚧 In Progress | IR construction from AST |

**Critical Blocker:** Module system for self-hosted files currently requires manual concatenation to validate multi-file programs.

---

## Planned

| Feature | Status | Notes |
|---------|--------|-------|
| Package registry server | 🚧 Planned | No server implementation exists |
| Registry dependencies | 🚧 Planned | Blocked on registry server |
| IDE plugins (VS Code, Zed) | 🚧 Planned | LSP exists, plugins not started |
| Linear types | 🚧 Planned | Runtime exists, language surface not defined |
| Session types | 🚧 Planned | Design phase only |
| Advanced supervision trees | 🚧 Planned | Documentation exists, not implemented |
| Grammar-constrained decoding | 🚧 Planned | XGrammar/llguidance integration |

---

## Recently Completed (Stable)

| Feature | Status | Completion |
|---------|--------|------------|
| Cranelift backend | ✅ Stable | Primary native backend |
| Type system | ✅ Stable | 1,058 tests |
| Effects system | ✅ Stable | Tracked and enforced |
| Pattern matching | ✅ Stable | Full ADT support |
| Generics | ✅ Stable | Type parameters, inference |
| Module system | ✅ Stable | Multi-file `use` resolution |
| LSP server | ✅ Stable | Diagnostics, completions, hover |
| Test framework | ✅ Stable | `@test` annotations |
| Query API | ✅ Stable | JSON output, compiler-as-agent |
| FFI / Extern | ✅ Stable | `@extern` and `@export` working |
