# Gradient Adversarial Error Handling & API Design Review

**Review Date:** 2026-04-06
**Scope:** Full codebase (compiler, build-system, devtools, test-framework)
**Focus Areas:** Panic-prone code, unwrap/expect usage, error propagation, public API design, FFI/async safety

---

## Summary Statistics

| Category | Count | Severity |
|----------|-------|----------|
| Critical (.unwrap()/.expect() in production) | 45+ | HIGH |
| High (.unwrap()/.expect() on collections) | 20+ | HIGH |
| Medium (Invariant assertions) | 15+ | MEDIUM |
| Low (Test-only panics) | 100+ | LOW |

---

## Critical Findings (Production Code)

### 1. CRITICAL: Registry Resolution Panic on Empty Version List
**File:** `codebase/build-system/src/resolver.rs:539`
**File:** `codebase/build-system/src/commands/add.rs:231`
**File:** `codebase/build-system/src/commands/fetch.rs:153`
**File:** `codebase/build-system/src/commands/update.rs:164`
```rust
semver::latest_version(&versions).expect("versions is not empty")
```
**Severity:** CRITICAL
**Explanation:** The `latest_version` function is called with `.expect()` on an external API result. If GitHub returns a repo with no semver tags, the entire build system crashes instead of returning a proper error.
**Proposed Fix:**
```rust
semver::latest_version(&versions)
    .ok_or_else(|| ResolveError::NoVersionsAvailable { name: name.to_string() })?
```

---

### 2. CRITICAL: IR Builder Unwrap on Missing Pre-registered Functions
**File:** `codebase/compiler/src/ir/builder/mod.rs:1487`
```rust
.expect("__gradient_contract_fail should be pre-registered")
```
**File:** `codebase/compiler/src/ir/builder/mod.rs:1796`
```rust
.expect("list_literal_N should be registered")
```
**File:** `codebase/compiler/src/ir/builder/mod.rs:1986`
```rust
.expect("string_concat should be pre-registered")
```
**File:** `codebase/compiler/src/ir/builder/mod.rs:2800`
```rust
.expect("list_length should be pre-registered")
```
**File:** `codebase/compiler/src/ir/builder/mod.rs:2835`
```rust
.expect("list_get should be pre-registered")
```
**File:** `codebase/compiler/src/ir/builder/mod.rs:3094`
```rust
.expect("string_eq should be pre-registered")
```
**File:** `codebase/compiler/src/ir/builder/mod.rs:3474`
```rust
.expect("__concurrent_scope_enter should be registered")
```
**File:** `codebase/compiler/src/ir/builder/mod.rs:3627`
```rust
.expect("__supervisor_create should be registered")
```
**Severity:** CRITICAL
**Explanation:** The IR builder assumes certain runtime functions are always pre-registered. If the registration logic changes or fails, the compiler will panic instead of gracefully reporting the missing function. These are internal invariant violations that should return `Result`.
**Proposed Fix:**
```rust
let func_ref = self.function_refs.get("string_concat").copied()
    .ok_or_else(|| IrError::MissingRuntimeFunction { name: "string_concat" })?;
```

---

### 3. HIGH: Agent Handler Unwrap Chain
**File:** `codebase/compiler/src/agent/handlers.rs:458`
```rust
let result = handle_context_budget(&budget_params, session.as_ref().unwrap()).unwrap();
```
**Severity:** HIGH
**Explanation:** In a test, but this pattern appears in handler code. Double-unwrap on session access followed by handler result. In production agent mode, this would crash the LSP/IDE integration.
**Proposed Fix:** Use proper `?` propagation or match on the Option.

---

### 4. HIGH: Cranelift Codegen Unwrap on Missing Function
**File:** `codebase/compiler/src/codegen/cranelift.rs:6703`
```rust
self.declared_functions.get(target_name).unwrap()
```
**Severity:** HIGH
**Explanation:** During code generation, if a function reference cannot be found in the declared functions map, the compiler panics. This could happen with malformed IR or missing extern declarations.
**Proposed Fix:**
```rust
let target_func_id = self.declared_functions.get(target_name)
    .ok_or_else(|| CodegenError::UndeclaredFunction { name: target_name.clone() })?;
```

---

### 5. HIGH: Main Entry Point Direct Indexing Without Bounds Check
**File:** `codebase/compiler/src/main.rs:257`
```rust
let input_file = positional_args[0].as_str();
```
**Severity:** MEDIUM-HIGH
**Explanation:** While there is a check at line 252, if this code is refactored or the check is removed, direct indexing will panic. Also uses `.as_str()` on an OsString which may panic on invalid UTF-8.
**Proposed Fix:**
```rust
let input_file = positional_args.first()
    .and_then(|s| s.to_str())
    .ok_or("Invalid input file path")?;
```

---

### 6. HIGH: String Interpolation Direct Vec Index
**File:** `codebase/compiler/src/ir/builder/mod.rs:2475`
```rust
let mut acc = string_vals[0];
```
**Severity:** MEDIUM-HIGH
**Explanation:** After the empty check at line 2467, this is technically safe, but the code path is fragile. If someone refactors and removes the check, this becomes a panic point.
**Proposed Fix:** Use `.first()` with a proper error or maintain the invariant with an explicit assertion comment.

---

### 7. HIGH: Typechecker Direct Arg Access Without Length Validation
**File:** `codebase/compiler/src/typechecker/checker.rs` (multiple locations)
```rust
let arg_ty = self.check_expr(&args[0]);
```
**Lines:** 3156, 3183, 3224, 3267, 3310, 3337, 3366, 3393, 3437
**Severity:** MEDIUM
**Explanation:** While most calls are guarded by length checks, some patterns access `args[0]` before validation. If the validation logic has a bug, this panics.
**Proposed Fix:** Use `.first()` and return an error if None.

---

### 8. MEDIUM: Indent Stack Unwrap in Lexer
**File:** `codebase/compiler/src/lexer/lexer.rs:372`
**File:** `codebase/compiler/src/lexer/lexer.rs:384`
**File:** `codebase/compiler/src/lexer/lexer.rs:393`
```rust
let current_indent = *self.indent_stack.last().unwrap();
while *self.indent_stack.last().unwrap() > indent {
```
**Severity:** MEDIUM
**Explanation:** The indent stack should never be empty (starts with 0), but this is an implicit invariant. If there's a bug in the lexer logic, this will panic.
**Proposed Fix:**
```rust
let current_indent = *self.indent_stack.last()
    .ok_or(LexerError::InvalidIndentState)?;
```

---

### 9. MEDIUM: Module Path Resolution Unwrap
**File:** `codebase/compiler/src/resolve.rs:328`
```rust
rel_path.push(format!("{}.gr", path_segments.last().unwrap()));
```
**Severity:** MEDIUM
**Explanation:** If an empty path segment array is passed, this will panic. The function should validate inputs.
**Proposed Fix:**
```rust
let last_seg = path_segments.last()
    .ok_or(ResolveError::EmptyPath)?;
```

---

### 10. MEDIUM: Dependency Parse Direct Index
**File:** `codebase/build-system/src/commands/add.rs:41`
```rust
let name = parts[0].to_string();
```
**Severity:** MEDIUM
**Explanation:** If `arg` is an empty string or contains only `@`, `split('@')` could return an empty first part (though in this case it would still have one element). Still fragile.
**Proposed Fix:**
```rust
let name = parts.first()
    .filter(|s| !s.is_empty())
    .ok_or("Invalid dependency name")?;
```

---

## Public API Design Flaws

### 11. HIGH: WasmBackend Default Constructor Panics
**File:** `codebase/compiler/src/codegen/wasm.rs:1003`
```rust
Self::new().expect("Failed to create default WasmBackend")
```
**Severity:** HIGH
**Explanation:** A `Default` implementation that panics violates Rust conventions. Users expect `Default::default()` to be infallible.
**Proposed Fix:**
```rust
// Remove Default impl, require explicit new() with Result
// Or implement TryDefault trait pattern
```

---

### 12. MEDIUM: BackendWrapper Unsafe Transmute Without Documentation
**File:** `codebase/compiler/src/codegen/mod.rs:160`
**File:** `codebase/compiler/src/codegen/mod.rs:205`
```rust
unsafe { std::mem::transmute(&*context) }
```
**Severity:** MEDIUM
**Explanation:** While the safety comment exists, this is a critical invariant. The transmute relies on the boxed context living as long as the wrapper. If the wrapper is misused, this is undefined behavior.
**Proposed Fix:** Wrap in a safer abstraction that enforces the lifetime relationship at compile time.

---

### 13. MEDIUM: Agent Protocol Notification Serialization Unwrap
**File:** `codebase/compiler/src/agent/protocol.rs:80`
```rust
serde_json::to_string(&obj).unwrap()
```
**Severity:** MEDIUM
**Explanation:** JSON serialization of a statically-known object should never fail, but using unwrap() in a public API is risky. If serde_json has a bug or the object is later modified, this panics.
**Proposed Fix:**
```rust
serde_json::to_string(&obj)
    .map_err(|e| format!("JSON serialization failed: {}", e))?
```

---

## FFI & Async Safety Issues

### 14. HIGH: Async Runtime Creation in Blocking Context
**File:** `codebase/build-system/src/commands/add.rs:238-243`
```rust
fn resolve_registry_version_blocking(name: &str) -> Result<String, String> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create runtime: {}", e))?;
    rt.block_on(resolve_registry_version(name))
}
```
**Severity:** HIGH
**Explanation:** Creating a new Tokio runtime for each blocking call is inefficient and can cause issues if called from an existing async context. This is a "async-in-sync" anti-pattern that can lead to deadlocks or panics.
**Proposed Fix:** Use `#[tokio::main]` or proper runtime propagation, or use `reqwest::blocking` for sync contexts.

---

### 15. MEDIUM: Agent Server Stdin Loop No Panic Recovery
**File:** `codebase/compiler/src/agent/server.rs:35-93`
**Explanation:** The agent server loop runs the entire compiler pipeline in a single thread. If any part of `dispatch()` panics, the entire agent process crashes with no recovery or error response to the client.
**Proposed Fix:** Wrap the dispatch in `std::panic::catch_unwind` and return a proper JSON-RPC error response.

---

### 16. MEDIUM: Backend Unwrap in Production Path
**File:** `codebase/compiler/src/bin/debug_actor.rs:66`
```rust
let mut cg = CraneliftCodegen::new().expect("CraneliftCodegen::new");
```
**Severity:** MEDIUM
**Explanation:** While this is a debug binary, similar patterns in production would be problematic. The error message is also not descriptive.
**Proposed Fix:** Return proper error codes.

---

## Error Propagation Gaps

### 17. HIGH: IR Builder Accumulates Errors But Doesn't Halt
**File:** `codebase/compiler/src/ir/builder/mod.rs` (multiple locations)
**Explanation:** The IR builder has an `errors: Vec<String>` field that collects errors, but the `build()` function doesn't return `Result`. Callers must remember to check `builder.errors` or risk generating invalid IR.
**Proposed Fix:** Change `build()` to return `Result<IrModule, Vec<IrError>>`.

---

### 18. MEDIUM: Typechecker Errors Collected But Not Propagated
**File:** `codebase/compiler/src/typechecker/checker.rs`
**Explanation:** Similar to the IR builder, the typechecker collects errors but the API doesn't enforce checking them. This can lead to generating code from an invalid AST.
**Proposed Fix:** Make `check()` return `Result<TypecheckedModule, Vec<TypeError>>`.

---

### 19. MEDIUM: Query System Unwrap on JSON Serialization
**File:** `codebase/compiler/src/query.rs:4678`
```rust
.expect("project index JSON should be valid")
```
**Severity:** LOW-MEDIUM
**Explanation:** In test code, but the production query system should handle serialization failures gracefully.
**Proposed Fix:** Return `Result<String, serde_json::Error>`.

---

## Invariant Violations Not Enforced

### 20. HIGH: IR Builder String Function Lookup Assumes Registration
**File:** `codebase/compiler/src/ir/builder/mod.rs:2420-2446`
```rust
self.function_refs.get("int_to_string").copied().unwrap();
self.function_refs.get("float_to_string").copied().unwrap();
self.function_refs.get("bool_to_string").copied().unwrap();
```
**Severity:** HIGH
**Explanation:** Three consecutive unwraps for string conversion functions. If the runtime doesn't register these, the compiler crashes.
**Proposed Fix:** Create a `RuntimeFunctions` struct that validates all required functions at startup.

---

### 21. MEDIUM: Wasm Codegen Value Map Lookup
**File:** `codebase/compiler/src/codegen/wasm.rs:486`
```rust
builder.instruction(&WasmInstr::LocalSet(*value_map.get(result).unwrap()));
```
**Severity:** MEDIUM
**Explanation:** If the IR value wasn't properly emitted, this panics. Should return a codegen error.
**Proposed Fix:**
```rust
let local_idx = value_map.get(result)
    .ok_or(CodegenError::UnknownValue { id: result.0 })?;
```

---

## Test-Only Issues (Lower Priority)

The following files contain extensive unwrap/expect/panic usage that is acceptable in tests but should be audited:

- `codebase/build-system/src/lockfile.rs` (lines 395-635) - 20+ unwraps in tests
- `codebase/build-system/src/resolver.rs` (lines 701-882) - 30+ unwraps in tests
- `codebase/compiler/src/parser/tests.rs` - 50+ panics in pattern matching
- `codebase/compiler/src/ir/builder/tests.rs` - 10+ expects

---

## Recommendations Summary

### Immediate Actions (Before Next Release)
1. Fix the 4 registry resolution `.expect()` calls in build-system
2. Fix the IR builder pre-registered function `.expect()` calls
3. Fix the Cranelift codegen `.unwrap()` on function lookup
4. Add panic recovery to the agent server loop

### Short-term (Next Sprint)
1. Audit all `vec[index]` patterns in production code
2. Convert IR builder to return `Result`
3. Convert typechecker to return `Result`
4. Fix the `Default` impl for `WasmBackend`

### Long-term (Technical Debt)
1. Create an internal invariant enforcement framework
2. Add panic-safe wrappers for all FFI boundaries
3. Implement proper async context propagation
4. Add property-based testing for error paths

---

## Appendix: Full File:Line List

### Critical (.expect/.unwrap in production paths)
```
codebase/build-system/src/resolver.rs:539
codebase/build-system/src/commands/add.rs:231
codebase/build-system/src/commands/fetch.rs:153
codebase/build-system/src/commands/update.rs:164
codebase/compiler/src/ir/builder/mod.rs:1487
codebase/compiler/src/ir/builder/mod.rs:1796
codebase/compiler/src/ir/builder/mod.rs:1986
codebase/compiler/src/ir/builder/mod.rs:2070
codebase/compiler/src/ir/builder/mod.rs:2800
codebase/compiler/src/ir/builder/mod.rs:2835
codebase/compiler/src/ir/builder/mod.rs:3094
codebase/compiler/src/ir/builder/mod.rs:3474
codebase/compiler/src/ir/builder/mod.rs:3512
codebase/compiler/src/ir/builder/mod.rs:3576
codebase/compiler/src/ir/builder/mod.rs:3627
codebase/compiler/src/ir/builder/mod.rs:3641
codebase/compiler/src/ir/builder/mod.rs:3655
codebase/compiler/src/ir/builder/mod.rs:3866
codebase/compiler/src/ir/builder/mod.rs:4082
codebase/compiler/src/ir/builder/mod.rs:4394
codebase/compiler/src/ir/builder/mod.rs:4403
codebase/compiler/src/ir/builder/mod.rs:2423
codebase/compiler/src/ir/builder/mod.rs:2432
codebase/compiler/src/ir/builder/mod.rs:2441
codebase/compiler/src/ir/builder/mod.rs:2476
codebase/compiler/src/codegen/cranelift.rs:6703
codebase/compiler/src/codegen/wasm.rs:1003
codebase/compiler/src/codegen/wasm.rs:486
codebase/compiler/src/agent/protocol.rs:80
codebase/compiler/src/main.rs:257
```

### High (Direct indexing, collections)
```
codebase/compiler/src/lexer/lexer.rs:372
codebase/compiler/src/lexer/lexer.rs:384
codebase/compiler/src/lexer/lexer.rs:393
codebase/compiler/src/resolve.rs:328
codebase/build-system/src/commands/add.rs:41
codebase/compiler/src/ir/builder/mod.rs:2475
codebase/compiler/src/ir/builder/mod.rs:1816
codebase/compiler/src/typechecker/checker.rs:3156
codebase/compiler/src/typechecker/checker.rs:3183
codebase/compiler/src/typechecker/checker.rs:3224
```

### FFI/Async Safety
```
codebase/build-system/src/commands/add.rs:238-243
codebase/compiler/src/agent/server.rs:35-93
codebase/compiler/src/codegen/mod.rs:160
codebase/compiler/src/codegen/mod.rs:205
```

---

**End of Review**
