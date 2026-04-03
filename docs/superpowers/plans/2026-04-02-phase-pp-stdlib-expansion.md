# Phase PP: Standard Library Expansion — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand Gradient's stdlib from 62+ to ~134 builtins across math, string, data structures, date/time, env/process, recursive union types, and JSON.

**Architecture:** All new builtins follow the existing 4-layer pattern: (1) register type signature in `typechecker/env.rs`, (2) register function + return type in `ir/builder/mod.rs`, (3) declare C function + implement codegen match arm in `codegen/cranelift.rs`, (4) add C helper in `runtime/gradient_runtime.c` if needed. New container types (Set, Queue, Stack) also need `Ty` enum variants and type-checker recognition.

**Tech Stack:** Rust (compiler), Cranelift (codegen), C (runtime), libm (math), POSIX APIs (time, process, env)

---

## File Map

| File | Path (relative to `codebase/compiler/`) | Changes |
|------|----------------------------------------|---------|
| Type checker env | `src/typechecker/env.rs` | Register ~72 new builtin FnSigs in `preload_builtins()` |
| Type checker types | `src/typechecker/types.rs` | Add `Set(Box<Ty>)`, `Queue(Box<Ty>)`, `Stack(Box<Ty>)` to `Ty` enum + Display |
| Type checker | `src/typechecker/checker.rs` | Handle Set/Queue/Stack in `resolve_type_expr`, recursive type detection |
| AST types | `src/ast/types.rs` | No changes needed — uses `TypeExpr::Generic` which already handles new names |
| IR builder | `src/ir/builder/mod.rs` | Register ~72 new functions + return types, update string/list tracking |
| Codegen | `src/codegen/cranelift.rs` | Declare C functions + ~72 match arms |
| C runtime | `runtime/gradient_runtime.c` | String helpers, Set/Queue/Stack, time, env/process, JSON parser |
| Tests | `tests/phase_pp_integration.rs` | ~105 integration tests |
| Roadmap | `../../docs/roadmap.md` | Document Phase PP |

**Important:** All file paths in this plan are relative to `codebase/compiler/` unless otherwise noted.

---

## Conventions

**Adding a new builtin** requires changes in 4 files, always in this order:

1. **`src/typechecker/env.rs`** — Add `self.define_fn(...)` in `preload_builtins()` (after line 1050)
2. **`src/ir/builder/mod.rs`** — Add `self.register_func("name"); self.function_return_types.insert(...)` in `register_functions()` (after line 490)
3. **`src/codegen/cranelift.rs`** — Declare C function signature (in declaration section, ~line 700+), add match arm in Call handler
4. **`runtime/gradient_runtime.c`** — Add C helper function (if needed)

**IR return type mapping:**
- `Int` → `Type::I64`
- `Float` → `Type::F64`
- `Bool` → `Type::Bool`
- `String` → `Type::Ptr`
- `()` → `Type::Void`
- `List[T]`, `Map[K,V]`, `Option[T]`, `Result[T,E]`, `Set[T]`, `Queue[T]`, `Stack[T]` → `Type::Ptr`
- Tuples → `Type::Ptr`

**String/list tracking in IR builder** (`src/ir/builder/mod.rs` ~line 1524): Builtins returning `String` must be added to the `string_values.insert(result)` match. Builtins returning `List[T]` must be added to `list_values.insert(result)`.

---

## Task 1: Test Infrastructure

**Files:**
- Create: `tests/phase_pp_integration.rs`

- [ ] **Step 1: Create test file with compile_and_run helper**

```rust
//! Phase PP integration tests — Standard Library Expansion
//!
//! Tests for: math, string, data structures, date/time, env/process, JSON.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::TempDir;

use gradient_compiler::codegen::CraneliftCodegen;
use gradient_compiler::ir::IrBuilder;
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker;

/// Compile Gradient source and run, returning (stdout, exit_code).
fn compile_and_run(src: &str) -> (String, i32) {
    compile_and_run_with_stdin(src, None)
}

/// Compile Gradient source and run with optional stdin, returning (stdout, exit_code).
fn compile_and_run_with_stdin(src: &str, stdin_input: Option<&[u8]>) -> (String, i32) {
    let tmp = TempDir::new().expect("failed to create temp dir");

    // 1. Lex
    let mut lexer = Lexer::new(src, 0);
    let tokens = lexer.tokenize();

    // 2. Parse
    let (ast_module, parse_errors) = parser::parse(tokens, 0);
    assert!(
        parse_errors.is_empty(),
        "parse errors: {:?}",
        parse_errors
    );

    // 3. Type check
    let type_errors = typechecker::check_module(&ast_module, 0);
    let real_errors: Vec<_> = type_errors.iter().filter(|e| !e.is_warning).collect();
    assert!(
        real_errors.is_empty(),
        "type errors: {:?}",
        real_errors
    );

    // 4. IR
    let (ir_module, ir_errors) = IrBuilder::build_module(&ast_module);
    assert!(ir_errors.is_empty(), "IR errors: {:?}", ir_errors);

    // 5. Codegen
    let mut cg = CraneliftCodegen::new().expect("CraneliftCodegen::new");
    cg.compile_module(&ir_module).expect("compile_module");
    let obj_bytes = cg.emit_bytes().expect("emit_bytes");

    // 6. Write object file
    let obj_path = tmp.path().join("out.o");
    let bin_path = tmp.path().join("out");
    fs::write(&obj_path, &obj_bytes).expect("write .o");

    // 7. Compile runtime + link
    let runtime_src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("gradient_runtime.c");
    let runtime_obj = tmp.path().join("gradient_runtime.o");
    let cc_status = Command::new("cc")
        .arg("-c")
        .arg(&runtime_src)
        .arg("-o")
        .arg(&runtime_obj)
        .arg("-lm")
        .status()
        .expect("cc compile runtime");
    assert!(cc_status.success(), "runtime compile failed");

    let link_status = Command::new("cc")
        .arg(&obj_path)
        .arg(&runtime_obj)
        .arg("-o")
        .arg(&bin_path)
        .arg("-lm")
        .status()
        .expect("cc link");
    assert!(link_status.success(), "link failed");

    // 8. Run
    let output = if let Some(input_bytes) = stdin_input {
        let mut child = Command::new(&bin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn binary");
        child.stdin.as_mut().unwrap().write_all(input_bytes).expect("write stdin");
        child.wait_with_output().expect("wait_with_output")
    } else {
        Command::new(&bin_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run binary")
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration --no-run`
Expected: Compiles successfully (no tests to run yet)

- [ ] **Step 3: Commit**

```bash
git add tests/phase_pp_integration.rs
git commit -m "feat(phase-pp): add integration test infrastructure"
```

---

## Task 2: Math — Trigonometric, Logarithmic, Rounding, Constants

**Files:**
- Modify: `src/typechecker/env.rs`
- Modify: `src/ir/builder/mod.rs`
- Modify: `src/codegen/cranelift.rs`
- Modify: `tests/phase_pp_integration.rs`

These are all pure functions calling libm. No C runtime helpers needed.

- [ ] **Step 1: Write failing tests**

Add to `tests/phase_pp_integration.rs`:

```rust
// ── Math: Trigonometric ─────────────────────────────────────────────

#[test]
fn test_sin() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let x: Float = sin(0.0)
    print_float(x)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.starts_with("0.0"), "sin(0.0) should be 0.0, got: {}", out);
}

#[test]
fn test_cos() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let x: Float = cos(0.0)
    print_float(x)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.starts_with("1.0"), "cos(0.0) should be 1.0, got: {}", out);
}

#[test]
fn test_tan() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let x: Float = tan(0.0)
    print_float(x)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.starts_with("0.0"), "tan(0.0) should be 0.0, got: {}", out);
}

#[test]
fn test_atan2() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let x: Float = atan2(1.0, 1.0)
    print_float(x)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    // atan2(1,1) = pi/4 ≈ 0.785398
    assert!(out.starts_with("0.785"), "atan2(1,1) ≈ 0.785, got: {}", out);
}

// ── Math: Logarithmic/Exponential ───────────────────────────────────

#[test]
fn test_log_exp() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: Float = log(1.0)
    print_float(a)
    println("")
    let b: Float = exp(0.0)
    print_float(b)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("0.0"), "log(1.0) should be 0.0, got: {}", lines[0]);
    assert!(lines[1].starts_with("1.0"), "exp(0.0) should be 1.0, got: {}", lines[1]);
}

#[test]
fn test_log2_log10() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: Float = log2(8.0)
    print_float(a)
    println("")
    let b: Float = log10(100.0)
    print_float(b)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("3.0"), "log2(8) should be 3.0, got: {}", lines[0]);
    assert!(lines[1].starts_with("2.0"), "log10(100) should be 2.0, got: {}", lines[1]);
}

// ── Math: Rounding ──────────────────────────────────────────────────

#[test]
fn test_floor_ceil_round() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: Float = floor(3.7)
    print_float(a)
    println("")
    let b: Float = ceil(3.2)
    print_float(b)
    println("")
    let c: Float = round(3.5)
    print_float(c)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("3.0"), "floor(3.7) should be 3.0, got: {}", lines[0]);
    assert!(lines[1].starts_with("4.0"), "ceil(3.2) should be 4.0, got: {}", lines[1]);
    assert!(lines[2].starts_with("4.0"), "round(3.5) should be 4.0, got: {}", lines[2]);
}

// ── Math: Constants ─────────────────────────────────────────────────

#[test]
fn test_math_pi_e() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let pi: Float = math_pi()
    print_float(pi)
    println("")
    let e: Float = math_e()
    print_float(e)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("3.14159"), "pi should start with 3.14159, got: {}", lines[0]);
    assert!(lines[1].starts_with("2.71828"), "e should start with 2.71828, got: {}", lines[1]);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_sin -- --nocapture 2>&1 | head -20`
Expected: FAIL (type error: unknown function `sin`)

- [ ] **Step 3: Register all math builtins in env.rs**

Add to `preload_builtins()` in `src/typechecker/env.rs`, after the last `map_keys` registration (~line 1050):

```rust
        // ── Phase PP: Math — Trigonometric (all Float -> Float, pure) ────
        for &name in &["sin", "cos", "tan", "asin", "acos", "atan"] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("x".into(), Ty::Float)],
                    ret: Ty::Float,
                    effects: vec![],
                },
            );
        }

        // atan2(Float, Float) -> Float
        self.define_fn(
            "atan2".into(),
            FnSig {
                type_params: vec![],
                params: vec![("y".into(), Ty::Float), ("x".into(), Ty::Float)],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // ── Phase PP: Math — Logarithmic/Exponential (all Float -> Float, pure) ──
        for &name in &["log", "log2", "log10", "exp"] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("x".into(), Ty::Float)],
                    ret: Ty::Float,
                    effects: vec![],
                },
            );
        }

        // ── Phase PP: Math — Rounding (all Float -> Float, pure) ─────────
        for &name in &["floor", "ceil", "round"] {
            self.define_fn(
                name.into(),
                FnSig {
                    type_params: vec![],
                    params: vec![("x".into(), Ty::Float)],
                    ret: Ty::Float,
                    effects: vec![],
                },
            );
        }

        // ── Phase PP: Math — Constants ───────────────────────────────────
        // math_pi() -> Float
        self.define_fn(
            "math_pi".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Float,
                effects: vec![],
            },
        );

        // math_e() -> Float
        self.define_fn(
            "math_e".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Float,
                effects: vec![],
            },
        );
```

- [ ] **Step 4: Register in IR builder**

Add to `register_functions()` in `src/ir/builder/mod.rs`, after the `map_keys` registration (~line 490):

```rust
        // ── Phase PP: Math — libm functions ──────────────────────────────
        for &name in &["sin", "cos", "tan", "asin", "acos", "atan", "atan2",
                        "log", "log2", "log10", "exp", "floor", "ceil", "round"] {
            self.register_func(name);
            self.function_return_types.insert(name.to_string(), Type::F64);
        }
        for &name in &["math_pi", "math_e"] {
            self.register_func(name);
            self.function_return_types.insert(name.to_string(), Type::F64);
        }
```

- [ ] **Step 5: Declare libm functions in codegen**

Add to the function declaration section in `src/codegen/cranelift.rs` (~line 700, near the map declarations):

```rust
        // ── Phase PP: Math — libm functions ──────────────────────────────
        // All are f64 -> f64 except atan2 which is (f64, f64) -> f64
        for &name in &["sin", "cos", "tan", "asin", "acos", "atan",
                        "log", "log2", "log10", "exp", "floor", "ceil", "round"] {
            if !self.declared_functions.contains_key(name) {
                let mut sig = self.module.make_signature();
                sig.params.push(AbiParam::new(cl_types::F64));
                sig.returns.push(AbiParam::new(cl_types::F64));
                let func_id = self.module
                    .declare_function(name, Linkage::Import, &sig)
                    .map_err(|e| format!("Failed to declare {}: {}", name, e))?;
                self.declared_functions.insert(name.to_string(), func_id);
            }
        }
        if !self.declared_functions.contains_key("atan2") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self.module
                .declare_function("atan2", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare atan2: {}", e))?;
            self.declared_functions.insert("atan2".to_string(), func_id);
        }
```

- [ ] **Step 6: Implement codegen match arms**

Add match arms in the `Call` instruction handler in `src/codegen/cranelift.rs`:

```rust
                        // ── Phase PP: Math — libm (f64 -> f64) ──────────────
                        "sin" | "cos" | "tan" | "asin" | "acos" | "atan"
                        | "log" | "log2" | "log10" | "exp"
                        | "floor" | "ceil" | "round" => {
                            let x = resolve_value(&value_map, &args[0])?;
                            let func_id = *self.declared_functions.get(name)
                                .ok_or(format!("{} not declared", name))?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[x]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        // atan2(y, x) -> f64
                        "atan2" => {
                            let y = resolve_value(&value_map, &args[0])?;
                            let x = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("atan2")
                                .ok_or("atan2 not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[y, x]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        // math_pi() -> f64, math_e() -> f64
                        "math_pi" => {
                            let result = builder.ins().f64const(std::f64::consts::PI);
                            value_map.insert(*dst, result);
                        }
                        "math_e" => {
                            let result = builder.ins().f64const(std::f64::consts::E);
                            value_map.insert(*dst, result);
                        }
```

- [ ] **Step 7: Run tests to verify pass**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration -- --nocapture 2>&1 | tail -20`
Expected: All math tests pass

- [ ] **Step 8: Commit**

```bash
git add src/typechecker/env.rs src/ir/builder/mod.rs src/codegen/cranelift.rs tests/phase_pp_integration.rs
git commit -m "feat(phase-pp): add math builtins — trig, log, rounding, constants"
```

---

## Task 3: Math — Random, GCD, Float Mod

**Files:**
- Modify: `src/typechecker/env.rs`
- Modify: `src/ir/builder/mod.rs`
- Modify: `src/codegen/cranelift.rs`
- Modify: `runtime/gradient_runtime.c`
- Modify: `tests/phase_pp_integration.rs`

- [ ] **Step 1: Write failing tests**

Add to `tests/phase_pp_integration.rs`:

```rust
#[test]
fn test_random_int() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let x: Int = random_int(1, 10)
    let valid: Bool = x >= 1 and x <= 10
    print_bool(valid)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "true");
}

#[test]
fn test_random_float() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let x: Float = random_float()
    let valid: Bool = x >= 0.0 and x < 1.0
    print_bool(valid)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "true");
}

#[test]
fn test_gcd() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: Int = gcd(12, 8)
    print_int(a)
    println("")
    let b: Int = gcd(7, 13)
    print_int(b)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "4");
    assert_eq!(lines[1], "1");
}

#[test]
fn test_float_mod() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let x: Float = float_mod(5.5, 2.0)
    print_float(x)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert!(out.starts_with("1.5"), "5.5 mod 2.0 should be 1.5, got: {}", out);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_gcd -- --nocapture 2>&1 | head -20`
Expected: FAIL

- [ ] **Step 3: Add C runtime helpers for random**

Add to `runtime/gradient_runtime.c`:

```c
/* ── Phase PP: Random number generation ────────────────────────────────── */

#include <time.h>

static int __gradient_rand_initialized = 0;

static void __gradient_ensure_rand_init(void) {
    if (!__gradient_rand_initialized) {
        srand((unsigned int)time(NULL));
        __gradient_rand_initialized = 1;
    }
}

/*
 * __gradient_random_int(min, max) -> int64_t
 * Returns a random integer in [min, max] inclusive.
 */
int64_t __gradient_random_int(int64_t min, int64_t max) {
    __gradient_ensure_rand_init();
    if (min > max) { int64_t tmp = min; min = max; max = tmp; }
    int64_t range = max - min + 1;
    return min + (int64_t)(rand() % (int)range);
}

/*
 * __gradient_random_float() -> double
 * Returns a random float in [0.0, 1.0).
 */
double __gradient_random_float(void) {
    __gradient_ensure_rand_init();
    return (double)rand() / ((double)RAND_MAX + 1.0);
}

/*
 * __gradient_gcd(a, b) -> int64_t
 * Euclidean GCD algorithm.
 */
int64_t __gradient_gcd(int64_t a, int64_t b) {
    if (a < 0) a = -a;
    if (b < 0) b = -b;
    while (b != 0) {
        int64_t t = b;
        b = a % b;
        a = t;
    }
    return a;
}
```

- [ ] **Step 4: Register in env.rs**

Add to `preload_builtins()`:

```rust
        // ── Phase PP: Math — Random ──────────────────────────────────────
        // random_int(Int, Int) -> !{IO} Int
        self.define_fn(
            "random_int".into(),
            FnSig {
                type_params: vec![],
                params: vec![("min".into(), Ty::Int), ("max".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec!["IO".into()],
            },
        );

        // random_float() -> !{IO} Float
        self.define_fn(
            "random_float".into(),
            FnSig {
                type_params: vec![],
                params: vec![],
                ret: Ty::Float,
                effects: vec!["IO".into()],
            },
        );

        // ── Phase PP: Math — Additional ──────────────────────────────────
        // gcd(Int, Int) -> Int
        self.define_fn(
            "gcd".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // float_mod(Float, Float) -> Float
        self.define_fn(
            "float_mod".into(),
            FnSig {
                type_params: vec![],
                params: vec![("a".into(), Ty::Float), ("b".into(), Ty::Float)],
                ret: Ty::Float,
                effects: vec![],
            },
        );
```

- [ ] **Step 5: Register in IR builder**

```rust
        // Phase PP: random, gcd, float_mod
        self.register_func("random_int");
        self.function_return_types.insert("random_int".to_string(), Type::I64);
        self.register_func("random_float");
        self.function_return_types.insert("random_float".to_string(), Type::F64);
        self.register_func("gcd");
        self.function_return_types.insert("gcd".to_string(), Type::I64);
        self.register_func("float_mod");
        self.function_return_types.insert("float_mod".to_string(), Type::F64);
```

- [ ] **Step 6: Declare C functions + codegen match arms**

Declarations:

```rust
        // __gradient_random_int(min: i64, max: i64) -> i64
        if !self.declared_functions.contains_key("__gradient_random_int") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self.module
                .declare_function("__gradient_random_int", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_random_int: {}", e))?;
            self.declared_functions.insert("__gradient_random_int".to_string(), func_id);
        }

        // __gradient_random_float() -> f64
        if !self.declared_functions.contains_key("__gradient_random_float") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self.module
                .declare_function("__gradient_random_float", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_random_float: {}", e))?;
            self.declared_functions.insert("__gradient_random_float".to_string(), func_id);
        }

        // __gradient_gcd(a: i64, b: i64) -> i64
        if !self.declared_functions.contains_key("__gradient_gcd") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self.module
                .declare_function("__gradient_gcd", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_gcd: {}", e))?;
            self.declared_functions.insert("__gradient_gcd".to_string(), func_id);
        }

        // fmod(a: f64, b: f64) -> f64  (from libm)
        if !self.declared_functions.contains_key("fmod") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.params.push(AbiParam::new(cl_types::F64));
            sig.returns.push(AbiParam::new(cl_types::F64));
            let func_id = self.module
                .declare_function("fmod", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare fmod: {}", e))?;
            self.declared_functions.insert("fmod".to_string(), func_id);
        }
```

Match arms:

```rust
                        "random_int" => {
                            let min_val = resolve_value(&value_map, &args[0])?;
                            let max_val = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("__gradient_random_int")
                                .ok_or("__gradient_random_int not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[min_val, max_val]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "random_float" => {
                            let func_id = *self.declared_functions.get("__gradient_random_float")
                                .ok_or("__gradient_random_float not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "gcd" => {
                            let a = resolve_value(&value_map, &args[0])?;
                            let b = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("__gradient_gcd")
                                .ok_or("__gradient_gcd not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[a, b]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "float_mod" => {
                            let a = resolve_value(&value_map, &args[0])?;
                            let b = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("fmod")
                                .ok_or("fmod not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[a, b]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }
```

- [ ] **Step 7: Run tests to verify pass**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration -- --nocapture 2>&1 | tail -20`
Expected: All math tests pass

- [ ] **Step 8: Commit**

```bash
git add src/typechecker/env.rs src/ir/builder/mod.rs src/codegen/cranelift.rs runtime/gradient_runtime.c tests/phase_pp_integration.rs
git commit -m "feat(phase-pp): add random, gcd, float_mod builtins"
```

---

## Task 4: String Utilities

**Files:**
- Modify: `src/typechecker/env.rs`
- Modify: `src/ir/builder/mod.rs`
- Modify: `src/codegen/cranelift.rs`
- Modify: `runtime/gradient_runtime.c`
- Modify: `tests/phase_pp_integration.rs`

- [ ] **Step 1: Write failing tests**

```rust
// ── String Utilities ────────────────────────────────────────────────

#[test]
fn test_string_join() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let parts: List[String] = ["hello", "world", "foo"]
    let joined: String = string_join(parts, ", ")
    println(joined)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "hello, world, foo");
}

#[test]
fn test_string_repeat() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let s: String = string_repeat("ab", 3)
    println(s)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "ababab");
}

#[test]
fn test_string_reverse() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let s: String = string_reverse("hello")
    println(s)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "olleh");
}

#[test]
fn test_string_pad() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let a: String = string_pad_left("42", 5, "0")
    println(a)
    let b: String = string_pad_right("hi", 5, ".")
    println(b)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "00042");
    assert_eq!(lines[1], "hi...");
}

#[test]
fn test_string_is_empty() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    print_bool(string_is_empty(""))
    println("")
    print_bool(string_is_empty("x"))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "true");
    assert_eq!(lines[1], "false");
}

#[test]
fn test_string_count() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let n: Int = string_count("abcabc", "bc")
    print_int(n)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "2");
}

#[test]
fn test_char_code_and_from_char_code() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let code: Int = char_code("A")
    print_int(code)
    println("")
    let ch: String = from_char_code(65)
    println(ch)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "65");
    assert_eq!(lines[1], "A");
}

#[test]
fn test_string_lines() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let lines: List[String] = string_lines("a\nb\nc")
    print_int(list_length(lines))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "3");
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_string_join -- --nocapture 2>&1 | head -20`
Expected: FAIL

- [ ] **Step 3: Add C runtime helpers**

Add to `runtime/gradient_runtime.c`:

```c
/* ── Phase PP: String utilities ────────────────────────────────────────── */

/*
 * __gradient_string_join(list, delim) -> char*
 * Joins a List[String] with delimiter. List layout: [len, cap, data...]
 */
char* __gradient_string_join(void* list, const char* delim) {
    int64_t* hdr = (int64_t*)list;
    int64_t len = hdr[0];
    int64_t* data = hdr + 2;
    if (len == 0) return strdup("");

    size_t delim_len = delim ? strlen(delim) : 0;
    /* Calculate total length */
    size_t total = 0;
    for (int64_t i = 0; i < len; i++) {
        const char* s = (const char*)(intptr_t)data[i];
        total += s ? strlen(s) : 0;
        if (i < len - 1) total += delim_len;
    }
    char* result = (char*)malloc(total + 1);
    char* p = result;
    for (int64_t i = 0; i < len; i++) {
        const char* s = (const char*)(intptr_t)data[i];
        if (s) { size_t slen = strlen(s); memcpy(p, s, slen); p += slen; }
        if (i < len - 1 && delim_len > 0) { memcpy(p, delim, delim_len); p += delim_len; }
    }
    *p = '\0';
    return result;
}

/*
 * __gradient_string_repeat(s, n) -> char*
 */
char* __gradient_string_repeat(const char* s, int64_t n) {
    if (!s || n <= 0) return strdup("");
    size_t slen = strlen(s);
    size_t total = slen * (size_t)n;
    char* result = (char*)malloc(total + 1);
    char* p = result;
    for (int64_t i = 0; i < n; i++) { memcpy(p, s, slen); p += slen; }
    *p = '\0';
    return result;
}

/*
 * __gradient_string_reverse(s) -> char*
 */
char* __gradient_string_reverse(const char* s) {
    if (!s) return strdup("");
    size_t len = strlen(s);
    char* result = (char*)malloc(len + 1);
    for (size_t i = 0; i < len; i++) {
        result[i] = s[len - 1 - i];
    }
    result[len] = '\0';
    return result;
}

/*
 * __gradient_string_pad_left(s, width, pad) -> char*
 */
char* __gradient_string_pad_left(const char* s, int64_t width, const char* pad) {
    if (!s) s = "";
    if (!pad || !*pad) pad = " ";
    size_t slen = strlen(s);
    if ((int64_t)slen >= width) return strdup(s);
    size_t pad_len = strlen(pad);
    size_t total = (size_t)width;
    char* result = (char*)malloc(total + 1);
    size_t fill = total - slen;
    for (size_t i = 0; i < fill; i++) result[i] = pad[i % pad_len];
    memcpy(result + fill, s, slen);
    result[total] = '\0';
    return result;
}

/*
 * __gradient_string_pad_right(s, width, pad) -> char*
 */
char* __gradient_string_pad_right(const char* s, int64_t width, const char* pad) {
    if (!s) s = "";
    if (!pad || !*pad) pad = " ";
    size_t slen = strlen(s);
    if ((int64_t)slen >= width) return strdup(s);
    size_t pad_len = strlen(pad);
    size_t total = (size_t)width;
    char* result = (char*)malloc(total + 1);
    memcpy(result, s, slen);
    size_t fill = total - slen;
    for (size_t i = 0; i < fill; i++) result[slen + i] = pad[i % pad_len];
    result[total] = '\0';
    return result;
}

/*
 * __gradient_string_count(s, substr) -> int64_t
 * Count non-overlapping occurrences of substr in s.
 */
int64_t __gradient_string_count(const char* s, const char* substr) {
    if (!s || !substr || !*substr) return 0;
    int64_t count = 0;
    size_t sub_len = strlen(substr);
    const char* p = s;
    while ((p = strstr(p, substr)) != NULL) { count++; p += sub_len; }
    return count;
}

/*
 * __gradient_char_code(s) -> int64_t
 * Returns the ASCII/UTF-8 byte value of the first character.
 */
int64_t __gradient_char_code(const char* s) {
    if (!s || !*s) return 0;
    return (int64_t)(unsigned char)s[0];
}

/*
 * __gradient_from_char_code(code) -> char*
 * Returns a single-character string from an ASCII code.
 */
char* __gradient_from_char_code(int64_t code) {
    char* result = (char*)malloc(2);
    result[0] = (char)code;
    result[1] = '\0';
    return result;
}
```

- [ ] **Step 4: Register in env.rs**

```rust
        // ── Phase PP: String Utilities ───────────────────────────────────
        // string_join(List[String], String) -> String
        self.define_fn(
            "string_join".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("parts".into(), Ty::List(Box::new(Ty::String))),
                    ("delim".into(), Ty::String),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_repeat(String, Int) -> String
        self.define_fn(
            "string_repeat".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String), ("n".into(), Ty::Int)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_reverse(String) -> String
        self.define_fn(
            "string_reverse".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_pad_left(String, Int, String) -> String
        self.define_fn(
            "string_pad_left".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String),
                    ("width".into(), Ty::Int),
                    ("pad".into(), Ty::String),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_pad_right(String, Int, String) -> String
        self.define_fn(
            "string_pad_right".into(),
            FnSig {
                type_params: vec![],
                params: vec![
                    ("s".into(), Ty::String),
                    ("width".into(), Ty::Int),
                    ("pad".into(), Ty::String),
                ],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_is_empty(String) -> Bool
        self.define_fn(
            "string_is_empty".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String)],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // string_count(String, String) -> Int
        self.define_fn(
            "string_count".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String), ("substr".into(), Ty::String)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // char_code(String) -> Int
        self.define_fn(
            "char_code".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String)],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // from_char_code(Int) -> String
        self.define_fn(
            "from_char_code".into(),
            FnSig {
                type_params: vec![],
                params: vec![("code".into(), Ty::Int)],
                ret: Ty::String,
                effects: vec![],
            },
        );

        // string_lines(String) -> List[String]
        self.define_fn(
            "string_lines".into(),
            FnSig {
                type_params: vec![],
                params: vec![("s".into(), Ty::String)],
                ret: Ty::List(Box::new(Ty::String)),
                effects: vec![],
            },
        );
```

- [ ] **Step 5: Register in IR builder + update string/list tracking**

IR builder registrations:
```rust
        // Phase PP: String utilities
        for &name in &["string_join", "string_repeat", "string_reverse",
                        "string_pad_left", "string_pad_right", "from_char_code"] {
            self.register_func(name);
            self.function_return_types.insert(name.to_string(), Type::Ptr);
        }
        self.register_func("string_is_empty");
        self.function_return_types.insert("string_is_empty".to_string(), Type::Bool);
        self.register_func("string_count");
        self.function_return_types.insert("string_count".to_string(), Type::I64);
        self.register_func("char_code");
        self.function_return_types.insert("char_code".to_string(), Type::I64);
        self.register_func("string_lines");
        self.function_return_types.insert("string_lines".to_string(), Type::Ptr);
```

Also update the string/list tracking in `build_call` (~line 1524):
- Add `"string_join" | "string_repeat" | "string_reverse" | "string_pad_left" | "string_pad_right" | "from_char_code"` to the `string_values.insert(result)` match.
- Add `"string_lines"` to the `list_values.insert(result)` match.

- [ ] **Step 6: Declare C functions + codegen match arms**

Declarations (each string function follows pattern: `ptr -> ptr` or `(ptr, ptr) -> ptr`):
```rust
        // Phase PP: String utility C helpers
        for &(name, n_params) in &[
            ("__gradient_string_join", 2),    // (list: ptr, delim: ptr) -> ptr
            ("__gradient_string_repeat", 2),  // (s: ptr, n: i64) -> ptr  (special)
            ("__gradient_string_reverse", 1), // (s: ptr) -> ptr
            ("__gradient_string_pad_left", 3),// (s: ptr, width: i64, pad: ptr) (special)
            ("__gradient_string_pad_right", 3),
            ("__gradient_string_count", 2),   // (s: ptr, sub: ptr) -> i64 (special)
            ("__gradient_char_code", 1),      // (s: ptr) -> i64 (special)
            ("__gradient_from_char_code", 1), // (code: i64) -> ptr (special)
        ] {
            // These have varying signatures; declare individually below
            let _ = (name, n_params);
        }

        // __gradient_string_join(list: ptr, delim: ptr) -> ptr
        if !self.declared_functions.contains_key("__gradient_string_join") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module
                .declare_function("__gradient_string_join", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_join: {}", e))?;
            self.declared_functions.insert("__gradient_string_join".to_string(), func_id);
        }

        // __gradient_string_repeat(s: ptr, n: i64) -> ptr
        if !self.declared_functions.contains_key("__gradient_string_repeat") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module
                .declare_function("__gradient_string_repeat", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_repeat: {}", e))?;
            self.declared_functions.insert("__gradient_string_repeat".to_string(), func_id);
        }

        // __gradient_string_reverse(s: ptr) -> ptr
        if !self.declared_functions.contains_key("__gradient_string_reverse") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module
                .declare_function("__gradient_string_reverse", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_reverse: {}", e))?;
            self.declared_functions.insert("__gradient_string_reverse".to_string(), func_id);
        }

        // __gradient_string_pad_left(s: ptr, width: i64, pad: ptr) -> ptr
        if !self.declared_functions.contains_key("__gradient_string_pad_left") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module
                .declare_function("__gradient_string_pad_left", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_pad_left: {}", e))?;
            self.declared_functions.insert("__gradient_string_pad_left".to_string(), func_id);
        }

        // __gradient_string_pad_right — same signature as pad_left
        if !self.declared_functions.contains_key("__gradient_string_pad_right") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module
                .declare_function("__gradient_string_pad_right", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_pad_right: {}", e))?;
            self.declared_functions.insert("__gradient_string_pad_right".to_string(), func_id);
        }

        // __gradient_string_count(s: ptr, sub: ptr) -> i64
        if !self.declared_functions.contains_key("__gradient_string_count") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self.module
                .declare_function("__gradient_string_count", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_string_count: {}", e))?;
            self.declared_functions.insert("__gradient_string_count".to_string(), func_id);
        }

        // __gradient_char_code(s: ptr) -> i64
        if !self.declared_functions.contains_key("__gradient_char_code") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self.module
                .declare_function("__gradient_char_code", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_char_code: {}", e))?;
            self.declared_functions.insert("__gradient_char_code".to_string(), func_id);
        }

        // __gradient_from_char_code(code: i64) -> ptr
        if !self.declared_functions.contains_key("__gradient_from_char_code") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module
                .declare_function("__gradient_from_char_code", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_from_char_code: {}", e))?;
            self.declared_functions.insert("__gradient_from_char_code".to_string(), func_id);
        }
```

Match arms:

```rust
                        // ── Phase PP: String utilities ────────────────────────
                        "string_join" => {
                            let list_ptr = resolve_value(&value_map, &args[0])?;
                            let delim_ptr = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("__gradient_string_join")
                                .ok_or("__gradient_string_join not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[list_ptr, delim_ptr]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "string_repeat" => {
                            let s = resolve_value(&value_map, &args[0])?;
                            let n = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("__gradient_string_repeat")
                                .ok_or("__gradient_string_repeat not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[s, n]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "string_reverse" => {
                            let s = resolve_value(&value_map, &args[0])?;
                            let func_id = *self.declared_functions.get("__gradient_string_reverse")
                                .ok_or("__gradient_string_reverse not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[s]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "string_pad_left" => {
                            let s = resolve_value(&value_map, &args[0])?;
                            let width = resolve_value(&value_map, &args[1])?;
                            let pad = resolve_value(&value_map, &args[2])?;
                            let func_id = *self.declared_functions.get("__gradient_string_pad_left")
                                .ok_or("__gradient_string_pad_left not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[s, width, pad]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "string_pad_right" => {
                            let s = resolve_value(&value_map, &args[0])?;
                            let width = resolve_value(&value_map, &args[1])?;
                            let pad = resolve_value(&value_map, &args[2])?;
                            let func_id = *self.declared_functions.get("__gradient_string_pad_right")
                                .ok_or("__gradient_string_pad_right not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[s, width, pad]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "string_is_empty" => {
                            let s = resolve_value(&value_map, &args[0])?;
                            // Load first byte; if 0 -> true, else false
                            let byte = builder.ins().load(cl_types::I8, MemFlags::new(), s, 0i32);
                            let zero = builder.ins().iconst(cl_types::I8, 0);
                            let result = builder.ins().icmp(IntCC::Equal, byte, zero);
                            value_map.insert(*dst, result);
                        }

                        "string_count" => {
                            let s = resolve_value(&value_map, &args[0])?;
                            let sub = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("__gradient_string_count")
                                .ok_or("__gradient_string_count not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[s, sub]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "char_code" => {
                            let s = resolve_value(&value_map, &args[0])?;
                            let func_id = *self.declared_functions.get("__gradient_char_code")
                                .ok_or("__gradient_char_code not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[s]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "from_char_code" => {
                            let code = resolve_value(&value_map, &args[0])?;
                            let func_id = *self.declared_functions.get("__gradient_from_char_code")
                                .ok_or("__gradient_from_char_code not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[code]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "string_lines" => {
                            // Delegate to existing string_split with "\n" delimiter.
                            // Emit a const for "\n", then call __gradient_string_split.
                            let s = resolve_value(&value_map, &args[0])?;
                            let newline_data = self.create_data_str("\n")?;
                            let newline_gv = self.module.declare_data_in_func(newline_data, builder.func);
                            let newline_ptr = builder.ins().global_value(pointer_type, newline_gv);
                            let func_id = *self.declared_functions.get("__gradient_string_split")
                                .ok_or("__gradient_string_split not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[s, newline_ptr]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }
```

**Note:** The `string_lines` implementation reuses the existing `__gradient_string_split` C function and `create_data_str` helper. If `create_data_str` doesn't exist as a method, create the "\n" string data section manually following the pattern used for other string constants in codegen.

- [ ] **Step 7: Run tests to verify pass**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_string -- --nocapture 2>&1 | tail -20`
Expected: All string tests pass

- [ ] **Step 8: Commit**

```bash
git add src/typechecker/env.rs src/ir/builder/mod.rs src/codegen/cranelift.rs runtime/gradient_runtime.c tests/phase_pp_integration.rs
git commit -m "feat(phase-pp): add string utility builtins"
```

---

## Task 5: Add Set, Queue, Stack Types to Type System

**Files:**
- Modify: `src/typechecker/types.rs`
- Modify: `src/typechecker/checker.rs`

- [ ] **Step 1: Add type variants to Ty enum**

In `src/typechecker/types.rs`, add after the `Map` variant:

```rust
    /// A set type, e.g. `Set[Int]`. Opaque C runtime type.
    Set(Box<Ty>),

    /// A queue type, e.g. `Queue[String]`. FIFO, opaque C runtime type.
    Queue(Box<Ty>),

    /// A stack type, e.g. `Stack[Int]`. LIFO, opaque C runtime type.
    Stack(Box<Ty>),
```

- [ ] **Step 2: Add Display implementations**

In the `Display` impl for `Ty`, add match arms:

```rust
            Ty::Set(elem) => write!(f, "Set[{}]", elem),
            Ty::Queue(elem) => write!(f, "Queue[{}]", elem),
            Ty::Stack(elem) => write!(f, "Stack[{}]", elem),
```

- [ ] **Step 3: Handle in type checker's resolve_type_expr**

In `src/typechecker/checker.rs`, in the `TypeExpr::Generic { name, args }` match arm (~line 4183), add handlers before the generic enum instantiation block:

```rust
                // Handle Set[T] type annotations.
                if name == "Set" {
                    if args.len() == 1 {
                        let elem_ty = self.resolve_type_expr(&args[0].node, args[0].span);
                        return Ty::Set(Box::new(elem_ty));
                    }
                    self.errors.push(TypeError {
                        message: "Set type requires exactly one type argument, e.g. Set[Int]".to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                    });
                    return Ty::Error;
                }

                // Handle Queue[T] type annotations.
                if name == "Queue" {
                    if args.len() == 1 {
                        let elem_ty = self.resolve_type_expr(&args[0].node, args[0].span);
                        return Ty::Queue(Box::new(elem_ty));
                    }
                    self.errors.push(TypeError {
                        message: "Queue type requires exactly one type argument, e.g. Queue[Int]".to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                    });
                    return Ty::Error;
                }

                // Handle Stack[T] type annotations.
                if name == "Stack" {
                    if args.len() == 1 {
                        let elem_ty = self.resolve_type_expr(&args[0].node, args[0].span);
                        return Ty::Stack(Box::new(elem_ty));
                    }
                    self.errors.push(TypeError {
                        message: "Stack type requires exactly one type argument, e.g. Stack[Int]".to_string(),
                        span,
                        expected: None,
                        found: None,
                        notes: vec![],
                        is_warning: false,
                    });
                    return Ty::Error;
                }
```

- [ ] **Step 4: Fix any exhaustive match compiler errors**

Run: `cd codebase/compiler && cargo check 2>&1 | head -40`

Search for non-exhaustive pattern match errors on `Ty` and add the new variants wherever needed. Common locations:
- Type unification/comparison in checker.rs
- Type printing in error messages
- Any other `match ty { ... }` patterns

For most match arms, `Set`/`Queue`/`Stack` should be handled similarly to `List` or `Map` (as opaque pointer types).

- [ ] **Step 5: Verify compilation**

Run: `cd codebase/compiler && cargo check`
Expected: Compiles clean

- [ ] **Step 6: Commit**

```bash
git add src/typechecker/types.rs src/typechecker/checker.rs
git commit -m "feat(phase-pp): add Set, Queue, Stack type constructors"
```

---

## Task 6: Set — C Runtime + Builtins + Tests

**Files:**
- Modify: `runtime/gradient_runtime.c`
- Modify: `src/typechecker/env.rs`
- Modify: `src/ir/builder/mod.rs`
- Modify: `src/codegen/cranelift.rs`
- Modify: `tests/phase_pp_integration.rs`

- [ ] **Step 1: Write failing tests**

```rust
// ── Data Structures: Set ────────────────────────────────────────────

#[test]
fn test_set_basic() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let s: Set[Int] = set_new()
    let s2: Set[Int] = set_add(s, 42)
    let s3: Set[Int] = set_add(s2, 10)
    let s4: Set[Int] = set_add(s3, 42)
    print_int(set_size(s4))
    println("")
    print_bool(set_contains(s4, 42))
    println("")
    print_bool(set_contains(s4, 99))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "2");     // duplicate 42 not counted
    assert_eq!(lines[1], "true");
    assert_eq!(lines[2], "false");
}

#[test]
fn test_set_remove_and_to_list() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let s: Set[Int] = set_new()
    let s2: Set[Int] = set_add(set_add(set_add(s, 1), 2), 3)
    let s3: Set[Int] = set_remove(s2, 2)
    print_int(set_size(s3))
    println("")
    let items: List[Int] = set_to_list(s3)
    print_int(list_length(items))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "2");
    assert_eq!(lines[1], "2");
}
```

- [ ] **Step 2: Add C runtime for Set**

Add to `runtime/gradient_runtime.c`:

```c
/* ── Phase PP: Set type ────────────────────────────────────────────────── */

/*
 * Set layout: reuses the same GradientMap struct but only uses keys
 * (values array stores 0s). For Set[Int], keys are stored as int64_t
 * cast to char* (we use a sentinel approach with snprintf for hashing).
 *
 * For simplicity in v0.1, Set stores int64_t values directly in the
 * values array, and uses snprintf'd keys for lookup.
 */

typedef struct {
    int64_t  size;
    int64_t  capacity;
    int64_t* elements;   /* raw i64 values */
} GradientSet;

#define GRADIENT_SET_INIT_CAP 8

static GradientSet* set_alloc(int64_t cap) {
    GradientSet* s = (GradientSet*)malloc(sizeof(GradientSet));
    s->size     = 0;
    s->capacity = cap;
    s->elements = (int64_t*)calloc((size_t)cap, sizeof(int64_t));
    return s;
}

static GradientSet* set_copy(GradientSet* src) {
    GradientSet* dst = (GradientSet*)malloc(sizeof(GradientSet));
    dst->size     = src->size;
    dst->capacity = src->capacity;
    dst->elements = (int64_t*)malloc((size_t)src->capacity * sizeof(int64_t));
    memcpy(dst->elements, src->elements, (size_t)src->capacity * sizeof(int64_t));
    return dst;
}

static int64_t set_find(GradientSet* s, int64_t val) {
    for (int64_t i = 0; i < s->size; i++) {
        if (s->elements[i] == val) return i;
    }
    return -1;
}

static void set_grow(GradientSet* s) {
    int64_t new_cap = s->capacity * 2;
    s->elements = (int64_t*)realloc(s->elements, (size_t)new_cap * sizeof(int64_t));
    s->capacity = new_cap;
}

void* __gradient_set_new(void) {
    return (void*)set_alloc(GRADIENT_SET_INIT_CAP);
}

void* __gradient_set_add(void* set, int64_t val) {
    GradientSet* src = (GradientSet*)set;
    GradientSet* s = set_copy(src);
    if (set_find(s, val) >= 0) return (void*)s;  /* already present */
    if (s->size >= s->capacity) set_grow(s);
    s->elements[s->size++] = val;
    return (void*)s;
}

void* __gradient_set_remove(void* set, int64_t val) {
    GradientSet* src = (GradientSet*)set;
    GradientSet* s = set_copy(src);
    int64_t idx = set_find(s, val);
    if (idx < 0) return (void*)s;
    for (int64_t i = idx; i < s->size - 1; i++) {
        s->elements[i] = s->elements[i + 1];
    }
    s->size--;
    return (void*)s;
}

int64_t __gradient_set_contains(void* set, int64_t val) {
    GradientSet* s = (GradientSet*)set;
    return set_find(s, val) >= 0 ? 1 : 0;
}

int64_t __gradient_set_size(void* set) {
    GradientSet* s = (GradientSet*)set;
    return s->size;
}

/* Returns a Gradient List[Int] */
void* __gradient_set_to_list(void* set) {
    GradientSet* s = (GradientSet*)set;
    int64_t n = s->size;
    void* list = malloc((size_t)(16 + n * 8));
    int64_t* hdr = (int64_t*)list;
    hdr[0] = n;
    hdr[1] = n;
    int64_t* data = hdr + 2;
    for (int64_t i = 0; i < n; i++) {
        data[i] = s->elements[i];
    }
    return list;
}
```

- [ ] **Step 3: Register in env.rs**

```rust
        // ── Phase PP: Set operations ─────────────────────────────────────
        // set_new() -> Set[T]
        self.define_fn(
            "set_new".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![],
                ret: Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // set_add(Set[T], T) -> Set[T]
        self.define_fn(
            "set_add".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    ("s".into(), Ty::Set(Box::new(Ty::TypeVar("T".into())))),
                    ("val".into(), Ty::TypeVar("T".into())),
                ],
                ret: Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // set_remove(Set[T], T) -> Set[T]
        self.define_fn(
            "set_remove".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    ("s".into(), Ty::Set(Box::new(Ty::TypeVar("T".into())))),
                    ("val".into(), Ty::TypeVar("T".into())),
                ],
                ret: Ty::Set(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );

        // set_contains(Set[T], T) -> Bool
        self.define_fn(
            "set_contains".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    ("s".into(), Ty::Set(Box::new(Ty::TypeVar("T".into())))),
                    ("val".into(), Ty::TypeVar("T".into())),
                ],
                ret: Ty::Bool,
                effects: vec![],
            },
        );

        // set_size(Set[T]) -> Int
        self.define_fn(
            "set_size".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    ("s".into(), Ty::Set(Box::new(Ty::TypeVar("T".into())))),
                ],
                ret: Ty::Int,
                effects: vec![],
            },
        );

        // set_to_list(Set[T]) -> List[T]
        self.define_fn(
            "set_to_list".into(),
            FnSig {
                type_params: vec!["T".into()],
                params: vec![
                    ("s".into(), Ty::Set(Box::new(Ty::TypeVar("T".into())))),
                ],
                ret: Ty::List(Box::new(Ty::TypeVar("T".into()))),
                effects: vec![],
            },
        );
```

- [ ] **Step 4: Register in IR builder + codegen declarations + match arms**

IR builder:
```rust
        // Phase PP: Set operations
        for &name in &["set_new", "set_add", "set_remove", "set_to_list"] {
            self.register_func(name);
            self.function_return_types.insert(name.to_string(), Type::Ptr);
        }
        self.register_func("set_contains");
        self.function_return_types.insert("set_contains".to_string(), Type::Bool);
        self.register_func("set_size");
        self.function_return_types.insert("set_size".to_string(), Type::I64);
```

Also add `"set_to_list"` to the `list_values.insert(result)` tracking match.

Codegen declarations (all use pointer_type for set args, i64 for values):

```rust
        // __gradient_set_new() -> ptr
        if !self.declared_functions.contains_key("__gradient_set_new") {
            let mut sig = self.module.make_signature();
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module.declare_function("__gradient_set_new", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_set_new: {}", e))?;
            self.declared_functions.insert("__gradient_set_new".to_string(), func_id);
        }
        // __gradient_set_add(set: ptr, val: i64) -> ptr
        if !self.declared_functions.contains_key("__gradient_set_add") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module.declare_function("__gradient_set_add", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_set_add: {}", e))?;
            self.declared_functions.insert("__gradient_set_add".to_string(), func_id);
        }
        // __gradient_set_remove(set: ptr, val: i64) -> ptr
        if !self.declared_functions.contains_key("__gradient_set_remove") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module.declare_function("__gradient_set_remove", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_set_remove: {}", e))?;
            self.declared_functions.insert("__gradient_set_remove".to_string(), func_id);
        }
        // __gradient_set_contains(set: ptr, val: i64) -> i64
        if !self.declared_functions.contains_key("__gradient_set_contains") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.params.push(AbiParam::new(cl_types::I64));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self.module.declare_function("__gradient_set_contains", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_set_contains: {}", e))?;
            self.declared_functions.insert("__gradient_set_contains".to_string(), func_id);
        }
        // __gradient_set_size(set: ptr) -> i64
        if !self.declared_functions.contains_key("__gradient_set_size") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(cl_types::I64));
            let func_id = self.module.declare_function("__gradient_set_size", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_set_size: {}", e))?;
            self.declared_functions.insert("__gradient_set_size".to_string(), func_id);
        }
        // __gradient_set_to_list(set: ptr) -> ptr
        if !self.declared_functions.contains_key("__gradient_set_to_list") {
            let mut sig = self.module.make_signature();
            sig.params.push(AbiParam::new(pointer_type));
            sig.returns.push(AbiParam::new(pointer_type));
            let func_id = self.module.declare_function("__gradient_set_to_list", Linkage::Import, &sig)
                .map_err(|e| format!("Failed to declare __gradient_set_to_list: {}", e))?;
            self.declared_functions.insert("__gradient_set_to_list".to_string(), func_id);
        }
```

Codegen match arms — all follow the simple "call C function" pattern:

```rust
                        "set_new" => {
                            let func_id = *self.declared_functions.get("__gradient_set_new")
                                .ok_or("__gradient_set_new not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }
                        "set_add" => {
                            let set = resolve_value(&value_map, &args[0])?;
                            let val = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("__gradient_set_add")
                                .ok_or("__gradient_set_add not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[set, val]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }
                        "set_remove" => {
                            let set = resolve_value(&value_map, &args[0])?;
                            let val = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("__gradient_set_remove")
                                .ok_or("__gradient_set_remove not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[set, val]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }
                        "set_contains" => {
                            let set = resolve_value(&value_map, &args[0])?;
                            let val = resolve_value(&value_map, &args[1])?;
                            let func_id = *self.declared_functions.get("__gradient_set_contains")
                                .ok_or("__gradient_set_contains not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[set, val]);
                            let result_i64 = builder.inst_results(call).to_vec()[0];
                            let result = builder.ins().ireduce(cl_types::I8, result_i64);
                            value_map.insert(*dst, result);
                        }
                        "set_size" => {
                            let set = resolve_value(&value_map, &args[0])?;
                            let func_id = *self.declared_functions.get("__gradient_set_size")
                                .ok_or("__gradient_set_size not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[set]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }
                        "set_to_list" => {
                            let set = resolve_value(&value_map, &args[0])?;
                            let func_id = *self.declared_functions.get("__gradient_set_to_list")
                                .ok_or("__gradient_set_to_list not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[set]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_set -- --nocapture`
Expected: All set tests pass

- [ ] **Step 6: Commit**

```bash
git add runtime/gradient_runtime.c src/typechecker/env.rs src/ir/builder/mod.rs src/codegen/cranelift.rs tests/phase_pp_integration.rs
git commit -m "feat(phase-pp): add Set data structure builtins"
```

---

## Task 7: Queue — C Runtime + Builtins + Tests

**Files:** Same as Task 6.

- [ ] **Step 1: Write failing tests**

```rust
// ── Data Structures: Queue ──────────────────────────────────────────

#[test]
fn test_queue_basic() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let q: Queue[Int] = queue_new()
    let q2: Queue[Int] = queue_push(q, 10)
    let q3: Queue[Int] = queue_push(q2, 20)
    let q4: Queue[Int] = queue_push(q3, 30)
    print_int(queue_size(q4))
    println("")
    print_bool(queue_is_empty(q))
    println("")
    print_bool(queue_is_empty(q4))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "3");
    assert_eq!(lines[1], "true");
    assert_eq!(lines[2], "false");
}

#[test]
fn test_queue_pop_peek() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let q: Queue[Int] = queue_new()
    let q2: Queue[Int] = queue_push(queue_push(q, 10), 20)
    match queue_peek(q2):
        Some(val):
            print_int(val)
        None:
            println("empty")
    println("")
    let result: (Queue[Int], Option[Int]) = queue_pop(q2)
    match result:
        (q3, Some(val)):
            print_int(val)
            println("")
            print_int(queue_size(q3))
        (q3, None):
            println("empty")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "10");  // peek returns front (FIFO)
    assert_eq!(lines[1], "10");  // pop returns front
    assert_eq!(lines[2], "1");   // one element left
}
```

- [ ] **Step 2: Add C runtime for Queue**

```c
/* ── Phase PP: Queue type (FIFO) ───────────────────────────────────────── */

typedef struct {
    int64_t  size;
    int64_t  capacity;
    int64_t  head;       /* index of front element */
    int64_t* elements;   /* circular buffer */
} GradientQueue;

#define GRADIENT_QUEUE_INIT_CAP 8

static GradientQueue* queue_alloc(int64_t cap) {
    GradientQueue* q = (GradientQueue*)malloc(sizeof(GradientQueue));
    q->size     = 0;
    q->capacity = cap;
    q->head     = 0;
    q->elements = (int64_t*)calloc((size_t)cap, sizeof(int64_t));
    return q;
}

static GradientQueue* queue_copy(GradientQueue* src) {
    GradientQueue* dst = (GradientQueue*)malloc(sizeof(GradientQueue));
    dst->size     = src->size;
    dst->capacity = src->capacity;
    dst->head     = src->head;
    dst->elements = (int64_t*)malloc((size_t)src->capacity * sizeof(int64_t));
    memcpy(dst->elements, src->elements, (size_t)src->capacity * sizeof(int64_t));
    return dst;
}

void* __gradient_queue_new(void) {
    return (void*)queue_alloc(GRADIENT_QUEUE_INIT_CAP);
}

void* __gradient_queue_push(void* queue, int64_t val) {
    GradientQueue* src = (GradientQueue*)queue;
    GradientQueue* q = queue_copy(src);
    if (q->size >= q->capacity) {
        int64_t new_cap = q->capacity * 2;
        int64_t* new_elems = (int64_t*)calloc((size_t)new_cap, sizeof(int64_t));
        /* Linearize circular buffer */
        for (int64_t i = 0; i < q->size; i++) {
            new_elems[i] = q->elements[(q->head + i) % q->capacity];
        }
        free(q->elements);
        q->elements = new_elems;
        q->head = 0;
        q->capacity = new_cap;
    }
    int64_t tail = (q->head + q->size) % q->capacity;
    q->elements[tail] = val;
    q->size++;
    return (void*)q;
}

/*
 * __gradient_queue_pop(queue) -> (new_queue_ptr, value, found)
 * Returns via out-params: *out_value = popped value, *out_found = 1 if non-empty.
 * Returns new queue pointer.
 */
void* __gradient_queue_pop(void* queue, int64_t* out_value, int64_t* out_found) {
    GradientQueue* src = (GradientQueue*)queue;
    GradientQueue* q = queue_copy(src);
    if (q->size == 0) {
        *out_found = 0;
        *out_value = 0;
        return (void*)q;
    }
    *out_found = 1;
    *out_value = q->elements[q->head];
    q->head = (q->head + 1) % q->capacity;
    q->size--;
    return (void*)q;
}

/*
 * __gradient_queue_peek(queue, out_found) -> int64_t
 */
int64_t __gradient_queue_peek(void* queue, int64_t* out_found) {
    GradientQueue* q = (GradientQueue*)queue;
    if (q->size == 0) { *out_found = 0; return 0; }
    *out_found = 1;
    return q->elements[q->head];
}

int64_t __gradient_queue_size(void* queue) {
    GradientQueue* q = (GradientQueue*)queue;
    return q->size;
}

int64_t __gradient_queue_is_empty(void* queue) {
    GradientQueue* q = (GradientQueue*)queue;
    return q->size == 0 ? 1 : 0;
}
```

- [ ] **Step 3: Register in env.rs, IR builder, codegen**

Follow the same pattern as Set. Register:
- `queue_new() -> Queue[T]` (generic over T)
- `queue_push(Queue[T], T) -> Queue[T]`
- `queue_pop(Queue[T]) -> (Queue[T], Option[T])`
- `queue_peek(Queue[T]) -> Option[T]`
- `queue_size(Queue[T]) -> Int`
- `queue_is_empty(Queue[T]) -> Bool`

**Important:** `queue_pop` returns a tuple `(Queue[T], Option[T])`. The codegen for this is complex — it needs to:
1. Allocate a stack slot for out_value and out_found
2. Call `__gradient_queue_pop(queue, &out_value, &out_found)`
3. Construct the Option (Some or None) based on out_found
4. Construct the Tuple `(new_queue_ptr, option_ptr)` on the heap

This follows the same pattern as `map_get` for Option construction, plus tuple construction like existing tuple codegen. The implementing agent should study `map_get`'s Option construction pattern in cranelift.rs (~line 3700) and the existing tuple construction pattern.

Similarly `queue_peek` returns `Option[T]` and needs the same Some/None construction.

- [ ] **Step 4: Run tests, commit**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_queue -- --nocapture`

```bash
git commit -m "feat(phase-pp): add Queue data structure builtins"
```

---

## Task 8: Stack — C Runtime + Builtins + Tests

**Files:** Same as Task 6.

Follows the same pattern as Queue. Stack is simpler (no circular buffer — just a dynamic array with push/pop at the end).

- [ ] **Step 1: Write failing tests**

```rust
// ── Data Structures: Stack ──────────────────────────────────────────

#[test]
fn test_stack_basic() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let s: Stack[Int] = stack_new()
    let s2: Stack[Int] = stack_push(stack_push(stack_push(s, 10), 20), 30)
    print_int(stack_size(s2))
    println("")
    match stack_peek(s2):
        Some(val):
            print_int(val)
        None:
            println("empty")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "3");
    assert_eq!(lines[1], "30");  // LIFO: peek returns last pushed
}

#[test]
fn test_stack_pop() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let s: Stack[Int] = stack_new()
    let s2: Stack[Int] = stack_push(stack_push(s, 10), 20)
    let result: (Stack[Int], Option[Int]) = stack_pop(s2)
    match result:
        (s3, Some(val)):
            print_int(val)
            println("")
            print_int(stack_size(s3))
        (s3, None):
            println("empty")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "20");  // LIFO: pop returns 20
    assert_eq!(lines[1], "1");
}
```

- [ ] **Step 2: Add C runtime for Stack**

```c
/* ── Phase PP: Stack type (LIFO) ───────────────────────────────────────── */

typedef struct {
    int64_t  size;
    int64_t  capacity;
    int64_t* elements;
} GradientStack;

#define GRADIENT_STACK_INIT_CAP 8

static GradientStack* stack_alloc(int64_t cap) {
    GradientStack* s = (GradientStack*)malloc(sizeof(GradientStack));
    s->size     = 0;
    s->capacity = cap;
    s->elements = (int64_t*)calloc((size_t)cap, sizeof(int64_t));
    return s;
}

static GradientStack* stack_copy(GradientStack* src) {
    GradientStack* dst = (GradientStack*)malloc(sizeof(GradientStack));
    dst->size     = src->size;
    dst->capacity = src->capacity;
    dst->elements = (int64_t*)malloc((size_t)src->capacity * sizeof(int64_t));
    memcpy(dst->elements, src->elements, (size_t)src->capacity * sizeof(int64_t));
    return dst;
}

void* __gradient_stack_new(void) {
    return (void*)stack_alloc(GRADIENT_STACK_INIT_CAP);
}

void* __gradient_stack_push(void* stack, int64_t val) {
    GradientStack* src = (GradientStack*)stack;
    GradientStack* s = stack_copy(src);
    if (s->size >= s->capacity) {
        int64_t new_cap = s->capacity * 2;
        s->elements = (int64_t*)realloc(s->elements, (size_t)new_cap * sizeof(int64_t));
        s->capacity = new_cap;
    }
    s->elements[s->size++] = val;
    return (void*)s;
}

void* __gradient_stack_pop(void* stack, int64_t* out_value, int64_t* out_found) {
    GradientStack* src = (GradientStack*)stack;
    GradientStack* s = stack_copy(src);
    if (s->size == 0) {
        *out_found = 0;
        *out_value = 0;
        return (void*)s;
    }
    *out_found = 1;
    *out_value = s->elements[--s->size];
    return (void*)s;
}

int64_t __gradient_stack_peek(void* stack, int64_t* out_found) {
    GradientStack* s = (GradientStack*)stack;
    if (s->size == 0) { *out_found = 0; return 0; }
    *out_found = 1;
    return s->elements[s->size - 1];
}

int64_t __gradient_stack_size(void* stack) {
    return ((GradientStack*)stack)->size;
}

int64_t __gradient_stack_is_empty(void* stack) {
    return ((GradientStack*)stack)->size == 0 ? 1 : 0;
}
```

- [ ] **Step 3: Register in env.rs, IR builder, codegen**

Identical pattern to Queue but with `Stack[T]` type and LIFO semantics:
- `stack_new() -> Stack[T]`
- `stack_push(Stack[T], T) -> Stack[T]`
- `stack_pop(Stack[T]) -> (Stack[T], Option[T])`
- `stack_peek(Stack[T]) -> Option[T]`
- `stack_size(Stack[T]) -> Int`
- `stack_is_empty(Stack[T]) -> Bool`

Codegen: Reuse the exact same out-param + Option construction pattern from Queue.

- [ ] **Step 4: Run tests, commit**

```bash
git commit -m "feat(phase-pp): add Stack data structure builtins"
```

---

## Task 9: Date/Time Builtins

**Files:**
- Modify: `runtime/gradient_runtime.c`
- Modify: `src/typechecker/env.rs`
- Modify: `src/ir/builder/mod.rs`
- Modify: `src/codegen/cranelift.rs`
- Modify: `tests/phase_pp_integration.rs`

- [ ] **Step 1: Write failing tests**

```rust
// ── Date/Time ───────────────────────────────────────────────────────

#[test]
fn test_time_now() {
    let src = r#"
mod test
fn main() -> !{IO, Time} ():
    let t: Int = time_now()
    let valid: Bool = t > 1000000000
    print_bool(valid)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "true");
}

#[test]
fn test_time_now_ms() {
    let src = r#"
mod test
fn main() -> !{IO, Time} ():
    let t: Int = time_now_ms()
    let valid: Bool = t > 1000000000000
    print_bool(valid)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "true");
}

#[test]
fn test_time_format() {
    let src = r#"
mod test
fn main() -> !{IO, Time} ():
    let formatted: String = time_format(0, "%Y")
    println(formatted)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "1970");
}

#[test]
fn test_time_diff() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let d: Int = time_diff(100, 50)
    print_int(d)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "50");
}

#[test]
fn test_sleep_ms() {
    let src = r#"
mod test
fn main() -> !{IO, Time} ():
    sleep_ms(10)
    println("done")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "done");
}
```

- [ ] **Step 2: Add C runtime helpers**

```c
/* ── Phase PP: Date/Time ───────────────────────────────────────────────── */

#include <sys/time.h>

int64_t __gradient_time_now(void) {
    return (int64_t)time(NULL);
}

int64_t __gradient_time_now_ms(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (int64_t)tv.tv_sec * 1000 + (int64_t)tv.tv_usec / 1000;
}

char* __gradient_time_format(int64_t epoch, const char* fmt) {
    time_t t = (time_t)epoch;
    struct tm* tm_info = gmtime(&t);
    char* buf = (char*)malloc(256);
    if (!tm_info || strftime(buf, 256, fmt, tm_info) == 0) {
        buf[0] = '\0';
    }
    return buf;
}

/*
 * __gradient_time_parse(s, fmt, out_found) -> int64_t
 * Parse time string, set *out_found=1 on success. Returns epoch seconds.
 */
int64_t __gradient_time_parse(const char* s, const char* fmt, int64_t* out_found) {
    struct tm tm_info;
    memset(&tm_info, 0, sizeof(tm_info));
    char* result = strptime(s, fmt, &tm_info);
    if (!result) { *out_found = 0; return 0; }
    *out_found = 1;
    return (int64_t)timegm(&tm_info);
}

void __gradient_sleep_ms(int64_t ms) {
    usleep((useconds_t)(ms * 1000));
}
```

- [ ] **Step 3: Register in env.rs**

```rust
        // ── Phase PP: Date/Time ──────────────────────────────────────────
        self.define_fn("time_now".into(), FnSig {
            type_params: vec![], params: vec![],
            ret: Ty::Int, effects: vec!["Time".into()],
        });
        self.define_fn("time_now_ms".into(), FnSig {
            type_params: vec![], params: vec![],
            ret: Ty::Int, effects: vec!["Time".into()],
        });
        self.define_fn("time_format".into(), FnSig {
            type_params: vec![],
            params: vec![("epoch".into(), Ty::Int), ("fmt".into(), Ty::String)],
            ret: Ty::String, effects: vec![],
        });
        self.define_fn("time_parse".into(), FnSig {
            type_params: vec![],
            params: vec![("s".into(), Ty::String), ("fmt".into(), Ty::String)],
            ret: option_int_ty.clone(),  // Option[Int] — construct from env's Option enum
            effects: vec![],
        });
        self.define_fn("sleep_ms".into(), FnSig {
            type_params: vec![], params: vec![("ms".into(), Ty::Int)],
            ret: Ty::Unit, effects: vec!["Time".into()],
        });
        self.define_fn("time_diff".into(), FnSig {
            type_params: vec![], params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
            ret: Ty::Int, effects: vec![],
        });
```

**Note:** For `time_parse`'s return type `Option[Int]`, construct the Option enum type the same way it's done for `list_find` and `map_get` in the existing code. Check how `Option[T]` is represented in existing builtin registrations.

- [ ] **Step 4: Register in IR builder, codegen declarations, match arms**

Follow the standard pattern. `time_format` and `from_char_code` return strings (add to string_values tracking). `time_parse` returns Option (pointer). `time_diff` is pure subtraction — can be implemented inline:

```rust
                        "time_diff" => {
                            let a = resolve_value(&value_map, &args[0])?;
                            let b = resolve_value(&value_map, &args[1])?;
                            let result = builder.ins().isub(a, b);
                            value_map.insert(*dst, result);
                        }
```

`time_parse` needs the same Option construction pattern as `map_get` (call C helper with out_found, construct Some/None).

- [ ] **Step 5: Run tests, commit**

```bash
git commit -m "feat(phase-pp): add date/time builtins"
```

---

## Task 10: Environment & Process Builtins

**Files:**
- Modify: `runtime/gradient_runtime.c`
- Modify: `src/typechecker/env.rs`
- Modify: `src/ir/builder/mod.rs`
- Modify: `src/codegen/cranelift.rs`
- Modify: `tests/phase_pp_integration.rs`

- [ ] **Step 1: Write failing tests**

```rust
// ── Environment & Process ───────────────────────────────────────────

#[test]
fn test_env_get_set() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    env_set("GRADIENT_TEST_VAR", "hello123")
    match env_get("GRADIENT_TEST_VAR"):
        Some(val):
            println(val)
        None:
            println("not found")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "hello123");
}

#[test]
fn test_env_remove() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    env_set("GRADIENT_RM_TEST", "x")
    env_remove("GRADIENT_RM_TEST")
    match env_get("GRADIENT_RM_TEST"):
        Some(val):
            println("found")
        None:
            println("gone")
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "gone");
}

#[test]
fn test_cwd() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let dir: String = cwd()
    let valid: Bool = string_length(dir) > 0
    print_bool(valid)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "true");
}

#[test]
fn test_process_exec() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match process_exec("echo hello_gradient"):
        Ok(output):
            print(output)
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "hello_gradient");
}

#[test]
fn test_process_exec_status() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let code: Int = process_exec_status("true")
    print_int(code)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "0");
}
```

- [ ] **Step 2: Add C runtime helpers**

```c
/* ── Phase PP: Environment & Process ───────────────────────────────────── */

/*
 * __gradient_env_get(name, out_found) -> char*
 * Returns env var value (strdup'd) or NULL.
 */
const char* __gradient_env_get(const char* name, int64_t* out_found) {
    const char* val = getenv(name);
    if (!val) { *out_found = 0; return NULL; }
    *out_found = 1;
    return strdup(val);
}

void __gradient_env_set(const char* name, const char* value) {
    setenv(name, value, 1);
}

void __gradient_env_remove(const char* name) {
    unsetenv(name);
}

/* Returns List[String] of env var names */
void* __gradient_env_vars(void) {
    extern char** environ;
    /* Count entries */
    int64_t n = 0;
    for (char** e = environ; *e; e++) n++;
    void* list = malloc((size_t)(16 + n * 8));
    int64_t* hdr = (int64_t*)list;
    hdr[0] = n;
    hdr[1] = n;
    int64_t* data = hdr + 2;
    for (int64_t i = 0; i < n; i++) {
        const char* entry = environ[i];
        const char* eq = strchr(entry, '=');
        size_t name_len = eq ? (size_t)(eq - entry) : strlen(entry);
        char* name_copy = (char*)malloc(name_len + 1);
        memcpy(name_copy, entry, name_len);
        name_copy[name_len] = '\0';
        data[i] = (int64_t)(intptr_t)name_copy;
    }
    return list;
}

char* __gradient_cwd(void) {
    char* buf = (char*)malloc(4096);
    if (!getcwd(buf, 4096)) { buf[0] = '\0'; }
    return buf;
}

int64_t __gradient_chdir(const char* path) {
    return chdir(path) == 0 ? 1 : 0;
}

/*
 * __gradient_process_exec(cmd, out_ok) -> char*
 * Runs command via popen, captures stdout.
 * Sets *out_ok = 1 if exit code 0, 0 otherwise.
 * Returns captured output (stdout on success, empty on failure).
 */
char* __gradient_process_exec(const char* cmd, int64_t* out_ok) {
    FILE* fp = popen(cmd, "r");
    if (!fp) { *out_ok = 0; return strdup(""); }
    size_t cap = 4096;
    size_t len = 0;
    char* buf = (char*)malloc(cap);
    size_t n;
    while ((n = fread(buf + len, 1, cap - len - 1, fp)) > 0) {
        len += n;
        if (len >= cap - 1) { cap *= 2; buf = (char*)realloc(buf, cap); }
    }
    buf[len] = '\0';
    int status = pclose(fp);
    *out_ok = (status == 0) ? 1 : 0;
    return buf;
}

int64_t __gradient_process_exec_status(const char* cmd) {
    int status = system(cmd);
    if (status == -1) return -1;
    return WEXITSTATUS(status);
}
```

Add `#include <sys/wait.h>` at the top of the file for `WEXITSTATUS`.

- [ ] **Step 3: Register in env.rs**

```rust
        // ── Phase PP: Environment & Process ──────────────────────────────
        self.define_fn("env_get".into(), FnSig {
            type_params: vec![],
            params: vec![("name".into(), Ty::String)],
            ret: /* Option[String] — use the same pattern as map_get */,
            effects: vec!["IO".into()],
        });
        self.define_fn("env_set".into(), FnSig {
            type_params: vec![],
            params: vec![("name".into(), Ty::String), ("value".into(), Ty::String)],
            ret: Ty::Unit, effects: vec!["IO".into()],
        });
        self.define_fn("env_remove".into(), FnSig {
            type_params: vec![],
            params: vec![("name".into(), Ty::String)],
            ret: Ty::Unit, effects: vec!["IO".into()],
        });
        self.define_fn("env_vars".into(), FnSig {
            type_params: vec![],
            params: vec![],
            ret: Ty::List(Box::new(Ty::String)), effects: vec!["IO".into()],
        });
        self.define_fn("cwd".into(), FnSig {
            type_params: vec![], params: vec![],
            ret: Ty::String, effects: vec!["IO".into()],
        });
        self.define_fn("chdir".into(), FnSig {
            type_params: vec![],
            params: vec![("path".into(), Ty::String)],
            ret: Ty::Bool, effects: vec!["IO".into()],
        });
        self.define_fn("process_exec".into(), FnSig {
            type_params: vec![],
            params: vec![("cmd".into(), Ty::String)],
            ret: /* Result[String, String] — construct from env's Result enum */,
            effects: vec!["IO".into()],
        });
        self.define_fn("process_exec_status".into(), FnSig {
            type_params: vec![],
            params: vec![("cmd".into(), Ty::String)],
            ret: Ty::Int, effects: vec!["IO".into()],
        });
```

**Note:** For `env_get` returning `Option[String]` and `process_exec` returning `Result[String, String]`, study how existing builtins construct these types. Look at `map_get`'s registration for `Option` and `is_ok`/`is_err` for `Result` pattern.

- [ ] **Step 4: Register in IR builder, codegen**

`env_get` follows `map_get`'s Option construction pattern (check null, build Some/None).
`process_exec` follows a similar Result construction pattern (check out_ok flag, build Ok/Err).
Other functions are straightforward C calls.

Add `"cwd" | "from_char_code"` to string_values tracking. Add `"env_vars"` to list_values tracking.

- [ ] **Step 5: Run tests, commit**

```bash
git commit -m "feat(phase-pp): add environment and process builtins"
```

---

## Task 11: Recursive Union Types — Type Checker

**Files:**
- Modify: `src/typechecker/checker.rs`
- Modify: `src/typechecker/types.rs` (possibly)
- Modify: `tests/phase_pp_integration.rs`

This is the most complex task. The goal is to allow enum types to reference themselves.

- [ ] **Step 1: Write failing test**

```rust
// ── Recursive Union Types ───────────────────────────────────────────

#[test]
fn test_recursive_type_basic() {
    let src = r#"
mod test

type Tree = Leaf(Int) | Branch(Tree, Tree)

fn sum_tree(t: Tree) -> Int:
    match t:
        Leaf(n):
            n
        Branch(left, right):
            sum_tree(left) + sum_tree(right)

fn main() -> !{IO} ():
    let tree: Tree = Branch(Leaf(1), Branch(Leaf(2), Leaf(3)))
    let total: Int = sum_tree(tree)
    print_int(total)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "6");
}

#[test]
fn test_recursive_linked_list() {
    let src = r#"
mod test

type IntList = Nil | Cons(Int, IntList)

fn length(lst: IntList) -> Int:
    match lst:
        Nil:
            0
        Cons(head, tail):
            1 + length(tail)

fn main() -> !{IO} ():
    let lst: IntList = Cons(1, Cons(2, Cons(3, Nil)))
    print_int(length(lst))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "3");
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_recursive_type_basic -- --nocapture 2>&1 | head -30`
Expected: FAIL (likely type error or infinite recursion during type resolution)

- [ ] **Step 3: Implement recursive type support in type checker**

The key challenge: when resolving `type Tree = Leaf(Int) | Branch(Tree, Tree)`, the type checker encounters `Tree` while it's still being defined.

**Approach:**
1. In the type checker's enum declaration handling, add the enum name to a "currently being defined" set before resolving variant types.
2. When resolving a `TypeExpr::Named(name)` that matches a name in the "currently being defined" set, return a self-referential `Ty::Enum` placeholder instead of erroring.
3. After all variants are resolved, remove the name from the set.

The implementing agent should:
- Find where enum types are registered in `checker.rs` (search for `ItemKind::EnumDecl` or `ItemKind::TypeDecl`)
- Add a `HashSet<String>` field like `defining_types` to the checker
- Before processing variants, insert the type name
- In `resolve_type_expr` for `TypeExpr::Named`, check if the name is in `defining_types` and return the appropriate recursive reference
- After processing variants, remove from `defining_types`

- [ ] **Step 4: Test type checker passes**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_recursive_type_basic -- --nocapture 2>&1 | head -30`
Expected: May now fail at codegen rather than type checking

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(phase-pp): support recursive union types in type checker"
```

---

## Task 12: Recursive Union Types — Codegen + Tests

**Files:**
- Modify: `src/codegen/cranelift.rs`
- Modify: `src/ir/builder/mod.rs` (if needed)

- [ ] **Step 1: Understand the current enum codegen**

Read the `ConstructVariant` and `GetVariantField` handling in `codegen/cranelift.rs`. Currently:
- `ConstructVariant`: allocates `(1 + field_count) * 8` bytes, stores tag at offset 0, fields at offsets 8, 16, etc.
- `GetVariantField`: loads field at `(index + 1) * 8` offset.

For recursive types like `Branch(Tree, Tree)`, the fields ARE pointers to other enum values (which are already heap-allocated pointers). So the existing codegen should work as-is — when you construct `Branch(left, right)`, `left` and `right` are already i64-sized pointers, and they get stored in the variant's payload slots.

**The key insight:** Since all enum values are already heap-allocated (via `ConstructVariant`'s malloc), recursive types don't need boxing — they're already boxed. The field values are pointers that fit in 8 bytes.

- [ ] **Step 2: Verify tests pass or identify remaining issues**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration test_recursive -- --nocapture`

If tests pass: the existing codegen handles recursive types correctly (because enums are already heap-allocated).

If tests fail: identify the specific failure (IR builder, codegen, or pattern matching) and fix. Common issues:
- IR builder may need to handle recursive type references when building variant constructors
- Pattern matching may need adjustment for multi-field destructuring of recursive variants

- [ ] **Step 3: Add more comprehensive tests**

```rust
#[test]
fn test_recursive_type_nested() {
    let src = r#"
mod test

type Expr = Num(Int) | Add(Expr, Expr) | Mul(Expr, Expr)

fn eval(e: Expr) -> Int:
    match e:
        Num(n):
            n
        Add(a, b):
            eval(a) + eval(b)
        Mul(a, b):
            eval(a) * eval(b)

fn main() -> !{IO} ():
    let expr: Expr = Add(Num(2), Mul(Num(3), Num(4)))
    print_int(eval(expr))
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "14");  // 2 + (3 * 4) = 14
}
```

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(phase-pp): recursive union types — codegen and tests"
```

---

## Task 13: JSON — C Runtime Parser/Serializer

**Files:**
- Modify: `runtime/gradient_runtime.c`

This is a standalone C task — implement a recursive-descent JSON parser that constructs Gradient enum values.

- [ ] **Step 1: Define JsonValue memory layout**

The JsonValue enum will be represented as a heap-allocated tagged union, same as all Gradient enums:

```
Layout: [tag: i64, payload_0: i64, ...]

Tags:
  0 = JsonNull     (no payload)
  1 = JsonBool     (payload: i64 — 0 or 1)
  2 = JsonInt      (payload: i64)
  3 = JsonFloat    (payload: f64 as i64 bits)
  4 = JsonString   (payload: char* as i64)
  5 = JsonArray    (payload: List[JsonValue]* as i64)
  6 = JsonObject   (payload: Map[String, JsonValue]* as i64)
```

- [ ] **Step 2: Implement JSON parser in C**

Add to `runtime/gradient_runtime.c`:

```c
/* ── Phase PP: JSON Parser/Serializer ──────────────────────────────────── */

#include <math.h>

/* JsonValue tag constants */
#define JSON_NULL   0
#define JSON_BOOL   1
#define JSON_INT    2
#define JSON_FLOAT  3
#define JSON_STRING 4
#define JSON_ARRAY  5
#define JSON_OBJECT 6

/* Allocate a JsonValue with given tag and payload slot count */
static void* json_alloc(int64_t tag, int n_payload) {
    void* ptr = malloc((size_t)(8 + n_payload * 8));
    ((int64_t*)ptr)[0] = tag;
    return ptr;
}

static void* json_null(void) { return json_alloc(JSON_NULL, 0); }

static void* json_bool(int64_t val) {
    void* p = json_alloc(JSON_BOOL, 1);
    ((int64_t*)p)[1] = val;
    return p;
}

static void* json_int(int64_t val) {
    void* p = json_alloc(JSON_INT, 1);
    ((int64_t*)p)[1] = val;
    return p;
}

static void* json_float(double val) {
    void* p = json_alloc(JSON_FLOAT, 1);
    /* Store f64 bits as i64 */
    memcpy(&((int64_t*)p)[1], &val, sizeof(double));
    return p;
}

static void* json_string(const char* s) {
    void* p = json_alloc(JSON_STRING, 1);
    ((int64_t*)p)[1] = (int64_t)(intptr_t)strdup(s);
    return p;
}

static void* json_array(void* list_ptr) {
    void* p = json_alloc(JSON_ARRAY, 1);
    ((int64_t*)p)[1] = (int64_t)(intptr_t)list_ptr;
    return p;
}

static void* json_object(void* map_ptr) {
    void* p = json_alloc(JSON_OBJECT, 1);
    ((int64_t*)p)[1] = (int64_t)(intptr_t)map_ptr;
    return p;
}

/* ── JSON Parser ─────────────────────────────────────────────────────── */

typedef struct {
    const char* input;
    size_t pos;
    char error[256];
} JsonParser;

static void json_skip_ws(JsonParser* p) {
    while (p->input[p->pos] && strchr(" \t\n\r", p->input[p->pos]))
        p->pos++;
}

static void* json_parse_value(JsonParser* p);

static char* json_parse_string_raw(JsonParser* p) {
    if (p->input[p->pos] != '"') {
        snprintf(p->error, sizeof(p->error), "Expected '\"' at pos %zu", p->pos);
        return NULL;
    }
    p->pos++; /* skip opening quote */
    size_t cap = 64;
    size_t len = 0;
    char* buf = (char*)malloc(cap);
    while (p->input[p->pos] && p->input[p->pos] != '"') {
        if (p->input[p->pos] == '\\') {
            p->pos++;
            switch (p->input[p->pos]) {
                case '"': buf[len++] = '"'; break;
                case '\\': buf[len++] = '\\'; break;
                case '/': buf[len++] = '/'; break;
                case 'b': buf[len++] = '\b'; break;
                case 'f': buf[len++] = '\f'; break;
                case 'n': buf[len++] = '\n'; break;
                case 'r': buf[len++] = '\r'; break;
                case 't': buf[len++] = '\t'; break;
                default: buf[len++] = p->input[p->pos]; break;
            }
        } else {
            buf[len++] = p->input[p->pos];
        }
        p->pos++;
        if (len >= cap - 1) { cap *= 2; buf = (char*)realloc(buf, cap); }
    }
    if (p->input[p->pos] != '"') {
        snprintf(p->error, sizeof(p->error), "Unterminated string at pos %zu", p->pos);
        free(buf);
        return NULL;
    }
    p->pos++; /* skip closing quote */
    buf[len] = '\0';
    return buf;
}

static void* json_parse_array(JsonParser* p) {
    p->pos++; /* skip '[' */
    json_skip_ws(p);
    /* Build a dynamic list of JsonValue pointers */
    int64_t cap = 8;
    int64_t len = 0;
    int64_t* items = (int64_t*)malloc((size_t)(cap * 8));
    if (p->input[p->pos] != ']') {
        while (1) {
            json_skip_ws(p);
            void* val = json_parse_value(p);
            if (!val && p->error[0]) { free(items); return NULL; }
            if (len >= cap) { cap *= 2; items = (int64_t*)realloc(items, (size_t)(cap * 8)); }
            items[len++] = (int64_t)(intptr_t)val;
            json_skip_ws(p);
            if (p->input[p->pos] == ',') { p->pos++; continue; }
            if (p->input[p->pos] == ']') break;
            snprintf(p->error, sizeof(p->error), "Expected ',' or ']' at pos %zu", p->pos);
            free(items);
            return NULL;
        }
    }
    p->pos++; /* skip ']' */
    /* Build Gradient List */
    void* list = malloc((size_t)(16 + len * 8));
    int64_t* hdr = (int64_t*)list;
    hdr[0] = len;
    hdr[1] = len;
    memcpy(hdr + 2, items, (size_t)(len * 8));
    free(items);
    return json_array(list);
}

static void* json_parse_object(JsonParser* p) {
    p->pos++; /* skip '{' */
    json_skip_ws(p);
    /* Use GradientMap (same as map_new) */
    void* map = __gradient_map_new();
    if (p->input[p->pos] != '}') {
        while (1) {
            json_skip_ws(p);
            char* key = json_parse_string_raw(p);
            if (!key) return NULL;
            json_skip_ws(p);
            if (p->input[p->pos] != ':') {
                snprintf(p->error, sizeof(p->error), "Expected ':' at pos %zu", p->pos);
                free(key);
                return NULL;
            }
            p->pos++; /* skip ':' */
            json_skip_ws(p);
            void* val = json_parse_value(p);
            if (!val && p->error[0]) { free(key); return NULL; }
            /* Store JsonValue ptr as i64 in map (reuse map_set_int pattern) */
            GradientMap* m = (GradientMap*)map;
            /* Direct insert (not copy-on-write since we own this map) */
            if (m->size >= m->capacity) map_grow(m);
            int64_t idx = m->size++;
            m->keys[idx] = key;
            m->values[idx] = (int64_t)(intptr_t)val;
            json_skip_ws(p);
            if (p->input[p->pos] == ',') { p->pos++; continue; }
            if (p->input[p->pos] == '}') break;
            snprintf(p->error, sizeof(p->error), "Expected ',' or '}' at pos %zu", p->pos);
            return NULL;
        }
    }
    p->pos++; /* skip '}' */
    return json_object(map);
}

static void* json_parse_number(JsonParser* p) {
    const char* start = p->input + p->pos;
    char* end;
    /* Try integer first */
    int64_t ival = strtoll(start, &end, 10);
    if (end > start && *end != '.' && *end != 'e' && *end != 'E') {
        p->pos += (size_t)(end - start);
        return json_int(ival);
    }
    /* Parse as float */
    double fval = strtod(start, &end);
    if (end > start) {
        p->pos += (size_t)(end - start);
        return json_float(fval);
    }
    snprintf(p->error, sizeof(p->error), "Invalid number at pos %zu", p->pos);
    return NULL;
}

static void* json_parse_value(JsonParser* p) {
    json_skip_ws(p);
    char c = p->input[p->pos];
    if (c == '"') {
        char* s = json_parse_string_raw(p);
        if (!s) return NULL;
        return json_string(s);
    }
    if (c == '[') return json_parse_array(p);
    if (c == '{') return json_parse_object(p);
    if (c == 't' && strncmp(p->input + p->pos, "true", 4) == 0) {
        p->pos += 4; return json_bool(1);
    }
    if (c == 'f' && strncmp(p->input + p->pos, "false", 5) == 0) {
        p->pos += 5; return json_bool(0);
    }
    if (c == 'n' && strncmp(p->input + p->pos, "null", 4) == 0) {
        p->pos += 4; return json_null();
    }
    if (c == '-' || (c >= '0' && c <= '9')) return json_parse_number(p);
    snprintf(p->error, sizeof(p->error), "Unexpected char '%c' at pos %zu", c, p->pos);
    return NULL;
}

/*
 * __gradient_json_parse(input, out_ok) -> void*
 * Parses JSON string. Sets *out_ok=1 on success.
 * Returns JsonValue ptr on success, error string ptr on failure.
 */
void* __gradient_json_parse(const char* input, int64_t* out_ok) {
    JsonParser parser = { .input = input, .pos = 0, .error = {0} };
    void* result = json_parse_value(&parser);
    if (!result || parser.error[0]) {
        *out_ok = 0;
        return (void*)(intptr_t)strdup(parser.error[0] ? parser.error : "Parse error");
    }
    *out_ok = 1;
    return result;
}

/* ── JSON Serializer ─────────────────────────────────────────────────── */

/* Forward declaration */
static void json_stringify_value(void* val, char** buf, size_t* len, size_t* cap);

static void json_buf_append(char** buf, size_t* len, size_t* cap, const char* s) {
    size_t slen = strlen(s);
    while (*len + slen >= *cap) { *cap *= 2; *buf = (char*)realloc(*buf, *cap); }
    memcpy(*buf + *len, s, slen);
    *len += slen;
}

static void json_stringify_string(const char* s, char** buf, size_t* len, size_t* cap) {
    json_buf_append(buf, len, cap, "\"");
    for (const char* p = s; *p; p++) {
        switch (*p) {
            case '"':  json_buf_append(buf, len, cap, "\\\""); break;
            case '\\': json_buf_append(buf, len, cap, "\\\\"); break;
            case '\n': json_buf_append(buf, len, cap, "\\n"); break;
            case '\r': json_buf_append(buf, len, cap, "\\r"); break;
            case '\t': json_buf_append(buf, len, cap, "\\t"); break;
            default: {
                char c[2] = {*p, '\0'};
                json_buf_append(buf, len, cap, c);
            }
        }
    }
    json_buf_append(buf, len, cap, "\"");
}

static void json_stringify_value(void* val, char** buf, size_t* len, size_t* cap) {
    int64_t tag = ((int64_t*)val)[0];
    switch (tag) {
        case JSON_NULL:
            json_buf_append(buf, len, cap, "null");
            break;
        case JSON_BOOL:
            json_buf_append(buf, len, cap, ((int64_t*)val)[1] ? "true" : "false");
            break;
        case JSON_INT: {
            char tmp[32];
            snprintf(tmp, sizeof(tmp), "%lld", (long long)((int64_t*)val)[1]);
            json_buf_append(buf, len, cap, tmp);
            break;
        }
        case JSON_FLOAT: {
            double f;
            memcpy(&f, &((int64_t*)val)[1], sizeof(double));
            char tmp[64];
            snprintf(tmp, sizeof(tmp), "%g", f);
            json_buf_append(buf, len, cap, tmp);
            break;
        }
        case JSON_STRING: {
            const char* s = (const char*)(intptr_t)((int64_t*)val)[1];
            json_stringify_string(s, buf, len, cap);
            break;
        }
        case JSON_ARRAY: {
            void* list = (void*)(intptr_t)((int64_t*)val)[1];
            int64_t* hdr = (int64_t*)list;
            int64_t count = hdr[0];
            int64_t* data = hdr + 2;
            json_buf_append(buf, len, cap, "[");
            for (int64_t i = 0; i < count; i++) {
                if (i > 0) json_buf_append(buf, len, cap, ",");
                json_stringify_value((void*)(intptr_t)data[i], buf, len, cap);
            }
            json_buf_append(buf, len, cap, "]");
            break;
        }
        case JSON_OBJECT: {
            GradientMap* m = (GradientMap*)(intptr_t)((int64_t*)val)[1];
            json_buf_append(buf, len, cap, "{");
            for (int64_t i = 0; i < m->size; i++) {
                if (i > 0) json_buf_append(buf, len, cap, ",");
                json_stringify_string(m->keys[i], buf, len, cap);
                json_buf_append(buf, len, cap, ":");
                json_stringify_value((void*)(intptr_t)m->values[i], buf, len, cap);
            }
            json_buf_append(buf, len, cap, "}");
            break;
        }
    }
}

char* __gradient_json_stringify(void* val) {
    size_t cap = 256;
    size_t len = 0;
    char* buf = (char*)malloc(cap);
    json_stringify_value(val, &buf, &len, &cap);
    buf[len] = '\0';
    return buf;
}
```

- [ ] **Step 3: Verify C compilation**

Run: `cd codebase/compiler && cc -c runtime/gradient_runtime.c -o /tmp/test_runtime.o -lm && echo "OK"`
Expected: OK

- [ ] **Step 4: Commit**

```bash
git add runtime/gradient_runtime.c
git commit -m "feat(phase-pp): add JSON parser and serializer to C runtime"
```

---

## Task 14: JSON — Builtins + Tests

**Files:**
- Modify: `src/typechecker/env.rs`
- Modify: `src/ir/builder/mod.rs`
- Modify: `src/codegen/cranelift.rs`
- Modify: `tests/phase_pp_integration.rs`

- [ ] **Step 1: Register JsonValue as a builtin enum type**

In `src/typechecker/env.rs`, in the method that registers builtin enums (where `Option` and `Result` are registered), add:

```rust
        // Register JsonValue builtin recursive enum
        let json_value_ty = Ty::Enum {
            name: "JsonValue".into(),
            variants: vec![
                ("JsonNull".into(), None),
                ("JsonBool".into(), Some(Ty::Bool)),
                ("JsonInt".into(), Some(Ty::Int)),
                ("JsonFloat".into(), Some(Ty::Float)),
                ("JsonString".into(), Some(Ty::String)),
                ("JsonArray".into(), Some(Ty::List(Box::new(
                    Ty::Enum { name: "JsonValue".into(), variants: vec![] } // self-reference placeholder
                )))),
                ("JsonObject".into(), Some(Ty::Map(
                    Box::new(Ty::String),
                    Box::new(Ty::Enum { name: "JsonValue".into(), variants: vec![] }) // self-reference
                ))),
            ],
        };
        self.define_enum("JsonValue".into(), json_value_ty.clone());
        // Register variant constructors
        self.define_binding("JsonNull".into(), json_value_ty.clone());
        // ... register each variant constructor
```

**Note:** The exact registration depends on how recursive types are resolved in Task 11. The implementing agent should adapt this based on what was implemented for recursive union types. If user-defined recursive enums work correctly, JsonValue could also be defined as a builtin that goes through the same path.

- [ ] **Step 2: Write failing tests**

```rust
// ── JSON ────────────────────────────────────────────────────────────

#[test]
fn test_json_parse_stringify() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    let input: String = "{\"name\":\"gradient\",\"version\":1}"
    match json_parse(input):
        Ok(val):
            let output: String = json_stringify(val)
            println(output)
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    // JSON key order may vary; check both possible orderings
    let trimmed = out.trim();
    assert!(
        trimmed.contains("\"name\"") && trimmed.contains("\"gradient\""),
        "Expected name:gradient in: {}", trimmed
    );
}

#[test]
fn test_json_get() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("{\"x\":42}"):
        Ok(val):
            match json_get(val, "x"):
                Some(xval):
                    println(json_type(xval))
                None:
                    println("not found")
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "int");
}

#[test]
fn test_json_parse_array() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("[1, 2, 3]"):
        Ok(val):
            println(json_type(val))
            println(json_stringify(val))
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "array");
    assert_eq!(lines[1], "[1,2,3]");
}

#[test]
fn test_json_is_null() {
    let src = r#"
mod test
fn main() -> !{IO} ():
    match json_parse("null"):
        Ok(val):
            print_bool(json_is_null(val))
        Err(msg):
            println(msg)
"#;
    let (out, code) = compile_and_run(src);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "true");
}
```

- [ ] **Step 3: Register JSON builtins in env.rs**

```rust
        // ── Phase PP: JSON ───────────────────────────────────────────────
        // json_parse(String) -> Result[JsonValue, String]
        self.define_fn("json_parse".into(), FnSig {
            type_params: vec![],
            params: vec![("input".into(), Ty::String)],
            ret: /* Result[JsonValue, String] */,
            effects: vec![],
        });

        // json_stringify(JsonValue) -> String
        self.define_fn("json_stringify".into(), FnSig {
            type_params: vec![],
            params: vec![("val".into(), json_value_ty.clone())],
            ret: Ty::String,
            effects: vec![],
        });

        // json_get(JsonValue, String) -> Option[JsonValue]
        self.define_fn("json_get".into(), FnSig {
            type_params: vec![],
            params: vec![
                ("val".into(), json_value_ty.clone()),
                ("key".into(), Ty::String),
            ],
            ret: /* Option[JsonValue] */,
            effects: vec![],
        });

        // json_get_index(JsonValue, Int) -> Option[JsonValue]
        self.define_fn("json_get_index".into(), FnSig {
            type_params: vec![],
            params: vec![
                ("val".into(), json_value_ty.clone()),
                ("index".into(), Ty::Int),
            ],
            ret: /* Option[JsonValue] */,
            effects: vec![],
        });

        // json_is_null(JsonValue) -> Bool
        self.define_fn("json_is_null".into(), FnSig {
            type_params: vec![],
            params: vec![("val".into(), json_value_ty.clone())],
            ret: Ty::Bool,
            effects: vec![],
        });

        // json_type(JsonValue) -> String
        self.define_fn("json_type".into(), FnSig {
            type_params: vec![],
            params: vec![("val".into(), json_value_ty.clone())],
            ret: Ty::String,
            effects: vec![],
        });
```

- [ ] **Step 4: Implement codegen**

Key codegen patterns:

```rust
                        "json_parse" => {
                            // Call __gradient_json_parse(input, &out_ok) -> ptr
                            // Construct Result[JsonValue, String]:
                            //   out_ok=1 -> Ok(json_ptr)
                            //   out_ok=0 -> Err(error_string_ptr)
                            // Follow the same pattern as process_exec's Result construction.
                        }

                        "json_stringify" => {
                            // Call __gradient_json_stringify(val) -> char*
                            let val = resolve_value(&value_map, &args[0])?;
                            let func_id = *self.declared_functions.get("__gradient_json_stringify")
                                .ok_or("__gradient_json_stringify not declared")?;
                            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                            let call = builder.ins().call(func_ref, &[val]);
                            let result = builder.inst_results(call).to_vec()[0];
                            value_map.insert(*dst, result);
                        }

                        "json_is_null" => {
                            // Load tag from JsonValue ptr, compare to 0 (JSON_NULL)
                            let val = resolve_value(&value_map, &args[0])?;
                            let tag = builder.ins().load(cl_types::I64, MemFlags::new(), val, 0i32);
                            let zero = builder.ins().iconst(cl_types::I64, 0);
                            let result = builder.ins().icmp(IntCC::Equal, tag, zero);
                            value_map.insert(*dst, result);
                        }

                        "json_type" => {
                            // Load tag, call a C helper that returns type string
                            // Or implement inline with a branch table
                        }

                        "json_get" => {
                            // Check tag == JSON_OBJECT (6)
                            // If object: get Map ptr from payload, call map_get
                            // Wrap result in Option
                        }

                        "json_get_index" => {
                            // Check tag == JSON_ARRAY (5)
                            // If array: get List ptr from payload, bounds check, get element
                            // Wrap result in Option
                        }
```

The implementing agent should add C helpers for `json_get`, `json_get_index`, and `json_type` in the runtime to avoid complex inline codegen:

```c
void* __gradient_json_get(void* val, const char* key, int64_t* out_found);
void* __gradient_json_get_index(void* val, int64_t index, int64_t* out_found);
char* __gradient_json_type(void* val);
```

- [ ] **Step 5: Run tests, commit**

```bash
git commit -m "feat(phase-pp): add JSON builtins — parse, stringify, accessors"
```

---

## Task 15: Update Roadmap + Final Verification

**Files:**
- Modify: `docs/roadmap.md`

- [ ] **Step 1: Run full test suite**

Run: `cd codebase/compiler && cargo test 2>&1 | tail -10`
Expected: All tests pass (existing 830+ plus ~105 new)

- [ ] **Step 2: Run only Phase PP tests**

Run: `cd codebase/compiler && cargo test --test phase_pp_integration 2>&1 | tail -20`
Expected: All Phase PP tests pass

- [ ] **Step 3: Update roadmap**

Add Phase PP entry to `docs/roadmap.md` following the existing format:

```markdown
### Phase PP — Standard Library Expansion

**Sub-projects:**
1. Math library: trig (sin, cos, tan, asin, acos, atan, atan2), logarithmic (log, log2, log10, exp), rounding (floor, ceil, round), constants (math_pi, math_e), random (random_int, random_float), additional (gcd, float_mod)
2. String utilities: string_join, string_repeat, string_reverse, string_pad_left, string_pad_right, string_is_empty, string_count, char_code, from_char_code, string_lines
3. Data structures: Set[T] (set_new, set_add, set_remove, set_contains, set_size, set_to_list), Queue[T] (queue_new, queue_push, queue_pop, queue_peek, queue_size, queue_is_empty), Stack[T] (stack_new, stack_push, stack_pop, stack_peek, stack_size, stack_is_empty)
4. Date/Time: time_now, time_now_ms, time_format, time_parse, sleep_ms, time_diff
5. Environment & Process: env_get, env_set, env_remove, env_vars, cwd, chdir, process_exec, process_exec_status
6. Recursive union types: language feature enabling self-referential enum definitions
7. JSON: JsonValue type, json_parse, json_stringify, json_get, json_get_index, json_is_null, json_type

**Totals:** ~72 new builtins, ~105 new tests, 3 new type constructors (Set, Queue, Stack), 1 language feature (recursive unions)
```

- [ ] **Step 4: Commit**

```bash
git add docs/roadmap.md
git commit -m "docs: update roadmap with Phase PP stdlib expansion"
```
