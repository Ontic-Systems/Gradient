# Gradient Self-Hosting Roadmap

## Overview

This document outlines the plan to rewrite the Gradient compiler in Gradient itself. This is a long-term goal that will proceed in phases, starting with the simplest components and gradually bootstrapping up to a full compiler.

## Current Status

**Rust Compiler Stats:**
- 40,934 lines of Rust code
- ~33 AST node types
- ~898 tests passing
- 3 backends (Cranelift, LLVM, WASM)
- SMT verification support

**Gradient Language Status:**
- ✅ Enums and pattern matching
- ✅ Generics (basic)
- ✅ Comptime (compile-time metaprogramming)
- ✅ Effects system
- ✅ List and Map types
- ✅ File I/O (basic)
- ✅ String operations
- 🟡 Advanced collections (HashMap equivalent)
- 🟡 Iterator abstractions
- 🟡 String manipulation (regex, advanced parsing)

## Phase 1: Foundation (Prerequisites)

Before we can write a compiler, we need a few more language features:

### 1.1 Advanced Collections

**Priority: HIGH**

The compiler needs efficient hash maps for symbol tables. Currently Gradient has `Map[String, T]` but we need:

- `HashMap[K, V]` with arbitrary key types
- `HashSet[T]` for tracking unique items
- `VecDeque[T]` for work queues

**Implementation:**
```gradient
// New stdlib modules
mod std.collections:
    type HashMap[K, V] = ...
    type HashSet[T] = ...
    type VecDeque[T] = ...
```

**Estimated Effort:** 2-3 days

### 1.2 Iterator Protocol

**Priority: HIGH**

Compilers constantly iterate over lists. We need:

```gradient
trait Iterator[T]:
    fn next(self) -> Option[T]
    fn map(self, f: fn(T) -> U) -> MapIterator[T, U]
    fn filter(self, pred: fn(T) -> Bool) -> FilterIterator[T]
    fn collect(self) -> List[T]
```

**Estimated Effort:** 2-3 days

### 1.3 String Builder & Advanced String Ops

**Priority: MEDIUM**

Compilers build strings constantly (error messages, codegen). Need:

```gradient
type StringBuilder:
    fn new() -> StringBuilder
    fn append(self, s: String) -> StringBuilder
    fn build(self) -> String
```

**Estimated Effort:** 1-2 days

### 1.4 File I/O Enhancements

**Priority: MEDIUM**

Need directory listing, file metadata:

```gradient
mod std.fs:
    fn read_dir(path: String) -> !{FS} List[String]
    fn exists(path: String) -> !{FS} Bool
    fn is_file(path: String) -> !{FS} Bool
    fn is_dir(path: String) -> !{FS} Bool
```

**Estimated Effort:** 1-2 days

## Phase 2: Self-Hosting Components (Bootstrap Order)

The key insight: we'll write Gradient components that can be compiled by the Rust compiler and integrated via FFI or IR linking. This lets us test each piece independently.

### 2.1 Token Module (Week 1)

**Why First:** Simple data structures, good test case for enums

```gradient
// compiler/token.gr
mod token:
    type Position:
        line: i32
        col: i32
        offset: i32

    type Span:
        file_id: i32
        start: Position
        end: Position

    enum TokenKind:
        // Literals
        IntLit(value: i64)
        FloatLit(value: f64)
        StringLit(value: String)
        BoolLit(value: Bool)

        // Keywords
        Fn, Let, Mut, If, Else, For, While, Match
        Ret, Type, Actor, Spawn, Send, Ask
        Use, Mod, Extern, Export, Comptime

        // Operators
        Plus, Minus, Star, Slash, Percent
        Eq, Ne, Lt, Le, Gt, Ge
        And, Or, Not
        Assign, Arrow, Pipe, Dot, DotDot

        // Delimiters
        LParen, RParen, LBracket, RBracket
        LBrace, RBrace, Colon, Comma

        // Special
        Ident(name: String)
        Indent, Dedent, Newline
        Eof
        Error(message: String)

    type Token:
        kind: TokenKind
        span: Span
```

**Testing Strategy:**
1. Write Gradient tokenizer
2. Compile to object file with Rust compiler
3. Link with Rust test harness
4. Compare output with Rust lexer

**Estimated Effort:** 3-4 days

### 2.2 Basic Lexer (Week 2-3)

```gradient
// compiler/lexer.gr
mod lexer:
    use token.{Token, TokenKind, Span, Position}

    type Lexer:
        source: String
        file_id: i32
        pos: Position
        indent_stack: List[i32]

    fn new(source: String, file_id: i32) -> Lexer:
        ...

    fn next_token(self) -> !{Pure} Token:
        ...

    fn tokenize(self) -> !{Pure} List[Token]:
        ...
```

**Key Challenges:**
- Python-style indentation handling
- String interpolation parsing
- Error recovery

**Estimated Effort:** 5-7 days

### 2.3 AST Definitions (Week 4)

```gradient
// compiler/ast.gr
mod ast:
    use token.{Span}

    // Spanned wrapper (like Rust's Spanned<T>)
    type Spanned[T]:
        node: T
        span: Span

    type Expr = Spanned[ExprKind]

    enum ExprKind:
        IntLit(value: i64)
        FloatLit(value: f64)
        StringLit(value: String)
        BoolLit(value: Bool)
        UnitLit
        Ident(name: String)
        TypedHole(label: Option[String])

        BinaryOp(op: BinOp, left: Expr, right: Expr)
        UnaryOp(op: UnaryOp, operand: Expr)
        Call(func: Expr, args: List[Expr])
        FieldAccess(object: Expr, field: String)
        If(condition: Expr, then_block: Block, else_ifs: List[(Expr, Block)], else_block: Option[Block])
        For(var: String, iter: Expr, body: Block)
        While(condition: Expr, body: Block)
        Match(scrutinee: Expr, arms: List[MatchArm])

    enum BinOp:
        Or, And, Eq, Ne, Lt, Le, Gt, Ge
        Add, Sub, Mul, Div, Mod, Pipe

    enum UnaryOp:
        Neg, Not

    type MatchArm:
        pattern: Pattern
        guard: Option[Expr]
        body: Block

    enum Pattern:
        Wildcard
        IntPat(value: i64)
        BoolPat(value: Bool)
        StringPat(value: String)
        VarPat(name: String)

    type Block = Spanned[List[Stmt]]

    enum Stmt:
        Let(name: String, type_ann: Option[TypeExpr], value: Expr, mutable: Bool)
        Assign(target: Expr, value: Expr)
        ExprStmt(expr: Expr)
        Ret(value: Option[Expr])

    enum TypeExpr:
        Named(name: String, cap: Option[Capability])
        Unit
        Fn(params: List[TypeExpr], ret: TypeExpr, effects: Option[EffectSet])
        Generic(name: String, args: List[TypeExpr])

    enum Capability:
        Iso, Val, Ref, Box, Trn, Tag

    type EffectSet:
        effects: List[String]
        polys: List[String]  // Effect variables

    // Items (top-level declarations)
    type Item = Spanned[ItemKind]

    enum ItemKind:
        FnDef(FnDef)
        ExternFn(ExternFnDecl)
        Let(name: String, type_ann: Option[TypeExpr], value: Expr, mutable: Bool)
        TypeDecl(name: String, type_expr: TypeExpr)
        EnumDecl(name: String, type_params: List[String], variants: List[EnumVariant])
        ActorDecl(name: String, state_fields: List[StateField], handlers: List[MessageHandler])
        CapDecl(allowed_effects: List[String])

    type FnDef:
        name: String
        type_params: List[TypeParam]
        params: List[Param]
        return_type: Option[TypeExpr]
        effects: Option[EffectSet]
        body: Block
        contracts: List[Contract]

    type Param:
        name: String
        type_ann: TypeExpr
        comptime: Bool

    type Contract:
        kind: ContractKind
        condition: Expr

    enum ContractKind:
        Requires, Ensures

    type EnumVariant:
        name: String
        field: Option[TypeExpr]

    type StateField:
        name: String
        type_ann: TypeExpr
        default: Expr

    type MessageHandler:
        message: String
        params: List[Param]
        return_type: Option[TypeExpr]
        body: Block

    type Module:
        name: String
        uses: List[UseDecl]
        items: List[Item]

    type UseDecl:
        path: List[String]
        imports: Option[List[String]]
```

**Estimated Effort:** 3-4 days

### 2.4 Parser (Week 5-8)

```gradient
// compiler/parser.gr
mod parser:
    use ast.{Expr, Stmt, Item, Module, Block}
    use token.{Token, TokenKind}

    type Parser:
        tokens: List[Token]
        pos: i32

    fn new(tokens: List[Token]) -> Parser:
        ...

    fn parse_module(self) -> !{Pure} Result[Module, ParseError]:
        ...

    fn parse_item(self) -> !{Pure} Result[Item, ParseError]:
        ...

    fn parse_fn_def(self) -> !{Pure} Result[FnDef, ParseError]:
        ...

    fn parse_expr(self) -> !{Pure} Result[Expr, ParseError]:
        ...

    fn parse_stmt(self) -> !{Pure} Result[Stmt, ParseError]:
        ...

    fn parse_block(self) -> !{Pure} Result[Block, ParseError]:
        ...

    fn expect(self, kind: TokenKind) -> !{Pure} Result[Token, ParseError]:
        ...

    fn peek(self) -> !{Pure} Token:
        ...

    fn advance(self) -> !{Pure} Token:
        ...
```

**Key Challenges:**
- Recursive descent with precedence climbing
- Indentation-aware parsing
- Error recovery (sync points)
- Type parameter parsing
- Effect annotation parsing

**Estimated Effort:** 10-15 days

### 2.5 Type System (Week 9-14)

```gradient
// compiler/types.gr
mod types:
    use ast.{TypeExpr, EffectSet}

    enum Ty:
        Unknown
        Never
        Unit
        I8, I16, I32, I64
        U8, U16, U32, U64
        F32, F64
        Bool
        String
        Char

        // Reference types
        Ref(comptime cap: Capability, pointee: Ty)

        // Aggregate types
        List(elem: Ty)
        Map(key: Ty, value: Ty)
        Tuple(elems: List[Ty])

        // Function types
        Fn(params: List[Ty], ret: Ty, effects: EffectSet)

        // User-defined
        Enum(name: String, variants: List[VariantInfo])
        Actor(name: String)
        Struct(name: String, fields: List[(String, Ty)])

        // Type variable (for generics)
        Var(id: i32)

        // Type constructor (for generic types)
        Constructor(name: String, args: List[Ty])

    type VariantInfo:
        name: String
        tag: i32
        payload: Option[Ty]

    enum Capability:
        Iso, Val, Ref, Box, Trn, Tag

    fn is_subtype(t1: Ty, t2: Ty) -> Bool:
        ...

    fn unify(t1: Ty, t2: Ty) -> !{Pure} Result[Ty, String]:
        ...
```

**Estimated Effort:** 15-20 days

### 2.6 Type Checker (Week 15-22)

```gradient
// compiler/checker.gr
mod checker:
    use ast.{Expr, Stmt, Item, FnDef}
    use types.{Ty}

    type TypeChecker:
        env: TypeEnv
        errors: List[TypeError]

    type TypeEnv:
        scopes: List[HashMap[String, Binding]]
        functions: HashMap[String, FnSig]
        types: HashMap[String, TypeDef]
        type_vars: HashMap[i32, Ty]

    type Binding:
        ty: Ty
        mutable: Bool
        consumed: Bool

    fn check_module(module: Module) -> !{Pure} (List[TypeError], ModuleSummary):
        ...

    fn check_fn_def(self, fn_def: FnDef) -> !{Pure} List[TypeError]:
        ...

    fn check_expr(self, expr: Expr, expected: Option[Ty]) -> !{Pure} Result[Ty, TypeError]:
        ...

    fn check_stmt(self, stmt: Stmt) -> !{Pure} List[TypeError]:
        ...

    fn infer_expr(self, expr: Expr) -> !{Pure} Result[Ty, TypeError]:
        ...
```

**Key Challenges:**
- Hindley-Milner type inference
- Effect tracking
- Capability tracking for reference types
- Generic instantiation
- Comptime evaluation

**Estimated Effort:** 20-25 days

### 2.7 IR (Intermediate Representation) (Week 23-26)

```gradient
// compiler/ir.gr
mod ir:
    // Three-address code IR
    enum Instruction:
        // Literals
        ConstInt(dest: Value, value: i64)
        ConstFloat(dest: Value, value: f64)
        ConstString(dest: Value, value: String)
        ConstBool(dest: Value, value: Bool)
        ConstUnit(dest: Value)

        // Arithmetic
        Add(dest: Value, left: Value, right: Value)
        Sub(dest: Value, left: Value, right: Value)
        Mul(dest: Value, left: Value, right: Value)
        Div(dest: Value, left: Value, right: Value)
        Mod(dest: Value, left: Value, right: Value)
        Neg(dest: Value, operand: Value)

        // Comparisons
        Eq(dest: Value, left: Value, right: Value)
        Ne(dest: Value, left: Value, right: Value)
        Lt(dest: Value, left: Value, right: Value)
        Le(dest: Value, left: Value, right: Value)
        Gt(dest: Value, left: Value, right: Value)
        Ge(dest: Value, left: Value, right: Value)

        // Control flow
        Branch(cond: Value, then_label: String, else_label: String)
        Jump(label: String)
        Call(dest: Value, func: String, args: List[Value])
        Ret(value: Option[Value])

        // Memory
        Alloc(dest: Value, ty: IrType)
        Load(dest: Value, ptr: Value)
        Store(ptr: Value, value: Value)
        GetField(dest: Value, obj: Value, field_idx: i32)
        SetField(obj: Value, field_idx: i32, value: Value)

        // Variants (enums)
        ConstructVariant(dest: Value, tag: i32, payload: Option[Value])
        GetVariantTag(dest: Value, obj: Value)
        GetVariantField(dest: Value, obj: Value, field_idx: i32)

        // Closures
        CreateClosure(dest: Value, fn_name: String, captures: List[Value])
        GetEnvPtr(dest: Value, closure: Value)

    type Value:
        id: i32
        ty: IrType

    enum IrType:
        I8, I16, I32, I64
        U8, U16, U32, U64
        F32, F64
        Bool, Void
        Ptr(pointee: IrType)
        Struct(fields: List[IrType])
        Array(elem: IrType, size: i32)
        Function(params: List[IrType], ret: IrType)

    type BasicBlock:
        name: String
        instructions: List[Instruction]
        terminator: Instruction

    type Function:
        name: String
        params: List[Value]
        ret_ty: IrType
        blocks: List[BasicBlock]
        entry: String
        is_export: Bool

    type Module:
        name: String
        functions: List[Function]
        globals: List[Global]
        string_pool: List[String]

    type Global:
        name: String
        ty: IrType
        init: Option[Constant]

    enum Constant:
        IntVal(i64)
        FloatVal(f64)
        StringVal(String)
        BoolVal(Bool)
        ArrayVal(List[Constant])
```

**Estimated Effort:** 10-12 days

### 2.8 IR Builder (Week 27-30)

```gradient
// compiler/ir_builder.gr
mod ir_builder:
    use ast.{Expr, Stmt, FnDef}
    use ir.{Instruction, Value, BasicBlock, Function, Module}
    use types.{Ty}

    type IrBuilder:
        current_fn: Function
        current_block: String
        value_counter: i32
        block_counter: i32
        var_map: HashMap[String, Value]
        loop_stack: List[(String, String)]  // (continue_label, break_label)

    fn build_module(module: ast.Module) -> !{Pure} (ir.Module, List[BuildError]):
        ...

    fn build_fn_def(self, fn_def: FnDef) -> !{Pure} Result[Function, BuildError]:
        ...

    fn build_expr(self, expr: Expr) -> !{Pure} Result[Value, BuildError]:
        ...

    fn build_stmt(self, stmt: Stmt) -> !{Pure} List[BuildError]:
        ...

    fn emit(self, instr: Instruction) -> !{Pure} Unit:
        ...

    fn new_value(self, ty: IrType) -> !{Pure} Value:
        ...

    fn create_block(self, name_hint: String) -> !{Pure} String:
        ...
```

**Estimated Effort:** 10-12 days

## Phase 3: Bootstrap Strategy

### Strategy A: Gradual Replacement (Recommended)

1. **Start with peripheral tools:**
   - Formatter written in Gradient
   - LSP protocol handler in Gradient
   - Documentation generator in Gradient

2. **Replace one compiler phase at a time:**
   - Keep Rust compiler as "reference"
   - Write Gradient version of one phase
   - Compare outputs on test suite
   - Switch over when outputs match

3. **Maintain compatibility:**
   - Both compilers must produce identical IR
   - Fuzz testing to verify equivalence

### Strategy B: Full Rewrite (Risky)

1. Write complete Gradient compiler
2. Use Rust compiler to compile it
3. Use compiled Gradient compiler to compile itself
4. Compare outputs

## Phase 4: Advanced Features

### 4.1 Optimization Passes

```gradient
mod opt:
    // Constant folding
    fn fold_constants(module: ir.Module) -> ir.Module

    // Dead code elimination
    fn eliminate_dead_code(module: ir.Module) -> ir.Module

    // Inlining
    fn inline_functions(module: ir.Module, threshold: i32) -> ir.Module

    // SSA form
    fn to_ssa(module: ir.Module) -> ir.Module
```

### 4.2 Advanced Type System Features

- Type classes/traits (if not already implemented)
- Higher-kinded types
- Dependent types (lightweight)

### 4.3 Incremental Compilation

- Module dependency tracking
- Cached compilation units
- Parallel compilation

## Timeline Estimate

| Phase | Duration | Cumulative |
|-------|----------|------------|
| Foundation (1.1-1.4) | 2 weeks | 2 weeks |
| Token Module | 1 week | 3 weeks |
| Lexer | 2 weeks | 5 weeks |
| AST | 1 week | 6 weeks |
| Parser | 4 weeks | 10 weeks |
| Type System | 2 weeks | 12 weeks |
| Type Checker | 4 weeks | 16 weeks |
| IR | 2 weeks | 18 weeks |
| IR Builder | 2 weeks | 20 weeks |
| Integration & Testing | 4 weeks | 24 weeks |
| **TOTAL** | ~6 months | |

## Success Criteria

The self-hosting effort is complete when:

1. ✅ Gradient compiler written in Gradient can compile itself
2. ✅ Output is byte-for-byte identical to Rust compiler output
3. ✅ All 898+ tests pass
4. ✅ Performance within 20% of Rust compiler
5. ✅ Can compile all existing Gradient programs

## Next Steps (Immediate Action Items)

1. **Implement HashMap** (this week)
   - Add to stdlib
   - Use open addressing or Robin Hood hashing
   - Test with symbol table use cases

2. **Implement Iterator protocol** (next week)
   - Core trait definition
   - Adapter methods (map, filter, fold)
   - Collect integration

3. **Start Token module** (week 3)
   - Port token definitions
   - Write tests
   - Verify with Rust reference

## Notes

- This is a 6-month project minimum
- Incremental approach reduces risk
- Each phase produces useful components
- Maintains working compiler throughout
- Can pause/resume between phases
