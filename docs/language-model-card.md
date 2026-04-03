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
| `if` | Conditional expression | `if x > 0:` |
| `else` | Alternative branch | `else:` |
| `for` | Iterator loop | `for i in range(10):` |
| `while` | Condition loop | `while i < 10:` |
| `in` | Iteration binding | `for x in items:` |
| `ret` | Return expression | `ret x + 1` |
| `match` | Pattern matching | `match x:` |
| `type` | Type/enum definition | `type Color = Red \| Blue` |
| `mod` | Module declaration | `mod math` |
| `use` | Import module | `use std.io` |
| `true` / `false` | Booleans | `let flag = true` |
| `and` / `or` / `not` | Logical operators | `if a and not b:` |
| `actor` | Actor definition | `actor Server:` |
| `spawn` | Create actor | `let a = spawn Actor` |
| `send` | Message actor | `send(a, Message)` |

### 1.2 Operators

**Precedence (lowest to highest):**

| Level | Operators | Assoc | Description |
|-------|-----------|-------|-------------|
| 1 | `or` | Left | Logical OR |
| 2 | `and` | Left | Logical AND |
| 3 | `not` | Prefix | Logical NOT |
| 4 | `== != < > <= >=` | **Non-assoc** | Comparison |
| 5 | `+ -` | Left | Add, subtract, concat |
| 6 | `* / %` | Left | Multiply, divide, modulo |
| 7 | `-` | Prefix | Unary negation |
| 8 | `()` `.` | Left | Call, field access |

**Important:** Comparisons are **non-associative** - `a < b < c` is an error. Use `a < b and b < c`.

### 1.3 Built-in Types

| Type | Description | Literals |
|------|-------------|----------|
| `Int` | 64-bit signed | `42`, `-7`, `1_000_000` |
| `Float` | 64-bit double | `3.14`, `-0.5`, `1e10` |
| `String` | UTF-8 immutable | `"hello"`, `"line\n"` |
| `Bool` | Boolean | `true`, `false` |
| `()` | Unit (void) | `()` |

### 1.4 Sigils

| Sigil | Meaning | Example |
|-------|---------|---------|
| `@` | Annotation | `@extern`, `@cap(IO)` |
| `!` | Effect set | `-> !{IO, Net} Type` |
| `?` | Typed hole | `let x: Int = ?todo` |
| `$` | Compile-time | `$sizeof(Int)` |

---

## Section 2: Grammar Patterns

### Pattern 1: Function Definition

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

// Multiple parameters with default values
fn max(a: Int, b: Int) -> Int:
    if a > b:
        ret a
    else:
        ret b
```

### Pattern 2: Let Binding

```gradient
let name: Type = value           // Explicit type
let name = value                  // Inferred type
let mut name: Type = value        // Mutable
```

**Examples:**
```gradient
let x: Int = 42
let greeting = "Hello"
let sum = add(3, 4)               // Type inferred from add()

let mut counter = 0               // Mutable
while counter < 10:
    print_int(counter)
    counter = counter + 1         // Reassignment
```

### Pattern 3: If / Else Expression

```gradient
if condition:
    then_branch

if condition:
    then_branch
else:
    else_branch

if condition:
    branch1
else if condition2:
    branch2
else:
    branch3
```

**Examples:**
```gradient
// As statement
if x > 0:
    print("positive")
else:
    print("non-positive")

// As expression
let sign = if x > 0:
    "positive"
else if x < 0:
    "negative"
else:
    "zero"
```

### Pattern 4: Pattern Matching

```gradient
match scrutinee:
    pattern1:
        body1
    pattern2:
        body2
    _:
        default_body
```

**Examples:**
```gradient
// Match on integer
type Code = Success | Error | Timeout

match status_code:
    200:
        "OK"
    404:
        "Not Found"
    _:
        "Other"

// Match on enum
type Option = Some(Int) | None

match opt:
    Some(n):
        n * 2
    None:
        0
```

### Pattern 5: Loop Patterns

```gradient
// For loop
for i in range(10):
    print_int(i)

// While loop
let mut i = 0
while i < 10:
    print_int(i)
    i = i + 1
```

### Pattern 6: Type Definitions

```gradient
// Type alias
type Count = Int
type Id = String

// Enum with unit variants
type Color = Red | Green | Blue

// Enum with tuple variants
type Option = Some(Int) | None
type Result = Ok(String) | Err(ErrorCode)

// Recursive enum
type List = Cons(Int, List) | Nil
```

### Pattern 7: Module Structure

```gradient
// file: math.gr
mod math

use std.io

fn add(a: Int, b: Int) -> Int:
    ret a + b

// file: main.gr
mod main

use math

fn main() -> !{IO} ():
    let sum = math.add(1, 2)
    print_int(sum)
```

---

## Section 3: Type System

### 3.1 Type Inference Rules

Gradient uses Hindley-Milner bidirectional type inference.

**Principle:** Annotate parameters, infer the rest.

```gradient
// Parameter annotations required
fn add(x: Int, y: Int) -> Int:     // Return type can be inferred
    ret x + y

// Local bindings inferred
let sum = add(3, 4)                 // sum: Int (inferred)
let msg = "hello"                   // msg: String (inferred)
```

### 3.2 Type Constraints

| Constraint | Example | Notes |
|------------|---------|-------|
| Parameter | `fn f(x: Type)` | Required |
| Return | `-> Type` | Optional (inferred) |
| Local let | `let x: Type = ...` | Optional |
| Effect | `-> !{Effects} Type` | Required for effects |

### 3.3 Polymorphism (Future)

```gradient
// Type variables
fn identity[T](x: T) -> T:
    ret x

// Multiple type variables
fn pair[A, B](a: A, b: B) -> (A, B):
    ret (a, b)
```

### 3.4 Type Error Examples

**Error 1: Type mismatch**
```gradient
let x: Int = "hello"        // ERROR: expected Int, found String
```

**Error 2: Missing annotation**
```gradient
fn add(x, y) -> Int:        // ERROR: parameters need annotations
    ret x + y
```

**Error 3: Inconsistent branches**
```gradient
let x = if condition:
    42                      // Int
else:
    "hello"                 // String - ERROR: inconsistent types
```

**Error 4: Missing effect annotation**
```gradient
fn greet() -> ():           // ERROR: missing !{IO}
    print("hi")             // print requires IO effect
```

---

## Section 4: Effect System

### 4.1 Core Principle

**Functions are pure by default.** The compiler proves purity. If a function has no `!{...}` annotation, calling effectful code from it is a compile error.

### 4.2 Canonical Effects

| Effect | Operations | Module |
|--------|------------|--------|
| `IO` | `print`, `println`, `read_line` | std.io |
| `Net` | `http_get`, `tcp_connect` | std.net |
| `FS` | `file_read`, `file_write` | std.io |
| `Mut` | Mutable shared state | (global) |
| `Time` | `now`, `sleep_ms` | std.time |

### 4.3 Effect Examples

**Example 1: Pure function**
```gradient
fn square(n: Int) -> Int:
    ret n * n
// Compiler proves: no IO, no Net, no FS, no Mut, no Time
```

**Example 2: Effectful function**
```gradient
fn log_and_square(n: Int) -> !{IO} Int:
    print_int(n)            // Requires IO
    ret n * n
// Must declare !{IO} in signature
```

**Example 3: Multiple effects**
```gradient
fn fetch_and_save(url: String, path: String) -> !{Net, FS, IO} ():
    let data = http_get(url)        // Net
    file_write(path, data)          // FS, IO
    print("Saved!")                  // IO
```

**Example 4: Effect polymorphism**
```gradient
fn apply_twice(f: fn(Int) -> Int, x: Int) -> Int:
    ret f(f(x))
// Inherits effects of f - if f is pure, this is pure
// If f has !{IO}, this has !{IO}
```

### 4.4 Module Capability Ceiling

```gradient
@cap(IO, Net)                      // Module can only use IO and Net
mod server

fn handler() -> !{IO} ():          // OK: within cap
    print("request")

fn fetch() -> !{Net} String:       // OK: within cap
    http_get("api.example.com")

fn save() -> !{FS} ():             // ERROR: FS not in @cap
    file_write("log.txt", "data")
```

### 4.5 Common Effect Patterns

| Pattern | Code | Effects |
|---------|------|---------|
| Pure computation | `fn f(x: Int) -> Int` | {} |
| Console I/O | `fn main() -> !{IO} ()` | {IO} |
| File processing | `fn process(p: String) -> !{FS, IO} ()` | {FS, IO} |
| Network client | `fn fetch(u: String) -> !{Net, IO} String` | {Net, IO} |
| Full access | `fn daemon() -> !{IO, Net, FS, Time} ()` | {IO, Net, FS, Time} |

---

## Section 5: Memory Model

### 5.1 Three-Tier Model

| Tier | Strategy | Safety | Overhead | Use |
|------|----------|--------|----------|-----|
| 1 | Arena allocation | Safe deallocation | None | 80% of code |
| 2 | Generational references | Use-after-free prevention | 8 bytes + check | 15% |
| 3 | Linear types | Full control | None | 5% (kernels) |

### 5.2 Arena Allocation (Default)

```gradient
fn process() -> !{IO} ():
    let arena = Arena.new()
    let data = arena.alloc(MyStruct)
    // ... use data ...
    defer arena.deinit()      // Bulk free
```

**Properties:**
- Bulk deallocation at scope exit
- No individual `free`
- Zero annotation burden
- Compile-time ownership inference

### 5.3 Generational References

```gradient
fn build_graph() -> Graph:
    let g = Graph.new()
    let n1 = g.add_node(1)
    let n2 = g.add_node(2)
    g.add_edge(n1, n2)        // Safe even with cycles
    ret g
```

**Safety mechanism:**
- Every reference has generation number
- Checked at dereference
- Use-after-free = defined trap (not UB)

### 5.4 Linear Types (Opt-in)

```gradient
@linear
fn acquire() -> Resource:
    // ...

fn use(r: Resource) -> ():     // Consumes r
    // ...

fn main() -> !{IO} ():
    let r = acquire()         // r: Resource (linear)
    use(r)                      // Consumes r
    // use(r)                  // ERROR: already consumed
```

---

## Section 6: Actors

### 6.1 Actor Definition

```gradient
actor ChatServer:
    state users: Map[UserId, User]
    state rooms: Map[RoomId, Room]
    
    on Join(user: User, room: RoomId):
        // Handle message
        
    on Leave(user: User):
        // Handle message
```

### 6.2 Actor Lifecycle

```gradient
fn main() -> !{IO} ():
    // 1. Spawn
    let server = spawn ChatServer
    
    // 2. Send (fire-and-forget)
    send(server, Join(user, room))
    
    // 3. Ask (request-response)
    let count = ask(server, GetUserCount)
    
    // 4. Stop
    stop(server)
```

### 6.3 Reference Capabilities

| Capability | Aliases | Mutable | Sendable | Use Case |
|------------|---------|---------|----------|----------|
| `iso` | None | Yes | Yes | Transfer ownership |
| `val` | Many | No | Yes | Share read-only |
| `ref` | Many | Yes | No | Actor-local state |
| `box` | Many | No | No | Read-only view |
| `trn` | None | Yes | No | Unique reference |
| `tag` | Opaque | Opaque | Yes | Actor identity |

### 6.4 Supervision Trees

```gradient
@supervision(strategy: one_for_one, max_restarts: 3)
actor Supervisor:
    on ChildFailed(child, error):
        match error:
            Recoverable:
                restart(child)
            Fatal:
                escalate(error)
```

**Strategies:**
- `one_for_one` - Restart only failed child
- `one_for_all` - Restart all children
- `rest_for_one` - Restart failed and younger siblings

### 6.5 Example: Counter Actor

```gradient
actor Counter:
    state count: Int = 0
    
    on Increment:
        count = count + 1
        
    on GetCount:
        ret count

fn main() -> !{IO} ():
    let c = spawn Counter
    send(c, Increment)
    send(c, Increment)
    let n = ask(c, GetCount)    // n = 2
```

---

## Section 7: Standard Library

### 7.1 Core Builtins (No Import Needed)

**Printing:**
```gradient
print(s: String) -> !{IO} ()
print_int(n: Int) -> !{IO} ()
print_float(f: Float) -> !{IO} ()
print_bool(b: Bool) -> !{IO} ()
```

**Math:**
```gradient
abs(n: Int) -> Int
min(a: Int, b: Int) -> Int
max(a: Int, b: Int) -> Int
mod_int(a: Int, b: Int) -> Int
sqrt(n: Float) -> Float
pow(base: Float, exp: Float) -> Float
```

**String:**
```gradient
to_string(n: Int) -> String
int_to_string(n: Int) -> String
float_to_string(f: Float) -> String
// Concatenate with +
let full = "Hello, " + name
```

**Iteration:**
```gradient
range(end: Int) -> Range           // 0 to end-1
range_from(start: Int, end: Int) -> Range
```

### 7.2 std.io Module

```gradient
println(s: String) -> !{IO} ()
read_line() -> !{IO} String
file_read(path: String) -> !{FS, IO} String
file_write(path: String, content: String) -> !{FS, IO} ()
file_exists(path: String) -> !{FS} Bool
```

### 7.3 std.collections Module

```gradient
// List
type List[T]
fn list_new[T]() -> List[T]
fn list_push[T](lst: List[T], item: T) -> List[T]
fn list_get[T](lst: List[T], idx: Int) -> Option[T]
fn list_length[T](lst: List[T]) -> Int

// Map
type Map[K, V]
fn map_new[K, V]() -> Map[K, V]
fn map_insert[K, V](m: Map[K, V], k: K, v: V) -> Map[K, V]
fn map_get[K, V](m: Map[K, V], k: K) -> Option[V]
```

### 7.4 std.option Module

```gradient
type Option[T] = Some(T) | None

fn is_some[T](opt: Option[T]) -> Bool
fn is_none[T](opt: Option[T]) -> Bool
fn unwrap[T](opt: Option[T]) -> T               // Panics if None
fn unwrap_or[T](opt: Option[T], default: T) -> T
```

### 7.5 std.result Module

```gradient
type Result[T, E] = Ok(T) | Err(E)

fn is_ok[T, E](res: Result[T, E]) -> Bool
fn is_err[T, E](res: Result[T, E]) -> Bool
fn unwrap[T, E](res: Result[T, E]) -> T         // Panics if Err
fn unwrap_or[T, E](res: Result[T, E], d: T) -> T
fn map_result[T, E, U](res: Result[T, E], f: fn(T) -> U) -> Result[U, E]
```

### 7.6 std.string Module

```gradient
fn trim(s: String) -> String
fn split(s: String, d: String) -> List[String]
fn join(parts: List[String], sep: String) -> String
fn replace(s: String, from: String, to: String) -> String
fn parse_int(s: String) -> Result[Int, String]
```

---

## Section 8: Anti-Patterns (What NOT to Do)

### Anti-Pattern 1: Missing Type Annotations on Parameters

```gradient
// WRONG
fn add(x, y):               // ERROR: parameters need type annotations
    ret x + y

// CORRECT
fn add(x: Int, y: Int) -> Int:
    ret x + y
```

### Anti-Pattern 2: Reassigning Immutable Bindings

```gradient
// WRONG
let x = 10
x = 20                      // ERROR: x is not mutable

// CORRECT
let mut x = 10
x = 20
```

### Anti-Pattern 3: Missing Effect Annotations

```gradient
// WRONG
fn log(msg: String) -> ():  // ERROR: missing !{IO}
    print(msg)

// CORRECT
fn log(msg: String) -> !{IO} ():
    print(msg)
```

### Anti-Pattern 4: Chained Comparisons

```gradient
// WRONG
if 0 < x < 10:              // ERROR: comparisons are non-associative

// CORRECT
if x > 0 and x < 10:
```

### Anti-Pattern 5: Missing Blocks

```gradient
// WRONG
if x > 0                    // ERROR: missing :
    print("positive")

// CORRECT
if x > 0:
    print("positive")
```

### Anti-Pattern 6: Wrong Indentation

```gradient
// WRONG
fn main() -> !{IO} ():
   print("hi")               // ERROR: must be 4 spaces, not 3

// CORRECT
fn main() -> !{IO} ():
    print("hi")               // Exactly 4 spaces
```

### Anti-Pattern 7: Semicolons

```gradient
// WRONG
let x = 10; let y = 20;     // ERROR: no semicolons in Gradient

// CORRECT
let x = 10
let y = 20
```

### Anti-Pattern 8: Braces for Blocks

```gradient
// WRONG
fn main() -> !{IO} () {
    print("hi")
}

// CORRECT
fn main() -> !{IO} ():
    print("hi")
```

### Anti-Pattern 9: Using Keywords as Identifiers

```gradient
// WRONG
let fn = 10                 // ERROR: fn is a keyword
let type = "value"          // ERROR: type is a keyword

// CORRECT
let func = 10
let kind = "value"
```

### Anti-Pattern 10: Non-exhaustive Pattern Matching

```gradient
// WRONG
type Color = Red | Green | Blue

match c:                    // Warning: missing Blue case
    Red:
        "red"
    Green:
        "green"

// CORRECT
match c:
    Red:
        "red"
    Green:
        "green"
    Blue:
        "blue"
    // OR use wildcard:
    _:
        "other"
```

### Anti-Pattern 11: Calling Pure from Effectful Only for Side Effects

```gradient
// WRONG (inefficient design)
fn process() -> !{IO} ():
    compute()               // compute returns value that's discarded
    // ...

fn compute() -> Int:         // Pure, but value ignored
    ret 42

// CORRECT
fn process() -> !{IO} ():
    let result = compute()  // Use the result
    print_int(result)
```

### Anti-Pattern 12: Using Option/Result Without Handling

```gradient
// WRONG
let x = list_get(lst, 5)    // Returns Option[T]
print_int(x)                // ERROR: can't print Option directly

// CORRECT
match list_get(lst, 5):
    Some(n):
        print_int(n)
    None:
        print("Not found")

// OR use unwrap_or
let x = unwrap_or(list_get(lst, 5), 0)
print_int(x)
```

### Anti-Pattern 13: String Concatenation with Non-Strings

```gradient
// WRONG
let msg = "Count: " + 42    // ERROR: can't concat String and Int

// CORRECT
let msg = "Count: " + to_string(42)
```

---

## Quick Reference Card

### Program Structure Template

```gradient
mod mymodule

use std.io
use std.collections

@cap(IO, FS)

fn helper(x: Int) -> Int:
    ret x * 2

fn main() -> !{IO} ():
    let result = helper(21)
    print_int(result)
```

### Indentation Rules

- Use **4 spaces** per level
- **No tabs**
- Every `:` at end of line starts new indented block
- Blank lines don't affect indentation tracking

### First Token Reference

| First token | What it means |
|-------------|---------------|
| `mod` | Module declaration |
| `use` | Import |
| `fn` | Function definition |
| `let` | Let binding |
| `if` | Conditional |
| `for` | Loop over iterator |
| `while` | Loop while condition |
| `match` | Pattern matching |
| `ret` | Return expression |
| `type` | Type/enum definition |
| `@` | Annotation |

---

*Gradient v0.1 - Language Model Card*
*For agent consumption and code generation*
