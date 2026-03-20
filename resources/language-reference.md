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
| `fn` `let` `if` `else` `for` `in` `ret` `type` `mod` `use` `true` `false` `and` `or` `not` | `impl` `match` |

### 1.3 Sigil Prefixes

| Sigil | Meaning | v0.1 status |
|---|---|---|
| `@` | Annotation (e.g. `@extern`) | Active -- only `@extern` is defined |
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

In v0.1 the only meaningful effect is `IO`. The effect system is advisory;
the compiler does not yet enforce it.

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

The initializer is mandatory in v0.1. All bindings are immutable.

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
annotate.

```
@extern
fn malloc(size: Int) -> Int
```

In v0.1 the only defined annotation is `@extern`.

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
| `ret` | Return statement |
| `type` | Type alias |
| `@` | Annotation (attaches to next item) |
| anything else | Expression statement |

The full formal grammar is in `grammar.peg`.

---

*Gradient v0.1 -- Specification date: 2026-03-19*
