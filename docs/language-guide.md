# Gradient v0.1 Language Guide

> **Audience:** AI agents and LLMs that need to read, write, and reason about Gradient code.
> After one pass through this document, you should be able to produce correct Gradient programs.

---

## Overview

Gradient is an LLM-first, agentic programming language designed to be unambiguous for both humans and language models. Its core properties:

- **ASCII-only, indentation-significant syntax** -- no Unicode operators, no brace-delimited blocks.
- **No semicolons, no braces for blocks** -- newlines separate statements; indentation defines scope.
- **Keyword-led statements** -- every construct begins with a reserved word (`fn`, `let`, `if`, `for`, ...).
- **Algebraic effects for side effects** -- all side effects are tracked in the type signature.
- **Three-tier memory model** -- arena allocation by default (details beyond v0.1 scope).

---

## Lexical Structure

### Keywords (reserved -- never use these as identifiers)

```
fn    let    if    else    for    in    ret
type  mod    use   impl    match
and   or     not   true    false
```

### Sigils

| Sigil | Meaning | Example |
|-------|---------|---------|
| `@` | Annotation | `@inline` |
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
- A block is opened by `:` at the end of a line, or implicitly by indenting after a keyword line (such as `fn`, `if`, `else`, `for`).
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

---

## Functions

### Syntax

```
fn name(param1: Type1, param2: Type2) -> ReturnType
    body
```

The signature goes on a single line. The body is an indented block below it.

### Pure functions (no effects)

```
fn add(a: Int, b: Int) -> Int
    ret a + b

fn square(n: Int) -> Int
    ret n * n

fn is_positive(x: Int) -> Bool
    ret x > 0
```

### Effectful functions

When a function performs side effects, declare them with `!{EffectSet}` between `->` and the return type:

```
fn greet(name: String) -> !{IO} ()
    print("Hello, " + name + "!")

fn read_file(path: String) -> !{IO, Alloc} String
    // IO for file system access, Alloc for memory allocation
    ...
```

### Rules

1. The signature (`fn` through return type) must be on **one line**.
2. Parameters **always** have type annotations.
3. Return type is **always** declared (use `()` for functions that return nothing meaningful).
4. The body is indented **4 spaces** below the signature.
5. Use `ret` to return a value. The keyword is `ret`, **not** `return`.

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

### Rules

1. **All bindings are immutable.** There is no `var`, no `mut`, no reassignment in v0.1.
2. Type annotation is optional when the type can be inferred.
3. Each `let` binding is a statement on its own line.

### Correct

```
let a = 10
let b = 20
let c = a + b
```

### Wrong

```
let a = 10; let b = 20    // NO semicolons
a = 15                     // NO reassignment
var x = 10                 // NO var keyword
let mut y = 10             // NO mut keyword
```

---

## Control Flow

### if / else

`if` is an **expression**, meaning it produces a value. You can bind its result with `let`.

```
let result = if x > 0
    "positive"
else if x == 0
    "zero"
else
    "negative"
```

Each branch is an indented block. The last expression in each branch is the value of that branch.

You can also use `if` as a standalone statement:

```
if temperature > 100
    print("Too hot!")
else
    print("Acceptable.")
```

### for loops

```
for i in range(10)
    print(i)

for item in collection
    process(item)
```

`for` iterates over a range or collection. The loop variable (`i`, `item`) is scoped to the loop body.

### match (pattern matching)

```
match value
    0
        print("zero")
    1
        print("one")
    _
        print("other")
```

`_` is the wildcard pattern (matches anything).

---

## Operators

### Precedence table (highest to lowest)

| Precedence | Operators | Associativity | Notes |
|------------|-----------|---------------|-------|
| 1 (highest) | `not`, `-` (unary negation) | Prefix | `not true` evaluates to `false` |
| 2 | `*`, `/` | Left-to-right | Arithmetic multiplication, division |
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

---

## Modules and Imports

### Declaring a module

Every source file starts with a `mod` declaration that must match the filename:

```
// file: math_utils.gr
mod math_utils
```

### Importing modules

```
use core.io
use core.math
```

### Rules

1. **Absolute paths only.** There are no relative imports.
2. Paths are dot-separated: `use core.io`, not `use core/io`.
3. `core.*` is the standard library namespace.
4. `mod` must be the first non-comment line in the file.

---

## Effects

Gradient tracks side effects in the type system using algebraic effects.

### Declaring effects on a function

```
fn main() -> !{IO} ()
    print("Hello, world!")
```

The syntax is `!{Effect1, Effect2, ...}` placed between `->` and the return type.

### Common effects in v0.1

| Effect | Meaning |
|--------|---------|
| `IO` | Console I/O, file system access, network |
| `Alloc` | Heap memory allocation beyond the default arena |

### Rules

1. If a function calls `print()` or any I/O operation, it **must** declare `!{IO}`.
2. Effects propagate: if `foo` calls `bar` which has `!{IO}`, then `foo` must also declare `!{IO}`.
3. A pure function (no effects) omits the `!{...}` entirely:
   ```
   fn add(a: Int, b: Int) -> Int
       ret a + b
   ```
4. `main` must declare `!{IO}` to perform any I/O.
5. The effect system is **row-polymorphic** -- functions can be generic over effect sets. In v0.1 this is mostly implicit; just declare the effects you use.

### Effect propagation example

```
fn helper() -> !{IO} ()
    print("from helper")

fn main() -> !{IO} ()
    helper()
    print("from main")
```

Both `main` and `helper` need `!{IO}` because both (directly or indirectly) call `print`.

---

## Common Mistakes (for agents)

These are the errors agents most frequently make when generating Gradient code. Check your output against this list.

| # | Mistake | Incorrect | Correct |
|---|---------|-----------|---------|
| 1 | Using braces for blocks | `fn add(a: Int, b: Int) -> Int { ret a + b }` | `fn add(a: Int, b: Int) -> Int`<br>&nbsp;&nbsp;&nbsp;&nbsp;`ret a + b` |
| 2 | Using semicolons | `let x = 1; let y = 2` | `let x = 1`<br>`let y = 2` |
| 3 | Forgetting effect annotations | `fn greet() -> ()`<br>&nbsp;&nbsp;&nbsp;&nbsp;`print("hi")` | `fn greet() -> !{IO} ()`<br>&nbsp;&nbsp;&nbsp;&nbsp;`print("hi")` |
| 4 | Writing `return` instead of `ret` | `return x + 1` | `ret x + 1` |
| 5 | Using `var` or `mut` | `var x = 10` / `let mut x = 10` | `let x = 10` |
| 6 | Using tabs | (tab character) | (4 spaces) |
| 7 | Chaining comparisons | `a < b < c` | `a < b and b < c` |
| 8 | Using relative imports | `use ../utils` | `use project.utils` |
| 9 | Omitting return type | `fn add(a: Int, b: Int)` | `fn add(a: Int, b: Int) -> Int` |
| 10 | Omitting parameter types | `fn add(a, b) -> Int` | `fn add(a: Int, b: Int) -> Int` |

---

## Complete Example

Below is a full, valid Gradient program that demonstrates functions, let bindings, if/else expressions, arithmetic, and IO effects.

```
// file: fizzbuzz.gr
mod fizzbuzz

use core.io

fn classify(n: Int) -> String
    let by3 = n % 3 == 0
    let by5 = n % 5 == 0
    let result = if by3 and by5
        "FizzBuzz"
    else if by3
        "Fizz"
    else if by5
        "Buzz"
    else
        int_to_string(n)
    ret result

fn run_fizzbuzz(limit: Int) -> !{IO} ()
    for i in range(1, limit + 1)
        let label = classify(i)
        print(label)

fn main() -> !{IO} ()
    let limit = 20
    print("FizzBuzz up to " + int_to_string(limit) + ":")
    run_fizzbuzz(limit)
```

### What to notice in this example

1. **`mod fizzbuzz`** -- module declaration matches the filename `fizzbuzz.gr`.
2. **`use core.io`** -- imports the IO module for `print`.
3. **`classify` is pure** -- no `!{...}` because it does no I/O; it only computes a string.
4. **`run_fizzbuzz` and `main` are effectful** -- both declare `!{IO}` because they call `print`.
5. **`if` is an expression** -- the result is bound to `result` via `let`.
6. **`ret` is used to return** -- not `return`.
7. **4-space indentation throughout** -- no tabs, no braces, no semicolons.
8. **All function signatures have full type annotations** -- parameter types and return types are explicit.

---

## Quick Reference Card

```
// Function (pure)
fn name(p1: T1, p2: T2) -> RetType
    ret expression

// Function (effectful)
fn name(p1: T1) -> !{Effect1, Effect2} RetType
    body

// Let binding
let x: Int = 42
let y = inferred_value

// If expression
let v = if condition
    value_a
else
    value_b

// For loop
for i in range(n)
    body

// Match
match expr
    pattern1
        body1
    pattern2
        body2
    _
        default_body

// Module and imports
mod my_module
use core.io
```

---

## Checklist Before Emitting Gradient Code

Use this checklist to validate your output:

- [ ] Every function has explicit parameter types and a return type.
- [ ] Every function that performs I/O (directly or transitively) has `!{IO}` in its signature.
- [ ] All indentation uses exactly 4 spaces per level, no tabs.
- [ ] No semicolons appear anywhere.
- [ ] No braces `{` `}` are used for blocks (only for effect sets like `!{IO}`).
- [ ] The return keyword is `ret`, not `return`.
- [ ] All bindings use `let`, never `var` or `let mut`.
- [ ] Comparisons are not chained; use `and`/`or` to combine them.
- [ ] The file starts with `mod <module_name>` matching the filename.
- [ ] Imports use absolute dot-separated paths: `use core.io`.
