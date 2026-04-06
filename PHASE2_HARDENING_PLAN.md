# Phase 2: Harden Thesis-Essential Core - Analysis & Plan

**Date:** 2026-04-05  
**Status:** Analysis Complete, Implementation Ready  
**Based on:** Adversarial synthesis recommendations

---

## Current State Analysis

### 1. Parser Stability

**What's Working:**
- Basic error recovery infrastructure exists (`synchronize()`, `synchronize_to_any()`)
- Error tracking accumulates multiple errors
- Can recover to statement boundaries

**Issues Found:**
- **Critical:** Error recovery functions marked with `#[allow(dead_code)]` - they're NOT actually used
- Recovery is only called in limited places (lines 848, 1451, 1946, 1970, 2767, 5013)
- No systematic recovery at every error point
- No recovery test coverage

**Hardening Needed:**
1. Activate error recovery at all parse error points
2. Add resync after failed top-level item parsing
3. Add parser recovery test suite
4. Test multi-error reporting (currently most tests check single errors)

---

### 2. Typechecker Semantic Hardening

**What's Working:**
- 1,030 tests passing
- Full type inference
- Generic type parameters
- Effect tracking

**Issues Found:**
- Exhaustiveness checking only covers enums and bools
- Missing: integer range exhaustiveness, string pattern exhaustiveness
- No semantic tests for edge cases in trait dispatch
- Limited tests for generic inference edge cases

**Hardening Needed:**
1. Expand exhaustiveness to cover all scalar types
2. Add negative tests (should-fail cases)
3. Add semantic edge case tests
4. Test error message quality

---

### 3. ADT/Match Correctness

**What's Working:**
- Enum exhaustiveness checking (line 4460-4486 in checker.rs)
- Bool exhaustiveness checking (line 4487-4504)
- Variant pattern binding
- OR patterns support (line 4363-4383)

**Issues Found:**
- Tuple patterns NOT supported in match (line 4238-4243) - explicit error only
- No exhaustiveness for nested patterns
- No reachability analysis for OR patterns
- Missing: integer literal exhaustiveness (e.g., matching 0, 1, 2 on Int)

**Hardening Needed:**
1. Verify exhaustiveness logic for all pattern types
2. Add tests for complex nested patterns
3. Add tests for OR pattern reachability
4. Add tests for guard interaction with exhaustiveness

---

### 4. Effects/Contracts Integration

**What's Working:**
- Effect inference (371 tests)
- Effect polymorphism (lines 1983-2100 in tests.rs)
- `@requires`/`@ensures` parsing and storage
- Effect tracking in query API

**Issues Found:**
- **No integration tests** verifying runtime contract checking
- Contracts are parsed but runtime checking is unclear
- No tests for contract violation at runtime
- Effect summary exists but not comprehensively tested

**Hardening Needed:**
1. Add end-to-end contract tests (compile + run + verify violation)
2. Test effect propagation through generic functions
3. Test effect summary accuracy via Query API
4. Add tests for effect errors vs warnings

---

### 5. Query API Stability (Best Differentiator)

**What's Working:**
- 5,266 lines of implementation
- Comprehensive API: `Session::from_source()`, `check()`, `symbols()`, `type_at()`
- JSON serialization for all outputs
- Used by REPL (lines 192, 200, 231, 311, 320, 360 in repl.rs)
- 20+ inline tests (starting line 2952)

**Issues Found:**
- Tests are inline in query.rs, not in dedicated test file
- No test for `from_file()` with imports
- Limited tests for error recovery in queries
- No tests for query performance/ caching behavior
- No `agent-mode` CLI wrapper (synthesis recommendation)

**Hardening Needed:**
1. Add dedicated query tests file
2. Test all query methods comprehensively
3. Test import resolution in queries
4. Add tests for partial success (parse ok, typecheck fails)
5. Document query API as primary agent interface

---

## Implementation Priority

### P0: Critical Gaps (Must Fix)

1. **Parser Error Recovery Activation**
   - Remove `#[allow(dead_code)]` from recovery functions
   - Add systematic `synchronize()` calls at all error points
   - Add recovery test suite

2. **Query API Test Hardening**
   - Move tests to dedicated file
   - Add comprehensive coverage
   - Document as primary agent interface

3. **Effects/Contracts Runtime Verification**
   - Add end-to-end contract tests
   - Verify runtime actually checks contracts

### P1: Important Improvements

4. **Exhaustiveness Expansion**
   - Add tests for edge cases
   - Verify OR pattern logic
   - Test guard interaction

5. **Semantic Edge Cases**
   - Add negative test cases
   - Test error message quality

### P2: Nice to Have

6. **Performance/Caching Tests**
7. **Documentation Improvements**

---

## Specific Commits to Make

### Commit 1: Parser Error Recovery Activation
```
feat(parser): activate error recovery system

- Remove #[allow(dead_code)] from synchronize functions
- Add recovery calls at all parse error points
- Add recovery test suite in parser/tests.rs
```

### Commit 2: Query API Test Hardening
```
test(query): add comprehensive query API tests

- Move inline tests to dedicated query/tests.rs
- Add tests for from_file with imports
- Add tests for partial success scenarios
- Document query API as primary agent interface
```

### Commit 3: Effects/Contracts Integration Tests
```
test(effects,contracts): add runtime integration tests

- Add end-to-end contract verification tests
- Test effect summary accuracy
- Verify runtime contract checking
```

### Commit 4: Match Exhaustiveness Hardening
```
test(match): add exhaustiveness edge case tests

- Test OR pattern reachability
- Test guard interaction
- Add negative exhaustiveness tests
```

---

## Success Criteria

- [ ] Parser recovery functions activated and tested
- [ ] Query API has dedicated test file with >30 tests
- [ ] Contract runtime verification tests pass
- [ ] Match exhaustiveness has comprehensive coverage
- [ ] All new tests pass (target: 1,100+ total)

---

## Files to Modify

1. `codebase/compiler/src/parser/parser.rs` - Activate recovery
2. `codebase/compiler/src/parser/tests.rs` - Add recovery tests
3. `codebase/compiler/src/query.rs` - Move tests out
4. `codebase/compiler/src/query/tests.rs` - Create (NEW)
5. `codebase/compiler/src/typechecker/tests.rs` - Add match/contract tests
6. `codebase/compiler/tests/` - Add integration tests

---

## Testing Commands

```bash
cargo test --release -p gradient-compiler --lib parser::tests
cargo test --release -p gradient-compiler --lib query::tests
cargo test --release -p gradient-compiler --lib typechecker::tests
cargo test --release --test "*integration*"
```

---

## Open Questions

1. Should we add `agent-mode` CLI flag as synthesis recommends?
2. Should parser recovery be the default or opt-in?
3. How to verify contract runtime checking actually happens?

---

**Next Step:** Begin with Commit 1 - Parser Error Recovery Activation
