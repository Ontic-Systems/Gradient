# Gradient Roadmap

## Current Status: Alpha

**881 tests passing.** The compiler works. Programs compile to native binaries.

---

## Completed Phases

### Phase 0 — Foundation
- PEG grammar, CLI scaffold, Cranelift codegen PoC

### Phase 1 — Frontend  
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

## In Progress

| Feature | Status |
|---------|--------|
| Canonical formatter (`gradient fmt`) | 🚧 Stubbed |
| Interactive REPL (`gradient repl`) | 🚧 Stubbed |
| LLVM backend | ⚠️ Feature flag only |
| SMT verification | ⚠️ Feature flag only |

---

## Planned

- WASM compilation target
- Package registry
- IDE plugins (VS Code, Zed)
