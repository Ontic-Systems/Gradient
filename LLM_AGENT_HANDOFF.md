# Gradient Project - LLM Agent Handoff Document

**Project:** Gradient Programming Language  
**Repository:** https://github.com/Ontic-Systems/Gradient  
**Local Path:** `/home/gray/TestingGround/Gradient`  
**Last Updated:** 2026-04-07  
**Status:** Active Development (Phase 3: Module System)

---

## 1. Project Overview

Gradient is a systems programming language with:
- **Strong static typing** with type inference
- **Effect system** for capability tracking
- **Actor model** concurrency
- **Self-hosting compiler** (in progress)
- **Query-based IDE integration** (agent mode)

### Architecture

```
codebase/
├── compiler/           # Core compiler (Rust)
│   ├── src/
│   │   ├── lexer/     # Lexical analysis
│   │   ├── parser/    # Recursive descent parser
│   │   ├── typechecker/ # Type checking with effect tracking
│   │   ├── ir/        # Intermediate representation
│   │   ├── codegen/   # LLVM, Cranelift, WASM backends
│   │   └── agent/     # JSON-RPC agent mode for IDE
│   └── tests/         # 1,100+ test cases
├── build-system/      # Package manager & build tool
│   ├── src/commands/  # build, run, test, add, fetch, etc.
│   └── src/registry/  # GitHub registry client
├── runtime/           # C runtime (GC, actors, memory)
│   ├── memory/        # Generational reference counting GC
│   └── vm/            # Actor scheduler
└── compiler/          # Self-hosted compiler (Gradient language)
    ├── *.gr           # 10 self-hosted source files
    └── GRADENT.md     # Language design notes
```

---

## 2. Current State (Milestones)

### ✅ Completed
- **Phase 1:** Core compiler infrastructure
- **Phase 2:** Record types with field reads, record-spread literals
- **Agent Mode v1:** JSON-RPC 2.0 protocol with 13 methods
- **Self-hosting:** All 10 compiler files have **zero parse errors**
- **CI/CD:** GitHub Actions with 1,091 passing tests
- **Issue-first workflow:** GitHub issues created for all known bugs

### 🔄 In Progress
- **Phase 3:** Module system for cross-file type checking
- **Adversarial fixes:** 18 GitHub issues for panic-prone code
- **Type resolution:** Types inside `mod` blocks not being registered

### ⏳ Pending
- Record codegen (field offsets, LoadField/StoreField IR)
- Module system implementation
- SMT contract verification
- Comptime type-level completion

---

## 3. MANDATORY: Development Workflow

### 3.1 GitNexus-First Code Exploration

**ALWAYS use GitNexus before reading files or searching:**

```bash
# Check index status
cd /home/gray/TestingGround/Gradient && gitnexus status

# Query for symbols, patterns, relationships
gitnexus query --repo Gradient "module resolution"
gitnexus context --repo Gradient "Function:codebase/compiler/src/typechecker/checker.rs:check_module"
gitnexus cypher --repo Gradient "MATCH (f:Function) WHERE f.name CONTAINS 'resolve' RETURN f.name, f.filePath"
gitnexus impact --repo Gradient "check_module"
```

**Why:** GitNexus uses a knowledge graph (4,675 symbols, 12,660 relationships) instead of expensive text searches. It provides relationship-aware queries, callers/callees, and impact analysis.

### 3.2 Issue-First Workflow

**NEVER commit directly to main. Always create issues and PRs:**

```bash
# 1. Create issue first
gh issue create --title "fix: description" --body "## Description..." --label bug

# 2. Create branch
pr-workflow branch-create fix/module-registration

# 3. Make changes, commit with conventional commits
git commit -m "fix(typechecker): add ModBlock handling to first pass

Types defined inside mod blocks weren't being registered.

Fixes #42"

# 4. Push and create PR
pr-workflow pr-create --title "fix: module type registration" --body "Fixes #42"

# 5. Monitor CI
pr-workflow pr-monitor --wait

# 6. Merge only after all checks pass
pr-workflow pr-merge
```

### 3.3 Commit Message Format

```
type(scope): short description (max 72 chars)

Longer description explaining what and why.
Can span multiple lines.

Fixes #XXX
Refs #YYY
```

Types: `fix`, `feat`, `docs`, `refactor`, `test`, `chore`, `perf`
Scopes: `parser`, `typechecker`, `ir`, `codegen`, `build-system`, `agent`, `runtime`

---

## 4. Using the Queryable Agent Mode API

The compiler exposes a **JSON-RPC 2.0 API** for IDE integration and agent development.

### CLI Entry

```bash
# Start agent mode
./codebase/target/release/gradient-compiler --agent

# Or with pretty printing for debugging
./codebase/target/release/gradient-compiler --agent --pretty
```

### Protocol (stdin/stdout, newline-delimited JSON)

**Available Methods:**

| Method | Description |
|--------|-------------|
| `load` | Load file, return full SessionReport (diagnostics, symbols, holes, effects) |
| `check` | Check file without full load |
| `symbols` | Get symbol table for a file |
| `holes` | Get typed holes with structured suggestions |
| `complete` | Get completions at a position |
| `type_at` | Get type at a specific line/column |
| `rename` | Rename a symbol |
| `doc` | Get documentation for a symbol |
| `effects` | Get effect information |
| `inspect` | Inspect IR |
| `call_graph` | Get call graph |
| `context_budget` | Get context budget info |
| `shutdown` | Graceful shutdown |

### Example Usage

```bash
# Check file for errors
./codebase/target/release/gradient-compiler --check compiler/token.gr

# Query via agent mode
echo '{"jsonrpc":"2.0","id":1,"method":"load","params":{"file":"compiler/token.gr"}}' | \
  ./codebase/target/release/gradient-compiler --agent
```

---

## 5. Active GitHub Issues (18 Open)

### Critical Priority (Fix First)
1. **#23** - IR Builder panics on missing pre-registered runtime functions
2. **#37** - IR Builder accumulates errors but build() doesn't return Result

### High Priority
3. **#24** - Agent handler double-unwrap can crash LSP/IDE integration
4. **#25** - Cranelift codegen panics on missing function reference
5. **#26** - Main entry point direct indexing without bounds check
6. **#27** - String interpolation direct Vec indexing is fragile
7. **#28** - Typechecker direct arg access without length validation
8. **#32** - WasmBackend Default implementation panics
9. **#35** - Async runtime creation in blocking context is anti-pattern
10. **#39** - IR Builder string conversion functions use consecutive unwraps

### Medium Priority
11. **#29** - Lexer indent stack unwrap could panic on logic error
12. **#30** - Module path resolution unwrap on empty segment
13. **#31** - Dependency parse direct indexing without validation
14. **#33** - BackendWrapper unsafe transmute
15. **#34** - Agent protocol JSON serialization unwrap
16. **#36** - Agent server has no panic recovery
17. **#38** - Typechecker collects errors but check() doesn't return Result
18. **#40** - Wasm codegen value map lookup unwrap

**View all:** `gh issue list --state open`

---

## 6. Key Technical Details

### 6.1 Module System Issue (Root Cause)

**Problem:** Types defined inside `mod token:` aren't being type-checked because the type checker's first pass doesn't recurse into `ModBlock`.

**Location:** `codebase/compiler/src/typechecker/checker.rs`
- First pass (lines ~183-350): Registers types/functions
- Missing: `ItemKind::ModBlock` handling

**Fix Approach:** Add recursive registration for ModBlock items in the first pass.

### 6.2 Self-Hosted Compiler Files

Located in `/home/gray/TestingGround/Gradient/compiler/`:
- `token.gr` - Token definitions
- `lexer.gr` - Lexical analysis
- `parser.gr` - Parser
- `types.gr` - Type definitions
- `types_positional.gr` - Positional types
- `checker.gr` - Type checker
- `ir.gr` - IR definitions
- `ir_builder.gr` - IR construction
- `compiler.gr` - Compiler driver
- `bootstrap.gr` - Bootstrap

All have **zero parse errors** (verified via agent mode API).

### 6.3 Testing

```bash
# Run all tests
cd /home/gray/TestingGround/Gradient/codebase && cargo test

# Run specific test
cargo test test_name

# Run with output
cargo test -- --nocapture
```

**Current status:** 1,091 passing, 7 ignored (file_io pre-existing bug)

### 6.4 Build Commands

```bash
# Build release compiler
cd /home/gray/TestingGround/Gradient/codebase && cargo build --release

# Build and run a Gradient file
./codebase/target/release/gradient-compiler --run file.gr

# Check file for errors
./codebase/target/release/gradient-compiler --check file.gr
```

---

## 7. Common Patterns & Conventions

### 7.1 Error Handling (Fixing Panic-Prone Code)

**Replace .expect()/.unwrap() with proper error handling:**

```rust
// BAD
let func = self.function_refs.get("name").copied()
    .expect("function should be registered");

// GOOD
let func = self.function_refs.get("name").copied()
    .ok_or_else(|| IrError::MissingRuntimeFunction { name: "name" })?;
```

### 7.2 Collection Access

```rust
// BAD
let first = vec[0];

// GOOD
let first = vec.first()
    .ok_or_else(|| Error::EmptyCollection)?;
```

### 7.3 Adding New Error Variants

1. Add variant to error enum
2. Add Display impl case
3. Use `?` operator to propagate

### 7.4 GitNexus Before File Operations

```bash
# Always query first
gitnexus query --repo Gradient "function registration"
gitnexus context --repo Gradient "Function:filepath:symbol_name"

# Then read specific files
read_file /home/gray/TestingGround/Gradient/codebase/compiler/src/ir/builder/mod.rs
```

---

## 8. Resources & Documentation

### In Repository
- `ADVERSARIAL_ERROR_REVIEW.md` - Full security/robustness audit
- `SECURITY_AUDIT_REPORT.md` - Security-focused review
- `HANDOFF_AGENT_MODE.md` - Agent mode design spec
- `comptime-implementation-plan.md` - Comptime feature plan
- `docs/superpowers/specs/2026-04-05-agent-mode-design.md` - Agent mode spec

### GitHub
- Issues: https://github.com/Ontic-Systems/Gradient/issues
- PRs: https://github.com/Ontic-Systems/Gradient/pulls
- Actions: https://github.com/Ontic-Systems/Gradient/actions

### Tools
- `pr-workflow` - GitHub PR/issue workflow automation
- `ci-monitor` - CI status monitoring
- `gitnexus` - Code intelligence and navigation

---

## 9. Quick Reference Commands

```bash
# Check current status
git status
gh issue list --state open

# Build and test
cd codebase && cargo build --release && cargo test

# Verify a fix
./codebase/target/release/gradient-compiler --check compiler/token.gr

# Create issue + branch + PR workflow
gh issue create --title "fix: description" --body "..." --label bug
pr-workflow branch-create fix/issue-number
# ... make changes ...
git add . && git commit -m "fix(scope): description

Fixes #XXX"
pr-workflow pr-create --title "fix: description"
pr-workflow pr-monitor --wait
pr-workflow pr-merge
```

---

## 10. Next Priorities (For New Agent)

1. **Fix critical issues #23 and #37** (IR Builder panics)
2. **Implement module system type registration** (Phase 3 blocker)
3. **Batch-fix high-priority unwrap issues** (#24-#28, #32, #35, #39)
4. **Continue self-hosting work** once module system is fixed

---

## End of Handoff Document

This document provides everything needed for a new LLM agent to continue Gradient development. Always follow the GitNexus-first workflow and issue-first PR process.
