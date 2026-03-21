//! Recursive descent parser for the Gradient language.
//!
//! The parser consumes a flat stream of [`Token`]s (produced by the lexer) and
//! builds a typed AST rooted at [`Module`]. It implements error recovery so
//! that a single malformed construct does not prevent the rest of the file
//! from being parsed.
//!
//! # Entry point
//!
//! ```ignore
//! let (module, errors) = Parser::parse(tokens, file_id);
//! ```

use crate::ast::block::Block;
use crate::ast::expr::{BinOp, Expr, ExprKind, MatchArm, Pattern, UnaryOp};
use crate::ast::item::{Annotation, ExternFnDecl, FnDef, Item, ItemKind, Param};
use crate::ast::module::{Module, ModuleDecl, UseDecl};
use crate::ast::span::{Position, Span, Spanned};
use crate::ast::stmt::{Stmt, StmtKind};
use crate::ast::types::{EffectSet, TypeExpr};
use crate::lexer::token::{Token, TokenKind};

use super::error::ParseError;

// ---------------------------------------------------------------------------
// Span helpers
// ---------------------------------------------------------------------------

/// Merge two spans to create a span covering both.
fn merge_spans(a: &Span, b: &Span) -> Span {
    a.merge(b)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// The recursive descent parser for Gradient source files.
///
/// Construct one via [`Parser::parse`], which is the only public entry point.
pub struct Parser {
    /// The flat token stream produced by the lexer.
    tokens: Vec<Token>,
    /// Current position in the token stream.
    pos: usize,
    /// Accumulated parse errors (supports error recovery).
    errors: Vec<ParseError>,
    /// The file id to stamp onto all AST spans.
    file_id: u32,
}

impl Parser {
    // -----------------------------------------------------------------------
    // Public entry point
    // -----------------------------------------------------------------------

    /// Parse a token stream into a [`Module`] AST and a list of errors.
    ///
    /// Even when errors are present the returned module will contain as much
    /// of the program as could be recovered. Callers should check
    /// `errors.is_empty()` before proceeding to later compiler phases.
    pub fn parse(tokens: Vec<Token>, file_id: u32) -> (Module, Vec<ParseError>) {
        let mut parser = Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            file_id,
        };
        let module = parser.parse_program();
        (module, parser.errors)
    }

    // -----------------------------------------------------------------------
    // Token stream helpers
    // -----------------------------------------------------------------------

    /// Peek at the current token kind without consuming it.
    fn peek(&self) -> &TokenKind {
        self.tokens
            .get(self.pos)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    /// Peek at the token kind `n` positions ahead without consuming.
    fn peek_ahead(&self, n: usize) -> &TokenKind {
        self.tokens
            .get(self.pos + n)
            .map(|t| &t.kind)
            .unwrap_or(&TokenKind::Eof)
    }

    /// Consume and return the current token, advancing the cursor.
    fn advance(&mut self) -> Token {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            tok
        } else {
            // Synthesize an EOF token at the end.
            Token::new(
                TokenKind::Eof,
                Span::point(
                    self.file_id,
                    Position::new(0, 0, 0),
                ),
            )
        }
    }

    /// Check whether the current token matches the given kind.
    ///
    /// For token variants that carry data (`IntLit`, `Ident`, etc.) this only
    /// checks the discriminant, not the payload.
    #[allow(dead_code)]
    fn check(&self, kind: &TokenKind) -> bool {
        discriminant_eq(self.peek(), kind)
    }

    /// Return `true` when the parser has reached the end of input.
    fn at_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    /// Get the AST span of the current token (without consuming it).
    fn current_span(&self) -> Span {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].span
        } else {
            Span::point(
                self.file_id,
                Position::new(0, 0, 0),
            )
        }
    }

    /// Get the AST span of the previously consumed token.
    fn prev_span(&self) -> Span {
        if self.pos > 0 {
            self.tokens[self.pos - 1].span
        } else {
            Span::point(self.file_id, Position::new(0, 0, 0))
        }
    }

    /// Consume the current token if it matches the expected kind.
    /// Returns the consumed token on success, or a [`ParseError`] on failure.
    fn expect(&mut self, kind: TokenKind) -> Result<Token, ParseError> {
        if discriminant_eq(self.peek(), &kind) {
            Ok(self.advance())
        } else {
            let err = self.error_expected(&[&format!("{}", kind)]);
            Err(err)
        }
    }

    /// Skip over any `Newline` tokens at the current position.
    fn skip_newlines(&mut self) {
        while matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
    }

    // -----------------------------------------------------------------------
    // Error helpers
    // -----------------------------------------------------------------------

    /// Record an error at the current position with a message.
    fn error(&mut self, message: &str) -> ParseError {
        let span = self.current_span();
        let found = format!("{}", self.peek());
        let err = ParseError::new(message, span, vec![], found);
        self.errors.push(err.clone());
        err
    }

    /// Record an error at the current position listing what was expected.
    fn error_expected(&mut self, expected: &[&str]) -> ParseError {
        let span = self.current_span();
        let found = format!("{}", self.peek());
        let expected_strs: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        let message = if expected_strs.len() == 1 {
            format!("expected {}", expected_strs[0])
        } else {
            format!("expected one of: {}", expected_strs.join(", "))
        };
        let err = ParseError::new(message, span, expected_strs, found);
        self.errors.push(err.clone());
        err
    }

    /// Error recovery: skip tokens until we reach something that could
    /// plausibly start a new statement or top-level item.
    fn synchronize(&mut self) {
        loop {
            match self.peek() {
                TokenKind::Fn
                | TokenKind::Let
                | TokenKind::If
                | TokenKind::For
                | TokenKind::While
                | TokenKind::Match
                | TokenKind::Ret
                | TokenKind::Type
                | TokenKind::Mod
                | TokenKind::Use
                | TokenKind::Dedent
                | TokenKind::Eof => break,
                TokenKind::Newline => {
                    self.advance();
                    break;
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Program / module-level rules
    // -----------------------------------------------------------------------

    /// ```text
    /// program <- module_decl? NEWLINE* (use_decl NEWLINE+)* (top_item NEWLINE*)* EOF
    /// ```
    fn parse_program(&mut self) -> Module {
        let start_span = self.current_span();

        // Optional module declaration.
        let module_decl = if matches!(self.peek(), TokenKind::Mod) {
            let md = self.parse_module_decl();
            Some(md)
        } else {
            None
        };

        self.skip_newlines();

        // Use declarations.
        let mut uses = Vec::new();
        while matches!(self.peek(), TokenKind::Use) {
            uses.push(self.parse_use_decl());
            self.skip_newlines();
        }

        // Top-level items.
        let mut items = Vec::new();
        while !self.at_end() {
            self.skip_newlines();
            if self.at_end() {
                break;
            }
            match self.parse_top_item() {
                Some(item) => items.push(item),
                None => {
                    // Could not parse a top-level item. Skip a token and try again.
                    if !self.at_end() {
                        let before = self.pos;
                        self.error("unexpected token at top level");
                        self.synchronize();
                        // synchronize() stops at tokens like Dedent, Ret, If,
                        // For, etc. without consuming them. Those tokens are
                        // valid statement starters but cannot begin a top-level
                        // item. If synchronize made no net progress (stopped on
                        // the same token it started on), force-consume one token
                        // so the loop always moves forward.
                        if self.pos == before && !self.at_end() {
                            self.advance();
                        }
                        // Also consume any stray Dedent tokens that should not
                        // appear at the top level.
                        while matches!(self.peek(), TokenKind::Dedent) {
                            self.advance();
                        }
                    }
                }
            }
            self.skip_newlines();
        }

        let end_span = self.prev_span();
        let span = if items.is_empty() && uses.is_empty() && module_decl.is_none() {
            start_span
        } else {
            merge_spans(&start_span, &end_span)
        };

        Module {
            module_decl,
            uses,
            items,
            span,
        }
    }

    /// ```text
    /// module_decl <- 'mod' module_path NEWLINE
    /// ```
    fn parse_module_decl(&mut self) -> ModuleDecl {
        let start = self.current_span();
        self.advance(); // consume 'mod'

        let path = self.parse_module_path();

        // Consume trailing newline (optional — might be EOF).
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let end = self.prev_span();
        ModuleDecl {
            path,
            span: merge_spans(&start, &end),
        }
    }

    /// ```text
    /// module_path <- IDENT ('.' IDENT)*
    /// ```
    ///
    /// Only consumes `'.' IDENT` pairs — stops before a `'.'` that is not
    /// followed by an identifier (e.g. `'.{'` in selective imports).
    fn parse_module_path(&mut self) -> Vec<String> {
        let mut path = Vec::new();

        match self.peek().clone() {
            TokenKind::Ident(name) => {
                path.push(name);
                self.advance();
            }
            _ => {
                self.error_expected(&["identifier"]);
                return path;
            }
        }

        while matches!(self.peek(), TokenKind::Dot) {
            // Only consume '.' if the token after it is an identifier,
            // so we don't eat the '.' that precedes '{' in use lists.
            if !matches!(self.peek_ahead(1), TokenKind::Ident(_)) {
                break;
            }
            self.advance(); // consume '.'
            match self.peek().clone() {
                TokenKind::Ident(name) => {
                    path.push(name);
                    self.advance();
                }
                _ => unreachable!("peek_ahead check guarantees Ident"),
            }
        }

        path
    }

    /// ```text
    /// use_decl <- 'use' module_path ('.' '{' use_list '}')? NEWLINE
    /// ```
    fn parse_use_decl(&mut self) -> UseDecl {
        let start = self.current_span();
        self.advance(); // consume 'use'

        let mut path = self.parse_module_path();

        let specific_imports = if matches!(self.peek(), TokenKind::Dot) {
            // Check if the next token after '.' is '{' — that means selective import.
            if matches!(self.peek_ahead(1), TokenKind::LBrace) {
                self.advance(); // consume '.'
                self.advance(); // consume '{'
                let imports = self.parse_use_list();
                if self.expect(TokenKind::RBrace).is_err() {
                    // error already recorded
                }
                Some(imports)
            } else {
                // It's just a longer module path segment like `use std.io.more`.
                // Continue parsing as module_path components.
                while matches!(self.peek(), TokenKind::Dot) {
                    if matches!(self.peek_ahead(1), TokenKind::LBrace) {
                        self.advance(); // consume '.'
                        self.advance(); // consume '{'
                        let imports = self.parse_use_list();
                        if self.expect(TokenKind::RBrace).is_err() {
                            // error already recorded
                        }
                        return UseDecl {
                            path,
                            specific_imports: Some(imports),
                            span: merge_spans(&start, &self.prev_span()),
                        };
                    }
                    self.advance(); // consume '.'
                    match self.peek().clone() {
                        TokenKind::Ident(name) => {
                            path.push(name);
                            self.advance();
                        }
                        _ => {
                            self.error_expected(&["identifier after '.'"]);
                            break;
                        }
                    }
                }
                None
            }
        } else {
            None
        };

        // Consume trailing newline.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let end = self.prev_span();
        UseDecl {
            path,
            specific_imports,
            span: merge_spans(&start, &end),
        }
    }

    /// ```text
    /// use_list <- IDENT (',' IDENT)* ','?
    /// ```
    fn parse_use_list(&mut self) -> Vec<String> {
        let mut names = Vec::new();

        match self.peek().clone() {
            TokenKind::Ident(name) => {
                names.push(name);
                self.advance();
            }
            _ => {
                self.error_expected(&["identifier in use list"]);
                return names;
            }
        }

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
            // Allow trailing comma.
            if matches!(self.peek(), TokenKind::RBrace) {
                break;
            }
            match self.peek().clone() {
                TokenKind::Ident(name) => {
                    names.push(name);
                    self.advance();
                }
                _ => {
                    self.error_expected(&["identifier in use list"]);
                    break;
                }
            }
        }

        names
    }

    // -----------------------------------------------------------------------
    // Top-level items
    // -----------------------------------------------------------------------

    /// ```text
    /// top_item <- annotation* (fn_def / let_stmt / type_decl / extern_fn_decl)
    /// ```
    fn parse_top_item(&mut self) -> Option<Item> {
        let start = self.current_span();

        // Collect annotations.
        let mut annotations = Vec::new();
        while matches!(self.peek(), TokenKind::At) {
            annotations.push(self.parse_annotation());
        }

        // Check for @cap(...) as a standalone module-level capability declaration.
        // @cap is always a module-level item, never attached to a function.
        if !annotations.is_empty()
            && annotations.len() == 1
            && annotations[0].name == "cap"
        {
            let ann = &annotations[0];
            let allowed_effects: Vec<String> = ann
                .args
                .iter()
                .filter_map(|arg| {
                    if let crate::ast::expr::ExprKind::Ident(name) = &arg.node {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let end = ann.span;
            return Some(Item::new(
                ItemKind::CapDecl { allowed_effects },
                merge_spans(&start, &end),
            ));
        }

        match self.peek() {
            TokenKind::Fn => {
                let item = self.parse_fn_item(annotations);
                Some(item)
            }
            TokenKind::Let => {
                let item = self.parse_let_item(start);
                Some(item)
            }
            TokenKind::Type => {
                let item = self.parse_type_decl();
                Some(item)
            }
            _ => {
                if !annotations.is_empty() {
                    self.error("annotations must be followed by a function, let, or type declaration");
                }
                None
            }
        }
    }

    /// ```text
    /// annotation <- '@' IDENT ('(' annotation_args ')')? NEWLINE
    /// ```
    fn parse_annotation(&mut self) -> Annotation {
        let start = self.current_span();
        self.advance(); // consume '@'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["annotation name"]);
                String::from("<error>")
            }
        };

        let args = if matches!(self.peek(), TokenKind::LParen) {
            self.advance(); // consume '('
            let mut args = Vec::new();
            if !matches!(self.peek(), TokenKind::RParen) {
                args.push(self.parse_expr());
                while matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                    if matches!(self.peek(), TokenKind::RParen) {
                        break;
                    }
                    args.push(self.parse_expr());
                }
            }
            if self.expect(TokenKind::RParen).is_err() {
                // error already recorded
            }
            args
        } else {
            Vec::new()
        };

        // Consume trailing newline.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let end = self.prev_span();
        Annotation {
            name,
            args,
            span: merge_spans(&start, &end),
        }
    }

    /// Parse a function item — decides between `fn_def` and `extern_fn_decl`
    /// based on whether a body follows.
    fn parse_fn_item(&mut self, annotations: Vec<Annotation>) -> Item {
        let start = self.current_span();
        self.advance(); // consume 'fn'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["function name"]);
                String::from("<error>")
            }
        };

        // Parameter list.
        if self.expect(TokenKind::LParen).is_err() {
            // error already recorded; try to recover
        }
        let params = if !matches!(self.peek(), TokenKind::RParen) {
            self.parse_param_list()
        } else {
            Vec::new()
        };
        if self.expect(TokenKind::RParen).is_err() {
            // error already recorded
        }

        // Optional return clause.
        let (effects, return_type) = self.parse_return_clause();

        // Decide: fn_def (has `:` NEWLINE INDENT block) vs extern_fn_decl.
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance(); // consume ':'
            // Expect NEWLINE then INDENT for block.
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
            let body = self.parse_block();
            let end = body.span;
            let fn_def = FnDef {
                name,
                params,
                return_type,
                effects,
                body,
                annotations,
            };
            Spanned::new(ItemKind::FnDef(fn_def), merge_spans(&start, &end))
        } else {
            // Extern fn declaration (no body).
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
            let end = self.prev_span();
            let decl = ExternFnDecl {
                name,
                params,
                return_type,
                effects,
                annotations,
            };
            Spanned::new(ItemKind::ExternFn(decl), merge_spans(&start, &end))
        }
    }

    /// Parse a top-level `let` binding as an item.
    fn parse_let_item(&mut self, start: Span) -> Item {
        let stmt = self.parse_let_stmt_inner();
        let end = self.prev_span();
        match stmt.node {
            StmtKind::Let {
                name,
                type_ann,
                value,
                mutable,
            } => Spanned::new(
                ItemKind::Let {
                    name,
                    type_ann,
                    value,
                    mutable,
                },
                merge_spans(&start, &end),
            ),
            _ => unreachable!("parse_let_stmt_inner always returns StmtKind::Let"),
        }
    }

    /// ```text
    /// param_list <- param (',' param)* ','?
    /// param <- IDENT ':' type_expr
    /// ```
    fn parse_param_list(&mut self) -> Vec<Param> {
        let mut params = Vec::new();

        params.push(self.parse_param());

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
            // Allow trailing comma.
            if matches!(self.peek(), TokenKind::RParen) {
                break;
            }
            params.push(self.parse_param());
        }

        params
    }

    /// Parse a single parameter: `IDENT ':' type_expr`.
    fn parse_param(&mut self) -> Param {
        let start = self.current_span();

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["parameter name"]);
                String::from("<error>")
            }
        };

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }

        let type_ann = self.parse_type_expr();
        let end = type_ann.span;

        Param {
            name,
            type_ann,
            span: merge_spans(&start, &end),
        }
    }

    /// ```text
    /// return_clause <- '->' effect_set? type_expr
    /// ```
    /// Returns `(Option<EffectSet>, Option<Spanned<TypeExpr>>)`.
    fn parse_return_clause(&mut self) -> (Option<EffectSet>, Option<Spanned<TypeExpr>>) {
        if !matches!(self.peek(), TokenKind::Arrow) {
            return (None, None);
        }
        self.advance(); // consume '->'

        // Optional effect set.
        let effects = if matches!(self.peek(), TokenKind::Bang) {
            Some(self.parse_effect_set())
        } else {
            None
        };

        let type_expr = self.parse_type_expr();
        (effects, Some(type_expr))
    }

    // -----------------------------------------------------------------------
    // Block and statements
    // -----------------------------------------------------------------------

    /// ```text
    /// block <- INDENT stmt+ DEDENT
    /// ```
    fn parse_block(&mut self) -> Block {
        let start = self.current_span();

        if self.expect(TokenKind::Indent).is_err() {
            // error already recorded — return an empty block.
            return Spanned::new(Vec::new(), start);
        }

        let mut stmts = Vec::new();
        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            stmts.push(self.parse_stmt());
            // Consume trailing newline after statement.
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        Spanned::new(stmts, merge_spans(&start, &end))
    }

    /// ```text
    /// stmt <- (let_stmt / ret_stmt / assign_stmt / if_stmt / for_stmt / while_stmt / expr_stmt) NEWLINE
    /// ```
    fn parse_stmt(&mut self) -> Stmt {
        match self.peek() {
            TokenKind::Let => self.parse_let_stmt_inner(),
            TokenKind::Ret => self.parse_ret_stmt(),
            _ => {
                // Check for assignment: Ident followed by '=' (but not '==').
                if let TokenKind::Ident(_) = self.peek() {
                    if matches!(self.peek_ahead(1), TokenKind::Assign) {
                        return self.parse_assign_stmt();
                    }
                }
                // if_stmt, for_stmt, while_stmt, and bare expressions are all
                // handled via parse_expr since `if`, `for`, and `while` are
                // expressions in Gradient.
                let expr = self.parse_expr();
                let span = expr.span;
                Spanned::new(StmtKind::Expr(expr), span)
            }
        }
    }

    /// ```text
    /// assign_stmt <- IDENT '=' expr
    /// ```
    fn parse_assign_stmt(&mut self) -> Stmt {
        let start = self.current_span();
        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => unreachable!("parse_assign_stmt called with non-ident"),
        };
        self.advance(); // consume '='
        let value = self.parse_expr();
        let end = value.span;
        Spanned::new(
            StmtKind::Assign { name, value },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// let_stmt <- 'let' 'mut'? IDENT (':' type_expr)? '=' expr
    /// ```
    fn parse_let_stmt_inner(&mut self) -> Stmt {
        let start = self.current_span();
        self.advance(); // consume 'let'

        // Check for 'mut' keyword.
        let mutable = if matches!(self.peek(), TokenKind::Mut) {
            self.advance(); // consume 'mut'
            true
        } else {
            false
        };

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["variable name"]);
                String::from("<error>")
            }
        };

        // Optional type annotation.
        let type_ann = if matches!(self.peek(), TokenKind::Colon) {
            self.advance(); // consume ':'
            Some(self.parse_type_expr())
        } else {
            None
        };

        if self.expect(TokenKind::Assign).is_err() {
            // error already recorded
        }

        let value = self.parse_expr();
        let end = value.span;

        Spanned::new(
            StmtKind::Let {
                name,
                type_ann,
                value,
                mutable,
            },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// ret_stmt <- 'ret' expr
    /// ```
    fn parse_ret_stmt(&mut self) -> Stmt {
        let start = self.current_span();
        self.advance(); // consume 'ret'
        let value = self.parse_expr();
        let end = value.span;
        Spanned::new(StmtKind::Ret(value), merge_spans(&start, &end))
    }

    // -----------------------------------------------------------------------
    // Type declarations
    // -----------------------------------------------------------------------

    /// ```text
    /// type_decl <- 'type' IDENT '=' type_expr NEWLINE
    /// ```
    fn parse_type_decl(&mut self) -> Item {
        let start = self.current_span();
        self.advance(); // consume 'type'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["type name"]);
                String::from("<error>")
            }
        };

        if self.expect(TokenKind::Assign).is_err() {
            // error already recorded
        }

        let type_expr = self.parse_type_expr();
        let end = type_expr.span;

        // Consume trailing newline.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        Spanned::new(
            ItemKind::TypeDecl { name, type_expr },
            merge_spans(&start, &end),
        )
    }

    // -----------------------------------------------------------------------
    // Expression parsing — precedence climbing
    // -----------------------------------------------------------------------

    /// ```text
    /// expr <- or_expr
    /// ```
    fn parse_expr(&mut self) -> Expr {
        // if, for, while, and match are expressions, handle them here.
        match self.peek() {
            TokenKind::If => self.parse_if_expr(),
            TokenKind::For => self.parse_for_expr(),
            TokenKind::While => self.parse_while_expr(),
            TokenKind::Match => self.parse_match_expr(),
            _ => self.parse_or_expr(),
        }
    }

    /// ```text
    /// or_expr <- and_expr ('or' and_expr)*
    /// ```
    fn parse_or_expr(&mut self) -> Expr {
        let mut left = self.parse_and_expr();

        while matches!(self.peek(), TokenKind::Or) {
            self.advance(); // consume 'or'
            let right = self.parse_and_expr();
            let span = merge_spans(&left.span, &right.span);
            left = Spanned::new(
                ExprKind::BinaryOp {
                    op: BinOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        left
    }

    /// ```text
    /// and_expr <- not_expr ('and' not_expr)*
    /// ```
    fn parse_and_expr(&mut self) -> Expr {
        let mut left = self.parse_not_expr();

        while matches!(self.peek(), TokenKind::And) {
            self.advance(); // consume 'and'
            let right = self.parse_not_expr();
            let span = merge_spans(&left.span, &right.span);
            left = Spanned::new(
                ExprKind::BinaryOp {
                    op: BinOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        left
    }

    /// ```text
    /// not_expr <- 'not' not_expr / cmp_expr
    /// ```
    fn parse_not_expr(&mut self) -> Expr {
        if matches!(self.peek(), TokenKind::Not) {
            let start = self.current_span();
            self.advance(); // consume 'not'
            let operand = self.parse_not_expr();
            let span = merge_spans(&start, &operand.span);
            Spanned::new(
                ExprKind::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                },
                span,
            )
        } else {
            self.parse_cmp_expr()
        }
    }

    /// ```text
    /// cmp_expr <- add_expr (cmp_op add_expr)?
    /// ```
    /// Comparison operators are **non-associative**: `a < b < c` is a parse error.
    fn parse_cmp_expr(&mut self) -> Expr {
        let left = self.parse_add_expr();

        if let Some(op) = self.peek_cmp_op() {
            self.advance(); // consume the comparison operator
            let right = self.parse_add_expr();

            // Check for non-associativity: if another cmp_op follows, error.
            if self.peek_cmp_op().is_some() {
                self.error("comparison operators are non-associative; use parentheses");
            }

            let span = merge_spans(&left.span, &right.span);
            Spanned::new(
                ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            )
        } else {
            left
        }
    }

    /// Check if the current token is a comparison operator and return the
    /// corresponding `BinOp`.
    fn peek_cmp_op(&self) -> Option<BinOp> {
        match self.peek() {
            TokenKind::Eq => Some(BinOp::Eq),
            TokenKind::Ne => Some(BinOp::Ne),
            TokenKind::Lt => Some(BinOp::Lt),
            TokenKind::Le => Some(BinOp::Le),
            TokenKind::Gt => Some(BinOp::Gt),
            TokenKind::Ge => Some(BinOp::Ge),
            _ => None,
        }
    }

    /// ```text
    /// add_expr <- mul_expr (add_op mul_expr)*
    /// add_op <- '+' / '-'
    /// ```
    fn parse_add_expr(&mut self) -> Expr {
        let mut left = self.parse_mul_expr();

        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul_expr();
            let span = merge_spans(&left.span, &right.span);
            left = Spanned::new(
                ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        left
    }

    /// ```text
    /// mul_expr <- unary_expr (mul_op unary_expr)*
    /// mul_op <- '*' / '/' / '%'
    /// ```
    fn parse_mul_expr(&mut self) -> Expr {
        let mut left = self.parse_unary_expr();

        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary_expr();
            let span = merge_spans(&left.span, &right.span);
            left = Spanned::new(
                ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        left
    }

    /// ```text
    /// unary_expr <- '-' unary_expr / postfix_expr
    /// ```
    fn parse_unary_expr(&mut self) -> Expr {
        if matches!(self.peek(), TokenKind::Minus) {
            let start = self.current_span();
            self.advance(); // consume '-'
            let operand = self.parse_unary_expr();
            let span = merge_spans(&start, &operand.span);
            Spanned::new(
                ExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                },
                span,
            )
        } else {
            self.parse_postfix_expr()
        }
    }

    /// ```text
    /// postfix_expr <- atom (call_args / '.' IDENT)*
    /// ```
    fn parse_postfix_expr(&mut self) -> Expr {
        let mut expr = self.parse_atom();

        loop {
            match self.peek() {
                TokenKind::LParen => {
                    // Function call.
                    self.advance(); // consume '('
                    let args = if !matches!(self.peek(), TokenKind::RParen) {
                        self.parse_arg_list()
                    } else {
                        Vec::new()
                    };
                    let rparen = self.expect(TokenKind::RParen);
                    let end = match rparen {
                        Ok(tok) => tok.span,
                        Err(_) => self.prev_span(),
                    };
                    let span = merge_spans(&expr.span, &end);
                    expr = Spanned::new(
                        ExprKind::Call {
                            func: Box::new(expr),
                            args,
                        },
                        span,
                    );
                }
                TokenKind::Dot => {
                    self.advance(); // consume '.'
                    let field = match self.peek().clone() {
                        TokenKind::Ident(name) => {
                            self.advance();
                            name
                        }
                        _ => {
                            self.error_expected(&["field name after '.'"]);
                            break;
                        }
                    };
                    let end = self.prev_span();
                    let span = merge_spans(&expr.span, &end);
                    expr = Spanned::new(
                        ExprKind::FieldAccess {
                            object: Box::new(expr),
                            field,
                        },
                        span,
                    );
                }
                _ => break,
            }
        }

        expr
    }

    /// ```text
    /// arg_list <- expr (',' expr)* ','?
    /// ```
    fn parse_arg_list(&mut self) -> Vec<Expr> {
        let mut args = Vec::new();

        args.push(self.parse_expr());

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
            // Allow trailing comma.
            if matches!(self.peek(), TokenKind::RParen) {
                break;
            }
            args.push(self.parse_expr());
        }

        args
    }

    /// ```text
    /// atom <- FLOAT_LIT / INT_LIT / STRING_LIT / BOOL_LIT / UNIT_LIT
    ///       / typed_hole / IDENT / '(' expr ')'
    /// ```
    fn parse_atom(&mut self) -> Expr {
        let start = self.current_span();

        match self.peek().clone() {
            TokenKind::FloatLit(val) => {
                self.advance();
                Spanned::new(ExprKind::FloatLit(val), start)
            }
            TokenKind::IntLit(val) => {
                self.advance();
                Spanned::new(ExprKind::IntLit(val), start)
            }
            TokenKind::StringLit(val) => {
                self.advance();
                Spanned::new(ExprKind::StringLit(val), start)
            }
            TokenKind::True => {
                self.advance();
                Spanned::new(ExprKind::BoolLit(true), start)
            }
            TokenKind::False => {
                self.advance();
                Spanned::new(ExprKind::BoolLit(false), start)
            }
            TokenKind::Ident(name) => {
                self.advance();
                Spanned::new(ExprKind::Ident(name), start)
            }
            TokenKind::Question => {
                self.advance(); // consume '?'
                // Optional label after '?'.
                let label = match self.peek().clone() {
                    TokenKind::Ident(name) => {
                        self.advance();
                        Some(name)
                    }
                    _ => None,
                };
                let end = self.prev_span();
                Spanned::new(
                    ExprKind::TypedHole(label),
                    merge_spans(&start, &end),
                )
            }
            TokenKind::LParen => {
                self.advance(); // consume '('
                // Unit literal: ()
                if matches!(self.peek(), TokenKind::RParen) {
                    self.advance();
                    let end = self.prev_span();
                    return Spanned::new(ExprKind::UnitLit, merge_spans(&start, &end));
                }
                let inner = self.parse_expr();
                let rparen = self.expect(TokenKind::RParen);
                let end = match rparen {
                    Ok(tok) => tok.span,
                    Err(_) => self.prev_span(),
                };
                Spanned::new(
                    ExprKind::Paren(Box::new(inner)),
                    merge_spans(&start, &end),
                )
            }
            _ => {
                self.error_expected(&[
                    "expression",
                ]);
                // Consume the unrecognized token so that the parser makes
                // progress and does not loop forever on the same token.
                if !self.at_end() {
                    self.advance();
                }
                // Return a synthetic error expression so the parser can
                // continue.
                Spanned::new(ExprKind::TypedHole(None), start)
            }
        }
    }

    // -----------------------------------------------------------------------
    // If / For expressions
    // -----------------------------------------------------------------------

    /// ```text
    /// if_stmt <- 'if' expr ':' NEWLINE block
    ///            ('else' 'if' expr ':' NEWLINE block)*
    ///            ('else' ':' NEWLINE block)?
    /// ```
    fn parse_if_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'if'

        let condition = self.parse_expr();

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let then_block = self.parse_block();

        // else-if and else branches.
        let mut else_ifs = Vec::new();
        let mut else_block = None;

        while matches!(self.peek(), TokenKind::Else) {
            self.advance(); // consume 'else'

            if matches!(self.peek(), TokenKind::If) {
                // else if
                self.advance(); // consume 'if'
                let ei_condition = self.parse_expr();
                if self.expect(TokenKind::Colon).is_err() {
                    // error already recorded
                }
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                let ei_block = self.parse_block();
                else_ifs.push((ei_condition, ei_block));
            } else {
                // else
                if self.expect(TokenKind::Colon).is_err() {
                    // error already recorded
                }
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                else_block = Some(self.parse_block());
                break; // 'else' is terminal
            }
        }

        let end = if let Some(ref eb) = else_block {
            eb.span
        } else if let Some(last_ei) = else_ifs.last() {
            last_ei.1.span
        } else {
            then_block.span
        };

        Spanned::new(
            ExprKind::If {
                condition: Box::new(condition),
                then_block,
                else_ifs,
                else_block,
            },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// for_stmt <- 'for' IDENT 'in' expr ':' NEWLINE block
    /// ```
    fn parse_for_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'for'

        let var = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["loop variable name"]);
                String::from("<error>")
            }
        };

        if self.expect(TokenKind::In).is_err() {
            // error already recorded
        }

        let iter = self.parse_expr();

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let body = self.parse_block();
        let end = body.span;

        Spanned::new(
            ExprKind::For {
                var,
                iter: Box::new(iter),
                body,
            },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// while_stmt <- 'while' expr ':' NEWLINE block
    /// ```
    fn parse_while_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'while'

        let condition = self.parse_expr();

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let body = self.parse_block();
        let end = body.span;

        Spanned::new(
            ExprKind::While {
                condition: Box::new(condition),
                body,
            },
            merge_spans(&start, &end),
        )
    }

    // -----------------------------------------------------------------------
    // Match expressions
    // -----------------------------------------------------------------------

    /// ```text
    /// match_expr <- 'match' expr ':' NEWLINE INDENT match_arm+ DEDENT
    /// match_arm  <- pattern ':' NEWLINE block
    /// pattern    <- INT_LIT / 'true' / 'false' / '_'
    /// ```
    fn parse_match_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'match'

        let scrutinee = self.parse_expr();

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Expect INDENT for the match body.
        if self.expect(TokenKind::Indent).is_err() {
            // error already recorded — return a match with no arms.
            return Spanned::new(
                ExprKind::Match {
                    scrutinee: Box::new(scrutinee),
                    arms: Vec::new(),
                },
                start,
            );
        }

        let mut arms = Vec::new();
        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            let arm_start = self.current_span();

            // Parse pattern.
            let pattern = self.parse_pattern();

            if self.expect(TokenKind::Colon).is_err() {
                // error already recorded
            }
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }

            let body = self.parse_block();
            let arm_end = body.span;

            arms.push(MatchArm {
                pattern,
                body,
                span: merge_spans(&arm_start, &arm_end),
            });

            // Skip trailing newlines between arms.
            self.skip_newlines();
        }

        // Expect DEDENT closing the match block.
        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        Spanned::new(
            ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a match arm pattern: integer literal, boolean literal, or wildcard `_`.
    fn parse_pattern(&mut self) -> Pattern {
        match self.peek().clone() {
            TokenKind::IntLit(n) => {
                self.advance();
                Pattern::IntLit(n)
            }
            TokenKind::Minus => {
                // Negative integer literal: '-' followed by IntLit.
                self.advance(); // consume '-'
                match self.peek().clone() {
                    TokenKind::IntLit(n) => {
                        self.advance();
                        Pattern::IntLit(-n)
                    }
                    _ => {
                        self.error_expected(&["integer literal after '-'"]);
                        Pattern::Wildcard
                    }
                }
            }
            TokenKind::True => {
                self.advance();
                Pattern::BoolLit(true)
            }
            TokenKind::False => {
                self.advance();
                Pattern::BoolLit(false)
            }
            TokenKind::Ident(ref name) if name == "_" => {
                self.advance();
                Pattern::Wildcard
            }
            _ => {
                self.error_expected(&["pattern (integer, true, false, or _)"]);
                // Consume the unrecognized token to make progress.
                if !self.at_end() {
                    self.advance();
                }
                Pattern::Wildcard
            }
        }
    }

    // -----------------------------------------------------------------------
    // Type expressions
    // -----------------------------------------------------------------------

    /// ```text
    /// type_expr <- IDENT / '(' ')'
    /// ```
    fn parse_type_expr(&mut self) -> Spanned<TypeExpr> {
        let start = self.current_span();

        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Spanned::new(TypeExpr::Named(name), start)
            }
            TokenKind::LParen => {
                self.advance(); // consume '('
                if self.expect(TokenKind::RParen).is_err() {
                    // error already recorded
                }
                let end = self.prev_span();
                Spanned::new(TypeExpr::Unit, merge_spans(&start, &end))
            }
            _ => {
                self.error_expected(&["type"]);
                // Return a synthetic unit type so parsing can continue.
                Spanned::new(TypeExpr::Unit, start)
            }
        }
    }

    /// ```text
    /// effect_set <- '!' '{' IDENT (',' IDENT)* ','? '}'
    /// ```
    fn parse_effect_set(&mut self) -> EffectSet {
        let start = self.current_span();
        self.advance(); // consume '!'

        if self.expect(TokenKind::LBrace).is_err() {
            return EffectSet {
                effects: Vec::new(),
                span: start,
            };
        }

        let mut effects = Vec::new();

        match self.peek().clone() {
            TokenKind::Ident(name) => {
                effects.push(name);
                self.advance();
            }
            _ => {
                self.error_expected(&["effect name"]);
            }
        }

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
            // Allow trailing comma.
            if matches!(self.peek(), TokenKind::RBrace) {
                break;
            }
            match self.peek().clone() {
                TokenKind::Ident(name) => {
                    effects.push(name);
                    self.advance();
                }
                _ => {
                    self.error_expected(&["effect name"]);
                    break;
                }
            }
        }

        if self.expect(TokenKind::RBrace).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        EffectSet {
            effects,
            span: merge_spans(&start, &end),
        }
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Compare two `TokenKind` values by discriminant only, ignoring payloads.
///
/// This is needed because `TokenKind::Ident("x") != TokenKind::Ident("y")`
/// but `check(&TokenKind::Ident(_))` should match any identifier.
fn discriminant_eq(a: &TokenKind, b: &TokenKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}
