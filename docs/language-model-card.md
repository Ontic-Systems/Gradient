# Gradient Language Model Card

> Target: ~10K tokens for agent context windows
> Version: v0.1
> Purpose: Complete quick reference for AI agents generating Gradient code

---

## Section 1: Quick Reference

### 1.1 Keywords

| Keyword | Purpose | Example |
|---------|---------|---------|
| `fn` | Define function | `fn add(x: Int) -> Int:` |
| `let` | Immutable binding | `let x = 42` |
| `let mut` | Mutable binding | `let mut i = 0` |
| `if` / `else` | Conditional | `if x > 0:` / `else:` |
| `for` / `in` | Iterator loop | `for i in range(10):` |
| `while` | Condition loop | `while i < 10:` |
| `ret` | Return expression | `ret x + 1` |
| `match` | Pattern matching | `match x:` |
| `type` | Type/enum definition | `type Color = Red \| Blue` |
| `mod` | Module declaration | `mod math` |
| `use` | Import module | `use std.io` |
| `true` / `false` | Booleans | `let flag = true` |
| `and` / `or` / `not` | Logical operators | `if a and not b:` |

### 1.2 Operators (precedence high→low)

| Level | Operators | Assoc |
|-------|-----------|-------|
| 1 | `()` `.` | Left | Call, field access |
| 2 | `*` / `%` | Left | Multiply, divide, modulo |
| 3 | `+` `-` | Left | Add, subtract, concat |
| 4 | `== != < > <= >=` | **Non-assoc** | Comparison |
| 5 | `and` | Left | Logical AND |
| 6 | `or` | Left | Logical OR |

**Important:** `a < b < c` is an error. Use `a < b and b < c`.

### 1.3 Built-in Types

| Type | Description | Literals |
|------|-------------|----------|
| `Int` | 64-bit signed | `42`, `-7` |
| `Float` | 64-bit double | `3.14`, `-0.5` |
| `String` | UTF-8 immutable | `"hello"` |
| `Bool` | Boolean | `true`, `false` |
| `()` | Unit (void) | `()` |

### 1.4 Sigils

| Sigil | Meaning | Example |
|-------|---------|---------|
| `@` | Annotation | `@cap(IO)` |
| `!` | Effect set | `-> !{IO} Type` |
| `?` | Typed hole | `let x: Int = ?hole` |

---

## Section 2: Grammar Patterns

### Function Definition

```gradient
fn name(param1: Type1, param2: Type2) -> ReturnType:
    body

fn name(param1: Type1) -> !{Effect} ReturnType:
    body
```

**Examples:**
```gradient
// Pure function
fn square(n: Int) -> Int:
    ret n * n

// Effectful function
fn greet(name: String) -> !{IO} ():
    print("Hello, " + name)
```

### Let Binding

```gradient
let name: Type = value           // Explicit type
let name = value                  // Inferred type
let mut name: Type = value        // Mutable
```

### If / Else Expression

```gradient
let sign = if x > 0:
    "positive"
else if x < 0:
    "negative"
else:
    "zero"
```

### Pattern Matching

```gradient
match opt:
    Some(n):
        n * 2
    None:
        0
    _:
        default_value
```

### Type Definitions

```gradient
// Unit variants
type Color = Red | Green | Blue

// Tuple variants
type Option = Some(Int) | None
type Result = Ok(String) | Err(ErrorCode)
```

### Module Structure

```gradient
// file: math.gr
mod math
fn add(a: Int, b: Int) -> Int:
    ret a + b

// file: main.gr
mod main
use math
fn main() -> !{IO} ():
    print_int(math.add(1, 2))
```

---

## Section 3: Effect System

**Functions are pure by default.** The compiler proves purity.

### Canonical Effects

| Effect | Operations |
|--------|------------|
| `IO` | `print`, `println`, `read_line` |
| `Net` | `http_get`, `tcp_connect` |
| `FS` | `file_read`, `file_write` |
| `Mut` | Mutable shared state |
| `Time` | `now`, `sleep_ms` |

### Examples

```gradient
// Pure - no effects
fn square(n: Int) -> Int:
    ret n * n

// Effectful - requires !{IO}
fn greet(name: String) -> !{IO} ():
    print("Hello, " + name)
```

### Module Capability Ceiling

```gradient
@cap(IO, Net)                      // Module can only use IO and Net
mod server

fn save() -> !{FS} ():             // ERROR: FS not in @cap
    file_write("log.txt", "data")
```

---

## Section 4: Standard Library

### Core Builtins (No Import)

| Function | Signature |
|----------|-----------|
| `print` / `println` | `(String) -> !{IO} ()` |
| `print_int` / `print_float` / `print_bool` | `(T) -> !{IO} ()` |
| `abs` / `min` / `max` | `(Int, Int) -> Int` |
| `to_string` | `(Int) -> String` |
| `range` / `range_from` | `(Int) -> Range` / `(Int, Int) -> Range` |

### std.io Module

```gradient
read_line() -> !{IO} String
file_read(path: String) -> !{FS, IO} String
file_write(path: String, content: String) -> !{FS, IO} ()
```

### std.collections Module

```gradient
type List[T] / type Map[K, V]
fn list_new[T]() / list_push[T] / list_get[T] / list_length[T]
fn map_new[K,V]() / map_insert[K,V] / map_get[K,V]
```

---

## Section 5: Common Mistakes

| # | Mistake | Wrong | Correct |
|---|---------|-------|---------|
| 1 | Braces for blocks | `fn f() { }` | `fn f():` then indented body |
| 2 | Semicolons | `let x = 1;` | `let x = 1` |
| 3 | Missing effects | `fn f() -> (): print("")` | `fn f() -> !{IO} ():` |
| 4 | `return` keyword | `return x` | `ret x` |
| 5 | `var` keyword | `var x = 10` | `let mut x = 10` |
| 6 | Tabs | (tab) | 4 spaces |
| 7 | Chained comparisons | `a < b < c` | `a < b and b < c` |
| 8 | Missing colon | `if x > 0` | `if x > 0:` |
| 9 | Missing param types | `fn add(a, b)` | `fn add(a: Int, b: Int)` |

---

## Quick Reference Card

### Program Template

```gradient
mod mymodule
use std.io

@cap(IO)

fn helper(x: Int) -> Int:
    ret x * 2

fn main() -> !{IO} ():
    let result = helper(21)
    print_int(result)
```

### Indentation Rules

- **4 spaces** per level, **no tabs**
- Every `:` at end of line starts new indented block

### First Token Reference

| Token | Meaning |
|-------|---------|
| `mod` | Module declaration |
| `use` | Import |
| `fn` | Function definition |
| `let` | Let binding |
| `if` / `for` / `while` / `match` | Control flow |
| `ret` | Return expression |
| `type` | Type definition |
| `@` | Annotation |

---

*Gradient v0.1 - Language Model Card*
*For agent consumption and code generation*
