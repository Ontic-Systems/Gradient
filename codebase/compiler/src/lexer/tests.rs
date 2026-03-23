//! Comprehensive tests for the Gradient lexer.

use super::*;

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

/// Tokenize source and return all token kinds (excluding Eof).
fn kinds(source: &str) -> Vec<TokenKind> {
    let mut lexer = Lexer::new(source, 0);
    lexer
        .tokenize()
        .into_iter()
        .map(|t| t.kind)
        .filter(|k| *k != TokenKind::Eof)
        .collect()
}

/// Tokenize source and return all token kinds (including Eof).
fn kinds_with_eof(source: &str) -> Vec<TokenKind> {
    let mut lexer = Lexer::new(source, 0);
    lexer.tokenize().into_iter().map(|t| t.kind).collect()
}

/// Tokenize and return full tokens for span inspection.
fn tokens(source: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(source, 0);
    lexer.tokenize()
}

// -----------------------------------------------------------------------
// Keywords
// -----------------------------------------------------------------------

#[test]
fn keywords_all() {
    let source = "fn let mut if else for in while ret type mod use impl match true false and or not";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Fn,
            TokenKind::Let,
            TokenKind::Mut,
            TokenKind::If,
            TokenKind::Else,
            TokenKind::For,
            TokenKind::In,
            TokenKind::While,
            TokenKind::Ret,
            TokenKind::Type,
            TokenKind::Mod,
            TokenKind::Use,
            TokenKind::Impl,
            TokenKind::Match,
            TokenKind::True,
            TokenKind::False,
            TokenKind::And,
            TokenKind::Or,
            TokenKind::Not,
        ]
    );
}

#[test]
fn keyword_prefix_is_ident() {
    // Identifiers that start with a keyword prefix should remain identifiers.
    let k = kinds("fns letter iffy elf format inner return typed module used implement matching");
    for kind in &k {
        assert!(
            matches!(kind, TokenKind::Ident(_)),
            "expected Ident, got {:?}",
            kind
        );
    }
}

// -----------------------------------------------------------------------
// Identifiers
// -----------------------------------------------------------------------

#[test]
fn simple_idents() {
    let k = kinds("foo bar _private __dunder x1 snake_case");
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("foo".into()),
            TokenKind::Ident("bar".into()),
            TokenKind::Ident("_private".into()),
            TokenKind::Ident("__dunder".into()),
            TokenKind::Ident("x1".into()),
            TokenKind::Ident("snake_case".into()),
        ]
    );
}

#[test]
fn single_char_ident() {
    let k = kinds("x _ A");
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("x".into()),
            TokenKind::Ident("_".into()),
            TokenKind::Ident("A".into()),
        ]
    );
}

// -----------------------------------------------------------------------
// Integer literals
// -----------------------------------------------------------------------

#[test]
fn integer_simple() {
    let k = kinds("0 1 42 1000000");
    assert_eq!(
        k,
        vec![
            TokenKind::IntLit(0),
            TokenKind::IntLit(1),
            TokenKind::IntLit(42),
            TokenKind::IntLit(1_000_000),
        ]
    );
}

#[test]
fn integer_with_underscores() {
    let k = kinds("1_000 1_000_000 1_2_3");
    assert_eq!(
        k,
        vec![
            TokenKind::IntLit(1_000),
            TokenKind::IntLit(1_000_000),
            TokenKind::IntLit(123),
        ]
    );
}

// -----------------------------------------------------------------------
// Float literals
// -----------------------------------------------------------------------

#[test]
fn float_simple() {
    let k = kinds("3.14 0.5 100.0");
    assert_eq!(
        k,
        vec![
            TokenKind::FloatLit(3.14),
            TokenKind::FloatLit(0.5),
            TokenKind::FloatLit(100.0),
        ]
    );
}

#[test]
fn float_with_underscores() {
    let k = kinds("1_000.50 3.14_15");
    assert_eq!(
        k,
        vec![
            TokenKind::FloatLit(1000.50),
            TokenKind::FloatLit(3.1415),
        ]
    );
}

#[test]
fn int_dot_ident_is_not_float() {
    // `42.method` should be IntLit(42), Dot, Ident("method")
    let k = kinds("42.method");
    assert_eq!(
        k,
        vec![
            TokenKind::IntLit(42),
            TokenKind::Dot,
            TokenKind::Ident("method".into()),
        ]
    );
}

// -----------------------------------------------------------------------
// String literals
// -----------------------------------------------------------------------

#[test]
fn string_simple() {
    let k = kinds(r#""hello""#);
    assert_eq!(k, vec![TokenKind::StringLit("hello".into())]);
}

#[test]
fn string_empty() {
    let k = kinds(r#""""#);
    assert_eq!(k, vec![TokenKind::StringLit("".into())]);
}

#[test]
fn string_escapes() {
    let k = kinds(r#""\n\r\t\\\"\0""#);
    assert_eq!(
        k,
        vec![TokenKind::StringLit("\n\r\t\\\"\0".into())]
    );
}

#[test]
fn string_with_content() {
    let k = kinds(r#""hello world""#);
    assert_eq!(k, vec![TokenKind::StringLit("hello world".into())]);
}

#[test]
fn string_unterminated() {
    let k = kinds(r#""hello"#);
    assert_eq!(
        k,
        vec![TokenKind::Error("unterminated string literal".into())]
    );
}

#[test]
fn string_unterminated_newline() {
    let k = kinds("\"hello\n");
    assert!(matches!(k[0], TokenKind::Error(_)));
}

#[test]
fn string_invalid_escape() {
    let k = kinds(r#""\q""#);
    assert!(matches!(k[0], TokenKind::Error(_)));
}

// -----------------------------------------------------------------------
// Boolean literals (keywords)
// -----------------------------------------------------------------------

#[test]
fn booleans() {
    let k = kinds("true false");
    assert_eq!(k, vec![TokenKind::True, TokenKind::False]);
}

// -----------------------------------------------------------------------
// Operators — single character
// -----------------------------------------------------------------------

#[test]
fn operators_single() {
    let k = kinds("+ - * / % < > =");
    assert_eq!(
        k,
        vec![
            TokenKind::Plus,
            TokenKind::Minus,
            TokenKind::Star,
            TokenKind::Slash,
            TokenKind::Percent,
            TokenKind::Lt,
            TokenKind::Gt,
            TokenKind::Assign,
        ]
    );
}

// -----------------------------------------------------------------------
// Operators — double character
// -----------------------------------------------------------------------

#[test]
fn operators_double() {
    let k = kinds("== != <= >= ->");
    assert_eq!(
        k,
        vec![
            TokenKind::Eq,
            TokenKind::Ne,
            TokenKind::Le,
            TokenKind::Ge,
            TokenKind::Arrow,
        ]
    );
}

// -----------------------------------------------------------------------
// Punctuation
// -----------------------------------------------------------------------

#[test]
fn punctuation() {
    let k = kinds("( ) { } , : . @ ! ?");
    assert_eq!(
        k,
        vec![
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::LBrace,
            TokenKind::RBrace,
            TokenKind::Comma,
            TokenKind::Colon,
            TokenKind::Dot,
            TokenKind::At,
            TokenKind::Bang,
            TokenKind::Question,
        ]
    );
}

// -----------------------------------------------------------------------
// Comments
// -----------------------------------------------------------------------

#[test]
fn comment_discarded() {
    let k = kinds("// this is a comment\n");
    // Comment + newline → blank; nothing emitted.
    assert!(k.is_empty(), "expected empty, got {:?}", k);
}

#[test]
fn comment_after_code() {
    let k = kinds("x // comment\n");
    // x, then newline (the comment is discarded, but the newline at
    // end of the code line is emitted by the main scanner before the
    // comment). Actually, the `//` is hit before the newline in the
    // main scanner, so the comment scanner eats the newline, then
    // we are at the start of a new (empty) logical line. Let's just
    // check we get the identifier.
    assert!(
        k.contains(&TokenKind::Ident("x".into())),
        "expected ident x, got {:?}",
        k
    );
    // And no comment token.
    for kind in &k {
        assert!(
            !matches!(kind, TokenKind::Error(_)),
            "unexpected error: {:?}",
            kind
        );
    }
}

#[test]
fn comment_only_lines_no_newline() {
    // Comment-only lines should not produce NEWLINE.
    let source = "x\n// comment\ny\n";
    let k = kinds(source);
    // Should be: Ident(x), Newline, Ident(y), Newline
    // The comment line should NOT produce a Newline.
    let newline_count = k.iter().filter(|t| **t == TokenKind::Newline).count();
    assert_eq!(newline_count, 2, "got {:?}", k);
}

// -----------------------------------------------------------------------
// Indentation — basic
// -----------------------------------------------------------------------

#[test]
fn indent_simple() {
    let source = "a\n    b\n";
    let k = kinds(source);
    // a NEWLINE INDENT b NEWLINE DEDENT
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("a".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("b".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn indent_dedent_back_to_zero() {
    let source = "a\n    b\nc\n";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("a".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("b".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Ident("c".into()),
            TokenKind::Newline,
        ]
    );
}

#[test]
fn nested_indent() {
    let source = "a\n    b\n        c\n";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("a".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("b".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("c".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn multiple_dedent_at_once() {
    let source = "a\n    b\n        c\nd\n";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("a".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("b".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("c".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Dedent,
            TokenKind::Ident("d".into()),
            TokenKind::Newline,
        ]
    );
}

#[test]
fn blank_lines_ignored() {
    let source = "a\n\n\n    b\n";
    let k = kinds(source);
    // The blank lines should not emit NEWLINE tokens.
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("a".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("b".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn blank_line_with_spaces_ignored() {
    let source = "a\n    \n    b\n";
    let k = kinds(source);
    // Line 2 has only spaces → blank, should be skipped.
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("a".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("b".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn eof_emits_remaining_dedents() {
    let source = "a\n    b\n        c";
    let k = kinds_with_eof(source);
    // a NEWLINE INDENT b NEWLINE INDENT c DEDENT DEDENT EOF
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("a".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("b".into()),
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("c".into()),
            TokenKind::Dedent,
            TokenKind::Dedent,
            TokenKind::Eof,
        ]
    );
}

// -----------------------------------------------------------------------
// Error cases
// -----------------------------------------------------------------------

#[test]
fn tab_in_indentation() {
    let source = "\tx\n";
    let k = kinds(source);
    // Should produce an error token for the tab, then the ident.
    assert!(
        k.iter().any(|t| matches!(t, TokenKind::Error(msg) if msg.contains("tabs"))),
        "expected tab error, got {:?}",
        k
    );
}

#[test]
fn inconsistent_indentation() {
    // Indent to 4, then dedent to 2 (which was never on the stack).
    let source = "a\n    b\n  c\n";
    let k = kinds(source);
    assert!(
        k.iter().any(|t| matches!(t, TokenKind::Error(msg) if msg.contains("inconsistent"))),
        "expected inconsistent indent error, got {:?}",
        k
    );
}

#[test]
fn unknown_character() {
    let k = kinds("~");
    assert!(
        matches!(&k[0], TokenKind::Error(msg) if msg.contains("unexpected character")),
        "got {:?}",
        k
    );
}

// -----------------------------------------------------------------------
// Spans
// -----------------------------------------------------------------------

#[test]
fn span_single_token() {
    let toks = tokens("fn");
    let tok = &toks[0];
    assert_eq!(tok.kind, TokenKind::Fn);
    assert_eq!(tok.span.start.line, 1);
    assert_eq!(tok.span.start.col, 1);
    assert_eq!(tok.span.start.offset, 0);
    assert_eq!(tok.span.end.line, 1);
    assert_eq!(tok.span.end.col, 3);
    assert_eq!(tok.span.end.offset, 2);
}

#[test]
fn span_second_line() {
    let toks = tokens("a\nb");
    // Find 'b'.
    let b_tok = toks.iter().find(|t| t.kind == TokenKind::Ident("b".into())).unwrap();
    assert_eq!(b_tok.span.start.line, 2);
    assert_eq!(b_tok.span.start.col, 1);
}

#[test]
fn span_string_includes_quotes() {
    let toks = tokens(r#""hi""#);
    let tok = &toks[0];
    assert_eq!(tok.kind, TokenKind::StringLit("hi".into()));
    // Span should cover the opening " through closing ": offsets 0..4
    assert_eq!(tok.span.start.offset, 0);
    assert_eq!(tok.span.end.offset, 4);
}

// -----------------------------------------------------------------------
// Display implementations
// -----------------------------------------------------------------------

#[test]
fn token_kind_display() {
    assert_eq!(format!("{}", TokenKind::Fn), "fn");
    assert_eq!(format!("{}", TokenKind::Arrow), "->");
    assert_eq!(format!("{}", TokenKind::IntLit(42)), "42");
    assert_eq!(format!("{}", TokenKind::Ident("foo".into())), "foo");
    assert_eq!(format!("{}", TokenKind::Newline), "NEWLINE");
    assert_eq!(format!("{}", TokenKind::Indent), "INDENT");
    assert_eq!(format!("{}", TokenKind::Dedent), "DEDENT");
    assert_eq!(format!("{}", TokenKind::Eof), "EOF");
}

// -----------------------------------------------------------------------
// Full program test
// -----------------------------------------------------------------------

#[test]
fn full_program_hello() {
    let source = "\
fn main():
    let msg = \"Hello from Gradient!\"
    print(msg)
";
    let k = kinds(source);

    assert_eq!(
        k,
        vec![
            // fn main():
            TokenKind::Fn,
            TokenKind::Ident("main".into()),
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::Colon,
            TokenKind::Newline,
            // INDENT
            TokenKind::Indent,
            // let msg = "Hello from Gradient!"
            TokenKind::Let,
            TokenKind::Ident("msg".into()),
            TokenKind::Assign,
            TokenKind::StringLit("Hello from Gradient!".into()),
            TokenKind::Newline,
            // print(msg)
            TokenKind::Ident("print".into()),
            TokenKind::LParen,
            TokenKind::Ident("msg".into()),
            TokenKind::RParen,
            TokenKind::Newline,
            // DEDENT (back to 0 at end)
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn full_program_fibonacci() {
    let source = "\
fn fib(n: i32) -> i32:
    if n <= 1:
        ret n
    ret fib(n - 1) + fib(n - 2)
";
    let k = kinds(source);

    assert_eq!(
        k,
        vec![
            // fn fib(n: i32) -> i32:
            TokenKind::Fn,
            TokenKind::Ident("fib".into()),
            TokenKind::LParen,
            TokenKind::Ident("n".into()),
            TokenKind::Colon,
            TokenKind::Ident("i32".into()),
            TokenKind::RParen,
            TokenKind::Arrow,
            TokenKind::Ident("i32".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            // INDENT (level 4)
            TokenKind::Indent,
            // if n <= 1:
            TokenKind::If,
            TokenKind::Ident("n".into()),
            TokenKind::Le,
            TokenKind::IntLit(1),
            TokenKind::Colon,
            TokenKind::Newline,
            // INDENT (level 8)
            TokenKind::Indent,
            // ret n
            TokenKind::Ret,
            TokenKind::Ident("n".into()),
            TokenKind::Newline,
            // DEDENT (back to 4)
            TokenKind::Dedent,
            // ret fib(n - 1) + fib(n - 2)
            TokenKind::Ret,
            TokenKind::Ident("fib".into()),
            TokenKind::LParen,
            TokenKind::Ident("n".into()),
            TokenKind::Minus,
            TokenKind::IntLit(1),
            TokenKind::RParen,
            TokenKind::Plus,
            TokenKind::Ident("fib".into()),
            TokenKind::LParen,
            TokenKind::Ident("n".into()),
            TokenKind::Minus,
            TokenKind::IntLit(2),
            TokenKind::RParen,
            TokenKind::Newline,
            // DEDENT (back to 0)
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn full_program_with_match() {
    let source = "\
fn describe(x: i32) -> String:
    match x:
        0:
            ret \"zero\"
        1:
            ret \"one\"
";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            // fn describe(x: i32) -> String:
            TokenKind::Fn,
            TokenKind::Ident("describe".into()),
            TokenKind::LParen,
            TokenKind::Ident("x".into()),
            TokenKind::Colon,
            TokenKind::Ident("i32".into()),
            TokenKind::RParen,
            TokenKind::Arrow,
            TokenKind::Ident("String".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            // INDENT (4)
            TokenKind::Indent,
            // match x:
            TokenKind::Match,
            TokenKind::Ident("x".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            // INDENT (8)
            TokenKind::Indent,
            // 0:
            TokenKind::IntLit(0),
            TokenKind::Colon,
            TokenKind::Newline,
            // INDENT (12)
            TokenKind::Indent,
            // ret "zero"
            TokenKind::Ret,
            TokenKind::StringLit("zero".into()),
            TokenKind::Newline,
            // DEDENT (back to 8)
            TokenKind::Dedent,
            // 1:
            TokenKind::IntLit(1),
            TokenKind::Colon,
            TokenKind::Newline,
            // INDENT (12)
            TokenKind::Indent,
            // ret "one"
            TokenKind::Ret,
            TokenKind::StringLit("one".into()),
            TokenKind::Newline,
            // DEDENT DEDENT DEDENT (12 -> 8 -> 4 -> 0)
            TokenKind::Dedent,
            TokenKind::Dedent,
            TokenKind::Dedent,
        ]
    );
}

// -----------------------------------------------------------------------
// Edge cases
// -----------------------------------------------------------------------

#[test]
fn empty_source() {
    let k = kinds_with_eof("");
    assert_eq!(k, vec![TokenKind::Eof]);
}

#[test]
fn only_whitespace() {
    let k = kinds_with_eof("   ");
    assert_eq!(k, vec![TokenKind::Eof]);
}

#[test]
fn only_newlines() {
    let k = kinds_with_eof("\n\n\n");
    // Blank lines produce no NEWLINE.
    assert_eq!(k, vec![TokenKind::Eof]);
}

#[test]
fn no_trailing_newline() {
    let k = kinds("x");
    assert_eq!(k, vec![TokenKind::Ident("x".into())]);
}

#[test]
fn multiple_tokens_on_one_line() {
    let k = kinds("x + y * z");
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("x".into()),
            TokenKind::Plus,
            TokenKind::Ident("y".into()),
            TokenKind::Star,
            TokenKind::Ident("z".into()),
        ]
    );
}

#[test]
fn arrow_vs_minus() {
    // -> is Arrow, - alone is Minus
    let k = kinds("-> -");
    assert_eq!(k, vec![TokenKind::Arrow, TokenKind::Minus]);
}

#[test]
fn bang_vs_ne() {
    // != is Ne, ! alone is Bang
    let k = kinds("!= !");
    assert_eq!(k, vec![TokenKind::Ne, TokenKind::Bang]);
}

#[test]
fn assign_vs_eq() {
    let k = kinds("= ==");
    assert_eq!(k, vec![TokenKind::Assign, TokenKind::Eq]);
}

#[test]
fn adjacent_operators() {
    let k = kinds("<=>=");
    assert_eq!(k, vec![TokenKind::Le, TokenKind::Ge]);
}

#[test]
fn comment_at_eof_no_newline() {
    let k = kinds("x // comment");
    // x should be there, comment discarded, no crash.
    assert!(k.contains(&TokenKind::Ident("x".into())));
}

#[test]
fn dot_after_int_at_eof() {
    // 42. at EOF → IntLit(42), Dot  (not a float because no digits after .)
    let k = kinds("42.");
    assert_eq!(k, vec![TokenKind::IntLit(42), TokenKind::Dot]);
}

#[test]
fn float_zero_point_zero() {
    let k = kinds("0.0");
    assert_eq!(k, vec![TokenKind::FloatLit(0.0)]);
}

#[test]
fn keyword_from_str_lookup() {
    assert_eq!(keyword_from_str("fn"), Some(TokenKind::Fn));
    assert_eq!(keyword_from_str("let"), Some(TokenKind::Let));
    assert_eq!(keyword_from_str("hello"), None);
    assert_eq!(keyword_from_str(""), None);
}

#[test]
fn carriage_return_newline() {
    let k = kinds("a\r\nb");
    assert!(k.contains(&TokenKind::Ident("a".into())));
    assert!(k.contains(&TokenKind::Ident("b".into())));
}

#[test]
fn all_punctuation_in_expression() {
    let k = kinds("f(a, b): @x.y? + !z");
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("f".into()),
            TokenKind::LParen,
            TokenKind::Ident("a".into()),
            TokenKind::Comma,
            TokenKind::Ident("b".into()),
            TokenKind::RParen,
            TokenKind::Colon,
            TokenKind::At,
            TokenKind::Ident("x".into()),
            TokenKind::Dot,
            TokenKind::Ident("y".into()),
            TokenKind::Question,
            TokenKind::Plus,
            TokenKind::Bang,
            TokenKind::Ident("z".into()),
        ]
    );
}

#[test]
fn for_in_loop() {
    let source = "\
for x in items:
    use x
";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::For,
            TokenKind::Ident("x".into()),
            TokenKind::In,
            TokenKind::Ident("items".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Use,
            TokenKind::Ident("x".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn type_definition() {
    let source = "\
type Point:
    x: f64
    y: f64
";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Type,
            TokenKind::Ident("Point".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("x".into()),
            TokenKind::Colon,
            TokenKind::Ident("f64".into()),
            TokenKind::Newline,
            TokenKind::Ident("y".into()),
            TokenKind::Colon,
            TokenKind::Ident("f64".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn impl_block() {
    let source = "\
impl Point:
    fn new(x: f64, y: f64) -> Point:
        ret Point { x: x, y: y }
";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Impl,
            TokenKind::Ident("Point".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Fn,
            TokenKind::Ident("new".into()),
            TokenKind::LParen,
            TokenKind::Ident("x".into()),
            TokenKind::Colon,
            TokenKind::Ident("f64".into()),
            TokenKind::Comma,
            TokenKind::Ident("y".into()),
            TokenKind::Colon,
            TokenKind::Ident("f64".into()),
            TokenKind::RParen,
            TokenKind::Arrow,
            TokenKind::Ident("Point".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ret,
            TokenKind::Ident("Point".into()),
            TokenKind::LBrace,
            TokenKind::Ident("x".into()),
            TokenKind::Colon,
            TokenKind::Ident("x".into()),
            TokenKind::Comma,
            TokenKind::Ident("y".into()),
            TokenKind::Colon,
            TokenKind::Ident("y".into()),
            TokenKind::RBrace,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn logical_operators() {
    let k = kinds("a and b or not c");
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("a".into()),
            TokenKind::And,
            TokenKind::Ident("b".into()),
            TokenKind::Or,
            TokenKind::Not,
            TokenKind::Ident("c".into()),
        ]
    );
}

#[test]
fn if_else() {
    let source = "\
if x > 0:
    ret true
else:
    ret false
";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::If,
            TokenKind::Ident("x".into()),
            TokenKind::Gt,
            TokenKind::IntLit(0),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ret,
            TokenKind::True,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Else,
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ret,
            TokenKind::False,
            TokenKind::Newline,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn mod_and_use() {
    let k = kinds("mod math\nuse math\n");
    assert_eq!(
        k,
        vec![
            TokenKind::Mod,
            TokenKind::Ident("math".into()),
            TokenKind::Newline,
            TokenKind::Use,
            TokenKind::Ident("math".into()),
            TokenKind::Newline,
        ]
    );
}

// -----------------------------------------------------------------------
// Mutable and while keywords
// -----------------------------------------------------------------------

#[test]
fn mut_keyword() {
    let k = kinds("mut");
    assert_eq!(k, vec![TokenKind::Mut]);
}

#[test]
fn while_keyword() {
    let k = kinds("while");
    assert_eq!(k, vec![TokenKind::While]);
}

#[test]
fn let_mut_binding() {
    let source = "let mut x = 5";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Let,
            TokenKind::Mut,
            TokenKind::Ident("x".into()),
            TokenKind::Assign,
            TokenKind::IntLit(5),
        ]
    );
}

#[test]
fn while_loop_tokens() {
    let source = "\
while x > 0:
    x = x - 1
";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::While,
            TokenKind::Ident("x".into()),
            TokenKind::Gt,
            TokenKind::IntLit(0),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("x".into()),
            TokenKind::Assign,
            TokenKind::Ident("x".into()),
            TokenKind::Minus,
            TokenKind::IntLit(1),
            TokenKind::Newline,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn mut_ident_prefix_is_ident() {
    // 'mutable' starts with 'mut' but should be an ident, not a keyword.
    let k = kinds("mutable");
    assert_eq!(k, vec![TokenKind::Ident("mutable".into())]);
}

#[test]
fn while_ident_prefix_is_ident() {
    let k = kinds("whileTrue");
    assert_eq!(k, vec![TokenKind::Ident("whileTrue".into())]);
}

// -----------------------------------------------------------------------
// Pipe token (for enum declarations)
// -----------------------------------------------------------------------

#[test]
fn pipe_token() {
    let k = kinds("|");
    assert_eq!(k, vec![TokenKind::Pipe]);
}

#[test]
fn enum_declaration_tokens() {
    let source = "type Color = Red | Green | Blue\n";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Type,
            TokenKind::Ident("Color".into()),
            TokenKind::Assign,
            TokenKind::Ident("Red".into()),
            TokenKind::Pipe,
            TokenKind::Ident("Green".into()),
            TokenKind::Pipe,
            TokenKind::Ident("Blue".into()),
            TokenKind::Newline,
        ]
    );
}

#[test]
fn enum_with_tuple_variant_tokens() {
    let source = "type Option = Some(Int) | None\n";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Type,
            TokenKind::Ident("Option".into()),
            TokenKind::Assign,
            TokenKind::Ident("Some".into()),
            TokenKind::LParen,
            TokenKind::Ident("Int".into()),
            TokenKind::RParen,
            TokenKind::Pipe,
            TokenKind::Ident("None".into()),
            TokenKind::Newline,
        ]
    );
}

// -----------------------------------------------------------------------
// Actor keywords
// -----------------------------------------------------------------------

#[test]
fn actor_keywords() {
    let k = kinds("actor state on spawn send ask");
    assert_eq!(
        k,
        vec![
            TokenKind::Actor,
            TokenKind::State,
            TokenKind::On,
            TokenKind::Spawn,
            TokenKind::Send,
            TokenKind::Ask,
        ]
    );
}

#[test]
fn actor_keyword_prefix_is_ident() {
    let k = kinds("actors stated only spawned sender asking");
    for kind in &k {
        assert!(
            matches!(kind, TokenKind::Ident(_)),
            "expected Ident, got {:?}",
            kind
        );
    }
}

#[test]
fn actor_declaration_tokens() {
    let source = "\
actor Counter:
    state count: Int = 0
    on Increment:
        count = count + 1
    on GetCount -> Int:
        ret count
";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Actor,
            TokenKind::Ident("Counter".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::State,
            TokenKind::Ident("count".into()),
            TokenKind::Colon,
            TokenKind::Ident("Int".into()),
            TokenKind::Assign,
            TokenKind::IntLit(0),
            TokenKind::Newline,
            TokenKind::On,
            TokenKind::Ident("Increment".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ident("count".into()),
            TokenKind::Assign,
            TokenKind::Ident("count".into()),
            TokenKind::Plus,
            TokenKind::IntLit(1),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::On,
            TokenKind::Ident("GetCount".into()),
            TokenKind::Arrow,
            TokenKind::Ident("Int".into()),
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Ret,
            TokenKind::Ident("count".into()),
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::Dedent,
        ]
    );
}

#[test]
fn spawn_send_ask_tokens() {
    let source = "spawn Counter\nsend c Increment\nask c GetCount\n";
    let k = kinds(source);
    assert_eq!(
        k,
        vec![
            TokenKind::Spawn,
            TokenKind::Ident("Counter".into()),
            TokenKind::Newline,
            TokenKind::Send,
            TokenKind::Ident("c".into()),
            TokenKind::Ident("Increment".into()),
            TokenKind::Newline,
            TokenKind::Ask,
            TokenKind::Ident("c".into()),
            TokenKind::Ident("GetCount".into()),
            TokenKind::Newline,
        ]
    );
}

// -----------------------------------------------------------------------
// Closure / lambda token sequences
// -----------------------------------------------------------------------

#[test]
fn closure_simple_pipe_tokens() {
    // |x| x + 1
    let k = kinds("|x| x + 1");
    assert_eq!(
        k,
        vec![
            TokenKind::Pipe,
            TokenKind::Ident("x".into()),
            TokenKind::Pipe,
            TokenKind::Ident("x".into()),
            TokenKind::Plus,
            TokenKind::IntLit(1),
        ]
    );
}

#[test]
fn closure_zero_param_tokens() {
    // || 42
    let k = kinds("|| 42");
    assert_eq!(
        k,
        vec![
            TokenKind::Pipe,
            TokenKind::Pipe,
            TokenKind::IntLit(42),
        ]
    );
}

#[test]
fn closure_typed_param_tokens() {
    // |x: Int, y: Int| -> Int: x + y
    let k = kinds("|x: Int, y: Int| -> Int: x + y");
    assert_eq!(
        k,
        vec![
            TokenKind::Pipe,
            TokenKind::Ident("x".into()),
            TokenKind::Colon,
            TokenKind::Ident("Int".into()),
            TokenKind::Comma,
            TokenKind::Ident("y".into()),
            TokenKind::Colon,
            TokenKind::Ident("Int".into()),
            TokenKind::Pipe,
            TokenKind::Arrow,
            TokenKind::Ident("Int".into()),
            TokenKind::Colon,
            TokenKind::Ident("x".into()),
            TokenKind::Plus,
            TokenKind::Ident("y".into()),
        ]
    );
}

// -----------------------------------------------------------------------
// Interpolated strings
// -----------------------------------------------------------------------

#[test]
fn interpolated_string_no_expressions() {
    let k = kinds(r#"f"hello world""#);
    assert_eq!(
        k,
        vec![TokenKind::InterpolatedString(vec![
            InterpolationPart::Literal("hello world".into()),
        ])]
    );
}

#[test]
fn interpolated_string_simple_expression() {
    let k = kinds(r#"f"hello {name}""#);
    assert_eq!(
        k,
        vec![TokenKind::InterpolatedString(vec![
            InterpolationPart::Literal("hello ".into()),
            InterpolationPart::Expr("name".into()),
        ])]
    );
}

#[test]
fn interpolated_string_multiple_expressions() {
    let k = kinds(r#"f"{x} items cost {price}""#);
    assert_eq!(
        k,
        vec![TokenKind::InterpolatedString(vec![
            InterpolationPart::Expr("x".into()),
            InterpolationPart::Literal(" items cost ".into()),
            InterpolationPart::Expr("price".into()),
        ])]
    );
}

#[test]
fn interpolated_string_expression_with_operators() {
    let k = kinds(r#"f"result = {2 + 2}""#);
    assert_eq!(
        k,
        vec![TokenKind::InterpolatedString(vec![
            InterpolationPart::Literal("result = ".into()),
            InterpolationPart::Expr("2 + 2".into()),
        ])]
    );
}

#[test]
fn interpolated_string_escaped_braces() {
    let k = kinds(r#"f"use {{braces}}""#);
    assert_eq!(
        k,
        vec![TokenKind::InterpolatedString(vec![
            InterpolationPart::Literal("use {braces}".into()),
        ])]
    );
}

#[test]
fn f_followed_by_non_quote_is_ident() {
    // f not followed by " should be a regular identifier
    let k = kinds("foo");
    assert_eq!(k, vec![TokenKind::Ident("foo".into())]);
}

#[test]
fn f_as_ident_alone() {
    // Just "f" followed by something other than " should be an identifier
    let k = kinds("f + 1");
    assert_eq!(
        k,
        vec![
            TokenKind::Ident("f".into()),
            TokenKind::Plus,
            TokenKind::IntLit(1),
        ]
    );
}

#[test]
fn interpolated_string_empty() {
    let k = kinds(r#"f"""#);
    assert_eq!(
        k,
        vec![TokenKind::InterpolatedString(vec![])]
    );
}
