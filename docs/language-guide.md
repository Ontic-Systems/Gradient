# Gradient v0.1 Language Guide

> **STATUS:** partial — Surface syntax (functions, contracts, effects, generics, ADTs, modules, traits, actors) is implemented. Effect-tier expansion has progressed (`!{Heap}` gates heap allocation; `!{Stack}`/`!{Static}`/`!{Async}`/`!{Send}`/`!{Atomic}`/`!{Volatile}` are accepted marker effects with stdlib `atomic_i64_fetch_add`, `volatile_load_i64`, `volatile_store_i64` primitives). `!{Throws}`, capability tokens, and arenas are planned (Epics #295, #296).

> **Audience:** AI agents and LLMs that need to read, write, and reason about Gradient code.
> After one pass through this document, you should be able to produce correct Gradient programs.

> **Design note:** Gradient's design is informed by academic research on LLM code generation. See the roadmap for details.

---

## Quick Start for LLMs

**Generate valid Gradient code in 5 rules:**

```gradient
mod example

@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

fn main() -> !{IO} ():
    print_int(factorial(5))
```

**The 5 non-negotiable rules:**

| Rule | What to do | Common error |
|------|------------|--------------|
| 1 | Start with `mod <filename>` matching the file name | Forgetting module declaration |
| 2 | Use `:` at end of every signature/condition line | `fn foo() -> Int` (missing colon) |
| 3 | Indent 4 spaces, no tabs, no braces | Using `{ }` blocks like C |
| 4 | Annotate every parameter type | `fn add(a, b)` instead of `fn add(a: Int, b: Int)` |
| 5 | Add `!{IO}` for any function calling `print` | Missing effect annotation |

**Quick type reference:**
- `Int`, `Float`, `String`, `Bool`, `()` — built-in types
- `type Option[T] = Some(T) | None` — generic enums
- `fn identity[T](x: T) -> T:` — generic functions

**Effects you can use:** `IO`, `Net`, `FS`, `Mut`, `Time`, `Actor`, `Async`, `Send`, `Atomic`, `Volatile`, `Heap`, `Stack`, `Static` — declare as `!{IO, Net}` between `->` and return type. `Heap` gates heap-backed allocation; `Stack`/`Static`/`Async`/`Send`/`Atomic`/`Volatile` are marker effects today. `atomic_i64_fetch_add` requires `!{Atomic}`; `volatile_load_i64`/`volatile_store_i64` require `!{Volatile}`.

**Need help?** Use typed holes: `let x: Int = ?help` — the compiler will report expected types.

---

## Overview

Gradient is an LLM-first, agentic programming language designed to be unambiguous for both humans and language models. Its core properties:

- **ASCII-only, indentation-significant syntax** -- no Unicode operators, no brace-delimited blocks.
- **No semicolons, no braces for blocks** -- newlines separate statements; indentation defines scope.
- **Colon-delimited blocks** -- every block-opening construct (`fn`, `if`, `else`, `for`, `while`, `match`) ends with `:` before its indented body.
- **Keyword-led statements** -- every construct begins with a reserved word (`fn`, `let`, `if`, `for`, ...).
- **Algebraic effects for side effects** -- all side effects are tracked in the type signature.

---

## Lexical Structure

### Keywords (reserved -- never use these as identifiers)

```
fn    let    if    else    for    in    ret
type  mod    use   impl    match  mut   while
and   or     not   true    false
actor spawn  send  ask     state  on
```

### Sigils

| Sigil | Meaning | Example |
|-------|---------|---------|
| `@` | Annotation | `@inline`, `@extern`, `@cap(IO, Net)` |
| `$` | Compile-time evaluation | `$sizeof(Int)` |
| `!` | Effect declaration | `!{IO}` |
| `?` | Typed hole (placeholder for inference) | `?` |

### Comments

Line comments only. There are no block comments.

```
// This is a comment.
// Each comment line starts with //.

/// This is a doc comment.
/// Doc comments are attached to the next declaration.
```

### Indentation Rules

- **4 spaces = 1 indentation level.** This is not configurable.
- **Tabs are forbidden.** Any tab character is a syntax error.
- A block is opened by `:` at the end of a line (after `fn`, `if`, `else`, `for`, `while`, `match`, etc.), followed by indented lines.
- Dedenting closes the block.

---

## Types (v0.1)

Gradient v0.1 ships with five built-in types:

| Type | Description | Literal examples |
|------|-------------|------------------|
| `Int` | 64-bit signed integer | `42`, `-7`, `0` |
| `Float` | 64-bit IEEE 754 floating point | `3.14`, `-0.001`, `1.0` |
| `String` | UTF-8 encoded string | `"hello"`, `""`, `"line\nbreak"` |
| `Bool` | Boolean | `true`, `false` |
| `()` | Unit type (no meaningful value) | *(implicit -- returned by effectful functions that produce no value)* |

Type annotations use `:` after the binding name or parameter name:

```
let x: Int = 42
let pi: Float = 3.14159
let greeting: String = "hi"
let flag: Bool = true
```

### Enum Types

Enum types (algebraic data types) define a closed set of named variants:

```
type Color = Red | Green | Blue
type Direction = North | South | East | West
```

Variants may carry payloads (tuple variants):

```
type Option = Some(Int) | None
type Result = Ok(Int) | Err(String)
```

> **Note:** Tuple variant syntax is parsed, but code generation for payloads is deferred. Unit variants (no payload) work end-to-end.

Use `match` to branch on enum variants:

```
type Direction = North | South | East | West

fn describe(d: Direction) -> String:
    match d:
        North:
            "up"
        South:
            "down"
        East:
            "right"
        West:
            "left"
```

---

## Functions

### Syntax

```
fn name(param1: Type1, param2: Type2) -> ReturnType:
    body
```

The signature goes on a single line, ending with `:`. The body is an indented block below it.

### Pure functions (no effects)

```
fn add(a: Int, b: Int) -> Int:
    ret a + b

fn square(n: Int) -> Int:
    ret n * n

fn is_positive(x: Int) -> Bool:
    ret x > 0
```

### Effectful functions

When a function performs side effects, declare them with `!{EffectSet}` between `->` and the return type:

```
fn greet(name: String) -> !{IO} ():
    print("Hello, " + name + "!")
```

### Rules

1. The signature (`fn` through `:`) must be on **one line**.
2. Parameters **always** have type annotations.
3. Return type is **always** declared (use `()` for functions that return nothing meaningful).
4. The signature ends with `:` which opens the body block.
5. The body is indented **4 spaces** below the signature.
6. Use `ret` to return a value. The keyword is `ret`, **not** `return`.

### Calling functions

```
let result = add(3, 4)
let s = square(5)
greet("Gradient")
```

No special syntax -- just `name(args)`.

---

## Let Bindings

```
let x: Int = 42           // explicit type annotation
let name = "Gradient"      // type inferred from the right-hand side
let sum = add(3, 4)        // inferred as Int from function return type
```

### Mutable Bindings

Add `mut` after `let` to create a binding that can be reassigned:

```
let mut counter: Int = 0
let mut label = "start"    // type inferred
```

Mutable bindings can be reassigned with `=`:

```
let mut x: Int = 0
x = 10
x = x + 1
```

Only `let mut` bindings may be reassigned. Assigning to a plain `let` binding is a compile error.

### Rules

1. **Bindings are immutable by default.** Use `let mut` to opt into mutability.
2. Only `let mut` bindings may appear on the left-hand side of an assignment (`=`).
3. Type annotation is optional when the type can be inferred.
4. Each `let` binding is a statement on its own line.

### Correct

```
let a = 10
let b = 20
let c = a + b

let mut counter = 0
counter = counter + 1
```

### Wrong

```
let a = 10; let b = 20    // NO semicolons
a = 15                     // NO reassignment of immutable binding
var x = 10                 // NO var keyword
```

---

## Control Flow

### if / else

`if` is an **expression**, meaning it produces a value. You can bind its result with `let`. Every branch is opened with `:`.

```
let result = if x > 0:
    "positive"
else if x == 0:
    "zero"
else:
    "negative"
```

Each branch is an indented block. The last expression in each branch is the value of that branch.

You can also use `if` as a standalone statement:

```
if temperature > 100:
    print("Too hot!")
else:
    print("Acceptable.")
```

### for loops

```
for i in range(10):
    print_int(i)
```

`for` iterates over a range or collection. The loop variable (`i`) is scoped to the loop body. The colon after the iterable opens the body block.

### while loops

```
while condition:
    body
```

`while` repeats its body as long as the condition (a `Bool` expression) is true. The colon after the condition opens the body block.

```
let mut i: Int = 0
while i < 5:
    print_int(i)
    i = i + 1
```

`while` loops pair naturally with mutable bindings. The loop variable is typically declared with `let mut` before the loop and updated inside the body.

### match (pattern matching)

`match` evaluates an expression and compares it against a series of patterns. The first matching arm executes. Supported patterns: integer literals, boolean literals, enum variants, and `_` (wildcard, matches anything).

```
match value:
    0:
        print("zero")
    1:
        print("one")
    _:
        print("other")
```

`match` is an expression -- you can bind its result with `let`:

```
let label: String = match code:
    0:
        "zero"
    1:
        "one"
    _:
        "other"
```

Matching on booleans:

```
match is_active:
    true:
        print("enabled")
    false:
        print("disabled")
```

Each arm is a pattern followed by `:` and an indented body block. The wildcard `_` must be the last arm if present.

---

## Operators

### Precedence table (highest to lowest)

| Precedence | Operators | Associativity | Notes |
|------------|-----------|---------------|-------|
| 1 (highest) | `not`, `-` (unary negation) | Prefix | `not true` evaluates to `false` |
| 2 | `*`, `/`, `%` | Left-to-right | Arithmetic multiplication, division, modulo |
| 3 | `+`, `-` | Left-to-right | Arithmetic addition, subtraction; `+` also concatenates strings |
| 4 | `==`, `!=`, `<`, `>`, `<=`, `>=` | **Non-associative** | Cannot chain: `a < b < c` is a syntax error |
| 5 | `and` | Left-to-right | Logical AND (short-circuiting) |
| 6 (lowest) | `or` | Left-to-right | Logical OR (short-circuiting) |

### Non-associative comparisons

This is correct:

```
let ok = a < b and b < c
```

This is a **syntax error**:

```
let ok = a < b < c        // WRONG -- comparisons are non-associative
```

### String concatenation

The `+` operator concatenates strings:

```
let full = "Hello, " + "world!"
```

### Modulo

The `%` operator performs integer modulo:

```
let remainder = 17 % 5     // result: 2
```

---

## Modules and Imports

### Declaring a module

Every source file starts with a `mod` declaration that must match the filename:

```
// file: math_utils.gr
mod math_utils
```

### Importing modules

`use` declarations are resolved to source files on disk. The compiler maps
dot-separated paths to file paths:

| Declaration | Resolved file |
|---|---|
| `use math` | `math.gr` |
| `use a.b` | `a/b.gr` |

After importing, call the module's functions with qualified syntax:

```
use math

fn main() -> !{IO} ():
    print_int(math.add(1, 2))
```

### Multi-file example

Two files calling each other:

```
// file: helpers.gr
mod helpers

use main_mod

fn double(n: Int) -> Int:
    ret main_mod.base_value(n) * 2
```

```
// file: main_mod.gr
mod main_mod

use helpers

fn base_value(n: Int) -> Int:
    ret n + 10

fn main() -> !{IO} ():
    let result: Int = helpers.double(5)
    print_int(result)
```

The compiler resolves `use helpers` to `helpers.gr` and `use main_mod` to
`main_mod.gr`, then compiles both files together.

### Selective imports

```
use core.io.{print, println}
```

Selective imports bring specific names into scope unqualified.

### Rules

1. **Absolute paths only.** There are no relative imports.
2. Paths are dot-separated: `use core.io`, not `use core/io`.
3. `core.*` is the standard library namespace.
4. `mod` must be the first non-comment line in the file.
5. `use math` resolves to `math.gr`; `use a.b` resolves to `a/b.gr`.
6. Imported functions are called with qualified syntax: `module.function(args)`.

---

## Effects

Gradient tracks side effects in the type system using algebraic effects.

### Declaring effects on a function

```
fn main() -> !{IO} ():
    print("Hello, world!")
```

The syntax is `!{Effect1, Effect2, ...}` placed between `->` and the return type.

### Canonical effects

| Effect | Meaning |
|--------|---------|
| `IO` | Console I/O (print, read) |
| `Net` | Network access |
| `FS` | File system access |
| `Mut` | Mutable state |
| `Time` | Clock / timer access |

Only these 5 effects are recognized. **Unknown effects are rejected** -- writing `!{Foo}` is a compile error with a helpful message listing the valid effects.

### Pure by default

A function with no `!{...}` annotation is **compiler-proven pure**. The compiler verifies through effect inference that the function body uses no effects. This is not just a convention -- `is_pure: true` in the symbol table means the compiler has actually checked.

### Module capability constraints (`@cap`)

The `@cap` annotation limits the maximum effects an entire module may use:

```
@cap(IO, Net)
mod my_server

// Functions in this module may use IO and Net, but not FS, Mut, or Time.
```

`@cap()` with no arguments means the module must be entirely pure:

```
@cap()
mod math_utils

// Every function in this module must be pure. Any effect usage is a compile error.
```

The compiler rejects any function whose effects exceed the module's capability ceiling.

### Rules

1. If a function calls `print()` or any I/O operation, it **must** declare `!{IO}`.
2. Effects propagate: if `foo` calls `bar` which has `!{IO}`, then `foo` must also declare `!{IO}`.
3. A pure function (no effects) omits the `!{...}` entirely:
   ```
   fn add(a: Int, b: Int) -> Int:
       ret a + b
   ```
4. `main` must declare `!{IO}` to perform any I/O.
5. Only known effects (IO, Net, FS, Mut, Time) may be used -- unknown effects are compile errors.
6. If a module has `@cap(...)`, no function may exceed the declared capability ceiling.

### Effect propagation example

```
fn helper() -> !{IO} ():
    print("from helper")

fn main() -> !{IO} ():
    helper()
    print("from main")
```

Both `main` and `helper` need `!{IO}` because both (directly or indirectly) call `print`.

---

## Contracts (Design-by-Contract)

Gradient supports design-by-contract via `@requires` and `@ensures` annotations on functions. Contracts declare what a function expects from its callers and what it guarantees in return.

### Preconditions (`@requires`)

`@requires(condition)` declares a precondition that must hold when the function is called. The condition is a boolean expression over the function's parameters.

```
@requires(n >= 0)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)
```

If the precondition is false at runtime, the program halts with a contract violation error.

### Postconditions (`@ensures`)

`@ensures(condition)` declares a postcondition that must hold when the function returns. The special keyword `result` refers to the function's return value.

```
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)
```

If the postcondition is false after the function body executes, the program halts with a contract violation error.

### Combining contracts

Multiple `@requires` and `@ensures` annotations can be stacked on a single function. All preconditions are checked on entry; all postconditions are checked on exit.

```
@requires(a > 0)
@requires(b > 0)
@ensures(result > 0)
fn multiply_positive(a: Int, b: Int) -> Int:
    ret a * b
```

### The `result` keyword

The `result` keyword is only valid inside `@ensures` conditions. It refers to the value the function returns.

```
@ensures(result == a + b)
fn add(a: Int, b: Int) -> Int:
    ret a + b
```

### Contract checking

Contracts are checked at runtime:

- **On function entry:** each `@requires` condition is evaluated. If any condition is false, a structured error is raised.
- **On function exit:** each `@ensures` condition is evaluated with `result` bound to the return value. If any condition is false, a structured error is raised.

### Contracts in the query API

Contracts are visible through the structured query API:

- `session.symbols()` includes contract information for each function.
- `session.module_contract()` includes contracts in the module's public API surface.

Both produce JSON-serializable output, so agents can read contracts programmatically.

### Rules

1. `@requires` and `@ensures` appear **before** the `fn` keyword, one per line.
2. The condition inside parentheses must be a boolean expression.
3. `result` is only valid inside `@ensures` conditions.
4. Contracts are checked at runtime via assertions.
5. Contract violation errors are structured and machine-readable.
6. A function may have zero, one, or multiple `@requires`/`@ensures` annotations.

### Complete example

```
mod validated

@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

@requires(x >= 0)
@ensures(result >= 0)
fn double(x: Int) -> Int:
    ret x * 2

fn main() -> !{IO} ():
    print_int(factorial(5))
    print_int(double(10))
```

---

## Generics

Gradient supports parametric polymorphism (generics) on both functions and enum types. Type parameters are written in square brackets.

### Generic Functions

Type parameters appear after the function name in square brackets:

```
fn identity[T](x: T) -> T:
    ret x

fn first[A, B](a: A, b: B) -> A:
    ret a
```

At call sites, the compiler uses bidirectional type inference to resolve type parameters. You can also specify them explicitly:

```
let x: Int = identity[Int](42)
let y: String = identity("hello")    // T inferred as String
```

### Generic Enum Types

Type parameters on enum declarations:

```
type Option[T] = Some(T) | None
type Result[T, E] = Ok(T) | Err(E)
type Pair[A, B] = Pair(A, B)
```

### Generic Functions over Generic Types

```
fn unwrap_or[T](opt: Option[T], default: T) -> T:
    match opt:
        Some(val):
            val
        None:
            default
```

### Rules

1. Type parameters are written in `[` `]` after the function or type name.
2. Type parameter names are uppercase identifiers (e.g., `T`, `A`, `B`, `E`).
3. The compiler resolves type parameters at call sites via bidirectional type inference (unification).
4. Type parameters are visible in the query API and module contracts.

---

## Effect Polymorphism

Functions can be polymorphic over effects using lowercase effect variables. This allows writing generic higher-order functions that work with both pure and effectful callbacks.

### Effect Variables

A lowercase name inside `!{...}` is an effect variable:

```
fn apply[T, U](f: (T) -> !{e} U, x: T) -> !{e} U:
    ret f(x)
```

The effect variable `e` resolves at each call site:

```
fn double(n: Int) -> Int:
    ret n * 2

fn print_and_return(n: Int) -> !{IO} Int:
    print_int(n)
    ret n

// e resolves to {} (empty -- pure)
let a: Int = apply(double, 21)

// e resolves to {IO}
let b: Int = apply(print_and_return, 42)
```

### Rules

1. Effect variables use lowercase names (e.g., `e`, `eff`).
2. Concrete effects use uppercase names (e.g., `IO`, `Net`, `FS`).
3. Effect variables resolve at call sites: passing a pure function resolves to empty, passing an effectful function resolves to the concrete effects.
4. `is_effect_polymorphic` is available in the query API to check if a function uses effect variables.

---

## Budget Annotations

Budget annotations declare resource limits on functions. The compiler checks that callees do not exceed their callers' budgets (budget containment).

### Syntax

```
@budget(cpu: 5s, mem: 100mb)
fn process_data(data: Int) -> Int:
    ret data * 2
```

### Budget Containment

If a function has a budget, any function it calls must have a budget that fits within the caller's limits:

```
@budget(cpu: 10s, mem: 200mb)
fn outer() -> Int:
    ret inner(42)

@budget(cpu: 5s, mem: 100mb)
fn inner(x: Int) -> Int:
    ret x * 2
```

This compiles because `inner`'s budget (5s cpu, 100mb mem) fits within `outer`'s budget (10s cpu, 200mb mem). If `inner` had `@budget(cpu: 15s, mem: 100mb)`, the compiler would reject it because 15s exceeds the caller's 10s limit.

### Budget Fields

| Field | Format | Meaning |
|-------|--------|---------|
| `cpu` | Duration (e.g., `5s`, `100ms`) | Maximum CPU time |
| `mem` | Size (e.g., `100mb`, `1gb`) | Maximum memory usage |

### Rules

1. `@budget(...)` annotations appear before the `fn` keyword, like `@requires`/`@ensures`.
2. Budget containment is checked at compile time: a callee's budget must not exceed the caller's budget.
3. Budgets are visible in the query API and module contracts.

---

## FFI (Foreign Function Interface)

Gradient provides FFI annotations for interoperating with C code. Functions can be imported from external libraries or exported with C-compatible linkage.

### Importing C functions (`@extern`)

Use `@extern` to declare a function that is defined in an external C library. Extern functions have no body -- they are resolved at link time.

```
@extern
fn write(fd: Int, buf: String, count: Int) -> Int
```

To specify the library name, pass it as a string argument to `@extern`:

```
@extern("libm")
fn sqrt(x: Float) -> Float

@extern("libm")
fn sin(x: Float) -> Float
```

Extern functions have `Linkage::Import` in the IR and are visible in the query API.

### Exporting Gradient functions (`@export`)

Use `@export` to make a Gradient function visible to C code with C-compatible linkage:

```
@export
fn gradient_add(a: Int, b: Int) -> Int:
    ret a + b
```

Exported functions have `Linkage::Export` in the IR and are visible in the query API. They can be called from C code that links against the compiled Gradient object file.

### FFI type restrictions

Only the following types are permitted in FFI function signatures (both `@extern` and `@export`):

| Type | C equivalent |
|------|-------------|
| `Int` | `int64_t` |
| `Float` | `double` |
| `Bool` | `bool` |
| `String` | `const char*` |
| `()` | `void` (return type only) |

Using any other type (e.g., enum types, generic types) in an FFI function signature is a compile error.

### Complete FFI example

```
mod ffi_demo

@extern("libm")
fn sqrt(x: Float) -> Float

@extern("libm")
fn pow(base: Float, exp: Float) -> Float

@export
fn gradient_square(x: Int) -> Int:
    ret x * x

fn main() -> !{IO} ():
    let val: Float = sqrt(16.0)
    print_float(val)
    let result: Float = pow(2.0, 10.0)
    print_float(result)
```

### Rules

1. `@extern` and `@export` appear **before** the `fn` keyword.
2. `@extern` functions have **no body** -- they end after the signature.
3. `@export` functions have a body, just like regular functions.
4. `@extern` accepts an optional string argument for the library name: `@extern("libm")`.
5. Only FFI-compatible types (`Int`, `Float`, `Bool`, `String`, `()`) are allowed in FFI signatures.
6. FFI functions are visible in the query API (`symbols()`, `module_contract()`).

---

## Actors

Gradient supports actor-based concurrency with message passing. Actors encapsulate state and communicate exclusively through messages.

### Declaring an actor

An `actor` declaration defines state fields and message handlers:

```
actor Counter:
    state count: Int = 0

    on Increment:
        count = count + 1

    on GetCount -> Int:
        ret count
```

### Spawning and messaging

Use `spawn` to create an actor instance, `send` for fire-and-forget messages, and `ask` for request-response:

```
fn main() -> !{Actor} ():
    let c = spawn Counter
    send c Increment
    send c Increment
    let value: Int = ask c GetCount
    print_int(value)
```

### Actor type and effect

- Actor instances have the `Actor` type (`Ty::Actor`).
- Functions that spawn or message actors must declare the `!{Actor}` effect.
- Actor information is available through the query API.

---

## Doc Comments

Gradient supports documentation comments using the `///` prefix. Doc comments are attached to the declaration that immediately follows them.

### Syntax

```
/// Computes the factorial of n.
/// Returns 1 for n <= 1.
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)
```

Doc comments can be attached to functions, types, enums, and actors:

```
/// A color in the RGB model.
type Color = Red | Green | Blue

/// A counter that tracks a running total.
actor Counter:
    state count: Int = 0

    on Increment:
        count = count + 1
```

### Programmatic access

- `session.documentation()` returns a `ModuleDocumentation` structure containing `FunctionDoc`, `TypeDoc`, and related items.
- `session.documentation_text()` returns a plain-text rendering of all documentation.
- `--doc` CLI flag prints documentation to stdout; `--doc --json` emits JSON.

### Rules

1. Doc comments use `///` (three slashes), not `//`.
2. Regular comments (`//`) are not included in documentation output.
3. Doc comments must appear immediately before the item they document (no blank lines between).
4. Multiple `///` lines are concatenated into a single doc string.

---

## Built-in Functions

These functions are available without any imports:

| Function | Signature | Description |
|---|---|---|
| `print` | `(value: String) -> !{IO} ()` | Print a string |
| `println` | `(value: String) -> !{IO} ()` | Print a string with newline |
| `print_int` | `(value: Int) -> !{IO} ()` | Print an integer |
| `print_float` | `(value: Float) -> !{IO} ()` | Print a float |
| `print_bool` | `(value: Bool) -> !{IO} ()` | Print a boolean |
| `abs` | `(n: Int) -> Int` | Absolute value |
| `min` | `(a: Int, b: Int) -> Int` | Minimum of two integers |
| `max` | `(a: Int, b: Int) -> Int` | Maximum of two integers |
| `mod_int` | `(a: Int, b: Int) -> Int` | Integer modulo (also available as `%`) |
| `to_string` | `(value: Int) -> String` | Convert integer to string |
| `int_to_string` | `(value: Int) -> String` | Convert integer to string |
| `range` | `(n: Int) -> Iterable` | Produce a range for iteration |

---

## Common Mistakes (for agents)

These are the errors agents most frequently make when generating Gradient code. Check your output against this list.

| # | Mistake | Incorrect | Correct |
|---|---------|-----------|---------|
| 1 | Using braces for blocks | `fn add(a: Int, b: Int) -> Int { ret a + b }` | `fn add(a: Int, b: Int) -> Int:`<br>&nbsp;&nbsp;&nbsp;&nbsp;`ret a + b` |
| 2 | Using semicolons | `let x = 1; let y = 2` | `let x = 1`<br>`let y = 2` |
| 3 | Forgetting effect annotations | `fn greet() -> ():`<br>&nbsp;&nbsp;&nbsp;&nbsp;`print("hi")` | `fn greet() -> !{IO} ():`<br>&nbsp;&nbsp;&nbsp;&nbsp;`print("hi")` |
| 4 | Writing `return` instead of `ret` | `return x + 1` | `ret x + 1` |
| 5 | Using `var` | `var x = 10` | `let x = 10` (immutable) or `let mut x = 10` (mutable) |
| 6 | Using tabs | (tab character) | (4 spaces) |
| 7 | Chaining comparisons | `a < b < c` | `a < b and b < c` |
| 8 | Using relative imports | `use ../utils` | `use project.utils` |
| 9 | Omitting return type | `fn add(a: Int, b: Int):` | `fn add(a: Int, b: Int) -> Int:` |
| 10 | Omitting parameter types | `fn add(a, b) -> Int:` | `fn add(a: Int, b: Int) -> Int:` |
| 11 | Forgetting the colon | `fn add(a: Int, b: Int) -> Int` | `fn add(a: Int, b: Int) -> Int:` |
| 12 | Forgetting colon on if/else | `if x > 0` | `if x > 0:` |

---

## Complete Example

Below is a full, valid Gradient program that demonstrates functions, let bindings, if/else expressions, arithmetic, and IO effects.

```
mod factorial

fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

fn main() -> !{IO} ():
    let result: Int = factorial(5)
    print_int(result)
```

### What to notice in this example

1. **`mod factorial`** -- module declaration matches the filename `factorial.gr`.
2. **`fn factorial(n: Int) -> Int:`** -- pure function with colon opening the body.
3. **`if n <= 1:`** -- conditional with colon opening the branch body.
4. **`ret 1`** -- explicit return using `ret`, not `return`.
5. **`fn main() -> !{IO} ():`** -- effectful entry point; `!{IO}` because it calls `print_int`.
6. **4-space indentation throughout** -- no tabs, no braces, no semicolons.
7. **All function signatures have full type annotations** -- parameter types and return types are explicit.

---

## Quick Reference Card

```
// Function (pure)
fn name(p1: T1, p2: T2) -> RetType:
    ret expression

// Function (effectful)
fn name(p1: T1) -> !{Effect1, Effect2} RetType:
    body

// Let binding (immutable)
let x: Int = 42
let y = inferred_value

// Let binding (mutable)
let mut counter: Int = 0
counter = counter + 1

// If expression
let v = if condition:
    value_a
else:
    value_b

// For loop
for i in range(n):
    body

// While loop
while condition:
    body

// Match expression
match expr:
    0:
        body_a
    1:
        body_b
    _:
        fallback

// Enum type
type Color = Red | Green | Blue

// Match on enum
match color:
    Red:
        "red"
    Green:
        "green"
    Blue:
        "blue"

// Contracts
@requires(n >= 0)
@ensures(result >= 1)
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

// Generic function
fn identity[T](x: T) -> T:
    ret x

// Generic enum
type Option[T] = Some(T) | None

// Effect-polymorphic function
fn apply[T, U](f: (T) -> !{e} U, x: T) -> !{e} U:
    ret f(x)

// Budget annotation
@budget(cpu: 5s, mem: 100mb)
fn bounded(x: Int) -> Int:
    ret x * 2

// FFI: import C function
@extern("libm")
fn sqrt(x: Float) -> Float

// FFI: export Gradient function
@export
fn my_add(a: Int, b: Int) -> Int:
    ret a + b

// Actor declaration
actor Counter:
    state count: Int = 0

    on Increment:
        count = count + 1

    on GetCount -> Int:
        ret count

// Spawn, send, ask
let c = spawn Counter
send c Increment
let val: Int = ask c GetCount

// Doc comments
/// Computes the factorial of n.
fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

// Module and imports
mod my_module
use core.io
```

---

## Checklist Before Emitting Gradient Code

Use this checklist to validate your output:

**Structure:**
- [ ] The file starts with `mod <module_name>` matching the filename.
- [ ] Imports use absolute dot-separated paths: `use core.io`.

**Functions:**
- [ ] Every function signature ends with `:` before its indented body.
- [ ] Every function has explicit parameter types and a return type.
- [ ] The return keyword is `ret`, not `return`.

**Control flow:**
- [ ] Every `if`, `else if`, `else`, `for`, `while`, and `match` line (including each arm) ends with `:`.
- [ ] Comparisons are not chained; use `and`/`or` to combine them.
- [ ] `match` arms cover all cases (use `_` wildcard as a catch-all if needed).

**Bindings and mutability:**
- [ ] Immutable bindings use `let`; mutable bindings use `let mut`. Never use `var`.
- [ ] Only `let mut` bindings appear on the left-hand side of assignment (`=`).

**Enum types:**
- [ ] Enum variants are `PascalCase`.
- [ ] Enum types are defined with `type Name = Variant1 | Variant2`.
- [ ] Enum variants are matched with `match`, not `if`/`else`.

**Contracts:**
- [ ] `@requires`/`@ensures` annotations appear before the `fn` keyword.
- [ ] `result` is only used inside `@ensures` conditions.
- [ ] Contract conditions are boolean expressions over parameters (for `@requires`) or parameters and `result` (for `@ensures`).

**Effects:**
- [ ] Every function that performs I/O (directly or transitively) has `!{IO}` in its signature.
- [ ] Effects propagate: callers of effectful functions must declare the same effects.
- [ ] Use known effects only: IO, Net, FS, Mut, Time.
- [ ] Consider adding `@cap()` to limit module effects.

**Generics:**
- [ ] Type parameters use `[` and `]` brackets: `fn identity[T](x: T) -> T`.
- [ ] Type parameter names are uppercase: `T`, `A`, `B`, not `t`, `a`, `b`.
- [ ] Generic enum types use the same bracket syntax: `type Option[T] = Some(T) | None`.

**Effect polymorphism:**
- [ ] Effect variables are lowercase: `!{e}`, not `!{E}`.
- [ ] Concrete effects are uppercase: `!{IO}`, `!{Net}`.

**Budgets:**
- [ ] `@budget(...)` appears before `fn`, like `@requires`/`@ensures`.
- [ ] Callee budgets must not exceed caller budgets.

**FFI:**
- [ ] `@extern` functions have no body (declaration only).
- [ ] `@export` functions have a body like normal functions.
- [ ] Only FFI-compatible types are used: `Int`, `Float`, `Bool`, `String`, `()`.
- [ ] Library name is a string argument if specified: `@extern("libm")`, not `@extern(libm)`.

**Formatting:**
- [ ] All indentation uses exactly 4 spaces per level, no tabs.
- [ ] No semicolons appear anywhere.
- [ ] No braces `{` `}` are used for blocks (only for effect sets like `!{IO}`).
