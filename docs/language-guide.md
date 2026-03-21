# Gradient v0.1 Language Guide

> **Audience:** AI agents and LLMs that need to read, write, and reason about Gradient code.
> After one pass through this document, you should be able to produce correct Gradient programs.

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

// Module and imports
mod my_module
use core.io
```

---

## Checklist Before Emitting Gradient Code

Use this checklist to validate your output:

- [ ] Every function signature ends with `:` before its indented body.
- [ ] Every `if`, `else if`, `else`, `for`, `while`, and `match` line (including each arm) ends with `:`.
- [ ] Every function has explicit parameter types and a return type.
- [ ] Every function that performs I/O (directly or transitively) has `!{IO}` in its signature.
- [ ] All indentation uses exactly 4 spaces per level, no tabs.
- [ ] No semicolons appear anywhere.
- [ ] No braces `{` `}` are used for blocks (only for effect sets like `!{IO}`).
- [ ] The return keyword is `ret`, not `return`.
- [ ] Immutable bindings use `let`; mutable bindings use `let mut`. Never use `var`.
- [ ] Comparisons are not chained; use `and`/`or` to combine them.
- [ ] The file starts with `mod <module_name>` matching the filename.
- [ ] Imports use absolute dot-separated paths: `use core.io`.
- [ ] Consider adding `@cap()` to limit module effects.
- [ ] Use known effects only: IO, Net, FS, Mut, Time.
