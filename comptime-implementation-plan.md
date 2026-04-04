# Gradient Comptime Metaprogramming Implementation Plan

## Overview
Implement Zig-style compile-time metaprogramming with `comptime` keyword and type-as-value system.

**Parallel Workstreams:** 3 concurrent

---

## Workstream 1: Parser & AST - Comptime Syntax Support
**Goal:** Add `comptime` keyword and parse comptime function parameters

**Files:**
- `codebase/compiler/src/lexer/mod.rs` - Add `comptime` keyword
- `codebase/compiler/src/parser/` - Update grammar and parsing
- `codebase/compiler/src/ast/item.rs` - Add comptime marker to Param
- `codebase/compiler/src/ast/types.rs` - Add TypeExpr::Type

**Tasks:**
1. Add `comptime` keyword to lexer:
   - Add to `TokenKind::Comptime` variant
   - Add to keyword recognition in lexer

2. Update AST structures:
   - In `ast/item.rs`, add `comptime: bool` field to `Param` struct
   - In `ast/types.rs`, add `Type` variant to `TypeExpr` enum
   - Update `Display` impls for new syntax

3. Update parser to handle:
   - `comptime` before parameter: `fn foo(comptime T: type)`
   - `type` as type expression (special case)
   - Parse function return type `-> type` for type constructors

4. Add tests for:
   - Parsing `fn Vector(comptime T: type, comptime N: usize) -> type`
   - Parsing `comptime` keyword in various positions
   - Error recovery for malformed comptime syntax

**Deliverable:** Can parse `comptime` parameters and `type` type expression

---

## Workstream 2: Type System - Ty::Type and Comptime Tracking
**Goal:** Add internal type representation for types-as-values and comptime tracking

**Files:**
- `codebase/compiler/src/typechecker/types.rs` - Add Ty::Type
- `codebase/compiler/src/typechecker/env.rs` - Add comptime tracking
- `codebase/compiler/src/typechecker/checker.rs` - Check comptime constraints

**Tasks:**
1. Add `Ty::Type` variant:
   ```rust
   pub enum Ty {
       // ... existing variants
       /// A type value (only valid at compile time).
       Type,
   }
   ```
   - Update `Display for Ty`
   - Add `is_comptime_only()` method returning true for Ty::Type

2. Add comptime tracking to type environment:
   - In `env.rs`, update `Binding` struct:
     ```rust
     pub struct Binding {
         pub ty: Ty,
         pub mutable: bool,
         pub comptime: bool,  // NEW
     }
     ```
   - Update all `add_binding` calls to accept comptime parameter
   - Add `is_comptime_known(&self, name: &str) -> bool` method

3. Update type checker for comptime:
   - Add `comptime` field to `TypeChecker` struct
   - Add `require_comptime(&mut self, expr: &Expr) -> Result<(), TypeError>`
   - In `check_fn_call`, check comptime args match params
   - Add error: "Expected comptime value, found runtime value"

4. Add `type` type resolution:
   - When resolving `TypeExpr::Type`, return `Ty::Type`
   - Ensure `type` can only be used in comptime contexts

5. Add tests:
   - Type checking `comptime T: type` parameter
   - Error on passing runtime value to comptime param
   - Tracking comptime through let bindings

**Deliverable:** Type system can represent types as values and track comptime

---

## Workstream 3: Comptime Evaluator - Compile-Time Execution
**Goal:** Create comptime evaluator for executing functions at compile time

**Files:**
- `codebase/compiler/src/comptime/mod.rs` - New module
- `codebase/compiler/src/comptime/evaluator.rs` - Core evaluator
- `codebase/compiler/src/comptime/value.rs` - Comptime values
- `codebase/compiler/src/typechecker/checker.rs` - Integration

**Tasks:**
1. Create comptime module structure:
   ```rust
   // comptime/mod.rs
   pub mod evaluator;
   pub mod value;
   
   pub use evaluator::ComptimeEvaluator;
   pub use value::ComptimeValue;
   ```

2. Create `ComptimeValue` enum:
   ```rust
   pub enum ComptimeValue {
       Type(Ty),
       Int(i64),
       Float(f64),
       Bool(bool),
       String(String),
       Unit,
       Error(String),
   }
   ```
   - Add `to_ty(&self) -> Option<Ty>` for type constructors
   - Add `to_const_expr(&self) -> Expr` for IR generation

3. Create `ComptimeEvaluator`:
   ```rust
   pub struct ComptimeEvaluator {
       env: HashMap<String, ComptimeValue>,
   }
   
   impl ComptimeEvaluator {
       pub fn new() -> Self;
       
       /// Evaluate a function at compile time
       pub fn eval_fn(
           &mut self,
           fn_def: &FnDef,
           comptime_args: HashMap<String, ComptimeValue>,
       ) -> Result<ComptimeValue, ComptimeError>;
       
       /// Evaluate an expression
       pub fn eval_expr(&mut self, expr: &Expr) -> Result<ComptimeValue, ComptimeError>;
   }
   ```

4. Implement expression evaluation:
   - Literals: return corresponding ComptimeValue
   - Variables: lookup in env
   - Binary ops: evaluate both sides, apply op
   - If: evaluate condition, then eval appropriate branch
   - Match: evaluate scrutinee, find matching arm
   - Block: eval statements, return last expression
   - Call: if comptime function, eval recursively

5. Integrate with type checker:
   - Add `comptime_eval: ComptimeEvaluator` to `TypeChecker`
   - In `check_fn_call` for type constructors:
     ```rust
     if ret_type == Ty::Type {
         // Evaluate at compile time
         let result = self.comptime_eval.eval_fn(fn_def, comptime_args)?;
         return result.to_ty().unwrap_or(Ty::Error);
     }
     ```

6. Add tests:
   - Evaluate `fn fib(comptime n: usize) -> usize`
   - Evaluate `fn Vector(comptime T: type, comptime N: usize) -> type`
   - Nested comptime calls
   - Error handling in comptime evaluation

**Deliverable:** Can execute functions at compile time and get results

---

## Integration Points

### Between Workstreams 1 & 2:
- Workstream 1 provides parsed `comptime` markers on params
- Workstream 2 uses them to check comptime constraints

### Between Workstreams 2 & 3:
- Workstream 2 provides comptime-validated values
- Workstream 3 evaluates them and returns results

### All Workstreams → Main:
- Must add `mod comptime;` to `compiler/src/lib.rs` or `main.rs`
- Must integrate comptime evaluator into type checking pipeline

---

## Testing Checklist

- [ ] Parse `comptime` keyword
- [ ] Parse `type` type expression
- [ ] Parse `fn Foo(comptime T: type)`
- [ ] Type check comptime parameter binding
- [ ] Error on runtime value passed to comptime param
- [ ] Evaluate simple comptime function (factorial)
- [ ] Evaluate type constructor returning struct
- [ ] Nested comptime evaluation
- [ ] Comptime in generic contexts
- [ ] IR generation for unfolded types

---

## Example Programs (Test Cases)

```gradient
# Test 1: Simple comptime function
fn fib(comptime n: usize) -> usize:
    if n <= 1: ret n
    ret fib(n - 1) + fib(n - 2)

let arr: [Int; fib(10)]  # Should create [Int; 55]

---

# Test 2: Type constructor
fn Vector(comptime T: type, comptime N: usize) -> type:
    struct {
        data: [T; N]
        len: usize
    }

let v: Vector(Int, 10)  # Should unfold to struct with [Int; 10]

---

# Test 3: Conditional compilation
fn choose_type(comptime use_float: bool) -> type:
    if use_float: ret Float
    ret Int

let x: choose_type(true) = 3.14  # x is Float

---

# Test 4: Error case
fn runtime_fn() -> usize: ret 10

let bad: [Int; runtime_fn()]  # ERROR: runtime value in comptime context
```

---

## File Structure

```
codebase/compiler/src/
├── lexer/mod.rs          # Add comptime keyword
├── parser/
│   ├── grammar.pest      # Update grammar
│   └── mod.rs            # Parse comptime params
├── ast/
│   ├── item.rs           # Add comptime marker
│   └── types.rs          # Add TypeExpr::Type
├── typechecker/
│   ├── types.rs          # Add Ty::Type
│   ├── env.rs            # Add comptime tracking
│   └── checker.rs        # Comptime integration
├── comptime/             # NEW MODULE
│   ├── mod.rs
│   ├── evaluator.rs      # ComptimeEvaluator
│   └── value.rs          # ComptimeValue
└── ir/builder/mod.rs     # Handle unfolded types
```

---

## Risk Mitigation

1. **Recursive comptime evaluation:** Add recursion limit (e.g., 1000 calls)
2. **Side effects in comptime:** Ban I/O effects in comptime contexts
3. **Performance:** Cache comptime evaluation results
4. **Type constructor errors:** Provide clear errors for unsupported constructs

---

## Self-Hosting Path

After comptime is complete, Gradient can express its own compiler:

```gradient
# Type system in Gradient
fn compile_type(comptime t: type) -> IRType:
    match t:
        Int: IRType::I64
        Float: IRType::F64
        Struct { name, fields }:
            IRType::Struct {
                name: name,
                fields: compile_fields(fields)
            }
        _:
            panic("Unsupported type: " + type_name(t))
```

This is the foundation for writing the Gradient compiler in Gradient itself.
