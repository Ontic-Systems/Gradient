//! Hand-written lexer for the Gradient programming language.
//!
//! Converts raw `.gr` source text into a stream of [`Token`]s, including
//! Python-style INDENT / DEDENT injection based on leading whitespace.

use std::collections::VecDeque;

use super::token::{keyword_from_str, InterpolationPart, Position, Span, Token, TokenKind};

/// The Gradient lexer.
///
/// Consumes a source string and produces a `Vec<Token>` ending with
/// [`TokenKind::Eof`]. Indentation-sensitive: it maintains a stack of
/// indentation levels and emits `INDENT` / `DEDENT` tokens at the start
/// of each logical line.
pub struct Lexer<'src> {
    /// The full source text being tokenized.
    source: &'src str,
    /// The source as a `Vec<char>` for convenient indexed access.
    chars: Vec<char>,
    /// Current position in `chars`.
    pos: usize,
    /// Current 1-based line number.
    line: u32,
    /// Current 1-based column number.
    col: u32,
    /// Current 0-based byte offset, incremented in `advance()`.
    byte_offset: u32,
    /// The file id for spans.
    file_id: u32,
    /// Stack of indentation levels (column counts). Starts with `[0]`.
    indent_stack: Vec<u32>,
    /// Queue for tokens that need to be emitted before scanning resumes
    /// (e.g. multiple DEDENTs or an INDENT followed by the first token
    /// on that line).
    pending_tokens: VecDeque<Token>,
    /// `true` when the lexer is at the very beginning of a logical line
    /// and needs to measure indentation before scanning the next token.
    at_line_start: bool,
}

impl<'src> Lexer<'src> {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new lexer for the given source text.
    pub fn new(source: &'src str, file_id: u32) -> Self {
        Self {
            source,
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            byte_offset: 0,
            file_id,
            indent_stack: vec![0],
            pending_tokens: VecDeque::new(),
            at_line_start: true,
        }
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    /// Return the original source text.
    pub fn source(&self) -> &str {
        self.source
    }

    /// Tokenize the entire source, returning a vector of tokens ending
    /// with `Eof`.
    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        tokens
    }

    // ------------------------------------------------------------------
    // Core scanning loop
    // ------------------------------------------------------------------

    /// Produce the next token. If there are pending tokens queued (from
    /// indent processing), drain those first.
    fn next_token(&mut self) -> Token {
        // Drain pending queue first.
        if let Some(tok) = self.pending_tokens.pop_front() {
            return tok;
        }

        // At the start of a logical line, handle indentation.
        if self.at_line_start {
            self.scan_indent();
            // scan_indent may have queued tokens.
            if let Some(tok) = self.pending_tokens.pop_front() {
                return tok;
            }
        }

        // Skip inline whitespace (spaces only; newlines are meaningful).
        self.skip_whitespace();

        // Check for end of file.
        if self.is_at_end() {
            return self.emit_eof();
        }

        let ch = self.peek().unwrap();

        match ch {
            // Newlines
            '\n' => {
                let start = self.current_position();
                self.advance();
                self.at_line_start = true;
                Token::new(
                    TokenKind::Newline,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }
            '\r' => {
                let start = self.current_position();
                self.advance();
                // Consume \r\n as a single newline.
                if self.peek() == Some('\n') {
                    self.advance();
                }
                self.at_line_start = true;
                Token::new(
                    TokenKind::Newline,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }

            // Doc comments (///) — must check before regular comments
            '/' if self.peek_at(1) == Some('/') && self.peek_at(2) == Some('/') => {
                self.scan_doc_comment()
            }

            // Comments (inline, after code on same line)
            '/' if self.peek_at(1) == Some('/') => {
                // Pass true to set at_line_start so indentation is tracked
                // for the next line after this inline comment.
                self.scan_comment(true);
                // After a comment, the rest of the line is consumed.
                // Recurse to get the next real token.
                self.next_token()
            }

            // String literals
            '"' => self.scan_string(),

            // Character literals
            '\'' => self.scan_char(),

            // Number literals
            c if c.is_ascii_digit() => self.scan_number(),

            // Interpolated string: f"..."
            'f' if self.peek_at(1) == Some('"') => self.scan_interpolated_string(),

            // Identifiers and keywords
            c if c.is_ascii_alphabetic() || c == '_' => self.scan_ident_or_keyword(),

            // Two-character operators (check before single-char)
            '-' if self.peek_at(1) == Some('>') => {
                let start = self.current_position();
                self.advance(); // -
                self.advance(); // >
                Token::new(
                    TokenKind::Arrow,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }
            '=' if self.peek_at(1) == Some('=') => {
                let start = self.current_position();
                self.advance(); // =
                self.advance(); // =
                Token::new(
                    TokenKind::Eq,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }
            '!' if self.peek_at(1) == Some('=') => {
                let start = self.current_position();
                self.advance(); // !
                self.advance(); // =
                Token::new(
                    TokenKind::Ne,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }
            '<' if self.peek_at(1) == Some('=') => {
                let start = self.current_position();
                self.advance(); // <
                self.advance(); // =
                Token::new(
                    TokenKind::Le,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }
            '>' if self.peek_at(1) == Some('=') => {
                let start = self.current_position();
                self.advance(); // >
                self.advance(); // =
                Token::new(
                    TokenKind::Ge,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }

            // Single-character operators and punctuation
            '+' => self.single_char_token(TokenKind::Plus),
            '-' => self.single_char_token(TokenKind::Minus),
            '*' => self.single_char_token(TokenKind::Star),
            '/' => self.single_char_token(TokenKind::Slash),
            '%' => self.single_char_token(TokenKind::Percent),
            '<' => self.single_char_token(TokenKind::Lt),
            '>' => self.single_char_token(TokenKind::Gt),
            '=' => self.single_char_token(TokenKind::Assign),
            '(' => self.single_char_token(TokenKind::LParen),
            ')' => self.single_char_token(TokenKind::RParen),
            '{' => self.single_char_token(TokenKind::LBrace),
            '}' => self.single_char_token(TokenKind::RBrace),
            '[' => self.single_char_token(TokenKind::LBracket),
            ']' => self.single_char_token(TokenKind::RBracket),
            ',' => self.single_char_token(TokenKind::Comma),
            ':' => self.single_char_token(TokenKind::Colon),
            '.' if self.peek_at(1) == Some('.') => {
                let start = self.current_position();
                self.advance(); // first .
                self.advance(); // second .
                Token::new(
                    TokenKind::DotDot,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }
            '.' => self.single_char_token(TokenKind::Dot),
            '@' => self.single_char_token(TokenKind::At),
            '!' => self.single_char_token(TokenKind::Bang),
            '?' => self.single_char_token(TokenKind::Question),
            '|' if self.peek_at(1) == Some('>') => {
                let start = self.current_position();
                self.advance(); // |
                self.advance(); // >
                Token::new(
                    TokenKind::PipeArrow,
                    Span::new(self.file_id, start, self.current_position()),
                )
            }
            '|' => self.single_char_token(TokenKind::Pipe),

            // Tab character is illegal
            '\t' => {
                let start = self.current_position();
                self.advance();
                let end = self.current_position();
                Token::new(
                    TokenKind::Error("tabs are not allowed; use spaces for indentation".into()),
                    Span::new(self.file_id, start, end),
                )
            }

            // Unknown character
            _ => {
                let start = self.current_position();
                let bad = self.advance().unwrap();
                let end = self.current_position();
                Token::new(
                    TokenKind::Error(format!("unexpected character: '{}'", bad)),
                    Span::new(self.file_id, start, end),
                )
            }
        }
    }

    // ------------------------------------------------------------------
    // Indentation handling
    // ------------------------------------------------------------------

    /// Called at the start of every logical line. Measures leading spaces,
    /// compares against the indentation stack, and queues INDENT / DEDENT /
    /// NEWLINE tokens as appropriate.
    ///
    /// Blank lines and comment-only lines are skipped entirely — they do
    /// not produce NEWLINE tokens.
    fn scan_indent(&mut self) {
        self.at_line_start = false;

        loop {
            let line_start_pos = self.current_position();
            let mut indent: u32 = 0;
            let mut has_tab = false;

            // Measure leading whitespace.
            while let Some(ch) = self.peek() {
                match ch {
                    ' ' => {
                        indent += 1;
                        self.advance();
                    }
                    '\t' => {
                        has_tab = true;
                        self.advance();
                        // We still count so we can report position, but we'll error.
                        indent += 1;
                    }
                    _ => break,
                }
            }

            if has_tab {
                let end = self.current_position();
                self.pending_tokens.push_back(Token::new(
                    TokenKind::Error("tabs are not allowed; use spaces for indentation".into()),
                    Span::new(self.file_id, line_start_pos, end),
                ));
                // Continue processing — treat the indentation level as
                // whatever we measured, so the rest of the line can still
                // be lexed.
            }

            // Check for blank line or comment-only line.
            match self.peek() {
                None => {
                    // EOF after whitespace — don't emit NEWLINE for a
                    // trailing blank line, just return so the main loop
                    // can handle EOF dedents.
                    return;
                }
                Some('\n') => {
                    // Blank line — consume the newline and loop to the
                    // next line. No NEWLINE token emitted.
                    self.advance();
                    continue;
                }
                Some('\r') => {
                    self.advance();
                    if self.peek() == Some('\n') {
                        self.advance();
                    }
                    continue;
                }
                Some('/') if self.peek_at(1) == Some('/') && self.peek_at(2) != Some('/') => {
                    // Comment-only line (NOT a doc comment) — consume the
                    // comment and newline, then loop.
                    // Pass false because we're already inside scan_indent
                    // handling the indentation; don't set at_line_start.
                    self.scan_comment(false);
                    continue;
                }
                _ => {}
            }

            // Determine whether this line starts with a doc comment.
            // We detect it here but emit the token AFTER processing
            // indentation changes (DEDENTs), so that the parser sees
            // the correct block structure before the doc comment.
            let is_doc_comment = matches!(self.peek(), Some('/'))
                && self.peek_at(1) == Some('/')
                && self.peek_at(2) == Some('/');

            // Compare against indentation stack.
            let Some(&current_indent) = self.indent_stack.last() else {
                let pos = self.current_position();
                self.pending_tokens.push_back(Token::new(
                    TokenKind::Error("invalid indentation state".into()),
                    Span::point(self.file_id, pos),
                ));
                return;
            };

            if indent > current_indent {
                // Increased indentation: push and emit INDENT.
                self.indent_stack.push(indent);
                let pos = self.current_position();
                self.pending_tokens.push_back(Token::new(
                    TokenKind::Indent,
                    Span::point(self.file_id, pos),
                ));
            } else if indent < current_indent {
                // Decreased indentation: pop and emit DEDENTs.
                while matches!(self.indent_stack.last(), Some(&level) if level > indent) {
                    self.indent_stack.pop();
                    let pos = self.current_position();
                    self.pending_tokens.push_back(Token::new(
                        TokenKind::Dedent,
                        Span::point(self.file_id, pos),
                    ));
                }
                // Verify we landed on a valid indentation level.
                if !matches!(self.indent_stack.last(), Some(&level) if level == indent) {
                    let pos = self.current_position();
                    self.pending_tokens.push_back(Token::new(
                        TokenKind::Error(
                            "inconsistent indentation: does not match any outer level".into(),
                        ),
                        Span::point(self.file_id, pos),
                    ));
                }
            }
            // If indent == current_indent, nothing to emit (same level).

            // If this line starts with a doc comment, emit the token now
            // (after any INDENT/DEDENT tokens have been queued).
            if is_doc_comment {
                let doc_tok = self.scan_doc_comment();
                self.pending_tokens.push_back(doc_tok);
            }

            return;
        }
    }

    // ------------------------------------------------------------------
    // Whitespace and comments
    // ------------------------------------------------------------------

    /// Skip inline whitespace (spaces only, NOT newlines).
    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == ' ' {
                self.advance();
            } else {
                break;
            }
        }
    }

    /// Skip a `//` comment to the end of line (consuming the newline).
    ///
    /// # Arguments
    /// * `set_line_start` - If true, sets `at_line_start = true` after consuming
    ///   the newline. This should be true when the comment follows code on the
    ///   same line (inline comment), so that indentation is properly tracked.
    ///   It should be false for comment-only lines where we're already inside
    ///   `scan_indent` handling the indentation.
    fn scan_comment(&mut self, set_line_start: bool) {
        // Consume the `//`.
        self.advance(); // /
        self.advance(); // /

        // Consume everything until newline or EOF.
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                // Consume the newline so that comment-only lines don't
                // trigger NEWLINE emission in the indent scanner.
                self.advance();
                if set_line_start {
                    // Mark that we're at the start of a new line so that
                    // indentation is properly tracked on the next token.
                    self.at_line_start = true;
                }
                return;
            }
            if ch == '\r' {
                self.advance();
                if self.peek() == Some('\n') {
                    self.advance();
                }
                if set_line_start {
                    // Mark that we're at the start of a new line so that
                    // indentation is properly tracked on the next token.
                    self.at_line_start = true;
                }
                return;
            }
            self.advance();
        }
    }

    /// Scan a `///` doc comment, returning a `DocComment` token.
    ///
    /// Consumes the `///` prefix and the rest of the line (including the
    /// trailing newline). A single leading space after `///` is stripped
    /// if present, so `/// Hello` produces `"Hello"`.
    fn scan_doc_comment(&mut self) -> Token {
        let start = self.current_position();

        // Consume the `///`.
        self.advance(); // /
        self.advance(); // /
        self.advance(); // /

        // Strip a single leading space if present.
        if self.peek() == Some(' ') {
            self.advance();
        }

        // Collect the rest of the line.
        let mut text = String::new();
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                self.advance();
                break;
            }
            if ch == '\r' {
                self.advance();
                if self.peek() == Some('\n') {
                    self.advance();
                }
                break;
            }
            text.push(ch);
            self.advance();
        }

        let end = self.current_position();
        Token::new(
            TokenKind::DocComment(text),
            Span::new(self.file_id, start, end),
        )
    }

    // ------------------------------------------------------------------
    // Number literals
    // ------------------------------------------------------------------

    /// Scan an integer or floating-point literal.
    ///
    /// Grammar:
    /// - INT_LIT:   `[0-9][0-9_]*`
    /// - FLOAT_LIT: `[0-9][0-9_]*.[0-9][0-9_]*`
    ///
    /// When we see `digits.digits` we produce a float. When we see
    /// `digits.ident` (i.e. the character after the dot is not a digit)
    /// we produce an integer and leave the `.` for the next token.
    fn scan_number(&mut self) -> Token {
        let start = self.current_position();

        // Scan the integer part.
        let int_start = self.pos;
        self.consume_digits();
        let int_end = self.pos;

        // Check for a fractional part.
        if self.peek() == Some('.') {
            // Look ahead past the dot.
            if let Some(after_dot) = self.peek_at(1) {
                if after_dot.is_ascii_digit() {
                    // It's a float literal.
                    self.advance(); // consume '.'
                    self.consume_digits();
                    let end = self.current_position();

                    // Build the text without underscores for parsing.
                    let raw: String = self.chars[int_start..self.pos]
                        .iter()
                        .filter(|c| **c != '_')
                        .collect();

                    match raw.parse::<f64>() {
                        Ok(val) => {
                            return Token::new(
                                TokenKind::FloatLit(val),
                                Span::new(self.file_id, start, end),
                            );
                        }
                        Err(e) => {
                            return Token::new(
                                TokenKind::Error(format!("invalid float literal: {}", e)),
                                Span::new(self.file_id, start, end),
                            );
                        }
                    }
                }
            }
            // Otherwise the dot is not part of the number (e.g. method call).
        }

        let end = self.current_position();

        // Integer literal.
        let raw: String = self.chars[int_start..int_end]
            .iter()
            .filter(|c| **c != '_')
            .collect();

        match raw.parse::<i64>() {
            Ok(val) => Token::new(TokenKind::IntLit(val), Span::new(self.file_id, start, end)),
            Err(e) => Token::new(
                TokenKind::Error(format!("invalid integer literal: {}", e)),
                Span::new(self.file_id, start, end),
            ),
        }
    }

    /// Consume a run of ASCII digits and underscores.
    fn consume_digits(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() || ch == '_' {
                self.advance();
            } else {
                break;
            }
        }
    }

    // ------------------------------------------------------------------
    // String literals
    // ------------------------------------------------------------------

    /// Scan a double-quoted string literal with escape handling.
    ///
    /// Supported escapes: `\n`, `\r`, `\t`, `\\`, `\"`, `\0`.
    fn scan_string(&mut self) -> Token {
        let start = self.current_position();
        self.advance(); // opening "

        let mut value = String::new();

        loop {
            match self.peek() {
                None | Some('\n') | Some('\r') => {
                    let end = self.current_position();
                    return Token::new(
                        TokenKind::Error("unterminated string literal".into()),
                        Span::new(self.file_id, start, end),
                    );
                }
                Some('"') => {
                    self.advance(); // closing "
                    let end = self.current_position();
                    return Token::new(
                        TokenKind::StringLit(value),
                        Span::new(self.file_id, start, end),
                    );
                }
                Some('\\') => {
                    self.advance(); // backslash
                    match self.peek() {
                        Some('n') => {
                            value.push('\n');
                            self.advance();
                        }
                        Some('r') => {
                            value.push('\r');
                            self.advance();
                        }
                        Some('t') => {
                            value.push('\t');
                            self.advance();
                        }
                        Some('\\') => {
                            value.push('\\');
                            self.advance();
                        }
                        Some('"') => {
                            value.push('"');
                            self.advance();
                        }
                        Some('0') => {
                            value.push('\0');
                            self.advance();
                        }
                        Some(c) => {
                            let end = self.current_position();
                            self.advance();
                            return Token::new(
                                TokenKind::Error(format!("invalid escape sequence: \\{}", c)),
                                Span::new(self.file_id, start, end),
                            );
                        }
                        None => {
                            let end = self.current_position();
                            return Token::new(
                                TokenKind::Error("unterminated string literal".into()),
                                Span::new(self.file_id, start, end),
                            );
                        }
                    }
                }
                Some(c) => {
                    value.push(c);
                    self.advance();
                }
            }
        }
    }

    /// Scan a single-quoted character literal with escape handling.
    ///
    /// Supported escapes: `\n`, `\r`, `\t`, `\\`, `\'`, `\0`.
    fn scan_char(&mut self) -> Token {
        let start = self.current_position();
        self.advance(); // opening '

        let c = match self.peek() {
            Some('\\') => {
                self.advance(); // backslash
                match self.peek() {
                    Some('n') => {
                        self.advance();
                        '\n'
                    }
                    Some('r') => {
                        self.advance();
                        '\r'
                    }
                    Some('t') => {
                        self.advance();
                        '\t'
                    }
                    Some('\\') => {
                        self.advance();
                        '\\'
                    }
                    Some('\'') => {
                        self.advance();
                        '\''
                    }
                    Some('0') => {
                        self.advance();
                        '\0'
                    }
                    Some(c) => {
                        let end = self.current_position();
                        self.advance();
                        return Token::new(
                            TokenKind::Error(format!("invalid escape sequence: \\{}", c)),
                            Span::new(self.file_id, start, end),
                        );
                    }
                    None => {
                        let end = self.current_position();
                        return Token::new(
                            TokenKind::Error("unterminated character literal".into()),
                            Span::new(self.file_id, start, end),
                        );
                    }
                }
            }
            Some(c) => {
                self.advance();
                c
            }
            None => {
                let end = self.current_position();
                return Token::new(
                    TokenKind::Error("unterminated character literal".into()),
                    Span::new(self.file_id, start, end),
                );
            }
        };

        // Expect closing quote
        if self.peek() == Some('\'') {
            self.advance(); // closing '
            let end = self.current_position();
            Token::new(TokenKind::CharLit(c), Span::new(self.file_id, start, end))
        } else {
            let end = self.current_position();
            Token::new(
                TokenKind::Error("expected closing ' for character literal".into()),
                Span::new(self.file_id, start, end),
            )
        }
    }

    // ------------------------------------------------------------------
    // Interpolated string literals
    // ------------------------------------------------------------------

    /// Scan an interpolated string literal: `f"...{expr}..."`.
    ///
    /// The `f` prefix has already been peeked (current char is `f`, next is `"`).
    /// Collects alternating literal segments and expression segments.
    /// `{{` is treated as an escaped literal `{`.
    fn scan_interpolated_string(&mut self) -> Token {
        let start = self.current_position();
        self.advance(); // consume 'f'
        self.advance(); // consume opening '"'

        let mut parts: Vec<InterpolationPart> = Vec::new();
        let mut current_literal = String::new();

        loop {
            match self.peek() {
                None | Some('\n') | Some('\r') => {
                    let end = self.current_position();
                    return Token::new(
                        TokenKind::Error("unterminated interpolated string literal".into()),
                        Span::new(self.file_id, start, end),
                    );
                }
                Some('"') => {
                    self.advance(); // closing "
                                    // Flush any remaining literal.
                    if !current_literal.is_empty() {
                        parts.push(InterpolationPart::Literal(current_literal));
                    }
                    let end = self.current_position();
                    return Token::new(
                        TokenKind::InterpolatedString(parts),
                        Span::new(self.file_id, start, end),
                    );
                }
                Some('{') => {
                    // Check for escaped brace `{{`.
                    if self.peek_at(1) == Some('{') {
                        self.advance(); // first {
                        self.advance(); // second {
                        current_literal.push('{');
                        continue;
                    }
                    // Flush the current literal segment.
                    if !current_literal.is_empty() {
                        parts.push(InterpolationPart::Literal(std::mem::take(
                            &mut current_literal,
                        )));
                    }
                    self.advance(); // consume '{'

                    // Collect the expression text until the matching '}'.
                    let mut expr_text = String::new();
                    let mut brace_depth: u32 = 1;
                    loop {
                        match self.peek() {
                            None | Some('\n') | Some('\r') => {
                                let end = self.current_position();
                                return Token::new(
                                    TokenKind::Error(
                                        "unterminated interpolation expression".into(),
                                    ),
                                    Span::new(self.file_id, start, end),
                                );
                            }
                            Some('{') => {
                                brace_depth += 1;
                                expr_text.push('{');
                                self.advance();
                            }
                            Some('}') => {
                                brace_depth -= 1;
                                if brace_depth == 0 {
                                    self.advance(); // consume closing '}'
                                    break;
                                }
                                expr_text.push('}');
                                self.advance();
                            }
                            Some('"') => {
                                // String literal inside the expression.
                                expr_text.push('"');
                                self.advance();
                                // Scan through the nested string.
                                loop {
                                    match self.peek() {
                                        None | Some('\n') | Some('\r') => break,
                                        Some('\\') => {
                                            expr_text.push('\\');
                                            self.advance();
                                            if let Some(c) = self.peek() {
                                                expr_text.push(c);
                                                self.advance();
                                            }
                                        }
                                        Some('"') => {
                                            expr_text.push('"');
                                            self.advance();
                                            break;
                                        }
                                        Some(c) => {
                                            expr_text.push(c);
                                            self.advance();
                                        }
                                    }
                                }
                            }
                            Some(c) => {
                                expr_text.push(c);
                                self.advance();
                            }
                        }
                    }
                    parts.push(InterpolationPart::Expr(expr_text));
                }
                Some('}') => {
                    // Check for escaped brace `}}`.
                    if self.peek_at(1) == Some('}') {
                        self.advance(); // first }
                        self.advance(); // second }
                        current_literal.push('}');
                        continue;
                    }
                    // Unmatched '}' — treat as literal for error recovery.
                    current_literal.push('}');
                    self.advance();
                }
                Some('\\') => {
                    // Handle escape sequences in the literal parts.
                    self.advance(); // consume backslash
                    match self.peek() {
                        Some('n') => {
                            current_literal.push('\n');
                            self.advance();
                        }
                        Some('r') => {
                            current_literal.push('\r');
                            self.advance();
                        }
                        Some('t') => {
                            current_literal.push('\t');
                            self.advance();
                        }
                        Some('\\') => {
                            current_literal.push('\\');
                            self.advance();
                        }
                        Some('"') => {
                            current_literal.push('"');
                            self.advance();
                        }
                        Some('0') => {
                            current_literal.push('\0');
                            self.advance();
                        }
                        Some(c) => {
                            let end = self.current_position();
                            self.advance();
                            return Token::new(
                                TokenKind::Error(format!("invalid escape sequence: \\{}", c)),
                                Span::new(self.file_id, start, end),
                            );
                        }
                        None => {
                            let end = self.current_position();
                            return Token::new(
                                TokenKind::Error("unterminated interpolated string literal".into()),
                                Span::new(self.file_id, start, end),
                            );
                        }
                    }
                }
                Some(c) => {
                    current_literal.push(c);
                    self.advance();
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Identifiers and keywords
    // ------------------------------------------------------------------

    /// Scan an identifier or keyword.
    fn scan_ident_or_keyword(&mut self) -> Token {
        let start = self.current_position();
        let ident_start = self.pos;

        // First character already verified to be alphabetic or underscore.
        self.advance();

        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                self.advance();
            } else {
                break;
            }
        }

        let end = self.current_position();
        let text: String = self.chars[ident_start..self.pos].iter().collect();

        let kind = keyword_from_str(&text).unwrap_or(TokenKind::Ident(text));
        Token::new(kind, Span::new(self.file_id, start, end))
    }

    // ------------------------------------------------------------------
    // EOF handling
    // ------------------------------------------------------------------

    /// Emit closing DEDENT tokens for every remaining indentation level,
    /// then the final Eof token.
    fn emit_eof(&mut self) -> Token {
        // Pop all remaining indentation levels, emitting DEDENT for each.
        while self.indent_stack.len() > 1 {
            self.indent_stack.pop();
            let pos = self.current_position();
            self.pending_tokens.push_back(Token::new(
                TokenKind::Dedent,
                Span::point(self.file_id, pos),
            ));
        }

        // Push the Eof token.
        let pos = self.current_position();
        self.pending_tokens
            .push_back(Token::new(TokenKind::Eof, Span::point(self.file_id, pos)));

        // Return the first pending token (either DEDENT or Eof).
        self.pending_tokens.pop_front().unwrap()
    }

    // ------------------------------------------------------------------
    // Character-level helpers
    // ------------------------------------------------------------------

    /// Peek at the current character without consuming it.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Peek at the character `offset` positions ahead of the current
    /// position.
    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    /// Advance past the current character, updating line/col/byte_offset
    /// tracking. Returns the consumed character.
    fn advance(&mut self) -> Option<char> {
        if let Some(ch) = self.chars.get(self.pos).copied() {
            self.pos += 1;
            self.byte_offset += ch.len_utf8() as u32;
            if ch == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            Some(ch)
        } else {
            None
        }
    }

    /// Returns `true` when all characters have been consumed.
    fn is_at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    /// Build a [`Position`] snapshot for the current cursor location.
    fn current_position(&self) -> Position {
        Position::new(self.line, self.col, self.byte_offset)
    }

    /// Convenience: produce a single-character token.
    fn single_char_token(&mut self, kind: TokenKind) -> Token {
        let start = self.current_position();
        self.advance();
        let end = self.current_position();
        Token::new(kind, Span::new(self.file_id, start, end))
    }
}
