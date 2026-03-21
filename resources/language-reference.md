# Gradient Language Reference -- v0.1

This document specifies the minimal viable surface of the Gradient programming
language (v0.1). It is the authoritative reference for the compiler frontend
implementation.

---

## 1. Lexical Structure

### 1.1 Character Set

Gradient source files are encoded in ASCII. No Unicode operators or identifiers
are permitted in v0.1.

### 1.2 Keywords

The following identifiers are reserved and cannot be used as names:

| Used in v0.1 | Reserved for future |
|---|---|
| `fn` `let` `if` `else` `for` `in` `ret` `type` `mod` `use` `true` `false` `and` `or` `not` `mut` `while` | `impl` `match` |

### 1.3 Sigil Prefixes

| Sigil | Meaning | v0.1 status |
|---|---|---|
| `@` | Annotation (e.g. `@extern`, `@cap`) | Active -- `@extern` and `@cap` are defined |
| `$` | Compile-time evaluation | Reserved |
| `!` | Effect set (e.g. `!{IO}`) | Active in return clauses |
| `?` | Typed hole (e.g. `?todo`) | Active in expressions |

### 1.4 Identifiers

An identifier starts with a letter (`a`-`z`, `A`-`Z`) or underscore, followed
by zero or more letters, digits, or underscores. Identifiers must not be
keywords.

```
IDENT = [a-zA-Z_][a-zA-Z0-9_]*
```

By convention:
- `snake_case` for values, functions, and modules.
- `PascalCase` for types.

### 1.5 Literals

**Integers** -- decimal digits, optionally separated by underscores for
readability.

```
42
1_000_000
```

**Floats** -- decimal digits with a mandatory decimal point and digits on
both sides.

```
3.14
1_000.0
```

**Strings** -- double-quoted, with escape sequences `\n`, `\r`, `\t`, `\\`,
`\"`, `\0`.

```
"hello, world\n"
"she said \"hi\""
```

**Booleans** -- `true` and `false`.

**Unit** -- `()`, the zero-element tuple, used as the "void" type and value.

### 1.6 Comments

Line comments begin with `//` and extend to the end of the line.

```
// This is a comment.
let x = 42  // inline comment
```

There are no block comments in v0.1.

### 1.7 Indentation and Blocks

Gradient uses significant indentation (like Python). The lexer tracks leading
whitespace and emits synthetic tokens:

- **INDENT** -- emitted when the indentation level increases.
- **DEDENT** -- emitted when the indentation level decreases (one DEDENT per
  level unwound).
- **NEWLINE** -- emitted at the end of each non-blank, non-comment-only line.

Rules:
1. Use spaces for indentation. Tabs are forbidden.
2. Each new indentation level must be exactly 4 spaces deeper than its parent.
3. A colon (`:`) at the end of a line introduces a new block; the next line
   must be indented.
4. Blank lines and comment-only lines are ignored by the indentation tracker.

There are no semicolons and no braces for block delimitation.

### 1.8 Operators and Punctuation

| Token | Meaning |
|---|---|
| `+` `-` `*` `/` `%` | Arithmetic |
| `==` `!=` `<` `>` `<=` `>=` | Comparison (non-associative) |
| `and` `or` `not` | Logical (keywords, not symbols) |
| `(` `)` | Grouping, call, unit |
| `,` | Separator |
| `:` | Block introducer, type annotation |
| `->` | Return type arrow |
| `=` | Binding / assignment |
| `.` | Field / method access |
| `@` `!` `?` `$` | Sigils (see above) |
| `{` `}` | Effect set delimiters only |

---

## 2. Types

v0.1 provides the following built-in types:

| Type | Description |
|---|---|
| `Int` | Signed integer (platform word size) |
| `Float` | IEEE 754 double-precision floating point |
| `String` | UTF-8 string (immutable) |
| `Bool` | `true` or `false` |
| `()` | Unit type (zero information) |

User-defined type aliases are supported:

```
type Count = Int
```

### 2.1 Effect Sets

Effect annotations declare which side effects a function may perform. They
appear between the `->` arrow and the return type:

```
fn greet(name: String) -> !{IO} ():
    print(name)
```

The compiler recognizes five canonical effects:

| Effect | Meaning |
|--------|---------|
| `IO`   | Console/terminal I/O |
| `Net`  | Network access |
| `FS`   | Filesystem access |
| `Mut`  | Observable mutation of shared state |
| `Time` | System clock access |

The effect system is **enforced**: calling a function with effects from a
context that does not declare those effects is a compile-time error. Unknown
effect names are rejected. Functions with no effect annotation are pure by
default -- the compiler proves they have no side effects.

### 2.2 Module Capability Constraints

A module may declare a capability ceiling using the `@cap` annotation:

```
@cap(IO)

fn main() -> !{IO} ():
    print("hello")
```

The `@cap` declaration limits which effects any function in the module may
use. A function that declares effects outside the `@cap` set is a
compile-time error. `@cap()` with no arguments means the module must be
entirely pure.

---

## 3. Expressions

Expressions are evaluated to produce values. Gradient is expression-oriented:
`if`/`else` blocks are expressions (their last line is the value).

### 3.1 Precedence Table (lowest to highest)

| Level | Operators | Associativity |
|---|---|---|
| 1 | `or` | Left |
| 2 | `and` | Left |
| 3 | `not` | Prefix (unary) |
| 4 | `==` `!=` `<` `>` `<=` `>=` | Non-associative |
| 5 | `+` `-` | Left |
| 6 | `*` `/` `%` | Left |
| 7 | unary `-` | Prefix |
| 8 | call `()`, field `.` | Postfix / left |

### 3.2 Function Calls

```
print("hello")
add(1, 2)
```

Trailing commas are permitted in argument lists.

### 3.3 Typed Holes

A `?` optionally followed by an identifier acts as a placeholder. The compiler
reports the expected type at that position, aiding development.

```
let x: Int = ?todo
```

---

## 4. Statements

### 4.1 Let Bindings

```
let name: Type = expr
let name = expr          // type inferred
```

The initializer is mandatory in v0.1. By default, bindings are immutable.

#### Mutable Bindings

The `mut` keyword after `let` creates a mutable binding that can be reassigned:

```
let mut counter: Int = 0
let mut name = "initial"   // type inferred
```

Mutable bindings follow the same scoping rules as immutable bindings. Only
bindings declared with `let mut` may appear on the left-hand side of an
assignment statement (see 4.4).

### 4.2 Return

```
ret expr
```

Returns `expr` from the enclosing function. If omitted, the last expression in
the block is the implicit return value.

### 4.3 Expression Statements

Any expression may appear as a statement. Its value is discarded (useful for
side-effecting calls like `print`).

```
print("effect only")
```

### 4.4 Assignment

```
name = expr
```

Assignment rebinds a mutable variable to a new value. The target must have been
declared with `let mut`. Assigning to an immutable binding is a compile-time
error. The assigned expression must match the binding's type.

```
let mut x: Int = 0
x = 10
x = x + 1
```

---

## 5. Control Flow

### 5.1 If / Else

```
if condition:
    body

if condition:
    body
else:
    body

if condition:
    body
else if other_condition:
    body
else:
    body
```

`if`/`else` is an expression. When used as an expression, all branches must be
present and their types must agree.

### 5.2 For Loop

```
for item in iterable:
    body
```

In v0.1 the loop body cannot produce a value (it evaluates to `()`).

### 5.3 While Loop

```
while condition:
    body
```

The `while` loop evaluates `condition` before each iteration. If the condition
is `true`, the body executes and the loop repeats. If `false`, execution
continues after the loop. The condition must be of type `Bool`. The loop body
evaluates to `()`.

```
let mut i: Int = 0
while i < 10:
    print_int(i)
    i = i + 1
```

---

## 6. Function Definitions

```
fn name(param1: Type1, param2: Type2) -> ReturnType:
    body
```

- Parameters require explicit type annotations.
- The return type clause (`-> Type`) is optional; when omitted it defaults
  to `()`.
- The body is an indentation-delimited block.
- The value of the last expression in the body is the implicit return value.

### 6.1 Extern Functions

Extern functions are declared with the `@extern` annotation and have no body:

```
@extern
fn print(msg: String) -> !{IO} ()
```

These provide stubs for foreign-function interface bindings.

---

## 7. Modules and Imports

> **v0.1 limitation:** The compiler currently supports single-file compilation only.
> Module declarations (`mod`) and imports (`use`) are parsed but not resolved.
> Builtin functions (`print`, `print_int`, etc.) are available globally without imports.

### 7.1 Module Declaration

A file may optionally declare its module path:

```
mod myapp.utils
```

If omitted, the module name is derived from the file path.

### 7.2 Imports

```
use std.io
use std.math.{sqrt, pow}
```

Selective imports use braces with a comma-separated list. Trailing commas are
permitted.

---

## 8. Annotations

Annotations are prefixed with `@` and appear on the line before the item they
annotate, or as standalone module-level declarations.

```
@extern
fn malloc(size: Int) -> Int
```

### 8.1 Defined Annotations

| Annotation | Scope | Meaning |
|------------|-------|---------|
| `@extern` | Function | Declares an external function (no body, FFI linkage) |
| `@cap(effects...)` | Module | Limits the effects any function in this module may use |

### 8.2 `@cap` -- Module Capability Ceiling

`@cap` appears as a standalone item at the top level of a module:

```
@cap(IO)

fn greet() -> !{IO} ():
    print("hello")

fn compute(x: Int) -> Int:
    x + 1
```

Any function that declares effects outside the `@cap` set is a compile-time
error. `@cap()` means the module must be entirely pure.

---

## 9. Example Programs

### 9.1 Hello World

```
// hello.gr -- Gradient v0.1 hello world

mod hello

use std.io

fn main() -> !{IO} ():
    print("Hello, world!")
```

### 9.2 Factorial

```
// factorial.gr -- Recursive factorial in Gradient v0.1

mod factorial

use std.io

fn factorial(n: Int) -> Int:
    if n <= 1:
        ret 1
    else:
        ret n * factorial(n - 1)

fn factorial_tail(n: Int, acc: Int) -> Int:
    if n <= 1:
        ret acc
    else:
        ret factorial_tail(n - 1, n * acc)

fn main() -> !{IO} ():
    let result: Int = factorial(10)
    let result2: Int = factorial_tail(10, 1)
    print_int(result)
    print_int(result2)
```

---

## 10. Grammar Summary

Every statement is keyword-led, making the language LL(1) parseable by
inspection of the first token:

| First token | Production |
|---|---|
| `mod` | Module declaration |
| `use` | Import |
| `fn` | Function definition |
| `let` | Let binding |
| `if` | If expression/statement |
| `for` | For loop |
| `while` | While loop |
| `ret` | Return statement |
| `type` | Type alias |
| `@` | Annotation (attaches to next item) |
| anything else | Expression statement |

The full formal grammar is in `grammar.peg`.

---

*Gradient v0.1 -- Specification date: 2026-03-20*
