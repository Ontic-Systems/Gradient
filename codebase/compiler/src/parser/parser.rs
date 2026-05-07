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
use crate::ast::expr::{
    BinOp, ChildSpec, ClosureParam, Expr, ExprKind, MatchArm, Pattern, RestartPolicy,
    RestartStrategy, StringInterpPart, UnaryOp,
};
use crate::ast::item::{
    Annotation, BudgetConstraint, Contract, ContractKind, EnumVariant, ExternFnDecl, FnDef, Item,
    ItemKind, MessageHandler, Param, StateField, TraitMethod, TypeParam, VariantField,
};
use crate::ast::module::{ImportKind, Module, ModuleDecl, UseDecl};
use crate::ast::span::{Position, Span, Spanned};
use crate::ast::stmt::{Stmt, StmtKind};
use crate::ast::types::{EffectSet, TypeExpr};
use crate::lexer::token::{InterpolationPart, Token, TokenKind};

use super::error::ParseError;

const MAX_EXPR_DEPTH: usize = 64;

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
    /// Current recursive expression depth.
    expr_depth: usize,
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
            expr_depth: 0,
        };
        parser.record_lex_errors();
        let module = parser.parse_program();
        (module, parser.errors)
    }

    /// Promote lexer `Error` tokens into parse diagnostics so callers can
    /// surface them without scraping the token stream.
    fn record_lex_errors(&mut self) {
        for token in &self.tokens {
            if let TokenKind::Error(message) = &token.kind {
                self.errors.push(ParseError::new(
                    message.clone(),
                    token.span,
                    vec![],
                    format!("{}", token.kind),
                ));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Token stream helpers
    // -----------------------------------------------------------------------

    /// Peek at the current token kind without consuming it.
    pub(crate) fn peek(&self) -> &TokenKind {
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
    pub(crate) fn advance(&mut self) -> Token {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            tok
        } else {
            // Synthesize an EOF token at the end.
            Token::new(
                TokenKind::Eof,
                Span::point(self.file_id, Position::new(0, 0, 0)),
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
    pub(crate) fn at_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    /// Get the AST span of the current token (without consuming it).
    fn current_span(&self) -> Span {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].span
        } else {
            Span::point(self.file_id, Position::new(0, 0, 0))
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
    pub(crate) fn synchronize(&mut self) {
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
                | TokenKind::Comptime
                | TokenKind::Actor
                | TokenKind::Trait
                | TokenKind::Impl
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

        // File-scope skipping (#360 + #318): the first thing we accept is an
        // optional set of file-scope attributes:
        //   * `@trusted` / `@untrusted` — sets trust posture (#360, no args).
        //   * `@panic(abort | unwind | none)` — sets panic strategy (#318).
        // Any other attribute here is left for parse_top_item.
        // At most one of each is allowed; duplicates are diagnosed.
        let mut trust = crate::ast::module::TrustMode::Trusted;
        let mut panic_strategy = crate::ast::module::PanicStrategy::default();
        let mut panic_strategy_set: Option<crate::ast::span::Span> = None;
        self.skip_newlines();
        while matches!(self.peek(), TokenKind::At) {
            // Look ahead: only swallow if this is a recognized
            // file-scope attribute. Otherwise let parse_top_item
            // handle it as an item attribute.
            let next = self.peek_ahead(1);
            let is_file_scope_trust = matches!(
                next,
                TokenKind::Ident(n) if n == "trusted" || n == "untrusted"
            );
            let is_file_scope_panic = matches!(
                next,
                TokenKind::Ident(n) if n == "panic"
            );
            if !is_file_scope_trust && !is_file_scope_panic {
                break;
            }
            // Capture which one before parse_annotation consumes it.
            let attr_name = match self.peek_ahead(1) {
                TokenKind::Ident(n) => n.clone(),
                _ => unreachable!(),
            };
            let ann = self.parse_annotation();
            if attr_name == "trusted" || attr_name == "untrusted" {
                if !ann.args.is_empty() {
                    self.errors.push(super::error::ParseError::new(
                        "@trusted / @untrusted take no arguments",
                        ann.span,
                        vec![],
                        String::new(),
                    ));
                }
                if attr_name == "untrusted" {
                    trust = crate::ast::module::TrustMode::Untrusted;
                }
            } else {
                // attr_name == "panic"
                if let Some(prev_span) = panic_strategy_set {
                    self.errors.push(super::error::ParseError::new(
                        "duplicate `@panic(...)` module attribute",
                        ann.span,
                        vec![format!("previous declaration at {:?}", prev_span)],
                        String::new(),
                    ));
                }
                panic_strategy_set = Some(ann.span);
                // Expect exactly one identifier argument: abort | unwind | none.
                if ann.args.len() != 1 {
                    self.errors.push(super::error::ParseError::new(
                        "@panic requires exactly one argument: `abort`, `unwind`, or `none`",
                        ann.span,
                        vec![],
                        String::new(),
                    ));
                } else {
                    use crate::ast::expr::ExprKind;
                    let arg = &ann.args[0];
                    match &arg.node {
                        ExprKind::Ident(name) => match name.as_str() {
                            "abort" => {
                                panic_strategy = crate::ast::module::PanicStrategy::Abort;
                            }
                            "unwind" => {
                                panic_strategy = crate::ast::module::PanicStrategy::Unwind;
                            }
                            "none" => {
                                panic_strategy = crate::ast::module::PanicStrategy::None;
                            }
                            other => {
                                self.errors.push(super::error::ParseError::new(
                                    format!(
                                        "unknown @panic strategy `{}`; expected `abort`, `unwind`, or `none`",
                                        other
                                    ),
                                    arg.span,
                                    vec![],
                                    String::new(),
                                ));
                            }
                        },
                        _ => {
                            self.errors.push(super::error::ParseError::new(
                                "@panic argument must be one of `abort`, `unwind`, or `none`",
                                arg.span,
                                vec![],
                                String::new(),
                            ));
                        }
                    }
                }
            }
            self.skip_newlines();
        }

        // Optional module declaration.
        // Check if 'mod' is followed by a colon - if so, it's a module block, not a declaration.
        let module_decl = if matches!(self.peek(), TokenKind::Mod) {
            // Look ahead: mod Name: indicates a module block, mod Name is a declaration
            let is_module_block = matches!(self.peek_ahead(2), TokenKind::Colon);
            if is_module_block {
                // This will be parsed as a module block in the top-item loop below
                None
            } else {
                let md = self.parse_module_decl();
                Some(md)
            }
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
            trust,
            panic_strategy,
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
    /// use_decl <- 'use' (module_path | file_path) NEWLINE
    /// file_path <- STRING_LITERAL
    /// ```
    fn parse_use_decl(&mut self) -> UseDecl {
        let start = self.current_span();
        self.advance(); // consume 'use'

        // Check if this is a file path import (string literal) or module path (identifiers)
        let import = match self.peek().clone() {
            TokenKind::StringLit(path) => {
                self.advance(); // consume string literal
                ImportKind::FilePath(path)
            }
            _ => {
                // Module path import
                let path = self.parse_module_path();
                ImportKind::ModulePath(path)
            }
        };

        // Handle specific imports { ... } for module paths only
        let specific_imports = if matches!(self.peek(), TokenKind::Dot) {
            // Only module paths can have specific imports
            if matches!(&import, ImportKind::ModulePath(_)) {
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
                    let mut path = match &import {
                        ImportKind::ModulePath(p) => p.clone(),
                        _ => vec![],
                    };
                    while matches!(self.peek(), TokenKind::Dot) {
                        if matches!(self.peek_ahead(1), TokenKind::LBrace) {
                            self.advance(); // consume '.'
                            self.advance(); // consume '{'
                            let imports = self.parse_use_list();
                            if self.expect(TokenKind::RBrace).is_err() {
                                // error already recorded
                            }
                            return UseDecl {
                                import: ImportKind::ModulePath(path),
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
                // File paths don't support specific imports yet
                self.error_expected(&["newline after file path import"]);
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
            import,
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

        // Collect doc comments (/// lines) that precede this item.
        let mut doc_lines: Vec<String> = Vec::new();
        while let TokenKind::DocComment(text) = self.peek().clone() {
            doc_lines.push(text);
            self.advance();
            // Skip newlines between doc comment lines.
            self.skip_newlines();
        }
        let doc_comment = if doc_lines.is_empty() {
            None
        } else {
            Some(doc_lines.join("\n"))
        };

        // Collect annotations and budget constraints.
        let mut annotations = Vec::new();
        let mut budget: Option<BudgetConstraint> = None;
        while matches!(self.peek(), TokenKind::At) {
            // Peek ahead to detect @budget, which uses key: value syntax
            // and needs a custom parser path.
            if self.peek_budget_annotation() {
                budget = Some(self.parse_budget_annotation());
            } else {
                annotations.push(self.parse_annotation());
                if annotations.len() == 1 && annotations[0].name == "cap" {
                    break;
                }
            }
        }

        // Check for @cap(...) as a standalone module-level capability declaration.
        // @cap is always a module-level item, never attached to a function.
        if !annotations.is_empty() && annotations.len() == 1 && annotations[0].name == "cap" {
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
                let item = self.parse_fn_item(annotations, budget, doc_comment);
                Some(item)
            }
            TokenKind::Let => {
                let item = self.parse_let_item(start);
                Some(item)
            }
            TokenKind::Type => {
                let item = self.parse_type_decl_with_doc(doc_comment);
                Some(item)
            }
            TokenKind::Actor => {
                let item = self.parse_actor_decl(doc_comment);
                Some(item)
            }
            TokenKind::Trait => {
                let item = self.parse_trait_decl(doc_comment);
                Some(item)
            }
            TokenKind::Impl => {
                let item = self.parse_impl_block();
                Some(item)
            }
            TokenKind::Mod => {
                let item = self.parse_mod_block(doc_comment);
                Some(item)
            }
            TokenKind::Enum => {
                let item = self.parse_enum_block(doc_comment);
                Some(item)
            }
            TokenKind::Import => {
                let item = self.parse_import();
                Some(item)
            }
            // Note: TokenKind::Use is handled in parse_use_decl at the start of parse_program
            // before top-level items are parsed. If we see 'use' here, it's an error.
            TokenKind::Use => {
                self.error(
                    "use declarations must appear before other items at the top of the file",
                );
                None
            }
            _ => {
                if !annotations.is_empty() {
                    self.error("annotations must be followed by a function, let, type, actor, mod, or enum declaration");
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

    /// Check whether the current position holds `@budget(...)`.
    fn peek_budget_annotation(&self) -> bool {
        matches!(self.peek(), TokenKind::At)
            && matches!(self.peek_ahead(1), TokenKind::Ident(n) if n == "budget")
    }

    /// Parse a `@budget(cpu: 5s, mem: 100mb)` annotation with key-value syntax.
    ///
    /// Budget annotations use `key: value` pairs rather than arbitrary
    /// expressions, so they need a dedicated parser path.
    fn parse_budget_annotation(&mut self) -> BudgetConstraint {
        let start = self.current_span();
        self.advance(); // consume '@'
        self.advance(); // consume 'budget'

        let mut cpu: Option<String> = None;
        let mut mem: Option<String> = None;

        if matches!(self.peek(), TokenKind::LParen) {
            self.advance(); // consume '('

            while !matches!(self.peek(), TokenKind::RParen | TokenKind::Eof) {
                // Parse key: value pair.
                let key = match self.peek().clone() {
                    TokenKind::Ident(name) => {
                        self.advance();
                        name
                    }
                    _ => {
                        self.error_expected(&["budget key (cpu, mem)"]);
                        break;
                    }
                };

                if self.expect(TokenKind::Colon).is_err() {
                    break;
                }

                // Parse the value: a number followed by a unit suffix (e.g. 5s, 100mb).
                // This may be an IntLit followed by an Ident, or just an Ident.
                let value = self.parse_budget_value();

                match key.as_str() {
                    "cpu" => cpu = Some(value),
                    "mem" => mem = Some(value),
                    _ => {
                        self.errors.push(super::error::ParseError::new(
                            format!(
                                "unknown budget key `{}`; valid keys are `cpu` and `mem`",
                                key
                            ),
                            self.prev_span(),
                            vec![],
                            String::new(),
                        ));
                    }
                }

                // Consume optional comma.
                if matches!(self.peek(), TokenKind::Comma) {
                    self.advance();
                }
            }

            if self.expect(TokenKind::RParen).is_err() {
                // error already recorded
            }
        } else {
            self.errors.push(super::error::ParseError::new(
                "@budget requires parenthesized key-value pairs, e.g. @budget(cpu: 5s, mem: 100mb)",
                self.prev_span(),
                vec![],
                String::new(),
            ));
        }

        // Consume trailing newline.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let end = self.prev_span();
        BudgetConstraint {
            cpu,
            mem,
            span: merge_spans(&start, &end),
        }
    }

    /// Parse a budget value like `5s`, `100mb`, `1gb`.
    ///
    /// The lexer tokenizes `5s` as `IntLit(5)` followed by `Ident("s")`,
    /// and `100mb` as `IntLit(100)` followed by `Ident("mb")`.
    fn parse_budget_value(&mut self) -> String {
        match self.peek().clone() {
            TokenKind::IntLit(n) => {
                self.advance();
                // Check for a unit suffix immediately following.
                match self.peek().clone() {
                    TokenKind::Ident(unit) => {
                        self.advance();
                        format!("{}{}", n, unit)
                    }
                    _ => {
                        // Bare number with no unit.
                        format!("{}", n)
                    }
                }
            }
            TokenKind::Ident(s) => {
                // Could be something like "unlimited" or a unit string.
                self.advance();
                s
            }
            _ => {
                self.error_expected(&["budget value (e.g. 5s, 100mb)"]);
                String::from("<error>")
            }
        }
    }

    /// Parse a function item — decides between `fn_def` and `extern_fn_decl`
    /// based on whether a body follows.
    fn parse_fn_item(
        &mut self,
        annotations: Vec<Annotation>,
        budget: Option<BudgetConstraint>,
        doc_comment: Option<String>,
    ) -> Item {
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

        // Optional type parameters: `[T, U]`.
        let type_params = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_type_param_list()
        } else {
            Vec::new()
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

        // Separate contract annotations (@requires, @ensures), @extern,
        // @export, @test, and @verified from regular annotations.
        // Budget annotations (@budget) are already parsed separately in parse_top_item.
        let mut contracts = Vec::new();
        let mut regular_annotations = Vec::new();
        let mut is_extern = false;
        let mut extern_lib: Option<String> = None;
        let mut is_export = false;
        let mut is_test = false;
        let mut is_verified = false;
        let mut pending_runtime_only_off_in_release: Option<Span> = None;
        for ann in annotations {
            match ann.name.as_str() {
                "requires" => {
                    if let Some(cond) = ann.args.into_iter().next() {
                        contracts.push(Contract {
                            kind: ContractKind::Requires,
                            condition: cond,
                            span: ann.span,
                            runtime_only_off_in_release: pending_runtime_only_off_in_release
                                .take()
                                .is_some(),
                        });
                    } else {
                        self.errors.push(super::error::ParseError::new(
                            "@requires must have a condition expression",
                            ann.span,
                            vec![],
                            String::new(),
                        ));
                    }
                }
                "ensures" => {
                    if let Some(cond) = ann.args.into_iter().next() {
                        contracts.push(Contract {
                            kind: ContractKind::Ensures,
                            condition: cond,
                            span: ann.span,
                            runtime_only_off_in_release: pending_runtime_only_off_in_release
                                .take()
                                .is_some(),
                        });
                    } else {
                        self.errors.push(super::error::ParseError::new(
                            "@ensures must have a condition expression",
                            ann.span,
                            vec![],
                            String::new(),
                        ));
                    }
                }
                "extern" => {
                    is_extern = true;
                    // Extract optional library name: @extern("libm")
                    if let Some(arg) = ann.args.into_iter().next() {
                        if let crate::ast::expr::ExprKind::StringLit(lib) = arg.node {
                            extern_lib = Some(lib);
                        }
                    }
                }
                "export" => {
                    is_export = true;
                }
                "test" if ann.args.is_empty() => {
                    is_test = true;
                }
                "runtime_only" => {
                    let valid = ann.args.len() == 1
                        && matches!(
                            &ann.args[0].node,
                            crate::ast::expr::ExprKind::Ident(mode) if mode == "off_in_release"
                        );
                    if !valid {
                        self.errors.push(super::error::ParseError::new(
                            "@runtime_only accepts exactly `off_in_release`: @runtime_only(off_in_release)",
                            ann.span,
                            vec![],
                            String::new(),
                        ));
                    }
                    if pending_runtime_only_off_in_release
                        .replace(ann.span)
                        .is_some()
                    {
                        self.errors.push(super::error::ParseError::new(
                            "@runtime_only(off_in_release) must be followed by a single @requires or @ensures contract",
                            ann.span,
                            vec![],
                            String::new(),
                        ));
                    }
                }
                "verified" => {
                    // @verified marks a function for the static contract
                    // verification tier (ADR 0003). The annotation accepts no
                    // arguments at the launch surface; sub-issues #328/#329
                    // will introduce optional `timeout = "..."` and
                    // `strict = true/false` arguments. For now, reject any
                    // arguments so the surface stays narrow.
                    if !ann.args.is_empty() {
                        self.errors.push(super::error::ParseError::new(
                            "@verified takes no arguments at the launch surface (ADR 0003); future versions may accept `timeout` and `strict`",
                            ann.span,
                            vec![],
                            String::new(),
                        ));
                    }
                    is_verified = true;
                }
                _ => {
                    regular_annotations.push(ann);
                }
            }
        }
        if let Some(span) = pending_runtime_only_off_in_release {
            self.errors.push(super::error::ParseError::new(
                "@runtime_only(off_in_release) must be followed by @requires or @ensures",
                span,
                vec![],
                String::new(),
            ));
        }

        // Decide: fn_def (has `:` NEWLINE INDENT block) vs extern_fn_decl.
        // If @extern was present, treat as extern declaration (no body).
        // If @export was present and there's a body, parse as fn_def with
        // the is_export flag set.
        if matches!(self.peek(), TokenKind::Colon) && !is_extern {
            self.advance(); // consume ':'
                            // Expect NEWLINE then INDENT for block.
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
            let body = self.parse_block();
            let end = body.span;
            let fn_def = FnDef {
                name,
                type_params,
                params,
                return_type,
                effects,
                body,
                annotations: regular_annotations,
                contracts,
                budget,
                is_export,
                is_test,
                is_verified,
                doc_comment,
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
                annotations: regular_annotations,
                extern_lib,
                doc_comment,
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
            StmtKind::LetTupleDestructure {
                names,
                type_ann,
                value,
            } => Spanned::new(
                ItemKind::LetTupleDestructure {
                    names,
                    type_ann,
                    value,
                },
                merge_spans(&start, &end),
            ),
            _ => unreachable!("parse_let_stmt_inner always returns StmtKind::Let or StmtKind::LetTupleDestructure"),
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

    /// Parse a single parameter: `[comptime] IDENT ':' type_expr`.
    /// Also handles bare `self` without a type annotation (used in impl blocks).
    fn parse_param(&mut self) -> Param {
        let start = self.current_span();

        // Check for optional `comptime` keyword.
        let comptime = matches!(self.peek(), TokenKind::Comptime);
        if comptime {
            self.advance(); // consume 'comptime'
        }

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            // Keywords that can be used as parameter names
            TokenKind::Ret => {
                self.advance();
                "ret".to_string()
            }
            TokenKind::Type => {
                self.advance();
                "type".to_string()
            }
            TokenKind::State => {
                self.advance();
                "state".to_string()
            }
            TokenKind::Result => {
                self.advance();
                "result".to_string()
            }
            _ => {
                self.error_expected(&["parameter name"]);
                String::from("<error>")
            }
        };

        // If the parameter is `self` and the next token is not `:`, treat it
        // as an implicit Self-typed parameter (used in trait/impl methods).
        if name == "self" && !matches!(self.peek(), TokenKind::Colon) {
            let end = self.prev_span();
            return Param {
                name,
                type_ann: Spanned::new(
                    TypeExpr::Named {
                        name: "Self".to_string(),
                        cap: None,
                    },
                    merge_spans(&start, &end),
                ),
                span: merge_spans(&start, &end),
                comptime,
            };
        }

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }

        let type_ann = self.parse_type_expr();
        let end = type_ann.span;

        Param {
            name,
            type_ann,
            span: merge_spans(&start, &end),
            comptime,
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
                // Check for assignment: Ident (or soft keyword) followed by
                // '=' (but not '==').
                if self.peek_soft_keyword_ident() && matches!(self.peek_ahead(1), TokenKind::Assign)
                {
                    return self.parse_assign_stmt();
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
        let name = self
            .soft_keyword_as_ident()
            .expect("parse_assign_stmt called with non-ident lookahead");
        self.advance(); // consume '='
        let value = self.parse_expr();
        let end = value.span;
        Spanned::new(StmtKind::Assign { name, value }, merge_spans(&start, &end))
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

        // Check for tuple destructuring: `let (a, b) = ...`
        if !mutable && matches!(self.peek(), TokenKind::LParen) {
            self.advance(); // consume '('
            let mut names = Vec::new();
            if !matches!(self.peek(), TokenKind::RParen) {
                match self.peek().clone() {
                    TokenKind::Ident(name) => {
                        self.advance();
                        names.push(name);
                    }
                    _ => {
                        self.error_expected(&["variable name in tuple pattern"]);
                        names.push(String::from("<error>"));
                    }
                }
                while matches!(self.peek(), TokenKind::Comma) {
                    self.advance(); // consume ','
                    if matches!(self.peek(), TokenKind::RParen) {
                        break; // trailing comma
                    }
                    match self.peek().clone() {
                        TokenKind::Ident(name) => {
                            self.advance();
                            names.push(name);
                        }
                        _ => {
                            self.error_expected(&["variable name in tuple pattern"]);
                            names.push(String::from("<error>"));
                        }
                    }
                }
            }
            if self.expect(TokenKind::RParen).is_err() {
                // error already recorded
            }

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

            return Spanned::new(
                StmtKind::LetTupleDestructure {
                    names,
                    type_ann,
                    value,
                },
                merge_spans(&start, &end),
            );
        }

        let name = match self.soft_keyword_as_ident() {
            Some(n) => n,
            None => {
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
    /// type_decl <- 'type' IDENT '=' (enum_variants / type_expr) NEWLINE
    /// enum_variants <- IDENT ('(' type_expr ')')? ('|' IDENT ('(' type_expr ')')?)*
    /// ```
    fn parse_type_decl_with_doc(&mut self, doc_comment: Option<String>) -> Item {
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

        // Optional type parameters: `[T, U]` after the type name.
        // Enums use simple type params (no bounds).
        let type_params = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_simple_type_param_list()
        } else {
            Vec::new()
        };

        // Support both `type Name = ...` and `type Name:` (record-style) syntax
        let use_record_syntax = matches!(self.peek(), TokenKind::Colon);
        let is_assign = matches!(self.peek(), TokenKind::Assign);

        if use_record_syntax {
            self.advance(); // consume ':'
        } else if is_assign {
            self.advance(); // consume '='
        } else if self.expect(TokenKind::Assign).is_err() {
            // error already recorded
        }

        // If using record syntax with indented block, check if it's actually an enum
        if use_record_syntax && matches!(self.peek(), TokenKind::Newline) {
            // Look ahead to see if this is an enum block (variants with possible constructors)
            // or a record type (fields with name: Type)
            if self.is_enum_block_rhs() {
                return self.parse_enum_block_from_type(name, type_params, start, doc_comment);
            }
            return self.parse_record_type_decl(name, type_params, start, doc_comment);
        }

        // Check if this is an enum declaration: Ident followed by `|`.
        // We look ahead to see if the pattern matches `Ident (Pipe | LParen | Newline/Eof)`.
        // If we see `Ident |` or `Ident(Type) |`, it's an enum.
        if self.is_enum_rhs() {
            return self.parse_enum_variants(name, type_params, start, doc_comment);
        }

        // Indented enum form after `=`:
        //     type TokenKind =
        //         | IntLit(Int)
        //         | FloatLit(Float)
        // Reuse the enum-block parser, which already tolerates a leading `|`.
        if is_assign && self.is_indented_enum_rhs() {
            return self.parse_enum_block_from_type(name, type_params, start, doc_comment);
        }

        let type_expr = self.parse_type_expr();
        let end = type_expr.span;

        // Consume trailing newline.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        Spanned::new(
            ItemKind::TypeDecl {
                name,
                type_expr,
                doc_comment,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a record-style type declaration (struct-like syntax):
    /// ```text
    /// type Name:
    ///     field1: Type1
    ///     field2: Type2
    /// ```
    fn parse_record_type_decl(
        &mut self,
        name: String,
        _type_params: Vec<String>,
        start: Span,
        doc_comment: Option<String>,
    ) -> Item {
        // Parse indented field definitions
        let mut fields = Vec::new();

        // Expect newline after colon
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Parse the record body block (INDENT ... DEDENT)
        if self.expect(TokenKind::Indent).is_err() {
            let end = self.prev_span();
            return Spanned::new(
                ItemKind::TypeDecl {
                    name,
                    type_expr: Spanned::new(
                        crate::ast::types::TypeExpr::Tuple(vec![]),
                        merge_spans(&start, &end),
                    ),
                    doc_comment,
                },
                merge_spans(&start, &end),
            );
        }

        // Parse each field within the indented block
        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();

            // Skip any doc comments before fields (they're attached to the field)
            while matches!(self.peek(), TokenKind::DocComment(_)) {
                self.advance(); // consume doc comment
                self.skip_newlines();
            }

            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            // Parse field name - can be identifier OR keyword
            let field_name = self.parse_record_field_name();
            if field_name == "<error>" {
                // Error recovery
                if !self.at_end() {
                    self.error("expected field name");
                    self.synchronize();
                }
                continue;
            }

            // Expect colon
            if self.expect(TokenKind::Colon).is_err() {
                // Try to recover
                while !matches!(
                    self.peek(),
                    TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
                ) {
                    self.advance();
                }
                continue;
            }

            // Parse field type
            let field_type = self.parse_type_expr();
            fields.push((field_name, field_type));

            // Consume trailing newline or comma
            self.skip_newlines();
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        // Create a tuple type representation for the fields
        let end = if let Some((_, last_ty)) = fields.last() {
            last_ty.span
        } else {
            self.prev_span()
        };

        // Preserve field names so the typechecker can resolve field reads.
        let record_type_expr = Spanned::new(
            crate::ast::types::TypeExpr::Record(fields.clone()),
            merge_spans(&start, &end),
        );

        Spanned::new(
            ItemKind::TypeDecl {
                name,
                type_expr: record_type_expr,
                doc_comment,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse enum block from a type declaration context.
    fn parse_enum_block_from_type(
        &mut self,
        name: String,
        type_params: Vec<String>,
        start: Span,
        doc_comment: Option<String>,
    ) -> Item {
        let mut variants = Vec::new();

        // Consume newline after colon
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Expect indent
        if self.expect(TokenKind::Indent).is_err() {
            let end = self.prev_span();
            return Spanned::new(
                ItemKind::EnumDecl {
                    name,
                    type_params,
                    variants: Vec::new(),
                    doc_comment,
                },
                merge_spans(&start, &end),
            );
        }

        // Parse variants separated by newlines or |
        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            // Tolerate a leading `|` before the first/next variant
            // (lets `type T =\n    | A\n    | B` parse the same as the
            // pipe-separated form).
            if matches!(self.peek(), TokenKind::Pipe) {
                self.advance();
                self.skip_newlines();
            }

            // Parse a variant
            variants.push(self.parse_single_variant());

            // After a variant, we can have:
            // - `|` followed by another variant (same line or next)
            // - newline(s) followed by another variant
            // - DEDENT to end the enum
            self.skip_newlines();

            // Check for | separator
            if matches!(self.peek(), TokenKind::Pipe) {
                self.advance(); // consume '|'
            }

            // If we see DEDENT or EOF, we're done
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = variants.last().map(|v| v.span).unwrap_or(start);

        Spanned::new(
            ItemKind::EnumDecl {
                name,
                type_params,
                variants,
                doc_comment,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a record field name, treating keywords as valid identifiers.
    /// Consume a token that can be used as a binding name and return its
    /// spelling. Plain identifiers are always accepted; a small set of soft
    /// keywords (`state`, `ret`, `type`, `on`, `spawn`, `send`, `ask`,
    /// `defer`, `mod`, `consumed`) are also accepted in binding position
    /// because they're only meaningful in their own dedicated contexts
    /// (actor blocks, return statements, type aliases, etc.) and rejecting
    /// them as plain variable names trips up code that didn't choose to
    /// avoid them — including the self-hosted compiler sources.
    fn soft_keyword_as_ident(&mut self) -> Option<String> {
        let name = match self.peek() {
            TokenKind::Ident(name) => name.clone(),
            TokenKind::State => "state".to_string(),
            TokenKind::Ret => "ret".to_string(),
            TokenKind::Type => "type".to_string(),
            TokenKind::On => "on".to_string(),
            TokenKind::Spawn => "spawn".to_string(),
            TokenKind::Send => "send".to_string(),
            TokenKind::Ask => "ask".to_string(),
            TokenKind::Defer => "defer".to_string(),
            TokenKind::Mod => "mod".to_string(),
            TokenKind::Consumed => "consumed".to_string(),
            TokenKind::Result => "result".to_string(),
            _ => return None,
        };
        self.advance();
        Some(name)
    }

    /// Non-consuming check matching the same token set as
    /// [`soft_keyword_as_ident`]. Used for lookahead in statement parsing.
    fn peek_soft_keyword_ident(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Ident(_)
                | TokenKind::State
                | TokenKind::Ret
                | TokenKind::Type
                | TokenKind::On
                | TokenKind::Spawn
                | TokenKind::Send
                | TokenKind::Ask
                | TokenKind::Defer
                | TokenKind::Mod
                | TokenKind::Consumed
                | TokenKind::Result
        )
    }

    fn parse_record_field_name(&mut self) -> String {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            // Keywords that can be used as field names (lowercase them)
            TokenKind::Ret => {
                self.advance();
                String::from("ret")
            }
            TokenKind::Consumed => {
                self.advance();
                String::from("consumed")
            }
            TokenKind::State => {
                self.advance();
                String::from("state")
            }
            TokenKind::On => {
                self.advance();
                String::from("on")
            }
            TokenKind::Spawn => {
                self.advance();
                String::from("spawn")
            }
            TokenKind::Send => {
                self.advance();
                String::from("send")
            }
            TokenKind::Ask => {
                self.advance();
                String::from("ask")
            }
            TokenKind::Defer => {
                self.advance();
                String::from("defer")
            }
            TokenKind::Type => {
                self.advance();
                String::from("type")
            }
            TokenKind::Mod => {
                self.advance();
                String::from("mod")
            }
            TokenKind::Use => {
                self.advance();
                String::from("use")
            }
            TokenKind::Impl => {
                self.advance();
                String::from("impl")
            }
            TokenKind::Match => {
                self.advance();
                String::from("match")
            }
            TokenKind::And => {
                self.advance();
                String::from("and")
            }
            TokenKind::Or => {
                self.advance();
                String::from("or")
            }
            TokenKind::Not => {
                self.advance();
                String::from("not")
            }
            TokenKind::Comptime => {
                self.advance();
                String::from("comptime")
            }
            TokenKind::Trait => {
                self.advance();
                String::from("trait")
            }
            TokenKind::Actor => {
                self.advance();
                String::from("actor")
            }
            TokenKind::Iso => {
                self.advance();
                String::from("iso")
            }
            TokenKind::Val => {
                self.advance();
                String::from("val")
            }
            TokenKind::Ref => {
                self.advance();
                String::from("ref")
            }
            TokenKind::Box => {
                self.advance();
                String::from("box")
            }
            TokenKind::Trn => {
                self.advance();
                String::from("trn")
            }
            TokenKind::Tag => {
                self.advance();
                String::from("tag")
            }
            TokenKind::Fn => {
                self.advance();
                String::from("fn")
            }
            TokenKind::Let => {
                self.advance();
                String::from("let")
            }
            TokenKind::Mut => {
                self.advance();
                String::from("mut")
            }
            TokenKind::If => {
                self.advance();
                String::from("if")
            }
            TokenKind::Else => {
                self.advance();
                String::from("else")
            }
            TokenKind::For => {
                self.advance();
                String::from("for")
            }
            TokenKind::In => {
                self.advance();
                String::from("in")
            }
            TokenKind::While => {
                self.advance();
                String::from("while")
            }
            TokenKind::Enum => {
                self.advance();
                String::from("enum")
            }
            TokenKind::True => {
                self.advance();
                String::from("true")
            }
            TokenKind::False => {
                self.advance();
                String::from("false")
            }
            _ => {
                self.error_expected(&["field name"]);
                String::from("<error>")
            }
        }
    }

    /// Check if the right-hand side of `type Name =` is an enum declaration.
    ///
    /// We detect enums by looking for the pattern: `Ident` followed by `|`
    /// (possibly with `(Type)` in between), or a single-variant tuple enum:
    /// `Ident(...)` followed by Newline/Eof (no Pipe needed).
    fn is_enum_rhs(&self) -> bool {
        // Check: Ident | ...  (unit variant followed by pipe)
        if matches!(self.peek(), TokenKind::Ident(_)) {
            // Ident followed by Pipe
            if matches!(self.peek_ahead(1), TokenKind::Pipe) {
                return true;
            }
            // Ident(Type...) followed by Pipe or Newline/Eof (single-variant tuple enum).
            if matches!(self.peek_ahead(1), TokenKind::LParen) {
                // Scan forward past the parenthesized type list to find the matching RParen.
                // Track nesting depth to handle nested generic types like List[T].
                let mut offset = 2;
                let mut depth = 1usize; // we're inside one LParen
                loop {
                    match self.peek_ahead(offset) {
                        TokenKind::LParen => {
                            depth += 1;
                            offset += 1;
                        }
                        TokenKind::RParen => {
                            depth -= 1;
                            offset += 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        TokenKind::Eof => return false,
                        _ => offset += 1,
                    }
                    if offset > 64 {
                        return false; // safety limit
                    }
                }
                // After the closing RParen: Pipe means multi-variant, Newline/Eof means single-variant.
                match self.peek_ahead(offset) {
                    TokenKind::Pipe => return true,
                    TokenKind::Newline | TokenKind::Eof => return true,
                    _ => {}
                }
            }
        }
        false
    }

    /// Check if the right-hand side of `type Name =` is an indented enum
    /// block, i.e. `=` followed by NEWLINE INDENT (optional `|`) Ident.
    fn is_indented_enum_rhs(&self) -> bool {
        if !matches!(self.peek(), TokenKind::Newline) {
            return false;
        }
        let mut offset = 1;
        if matches!(self.peek_ahead(offset), TokenKind::Indent) {
            offset += 1;
        } else {
            return false;
        }
        // Skip blank lines
        while matches!(self.peek_ahead(offset), TokenKind::Newline) {
            offset += 1;
        }
        // Optional leading `|`
        if matches!(self.peek_ahead(offset), TokenKind::Pipe) {
            offset += 1;
            while matches!(self.peek_ahead(offset), TokenKind::Newline) {
                offset += 1;
            }
        }
        matches!(self.peek_ahead(offset), TokenKind::Ident(_))
    }

    /// Check if the right-hand side of `type Name:` (with newline) is an enum block.
    fn is_enum_block_rhs(&self) -> bool {
        // Look ahead past newline and optional indent to find the first content token
        let mut offset = 1; // Start after the current newline

        // Skip indent if present
        if matches!(self.peek_ahead(offset), TokenKind::Indent) {
            offset += 1;
        }

        // Skip any additional newlines (blank lines)
        while matches!(self.peek_ahead(offset), TokenKind::Newline) {
            offset += 1;
        }

        // Now check what we have
        if let TokenKind::Ident(ref name) = self.peek_ahead(offset) {
            // If it starts with uppercase, it's likely an enum variant
            if name.starts_with(|c: char| c.is_uppercase())
                && matches!(
                    self.peek_ahead(offset + 1),
                    TokenKind::LParen
                        | TokenKind::Pipe
                        | TokenKind::Newline
                        | TokenKind::Dedent
                        | TokenKind::Colon
                )
            {
                return true;
            }
        }

        false
    }

    /// Parse enum variants: `Variant1 | Variant2(Type) | Variant3`.
    fn parse_enum_variants(
        &mut self,
        name: String,
        type_params: Vec<String>,
        start: Span,
        doc_comment: Option<String>,
    ) -> Item {
        let mut variants = Vec::new();

        // Parse the first variant.
        variants.push(self.parse_single_variant());

        // Parse remaining variants separated by `|`.
        while matches!(self.peek(), TokenKind::Pipe) {
            self.advance(); // consume '|'
            variants.push(self.parse_single_variant());
        }

        let end = variants.last().map(|v| v.span).unwrap_or(start);

        // Consume trailing newline.
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        Spanned::new(
            ItemKind::EnumDecl {
                name,
                type_params,
                variants,
                doc_comment,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a single enum variant: `VariantName` or `VariantName(Type)` or `VariantName(field: Type)`.
    /// Supports both anonymous tuple fields and named struct-like fields.
    /// Also handles doc comments before variants and keywords as field names.
    fn parse_single_variant(&mut self) -> EnumVariant {
        let var_start = self.current_span();

        // Check for optional doc comment before the variant
        let doc_comment = if matches!(self.peek(), TokenKind::DocComment(_)) {
            let doc = match self.peek() {
                TokenKind::DocComment(text) => Some(text.clone()),
                _ => None,
            };
            self.advance();
            doc
        } else {
            None
        };

        let variant_name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["variant name"]);
                String::from("<error>")
            }
        };

        // Check for optional fields: `(Type)` or `(field: Type, ...)`.
        let fields: Option<Vec<VariantField>> = if matches!(self.peek(), TokenKind::LParen) {
            self.advance(); // consume '('
            self.parse_variant_fields()
        } else {
            None
        };

        let var_end = self.prev_span();
        EnumVariant {
            name: variant_name,
            fields,
            span: merge_spans(&var_start, &var_end),
            doc_comment,
        }
    }

    /// Parse fields within an enum variant's parentheses.
    /// Supports both anonymous `(Type, Type)` and named `(field: Type)` syntax.
    fn parse_variant_fields(&mut self) -> Option<Vec<VariantField>> {
        // Check for empty parentheses: `()`
        if matches!(self.peek(), TokenKind::RParen) {
            self.advance(); // consume ')'
            return Some(vec![]);
        }

        let mut fields = Vec::new();

        loop {
            // Check if this is a named field (lookahead for `ident:`)
            let is_named_field = self.is_named_field_lookahead();

            if is_named_field {
                // Parse named field: `field_name: Type`
                let field_name = self.parse_field_name_as_ident();
                if self.expect(TokenKind::Colon).is_err() {
                    // Try to recover
                    self.skip_to_field_end();
                } else {
                    let type_expr = self.parse_type_expr();
                    fields.push(VariantField::Named {
                        name: field_name,
                        type_expr,
                    });
                }
            } else {
                // Parse anonymous field: `Type`
                let type_expr = self.parse_type_expr();
                fields.push(VariantField::Anonymous(type_expr));
            }

            // Check for comma separator or closing paren
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance(); // consume ','
            } else if matches!(self.peek(), TokenKind::RParen) {
                self.advance(); // consume ')'
                break;
            } else {
                // Expected comma or closing paren
                if self.expect(TokenKind::RParen).is_err() {
                    // Try to recover by skipping to next comma or paren
                    self.skip_to_field_end();
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance();
                    } else if matches!(self.peek(), TokenKind::RParen) {
                        self.advance();
                        break;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
        }

        Some(fields)
    }

    /// Check if the next tokens form a named field (`ident:` or `keyword:`).
    fn is_named_field_lookahead(&self) -> bool {
        // Check for `Ident` followed by `:`
        if matches!(self.peek(), TokenKind::Ident(_)) {
            return matches!(self.peek_ahead(1), TokenKind::Colon);
        }

        // Check for keyword followed by `:` (treating keyword as field name)
        if self.peek_is_keyword() && matches!(self.peek_ahead(1), TokenKind::Colon) {
            return true;
        }

        false
    }

    /// Check if current token is a keyword that can be used as a field name.
    fn peek_is_keyword(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Ret
                | TokenKind::State
                | TokenKind::On
                | TokenKind::Spawn
                | TokenKind::Send
                | TokenKind::Ask
                | TokenKind::Defer
                | TokenKind::Type
                | TokenKind::Mod
                | TokenKind::Use
                | TokenKind::Impl
                | TokenKind::Match
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::Not
                | TokenKind::Comptime
                | TokenKind::Trait
                | TokenKind::Actor
                | TokenKind::Iso
                | TokenKind::Val
                | TokenKind::Ref
                | TokenKind::Box
                | TokenKind::Trn
                | TokenKind::Tag
        )
    }

    /// Parse a field name, treating keywords as valid identifiers.
    fn parse_field_name_as_ident(&mut self) -> String {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            // Keywords that can be used as field names
            TokenKind::Ret => {
                self.advance();
                String::from("ret")
            }
            TokenKind::State => {
                self.advance();
                String::from("state")
            }
            TokenKind::On => {
                self.advance();
                String::from("on")
            }
            TokenKind::Spawn => {
                self.advance();
                String::from("spawn")
            }
            TokenKind::Send => {
                self.advance();
                String::from("send")
            }
            TokenKind::Ask => {
                self.advance();
                String::from("ask")
            }
            TokenKind::Defer => {
                self.advance();
                String::from("defer")
            }
            TokenKind::Type => {
                self.advance();
                String::from("type")
            }
            TokenKind::Mod => {
                self.advance();
                String::from("mod")
            }
            TokenKind::Use => {
                self.advance();
                String::from("use")
            }
            TokenKind::Impl => {
                self.advance();
                String::from("impl")
            }
            TokenKind::Match => {
                self.advance();
                String::from("match")
            }
            TokenKind::And => {
                self.advance();
                String::from("and")
            }
            TokenKind::Or => {
                self.advance();
                String::from("or")
            }
            TokenKind::Not => {
                self.advance();
                String::from("not")
            }
            TokenKind::Comptime => {
                self.advance();
                String::from("comptime")
            }
            TokenKind::Trait => {
                self.advance();
                String::from("trait")
            }
            TokenKind::Actor => {
                self.advance();
                String::from("actor")
            }
            TokenKind::Iso => {
                self.advance();
                String::from("iso")
            }
            TokenKind::Val => {
                self.advance();
                String::from("val")
            }
            TokenKind::Ref => {
                self.advance();
                String::from("ref")
            }
            TokenKind::Box => {
                self.advance();
                String::from("box")
            }
            TokenKind::Trn => {
                self.advance();
                String::from("trn")
            }
            TokenKind::Tag => {
                self.advance();
                String::from("tag")
            }
            _ => {
                self.error_expected(&["field name"]);
                String::from("<error>")
            }
        }
    }

    /// Skip tokens until we reach a comma, closing paren, or end of relevant context.
    fn skip_to_field_end(&mut self) {
        while !self.at_end()
            && !matches!(
                self.peek(),
                TokenKind::Comma | TokenKind::RParen | TokenKind::Newline | TokenKind::Eof
            )
        {
            self.advance();
        }
    }

    // -----------------------------------------------------------------------
    // Actor declarations
    // -----------------------------------------------------------------------

    /// ```text
    /// actor_decl <- 'actor' IDENT ':' NEWLINE INDENT
    ///              (state_field NEWLINE)*
    ///              (message_handler NEWLINE)*
    ///              DEDENT
    /// ```
    fn parse_actor_decl(&mut self, doc_comment: Option<String>) -> Item {
        let start = self.current_span();
        self.advance(); // consume 'actor'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["actor name"]);
                String::from("<error>")
            }
        };

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Parse the actor body block (INDENT ... DEDENT).
        if self.expect(TokenKind::Indent).is_err() {
            let end = self.prev_span();
            return Spanned::new(
                ItemKind::ActorDecl {
                    name,
                    state_fields: Vec::new(),
                    handlers: Vec::new(),
                    doc_comment,
                },
                merge_spans(&start, &end),
            );
        }

        let mut state_fields = Vec::new();
        let mut handlers = Vec::new();

        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            match self.peek() {
                TokenKind::State => {
                    state_fields.push(self.parse_state_field());
                }
                TokenKind::On => {
                    handlers.push(self.parse_message_handler());
                }
                _ => {
                    self.error_expected(&["'state' or 'on'"]);
                    self.synchronize();
                }
            }

            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        Spanned::new(
            ItemKind::ActorDecl {
                name,
                state_fields,
                handlers,
                doc_comment,
            },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// state_field <- 'state' IDENT ':' type_expr '=' expr
    /// ```
    fn parse_state_field(&mut self) -> StateField {
        let start = self.current_span();
        self.advance(); // consume 'state'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["state field name"]);
                String::from("<error>")
            }
        };

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }

        let type_ann = self.parse_type_expr();

        if self.expect(TokenKind::Assign).is_err() {
            // error already recorded
        }

        let default_value = self.parse_expr();
        let end = default_value.span;

        StateField {
            name,
            type_ann,
            default_value,
            span: merge_spans(&start, &end),
        }
    }

    /// ```text
    /// message_handler <- 'on' IDENT ('->' type_expr)? ':' NEWLINE block
    /// ```
    fn parse_message_handler(&mut self) -> MessageHandler {
        let start = self.current_span();
        self.advance(); // consume 'on'

        let message_name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["message name"]);
                String::from("<error>")
            }
        };

        // Optional return type: `-> Type`.
        let return_type = if matches!(self.peek(), TokenKind::Arrow) {
            self.advance(); // consume '->'
            Some(self.parse_type_expr())
        } else {
            None
        };

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let body = self.parse_block();
        let end = body.span;

        MessageHandler {
            message_name,
            return_type,
            body,
            span: merge_spans(&start, &end),
        }
    }

    // -----------------------------------------------------------------------
    // Trait and impl declarations
    // -----------------------------------------------------------------------

    /// ```text
    /// trait_decl <- 'trait' IDENT ':' NEWLINE INDENT
    ///              (trait_method NEWLINE)*
    ///              DEDENT
    /// ```
    fn parse_trait_decl(&mut self, doc_comment: Option<String>) -> Item {
        let start = self.current_span();
        self.advance(); // consume 'trait'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["trait name"]);
                String::from("<error>")
            }
        };

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Parse the trait body block (INDENT ... DEDENT).
        if self.expect(TokenKind::Indent).is_err() {
            let end = self.prev_span();
            return Spanned::new(
                ItemKind::TraitDecl {
                    name,
                    methods: Vec::new(),
                    doc_comment,
                },
                merge_spans(&start, &end),
            );
        }

        let mut methods = Vec::new();

        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            match self.peek() {
                TokenKind::Fn => {
                    methods.push(self.parse_trait_method());
                }
                _ => {
                    self.error_expected(&["'fn'"]);
                    self.synchronize();
                }
            }

            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        Spanned::new(
            ItemKind::TraitDecl {
                name,
                methods,
                doc_comment,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a trait method signature (no body):
    /// ```text
    /// trait_method <- 'fn' IDENT '(' param_list ')' return_clause?
    /// ```
    fn parse_trait_method(&mut self) -> TraitMethod {
        let start = self.current_span();
        self.advance(); // consume 'fn'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["method name"]);
                String::from("<error>")
            }
        };

        // Parameter list.
        if self.expect(TokenKind::LParen).is_err() {
            // error already recorded
        }
        let params = if !matches!(self.peek(), TokenKind::RParen) {
            self.parse_trait_param_list()
        } else {
            Vec::new()
        };
        if self.expect(TokenKind::RParen).is_err() {
            // error already recorded
        }

        // Optional return clause.
        let (effects, return_type) = self.parse_return_clause();

        let end = self.prev_span();

        TraitMethod {
            name,
            params,
            return_type,
            effects,
            span: merge_spans(&start, &end),
        }
    }

    /// Parse a parameter list for a trait method, where `self` is allowed
    /// without a type annotation.
    fn parse_trait_param_list(&mut self) -> Vec<Param> {
        let mut params = Vec::new();

        params.push(self.parse_trait_param());

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
            if matches!(self.peek(), TokenKind::RParen) {
                break; // trailing comma
            }
            params.push(self.parse_trait_param());
        }

        params
    }

    /// Parse a single trait parameter. `self` is allowed without a type
    /// annotation (it gets a placeholder `Self` type).
    fn parse_trait_param(&mut self) -> Param {
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

        // If the parameter is `self`, it doesn't need a type annotation.
        if name == "self" && !matches!(self.peek(), TokenKind::Colon) {
            let end = self.prev_span();
            return Param {
                name,
                type_ann: Spanned::new(
                    TypeExpr::Named {
                        name: "Self".to_string(),
                        cap: None,
                    },
                    merge_spans(&start, &end),
                ),
                span: merge_spans(&start, &end),
                comptime: false,
            };
        }

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }

        let type_ann = self.parse_type_expr();
        let end = type_ann.span;

        Param {
            name,
            type_ann,
            span: merge_spans(&start, &end),
            comptime: false,
        }
    }

    /// ```text
    /// impl_block <- 'impl' IDENT 'for' IDENT ':' NEWLINE INDENT
    ///              (fn_def NEWLINE)*
    ///              DEDENT
    /// ```
    fn parse_impl_block(&mut self) -> Item {
        let start = self.current_span();
        self.advance(); // consume 'impl'

        let trait_name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["trait name"]);
                String::from("<error>")
            }
        };

        // Expect 'for' keyword (parsed as Ident since it's also a keyword).
        if self.expect(TokenKind::For).is_err() {
            // error already recorded
        }

        let target_type = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["type name"]);
                String::from("<error>")
            }
        };

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Parse the impl body block (INDENT ... DEDENT).
        if self.expect(TokenKind::Indent).is_err() {
            let end = self.prev_span();
            return Spanned::new(
                ItemKind::ImplBlock {
                    trait_name,
                    target_type,
                    methods: Vec::new(),
                },
                merge_spans(&start, &end),
            );
        }

        let mut methods = Vec::new();

        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            match self.peek() {
                TokenKind::Fn => {
                    // Parse a full function definition inside the impl block.
                    let fn_item = self.parse_fn_item(Vec::new(), None, None);
                    if let ItemKind::FnDef(fn_def) = fn_item.node {
                        methods.push(fn_def);
                    }
                }
                _ => {
                    self.error_expected(&["'fn'"]);
                    self.synchronize();
                }
            }

            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        Spanned::new(
            ItemKind::ImplBlock {
                trait_name,
                target_type,
                methods,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a module block: `mod Name:` followed by indented items.
    /// ```text
    /// mod_block <- 'mod' IDENT ':' NEWLINE INDENT (top_item NEWLINE*)* DEDENT
    /// ```
    fn parse_mod_block(&mut self, doc_comment: Option<String>) -> Item {
        let start = self.current_span();
        self.advance(); // consume 'mod'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["module name"]);
                String::from("<error>")
            }
        };

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Parse the module body block (INDENT ... DEDENT).
        if self.expect(TokenKind::Indent).is_err() {
            let end = self.prev_span();
            return Spanned::new(
                ItemKind::ModBlock {
                    name,
                    items: Vec::new(),
                    doc_comment,
                },
                merge_spans(&start, &end),
            );
        }

        let mut items = Vec::new();

        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            if let Some(item) = self.parse_top_item() {
                items.push(item);
            } else {
                // Could not parse a top-level item. Skip a token and try again.
                if !self.at_end() {
                    let before = self.pos;
                    self.error("unexpected token in mod block");
                    self.synchronize();
                    if self.pos == before && !self.at_end() {
                        self.advance();
                    }
                }
            }

            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        Spanned::new(
            ItemKind::ModBlock {
                name,
                items,
                doc_comment,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse an import statement: `import "path.gr"` or `import "path.gr" as alias`
    fn parse_import(&mut self) -> Item {
        let start = self.current_span();
        self.advance(); // consume 'import'

        let path = match self.peek().clone() {
            TokenKind::StringLit(path) => {
                self.advance();
                path
            }
            _ => {
                self.error_expected(&["string path"]);
                String::from("<error>")
            }
        };

        let alias = if matches!(self.peek(), TokenKind::Ident(name) if name == "as") {
            self.advance(); // consume 'as'
            match self.peek().clone() {
                TokenKind::Ident(alias) => {
                    self.advance();
                    Some(alias)
                }
                _ => {
                    self.error_expected(&["alias name"]);
                    None
                }
            }
        } else {
            None
        };

        let end = self.prev_span();
        Spanned::new(ItemKind::Import { path, alias }, merge_spans(&start, &end))
    }

    /// Parse an enum block: `enum Name:` followed by indented variants.
    fn parse_enum_block(&mut self, doc_comment: Option<String>) -> Item {
        let start = self.current_span();
        self.advance(); // consume 'enum'

        let name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["enum name"]);
                String::from("<error>")
            }
        };

        // Optional type parameters: `[T, U]` after the enum name.
        let type_params = if matches!(self.peek(), TokenKind::LBracket) {
            self.parse_simple_type_param_list()
        } else {
            Vec::new()
        };

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Parse the enum body block (INDENT ... DEDENT).
        if self.expect(TokenKind::Indent).is_err() {
            let end = self.prev_span();
            return Spanned::new(
                ItemKind::EnumDecl {
                    name,
                    type_params,
                    variants: Vec::new(),
                    doc_comment,
                },
                merge_spans(&start, &end),
            );
        }

        let mut variants = Vec::new();

        // Parse variants separated by newlines or |
        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            // Parse a variant - could be unit, tuple, or struct style
            if let Some(variant) = self.parse_enum_block_variant() {
                variants.push(variant);
            } else {
                // Error recovery
                if !self.at_end() {
                    let before = self.pos;
                    self.error("expected enum variant");
                    self.synchronize();
                    if self.pos == before && !self.at_end() {
                        self.advance();
                    }
                }
            }

            // After a variant, we can have:
            // - `|` followed by another variant (same line or next)
            // - newline(s) followed by another variant
            // - DEDENT to end the enum
            self.skip_newlines();

            // Check for | separator
            if matches!(self.peek(), TokenKind::Pipe) {
                self.advance(); // consume '|'
                                // Continue to parse next variant
            }

            // If we see DEDENT or EOF, we're done
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        Spanned::new(
            ItemKind::EnumDecl {
                name,
                type_params,
                variants,
                doc_comment,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a single enum variant within an enum block.
    /// Returns None if parsing fails.
    /// Handles both regular identifiers and keywords as variant names.
    fn parse_enum_block_variant(&mut self) -> Option<EnumVariant> {
        let var_start = self.current_span();

        // Check for optional doc comment before the variant
        let doc_comment = if matches!(self.peek(), TokenKind::DocComment(_)) {
            let doc = match self.peek() {
                TokenKind::DocComment(text) => Some(text.clone()),
                _ => None,
            };
            self.advance();
            doc
        } else {
            None
        };

        // Parse variant name - can be an identifier OR certain keywords
        let variant_name = self.parse_variant_name_as_ident();
        if variant_name == "<error>" {
            return None;
        }

        // Check for variant fields: (field1, field2) or (name: Type, name2: Type2)
        let fields = if matches!(self.peek(), TokenKind::LParen) {
            self.advance(); // consume '('
            let mut fields = Vec::new();

            if !matches!(self.peek(), TokenKind::RParen) {
                // Check if this is a named field (name: Type) or anonymous (Type)
                let first_field = self.parse_enum_variant_field()?;
                fields.push(first_field);

                while matches!(self.peek(), TokenKind::Comma) {
                    self.advance(); // consume ','
                    if matches!(self.peek(), TokenKind::RParen) {
                        break;
                    }
                    let field = self.parse_enum_variant_field()?;
                    fields.push(field);
                }
            }

            if self.expect(TokenKind::RParen).is_err() {
                // error already recorded
            }
            Some(fields)
        } else {
            None
        };

        let var_end = self.prev_span();
        Some(EnumVariant {
            name: variant_name,
            fields,
            span: merge_spans(&var_start, &var_end),
            doc_comment,
        })
    }

    /// Parse a variant name, treating keywords as valid identifiers.
    /// This allows keywords like `Ref`, `Iso`, `Val` to be used as enum variant names.
    fn parse_variant_name_as_ident(&mut self) -> String {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            // Capability keywords that can be used as variant names
            TokenKind::Ref => {
                self.advance();
                String::from("Ref")
            }
            TokenKind::Iso => {
                self.advance();
                String::from("Iso")
            }
            TokenKind::Val => {
                self.advance();
                String::from("Val")
            }
            TokenKind::Box => {
                self.advance();
                String::from("Box")
            }
            TokenKind::Trn => {
                self.advance();
                String::from("Trn")
            }
            TokenKind::Tag => {
                self.advance();
                String::from("Tag")
            }
            // Other keywords that might be used as variant names
            TokenKind::Ret => {
                self.advance();
                String::from("Ret")
            }
            TokenKind::State => {
                self.advance();
                String::from("State")
            }
            TokenKind::On => {
                self.advance();
                String::from("On")
            }
            TokenKind::Spawn => {
                self.advance();
                String::from("Spawn")
            }
            TokenKind::Send => {
                self.advance();
                String::from("Send")
            }
            TokenKind::Ask => {
                self.advance();
                String::from("Ask")
            }
            TokenKind::Defer => {
                self.advance();
                String::from("Defer")
            }
            TokenKind::Type => {
                self.advance();
                String::from("Type")
            }
            TokenKind::Mod => {
                self.advance();
                String::from("Mod")
            }
            TokenKind::Use => {
                self.advance();
                String::from("Use")
            }
            TokenKind::Impl => {
                self.advance();
                String::from("Impl")
            }
            TokenKind::Match => {
                self.advance();
                String::from("Match")
            }
            TokenKind::And => {
                self.advance();
                String::from("And")
            }
            TokenKind::Or => {
                self.advance();
                String::from("Or")
            }
            TokenKind::Not => {
                self.advance();
                String::from("Not")
            }
            TokenKind::Comptime => {
                self.advance();
                String::from("Comptime")
            }
            TokenKind::Trait => {
                self.advance();
                String::from("Trait")
            }
            TokenKind::Actor => {
                self.advance();
                String::from("Actor")
            }
            TokenKind::Fn => {
                self.advance();
                String::from("Fn")
            }
            TokenKind::Let => {
                self.advance();
                String::from("Let")
            }
            TokenKind::Mut => {
                self.advance();
                String::from("Mut")
            }
            TokenKind::If => {
                self.advance();
                String::from("If")
            }
            TokenKind::Else => {
                self.advance();
                String::from("Else")
            }
            TokenKind::For => {
                self.advance();
                String::from("For")
            }
            TokenKind::In => {
                self.advance();
                String::from("In")
            }
            TokenKind::While => {
                self.advance();
                String::from("While")
            }
            TokenKind::Enum => {
                self.advance();
                String::from("Enum")
            }
            TokenKind::True => {
                self.advance();
                String::from("True")
            }
            TokenKind::False => {
                self.advance();
                String::from("False")
            }
            _ => {
                self.error_expected(&["variant name"]);
                String::from("<error>")
            }
        }
    }

    /// Parse a single field in an enum variant.
    /// Can be either named (name: Type) or anonymous (Type).
    /// Handles keywords as field names (e.g., `ret: Type`, `state: Type`).
    fn parse_enum_variant_field(&mut self) -> Option<VariantField> {
        // Look ahead to see if this is a named field
        // Named field: Ident Colon Type or Keyword Colon Type
        // Anonymous field: Type (starts with Ident but no Colon after)
        let is_named_field = self.is_named_enum_field_lookahead();

        if is_named_field {
            // Named field: name: Type (where name can be a keyword)
            let name = self.parse_field_name_as_ident();
            if name == "<error>" {
                return None;
            }
            self.advance(); // consume ':'
            let type_expr = self.parse_type_expr();
            return Some(VariantField::Named { name, type_expr });
        }

        // Anonymous field: just a type expression
        let type_expr = self.parse_type_expr();
        Some(VariantField::Anonymous(type_expr))
    }

    /// Check if the next tokens form a named field in an enum variant.
    /// This checks for `Ident:` or `Keyword:` patterns.
    fn is_named_enum_field_lookahead(&self) -> bool {
        // Check for `Ident` followed by `:`
        if matches!(self.peek(), TokenKind::Ident(_)) {
            return matches!(self.peek_ahead(1), TokenKind::Colon);
        }

        // Check for keyword followed by `:` (treating keyword as field name)
        // This allows field names like `ret:`, `state:`, etc.
        if self.peek_is_keyword_for_field() && matches!(self.peek_ahead(1), TokenKind::Colon) {
            return true;
        }

        false
    }

    /// Check if current token is a keyword that can be used as a field name.
    fn peek_is_keyword_for_field(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Ret
                | TokenKind::State
                | TokenKind::On
                | TokenKind::Spawn
                | TokenKind::Send
                | TokenKind::Ask
                | TokenKind::Defer
                | TokenKind::Type
                | TokenKind::Mod
                | TokenKind::Use
                | TokenKind::Impl
                | TokenKind::Match
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::Not
                | TokenKind::Comptime
                | TokenKind::Trait
                | TokenKind::Actor
                | TokenKind::Iso
                | TokenKind::Val
                | TokenKind::Ref
                | TokenKind::Box
                | TokenKind::Trn
                | TokenKind::Tag
                | TokenKind::Fn
                | TokenKind::Let
                | TokenKind::Mut
                | TokenKind::If
                | TokenKind::Else
                | TokenKind::For
                | TokenKind::In
                | TokenKind::While
                | TokenKind::Enum
                | TokenKind::True
                | TokenKind::False
        )
    }

    // -----------------------------------------------------------------------
    // Expression parsing — precedence climbing
    // -----------------------------------------------------------------------

    /// ```text
    /// expr <- pipe_expr
    /// ```
    fn parse_expr(&mut self) -> Expr {
        let start = self.current_span();
        if self.expr_depth >= MAX_EXPR_DEPTH {
            return self.expression_depth_error(start);
        }
        self.expr_depth += 1;
        // if, for, while, match, spawn, send, ask, defer, concurrent_scope, and supervisor are expressions, handle them here.
        let expr = match self.peek() {
            TokenKind::If => self.parse_if_expr(),
            TokenKind::For => self.parse_for_expr(),
            TokenKind::While => self.parse_while_expr(),
            TokenKind::Match => self.parse_match_expr(),
            TokenKind::Spawn => self.parse_spawn_expr(),
            TokenKind::Send => self.parse_send_expr(),
            TokenKind::Ask => self.parse_ask_expr(),
            TokenKind::Defer => self.parse_defer_expr(),
            TokenKind::ConcurrentScope => self.parse_concurrent_scope_expr(),
            TokenKind::Supervisor => self.parse_supervisor_expr(),
            TokenKind::Fn => self.parse_fn_closure_expr(),
            _ => self.parse_pipe_expr(),
        };
        self.expr_depth -= 1;
        expr
    }

    /// ```text
    /// pipe_expr <- or_expr ('|>' or_expr)*
    /// ```
    /// The pipe operator has the lowest precedence among binary operators.
    fn parse_pipe_expr(&mut self) -> Expr {
        let mut left = self.parse_or_expr();

        while matches!(self.peek(), TokenKind::PipeArrow) {
            self.advance(); // consume '|>'
            let right = self.parse_or_expr();
            let span = merge_spans(&left.span, &right.span);
            left = Spanned::new(
                ExprKind::BinaryOp {
                    op: BinOp::Pipe,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        left
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
            if self.expr_depth >= MAX_EXPR_DEPTH {
                return self.expression_depth_error(start);
            }
            self.expr_depth += 1;
            self.advance(); // consume 'not'
            let operand = self.parse_not_expr();
            self.expr_depth -= 1;
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
    /// cmp_expr <- range_expr (cmp_op range_expr)?
    /// ```
    /// Comparison operators are **non-associative**: `a < b < c` is a parse error.
    fn parse_cmp_expr(&mut self) -> Expr {
        let left = self.parse_range_expr();

        self.skip_newlines();
        if let Some(op) = self.peek_cmp_op() {
            self.advance(); // consume the comparison operator
            let right = self.parse_range_expr();

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

    /// ```text
    /// range_expr <- add_expr ('..' add_expr)?
    /// ```
    /// The range operator `..` has lower precedence than arithmetic but
    /// higher than comparison. It is non-associative.
    fn parse_range_expr(&mut self) -> Expr {
        let left = self.parse_add_expr();

        self.skip_newlines();
        if matches!(self.peek(), TokenKind::DotDot) {
            self.advance(); // consume '..'
            let right = self.parse_add_expr();
            let span = merge_spans(&left.span, &right.span);
            Spanned::new(
                ExprKind::Range {
                    start: Box::new(left),
                    end: Box::new(right),
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
            self.skip_newlines();
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
            self.skip_newlines();
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
            if self.expr_depth >= MAX_EXPR_DEPTH {
                return self.expression_depth_error(start);
            }
            self.expr_depth += 1;
            self.advance(); // consume '-'
            let operand = self.parse_unary_expr();
            self.expr_depth -= 1;
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
                    // Function call or enum/struct constructor with named fields.
                    self.advance(); // consume '('

                    // Check if this is a constructor call with named fields
                    // Pattern: ConstructorName(field: value, field2: value2)
                    let is_named_constructor = matches!(&expr.node, ExprKind::Ident(_))
                        && self.peek_named_constructor_field();

                    if is_named_constructor {
                        // Parse as Construct with named fields
                        let name = match &expr.node {
                            ExprKind::Ident(n) => n.clone(),
                            _ => unreachable!(),
                        };
                        let fields = self.parse_named_constructor_fields();
                        let rparen = self.expect(TokenKind::RParen);
                        let end = match rparen {
                            Ok(tok) => tok.span,
                            Err(_) => self.prev_span(),
                        };
                        let span = merge_spans(&expr.span, &end);
                        expr = Spanned::new(ExprKind::Construct { name, fields }, span);
                    } else {
                        // Regular function call with positional arguments
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
                }
                TokenKind::Dot => {
                    self.advance(); // consume '.'
                    match self.peek().clone() {
                        TokenKind::Ident(name) => {
                            self.advance();
                            let end = self.prev_span();
                            let span = merge_spans(&expr.span, &end);
                            expr = Spanned::new(
                                ExprKind::FieldAccess {
                                    object: Box::new(expr),
                                    field: name,
                                },
                                span,
                            );
                        }
                        TokenKind::IntLit(index) => {
                            self.advance();
                            let end = self.prev_span();
                            let span = merge_spans(&expr.span, &end);
                            expr = Spanned::new(
                                ExprKind::TupleField {
                                    tuple: Box::new(expr),
                                    index: index as usize,
                                },
                                span,
                            );
                        }
                        _ => {
                            self.error_expected(&["field name or tuple index after '.'"]);
                            break;
                        }
                    }
                }
                TokenKind::Bang => {
                    // Actor send operator: `actor_ref ! Message`
                    // The `!` consumes the next identifier as a message name
                    let start_span = expr.span;
                    self.advance(); // consume '!'

                    // Skip newlines to allow multi-line send expressions
                    self.skip_newlines();

                    let message = match self.peek().clone() {
                        TokenKind::Ident(name) => {
                            self.advance();
                            name
                        }
                        _ => {
                            self.error_expected(&["message name after '!'"]);
                            String::from("<error>")
                        }
                    };

                    let end = self.prev_span();
                    expr = Spanned::new(
                        ExprKind::Send {
                            target: Box::new(expr),
                            message,
                        },
                        merge_spans(&start_span, &end),
                    );
                }
                TokenKind::Question => {
                    // Check if this is an actor ask operator: `actor_ref ? Message`
                    // or the try operator: `expr?`
                    // If followed by an identifier, it's an ask expression.
                    let start_span = expr.span;

                    // Peek ahead to see if next token is an identifier
                    if matches!(self.peek_ahead(1), TokenKind::Ident(_)) {
                        // Actor ask: `actor_ref ? Message`
                        self.advance(); // consume '?'

                        // Skip newlines to allow multi-line ask expressions
                        self.skip_newlines();

                        let message = match self.peek().clone() {
                            TokenKind::Ident(name) => {
                                self.advance();
                                name
                            }
                            _ => {
                                self.error_expected(&["message name after '?'"]);
                                String::from("<error>")
                            }
                        };

                        let end = self.prev_span();
                        expr = Spanned::new(
                            ExprKind::Ask {
                                target: Box::new(expr),
                                message,
                            },
                            merge_spans(&start_span, &end),
                        );
                    } else {
                        // Try operator: `expr?`
                        self.advance(); // consume '?'
                        expr = Spanned::new(
                            ExprKind::Try(Box::new(expr)),
                            merge_spans(&start_span, &self.prev_span()),
                        );
                    }
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
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::RParen) {
                break;
            }
            args.push(self.parse_expr());
        }

        args
    }

    /// Check if the current position looks like a named constructor field.
    /// Pattern: Ident ':' expr
    fn peek_named_constructor_field(&self) -> bool {
        // Look ahead: should be Ident followed by ':'
        match self.peek() {
            TokenKind::Ident(_) => {
                matches!(self.peek_ahead(1), TokenKind::Colon)
            }
            // Keywords can also be field names
            TokenKind::Ret | TokenKind::Type | TokenKind::State => {
                matches!(self.peek_ahead(1), TokenKind::Colon)
            }
            _ => false,
        }
    }

    /// Parse named constructor fields: field_name: expr (',' field_name: expr)*
    fn parse_named_constructor_fields(&mut self) -> Vec<(String, Expr)> {
        let mut fields = Vec::new();

        loop {
            // Parse field name
            let field_name = match self.peek().clone() {
                TokenKind::Ident(name) => {
                    self.advance();
                    name
                }
                // Keywords that can be field names
                TokenKind::Ret => {
                    self.advance();
                    "ret".to_string()
                }
                TokenKind::Type => {
                    self.advance();
                    "type".to_string()
                }
                TokenKind::State => {
                    self.advance();
                    "state".to_string()
                }

                _ => break,
            };

            // Expect colon
            if self.expect(TokenKind::Colon).is_err() {
                break;
            }

            // Parse field value
            let value = self.parse_expr();
            fields.push((field_name, value));

            // Check for comma or end
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance();
                self.skip_newlines();
            } else {
                break;
            }
        }

        fields
    }
    /// Check if the current position looks like a record literal field.
    fn peek_record_literal_field(&self) -> bool {
        if !matches!(self.peek(), TokenKind::Colon) {
            return false;
        }

        // Look ahead past potential newlines and indents to find the field name
        let mut offset = 1;
        loop {
            match self.peek_ahead(offset) {
                TokenKind::Newline | TokenKind::Indent => {
                    offset += 1;
                    continue;
                }
                TokenKind::Ident(_) | TokenKind::Ret | TokenKind::Type | TokenKind::State => {
                    let mut colon_offset = offset + 1;
                    loop {
                        match self.peek_ahead(colon_offset) {
                            TokenKind::Newline | TokenKind::Indent => {
                                colon_offset += 1;
                                continue;
                            }
                            TokenKind::Colon => {
                                let mut value_offset = colon_offset + 1;
                                loop {
                                    let val_tok = self.peek_ahead(value_offset);
                                    match val_tok {
                                        TokenKind::Newline | TokenKind::Indent => {
                                            value_offset += 1;
                                            continue;
                                        }
                                        // Expression values that can follow field: in record literal
                                        TokenKind::IntLit(_)
                                        | TokenKind::FloatLit(_)
                                        | TokenKind::StringLit(_)
                                        | TokenKind::True
                                        | TokenKind::False
                                        | TokenKind::Ident(_)
                                        | TokenKind::LParen
                                        | TokenKind::LBracket
                                        | TokenKind::Minus
                                        | TokenKind::Pipe => {
                                            return true;
                                        }
                                        // Statement keywords like 'ret' indicate this is NOT a record field
                                        // because record fields need expressions, not statements
                                        TokenKind::Ret
                                        | TokenKind::Match
                                        | TokenKind::If
                                        | TokenKind::For
                                        | TokenKind::While => {
                                            return false;
                                        }
                                        TokenKind::Dedent => {
                                            return false;
                                        }
                                        _ => {
                                            return false;
                                        }
                                    }
                                }
                            }
                            // If we see '(' after the field name, it's a constructor call like
                            // "Type: Constructor(args)" - this is a typed expression, not record literal
                            TokenKind::LParen => {
                                return false;
                            }
                            TokenKind::Ident(ref name)
                                if name.starts_with(|c: char| c.is_uppercase()) =>
                            {
                                return false;
                            }
                            _ => {
                                return false;
                            }
                        }
                    }
                }
                TokenKind::Dedent => {
                    return false;
                }
                _ => {
                    return false;
                }
            }
        }
    }
    /// Look ahead from a left brace to confirm this is a brace-form record
    /// literal rather than something else (e.g. a stray brace). We require
    /// the shape `LBrace Ident Assign ...` (or `LBrace RBrace` for an empty
    /// record).
    fn peek_brace_record_literal(&self) -> bool {
        if !matches!(self.peek(), TokenKind::LBrace) {
            return false;
        }
        // Look past optional NEWLINE INDENT for the multi-line form:
        //     Position {
        //         line = 1,
        //         col = 2,
        //     }
        let mut offset = 1;
        while matches!(
            self.peek_ahead(offset),
            TokenKind::Newline | TokenKind::Indent
        ) {
            offset += 1;
        }
        // `{ }` empty record
        if matches!(self.peek_ahead(offset), TokenKind::RBrace) {
            return true;
        }
        // `{ ..base, ... }` record-spread form
        if matches!(self.peek_ahead(offset), TokenKind::DotDot) {
            return true;
        }
        // `{ Ident = ...` or `{ Ident: ...` (both syntaxes supported)
        matches!(self.peek_ahead(offset), TokenKind::Ident(_))
            && matches!(
                self.peek_ahead(offset + 1),
                TokenKind::Assign | TokenKind::Colon
            )
    }

    /// Parse a brace-form record literal: `Type { field = value, ... }`.
    /// Used by the self-hosted compiler sources.
    fn parse_brace_record_literal(&mut self, type_name: String, start: Span) -> Expr {
        let _ = self.expect(TokenKind::LBrace);

        let mut fields = Vec::new();
        let mut base: Option<Box<Expr>> = None;
        // Allow optional newlines and a leading INDENT after `{` so that
        // the multi-line indented form parses:
        //     Position {
        //         line = 1,
        //         col = 2,
        //     }
        while matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }
        let consumed_indent = matches!(self.peek(), TokenKind::Indent);
        if consumed_indent {
            self.advance();
        }

        // Optional record-spread base: `Type { ..expr, field = value, ... }`.
        // The spread must appear before any explicit field. The lexer emits
        // `..` as a single DotDot token.
        if matches!(self.peek(), TokenKind::DotDot) {
            self.advance();
            base = Some(Box::new(self.parse_expr()));
            // Tolerate trailing comma / newline / indent before the first field.
            while matches!(
                self.peek(),
                TokenKind::Comma | TokenKind::Newline | TokenKind::Indent
            ) {
                self.advance();
            }
        }

        while !matches!(
            self.peek(),
            TokenKind::RBrace | TokenKind::Dedent | TokenKind::Eof
        ) {
            let field_name = match self.peek().clone() {
                TokenKind::Ident(name) => {
                    self.advance();
                    name
                }
                _ => {
                    self.error_expected(&["field name"]);
                    break;
                }
            };

            // Accept both `field = value` and `field: value` syntax
            if !matches!(self.peek(), TokenKind::Assign | TokenKind::Colon) {
                self.error_expected(&["`=` or `:` after field name"]);
                break;
            }
            self.advance(); // consume '=' or ':'

            let value = self.parse_expr();
            fields.push((field_name, value));

            // Field separator: comma and/or newlines, plus tolerate the
            // synthetic DEDENT that closes the indented block before `}`.
            while matches!(
                self.peek(),
                TokenKind::Comma | TokenKind::Newline | TokenKind::Indent
            ) {
                self.advance();
            }
        }

        if consumed_indent && matches!(self.peek(), TokenKind::Dedent) {
            self.advance();
        }

        let _ = self.expect(TokenKind::RBrace);
        let end = self.prev_span();
        Spanned::new(
            ExprKind::RecordLit {
                type_name,
                base,
                fields,
            },
            merge_spans(&start, &end),
        )
    }

    fn parse_record_literal(&mut self, type_name: String, start: Span) -> Expr {
        let _ = self.expect(TokenKind::Colon); // consume ':' after type name

        // Handle optional newline before indented fields
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Handle indented block of fields
        let consumed_indent = matches!(self.peek(), TokenKind::Indent);
        if consumed_indent {
            self.advance(); // consume INDENT
        }

        let mut fields = Vec::new();

        // Parse at least one field
        while !matches!(
            self.peek(),
            TokenKind::Dedent | TokenKind::Eof | TokenKind::Newline
        ) {
            // Parse field name
            let field_name = match self.peek().clone() {
                TokenKind::Ident(name) => {
                    self.advance();
                    name
                }
                // Keywords that can be field names
                TokenKind::Ret => {
                    self.advance();
                    "ret".to_string()
                }
                TokenKind::Type => {
                    self.advance();
                    "type".to_string()
                }
                TokenKind::State => {
                    self.advance();
                    "state".to_string()
                }
                _ => {
                    self.error_expected(&["field name"]);
                    break;
                }
            };

            // Expect colon after field name
            if self.expect(TokenKind::Colon).is_err() {
                break;
            }

            // Parse field value expression
            let value = self.parse_expr();
            fields.push((field_name, value));

            // Skip newlines between fields
            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }

            // If we see DEDENT or EOF, we're done
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
        }

        // Expect DEDENT only if we consumed INDENT at the start
        if consumed_indent && matches!(self.peek(), TokenKind::Dedent) {
            self.advance();
        }

        let end = self.prev_span();
        Spanned::new(
            ExprKind::RecordLit {
                type_name,
                base: None,
                fields,
            },
            merge_spans(&start, &end),
        )
    }

    fn peek_typed_expr_value(&self) -> bool {
        if !matches!(self.peek(), TokenKind::Colon) {
            return false;
        }

        let mut offset = 1;
        while matches!(
            self.peek_ahead(offset),
            TokenKind::Newline | TokenKind::Indent
        ) {
            offset += 1;
        }

        match self.peek_ahead(offset) {
            TokenKind::Ident(ref name) => name.starts_with(|c: char| c.is_uppercase()),
            TokenKind::IntLit(_)
            | TokenKind::FloatLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::LBrace
            | TokenKind::Minus
            | TokenKind::Pipe
            | TokenKind::Spawn
            | TokenKind::Send
            | TokenKind::Ask
            | TokenKind::Match => true,
            _ => false,
        }
    }

    fn parse_typed_expr_after_colon(&mut self, start: Span, type_expr: TypeExpr) -> Expr {
        let _ = self.expect(TokenKind::Colon);

        while matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let consumed_indent = matches!(self.peek(), TokenKind::Indent);
        if consumed_indent {
            self.advance();
        }

        let value = self.parse_expr();

        if consumed_indent && matches!(self.peek(), TokenKind::Dedent) {
            self.advance();
        }

        let end = self.prev_span();
        Spanned::new(
            ExprKind::TypedExpr {
                type_expr,
                value: Box::new(value),
            },
            merge_spans(&start, &end),
        )
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
            TokenKind::InterpolatedString(parts) => {
                self.advance();
                let ast_parts = self.parse_interpolation_parts(&parts, start);
                let end = self.prev_span();
                Spanned::new(
                    ExprKind::StringInterp { parts: ast_parts },
                    merge_spans(&start, &end),
                )
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
                // Look ahead for type application: Option[Type] or Option[Type]: value
                if matches!(self.peek_ahead(1), TokenKind::LBracket) {
                    // This could be a typed expression with type application
                    // Let the typed expression parser handle it
                    let type_expr_spanned = self.parse_type_expr();
                    let type_expr = type_expr_spanned.node; // Extract inner TypeExpr
                    if matches!(self.peek(), TokenKind::Colon) && self.peek_typed_expr_value() {
                        return self.parse_typed_expr_after_colon(start, type_expr);
                    }
                    // Not a typed expression, return as type expression
                    // Need to wrap in something - use a placeholder for now
                    return Spanned::new(ExprKind::TypedHole(None), start);
                }

                self.advance();
                // Brace-form record literal: TypeName { field = value, field2 = value2 }
                if matches!(self.peek(), TokenKind::LBrace) && self.peek_brace_record_literal() {
                    return self.parse_brace_record_literal(name, start);
                }
                // Check for record literal syntax: TypeName: field: value field2: value2
                // Only check for record literals if the identifier starts with uppercase (type name)
                if matches!(self.peek(), TokenKind::Colon)
                    && name.starts_with(|c: char| c.is_uppercase())
                {
                    // Check if next token after colon is a field name (identifier or keyword) followed by colon
                    if self.peek_record_literal_field() {
                        return self.parse_record_literal(name, start);
                    }
                    if self.peek_typed_expr_value() {
                        return self.parse_typed_expr_after_colon(
                            start,
                            TypeExpr::Named { name, cap: None },
                        );
                    }
                }
                Spanned::new(ExprKind::Ident(name), start)
            }
            // Keywords that can be used as identifiers in expression position
            TokenKind::Ret => {
                self.advance();
                Spanned::new(ExprKind::Ident("ret".to_string()), start)
            }
            TokenKind::Type => {
                self.advance();
                Spanned::new(ExprKind::Ident("type".to_string()), start)
            }
            TokenKind::State => {
                self.advance();
                Spanned::new(ExprKind::Ident("state".to_string()), start)
            }
            TokenKind::Result => {
                self.advance();
                Spanned::new(ExprKind::Ident("result".to_string()), start)
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
                Spanned::new(ExprKind::TypedHole(label), merge_spans(&start, &end))
            }
            TokenKind::LParen => {
                self.advance(); // consume '('
                                // Unit literal: ()
                if matches!(self.peek(), TokenKind::RParen) {
                    self.advance();
                    let end = self.prev_span();
                    return Spanned::new(ExprKind::UnitLit, merge_spans(&start, &end));
                }
                let first = self.parse_expr();
                // If there's a comma, this is a tuple expression.
                if matches!(self.peek(), TokenKind::Comma) {
                    let mut elems = vec![first];
                    while matches!(self.peek(), TokenKind::Comma) {
                        self.advance(); // consume ','
                        if matches!(self.peek(), TokenKind::RParen) {
                            break; // trailing comma
                        }
                        elems.push(self.parse_expr());
                    }
                    let rparen = self.expect(TokenKind::RParen);
                    let end = match rparen {
                        Ok(tok) => tok.span,
                        Err(_) => self.prev_span(),
                    };
                    Spanned::new(ExprKind::Tuple(elems), merge_spans(&start, &end))
                } else {
                    // Parenthesized expression.
                    let rparen = self.expect(TokenKind::RParen);
                    let end = match rparen {
                        Ok(tok) => tok.span,
                        Err(_) => self.prev_span(),
                    };
                    Spanned::new(ExprKind::Paren(Box::new(first)), merge_spans(&start, &end))
                }
            }
            TokenKind::LBracket => {
                self.advance(); // consume '['
                                // Empty list literal: []
                if matches!(self.peek(), TokenKind::RBracket) {
                    self.advance(); // consume ']'
                    let end = self.prev_span();
                    return Spanned::new(ExprKind::ListLit(Vec::new()), merge_spans(&start, &end));
                }
                // Non-empty list literal: [expr, expr, ...]
                let mut elements = vec![self.parse_expr()];
                while matches!(self.peek(), TokenKind::Comma) {
                    self.advance(); // consume ','
                    if matches!(self.peek(), TokenKind::RBracket) {
                        break; // trailing comma
                    }
                    elements.push(self.parse_expr());
                }
                let rbracket = self.expect(TokenKind::RBracket);
                let end = match rbracket {
                    Ok(tok) => tok.span,
                    Err(_) => self.prev_span(),
                };
                Spanned::new(ExprKind::ListLit(elements), merge_spans(&start, &end))
            }

            TokenKind::Pipe => self.parse_closure_expr(),

            _ => {
                self.error_expected(&["expression"]);
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

    fn expression_depth_error(&mut self, span: Span) -> Expr {
        self.errors.push(ParseError::new(
            format!(
                "expression nesting exceeds maximum depth of {}",
                MAX_EXPR_DEPTH
            ),
            span,
            vec![],
            format!("{}", self.peek()),
        ));
        if !self.at_end() {
            self.advance();
        }
        Spanned::new(ExprKind::TypedHole(None), span)
    }

    // -----------------------------------------------------------------------
    // Closure expressions
    // -----------------------------------------------------------------------

    /// Parse a closure (lambda) expression.
    ///
    /// ```text
    /// closure <- '|' params? '|' ('->' type_expr)? ':'? expr
    /// params  <- param (',' param)* ','?
    /// param   <- IDENT (':' type_expr)?
    /// ```
    fn parse_closure_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume opening '|'

        let mut params = Vec::new();

        // Check for zero-parameter closure: `||`
        if !matches!(self.peek(), TokenKind::Pipe) {
            // Parse the first parameter.
            params.push(self.parse_closure_param());

            // Parse remaining comma-separated parameters.
            while matches!(self.peek(), TokenKind::Comma) {
                self.advance(); // consume ','
                                // Allow trailing comma before closing `|`.
                if matches!(self.peek(), TokenKind::Pipe) {
                    break;
                }
                params.push(self.parse_closure_param());
            }
        }

        // Expect the closing `|`.
        if self.expect(TokenKind::Pipe).is_err() {
            // error already recorded
        }

        // Optional return type annotation: `-> Type`
        let return_type = if matches!(self.peek(), TokenKind::Arrow) {
            self.advance(); // consume '->'
            Some(self.parse_type_expr())
        } else {
            None
        };

        // Optional colon before the body expression.
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance(); // consume ':'
        }

        // Parse the body expression.
        let body = self.parse_expr();
        let end = body.span;

        Spanned::new(
            ExprKind::Closure {
                params,
                return_type,
                body: Box::new(body),
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a fn-style closure (lambda) expression.
    ///
    /// ```text
    /// fn_closure <- 'fn' '(' params? ')' '->' type_expr ':' expr
    /// params     <- param (',' param)* ','?
    /// param      <- IDENT (':' type_expr)?
    /// ```
    fn parse_fn_closure_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'fn'

        if self.expect(TokenKind::LParen).is_err() {
            return self.expression_depth_error(start);
        }

        let mut params = Vec::new();

        // Check for zero-parameter closure: `fn()`
        if !matches!(self.peek(), TokenKind::RParen) {
            // Parse the first parameter.
            params.push(self.parse_closure_param());

            // Parse remaining comma-separated parameters.
            while matches!(self.peek(), TokenKind::Comma) {
                self.advance(); // consume ','
                                // Allow trailing comma before closing `)`.
                if matches!(self.peek(), TokenKind::RParen) {
                    break;
                }
                params.push(self.parse_closure_param());
            }
        }

        // Expect the closing `)`.
        if self.expect(TokenKind::RParen).is_err() {
            return self.expression_depth_error(start);
        }

        // Expect '->' followed by return type
        if self.expect(TokenKind::Arrow).is_err() {
            return self.expression_depth_error(start);
        }

        let return_type = Some(self.parse_type_expr());

        // Expect colon before the body expression.
        if self.expect(TokenKind::Colon).is_err() {
            return self.expression_depth_error(start);
        }

        // Handle optional newline for multi-line body
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance(); // consume NEWLINE
        }

        // Handle indented body: consume INDENT, parse expr, expect DEDENT
        if matches!(self.peek(), TokenKind::Indent) {
            self.advance(); // consume INDENT
        }

        // Parse the body expression
        let body = self.parse_expr();

        // If we consumed an INDENT, expect a matching DEDENT
        if matches!(self.peek(), TokenKind::Dedent) {
            self.advance(); // consume DEDENT
        }

        let end = body.span;

        Spanned::new(
            ExprKind::Closure {
                params,
                return_type,
                body: Box::new(body),
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a single closure parameter: `name` or `name: Type`.
    fn parse_closure_param(&mut self) -> ClosureParam {
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

        let type_ann = if matches!(self.peek(), TokenKind::Colon) {
            self.advance(); // consume ':'
            Some(self.parse_type_expr())
        } else {
            None
        };

        let end = self.prev_span();
        ClosureParam {
            name,
            type_ann,
            span: merge_spans(&start, &end),
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

            // Consume optional 'case' keyword (match arm introducer).
            // 'case' is tokenized as an identifier, not a keyword.
            if matches!(self.peek(), TokenKind::Ident(ref s) if s == "case") {
                self.advance(); // consume 'case'
            }

            // Parse pattern.
            let pattern = self.parse_pattern();

            // Parse optional guard: `if <expr>`.
            let guard = if matches!(self.peek(), TokenKind::If) {
                self.advance(); // consume 'if'
                Some(self.parse_expr())
            } else {
                None
            };

            if self.expect(TokenKind::Colon).is_err() {
                // error already recorded
            }

            // Match arm body: either a newline-indented block, or an
            // inline single statement on the same line as the colon
            // (`Pattern: ret expr`).
            let body = if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
                self.parse_block()
            } else {
                let stmt_start = self.current_span();
                let stmt = self.parse_stmt();
                let stmt_span = stmt.span;
                if matches!(self.peek(), TokenKind::Newline) {
                    self.advance();
                }
                Spanned::new(vec![stmt], merge_spans(&stmt_start, &stmt_span))
            };
            let arm_end = body.span;

            arms.push(MatchArm {
                pattern,
                guard,
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

    /// Check if the current token can be used as a pattern binding name.
    /// Returns the binding name string if valid, None otherwise.
    /// This accepts both identifiers and certain keywords that are commonly
    /// used as variable names (like `ret`, `type`, etc.).
    fn peek_pattern_binding_name(&self) -> Option<String> {
        match self.peek() {
            TokenKind::Ident(name) => Some(name.clone()),
            // Keywords that can be used as binding names in patterns
            TokenKind::Ret => Some("ret".to_string()),
            TokenKind::Type => Some("type".to_string()),
            _ => None,
        }
    }

    /// Parse a match arm pattern: integer literal, boolean literal, wildcard `_`,
    /// enum variant name, or pattern alternatives with `|`: `I8 | I16 | I32`.
    fn parse_pattern(&mut self) -> Pattern {
        let first_pattern = self.parse_single_pattern();

        // Check for pattern alternatives with `|`
        if matches!(self.peek(), TokenKind::Pipe) {
            let mut alternatives = vec![first_pattern];

            while matches!(self.peek(), TokenKind::Pipe) {
                self.advance(); // consume '|'
                alternatives.push(self.parse_single_pattern());
            }

            return Pattern::Or(alternatives);
        }

        first_pattern
    }

    /// Parse a single pattern (without `|` alternatives).
    fn parse_single_pattern(&mut self) -> Pattern {
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
            TokenKind::StringLit(s) => {
                self.advance();
                Pattern::StringLit(s)
            }
            TokenKind::Ident(ref name) if name == "_" => {
                self.advance();
                Pattern::Wildcard
            }
            TokenKind::Ident(ref name) if name.starts_with(|c: char| c.is_uppercase()) => {
                // Uppercase identifier: enum variant pattern `Red` or `Some(x)`.
                let name = name.clone();
                self.advance();

                // Check for tuple variant binding: `VariantName(binding)` or `VariantName(b1, b2, ...)`.
                let bindings = if matches!(self.peek(), TokenKind::LParen) {
                    self.advance(); // consume '('
                    let mut names = Vec::new();
                    if let Some(bname) = self.peek_pattern_binding_name() {
                        self.advance();
                        names.push(bname);
                        while matches!(self.peek(), TokenKind::Comma) {
                            self.advance(); // consume ','
                            if let Some(bname2) = self.peek_pattern_binding_name() {
                                self.advance();
                                names.push(bname2);
                            } else {
                                self.error_expected(&["binding name in variant pattern"]);
                                break;
                            }
                        }
                    } else {
                        self.error_expected(&["binding name in variant pattern"]);
                    }
                    if self.expect(TokenKind::RParen).is_err() {
                        // error already recorded
                    }
                    names
                } else {
                    vec![]
                };

                Pattern::Variant {
                    variant: name,
                    bindings,
                }
            }
            TokenKind::Ident(name) => {
                // Lowercase identifier: variable binding pattern.
                self.advance();
                Pattern::Variable(name)
            }
            TokenKind::LParen => {
                // Tuple pattern: (P1, P2, ...)
                self.advance(); // consume '('
                let mut elems = Vec::new();

                // Check for empty tuple pattern ()
                if matches!(self.peek(), TokenKind::RParen) {
                    self.advance(); // consume ')'
                    return Pattern::Tuple(vec![]);
                }

                // Parse first element
                elems.push(self.parse_pattern());

                // Parse remaining elements
                while matches!(self.peek(), TokenKind::Comma) {
                    self.advance(); // consume ','
                    if matches!(self.peek(), TokenKind::RParen) {
                        break; // trailing comma
                    }
                    elems.push(self.parse_pattern());
                }

                if self.expect(TokenKind::RParen).is_err() {
                    // error already recorded
                }

                Pattern::Tuple(elems)
            }
            _ => {
                self.error_expected(&["pattern (integer, true, false, _, or variant)"]);
                // Consume the unrecognized token to make progress.
                if !self.at_end() {
                    self.advance();
                }
                Pattern::Wildcard
            }
        }
    }
    // -----------------------------------------------------------------------
    // Actor expressions: spawn, send, ask
    // -----------------------------------------------------------------------

    /// ```text
    /// spawn_expr <- 'spawn' IDENT
    /// ```
    fn parse_spawn_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'spawn'

        let actor_name = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["actor name"]);
                String::from("<error>")
            }
        };

        let end = self.prev_span();
        Spanned::new(ExprKind::Spawn { actor_name }, merge_spans(&start, &end))
    }

    /// ```text
    /// send_expr <- 'send' expr IDENT
    /// ```
    fn parse_send_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'send'

        let target = self.parse_postfix_expr();

        let message = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["message name"]);
                String::from("<error>")
            }
        };

        let end = self.prev_span();
        Spanned::new(
            ExprKind::Send {
                target: Box::new(target),
                message,
            },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// ask_expr <- 'ask' expr IDENT
    /// ```
    fn parse_ask_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'ask'

        let target = self.parse_postfix_expr();

        let message = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["message name"]);
                String::from("<error>")
            }
        };

        let end = self.prev_span();
        Spanned::new(
            ExprKind::Ask {
                target: Box::new(target),
                message,
            },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// defer_expr <- 'defer' expr
    /// ```
    fn parse_defer_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'defer'

        let body = self.parse_expr();
        let end = body.span;

        Spanned::new(
            ExprKind::Defer {
                body: Box::new(body),
            },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// concurrent_scope_expr <- 'concurrent_scope' ':' NEWLINE block
    /// ```
    fn parse_concurrent_scope_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'concurrent_scope'

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        let body = self.parse_block();
        let end = body.span;

        Spanned::new(
            ExprKind::ConcurrentScope { body },
            merge_spans(&start, &end),
        )
    }

    /// ```text
    /// supervisor_expr <- 'supervisor' strategy_clause? ':' NEWLINE INDENT child_spec+ DEDENT
    /// strategy_clause <- 'strategy' '=' strategy_name (',' 'max_restarts' '=' INT_LIT)?
    /// strategy_name <- 'one_for_one' / 'one_for_all' / 'rest_for_one'
    /// child_spec <- 'child' IDENT (',' 'restart' '=' restart_policy)?
    /// restart_policy <- 'permanent' / 'transient' / 'temporary'
    /// ```
    fn parse_supervisor_expr(&mut self) -> Expr {
        let start = self.current_span();
        self.advance(); // consume 'supervisor'

        // Parse optional strategy clause
        let mut strategy = RestartStrategy::OneForOne; // default
        let mut max_restarts: Option<i64> = None;

        if matches!(self.peek(), TokenKind::Strategy) {
            self.advance(); // consume 'strategy'
            if self.expect(TokenKind::Assign).is_err() {
                // error already recorded
            }

            // Parse strategy name
            strategy = match self.peek() {
                TokenKind::OneForOne => {
                    self.advance();
                    RestartStrategy::OneForOne
                }
                TokenKind::OneForAll => {
                    self.advance();
                    RestartStrategy::OneForAll
                }
                TokenKind::RestForOne => {
                    self.advance();
                    RestartStrategy::RestForOne
                }
                _ => {
                    self.error_expected(&["one_for_one", "one_for_all", "rest_for_one"]);
                    RestartStrategy::OneForOne
                }
            };

            // Parse optional max_restarts
            if matches!(self.peek(), TokenKind::Comma) {
                self.advance(); // consume ','
                if matches!(self.peek(), TokenKind::MaxRestarts) {
                    self.advance(); // consume 'max_restarts'
                    if self.expect(TokenKind::Assign).is_err() {
                        // error already recorded
                    }
                    if let TokenKind::IntLit(n) = self.peek() {
                        max_restarts = Some(*n);
                        self.advance();
                    } else {
                        self.error_expected(&["integer literal"]);
                    }
                }
            }
        }

        if self.expect(TokenKind::Colon).is_err() {
            // error already recorded
        }
        if matches!(self.peek(), TokenKind::Newline) {
            self.advance();
        }

        // Parse child specifications
        let mut children = Vec::new();

        if self.expect(TokenKind::Indent).is_err() {
            let end = self.prev_span();
            return Spanned::new(
                ExprKind::Supervisor {
                    strategy,
                    max_restarts,
                    children,
                },
                merge_spans(&start, &end),
            );
        }

        while !matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }

            match self.peek() {
                TokenKind::Child => {
                    children.push(self.parse_child_spec());
                }
                _ => {
                    self.error_expected(&["'child'"]);
                    self.synchronize();
                }
            }

            if matches!(self.peek(), TokenKind::Newline) {
                self.advance();
            }
        }

        if self.expect(TokenKind::Dedent).is_err() {
            // error already recorded
        }

        let end = self.prev_span();
        Spanned::new(
            ExprKind::Supervisor {
                strategy,
                max_restarts,
                children,
            },
            merge_spans(&start, &end),
        )
    }

    /// Parse a single child specification.
    /// ```text
    /// child_spec <- 'child' IDENT (',' 'restart' '=' restart_policy)?
    /// restart_policy <- 'permanent' / 'transient' / 'temporary'
    /// ```
    fn parse_child_spec(&mut self) -> ChildSpec {
        let start = self.current_span();
        self.advance(); // consume 'child'

        let actor_type = match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.error_expected(&["actor type name"]);
                String::from("<error>")
            }
        };

        let mut restart_policy = RestartPolicy::Permanent; // default

        // Parse optional restart policy
        if matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
            if matches!(self.peek(), TokenKind::Restart) {
                self.advance(); // consume 'restart'
                if self.expect(TokenKind::Assign).is_err() {
                    // error already recorded
                }

                restart_policy = match self.peek() {
                    TokenKind::Permanent => {
                        self.advance();
                        RestartPolicy::Permanent
                    }
                    TokenKind::Transient => {
                        self.advance();
                        RestartPolicy::Transient
                    }
                    TokenKind::Temporary => {
                        self.advance();
                        RestartPolicy::Temporary
                    }
                    _ => {
                        self.error_expected(&["permanent", "transient", "temporary"]);
                        RestartPolicy::Permanent
                    }
                };
            }
        }

        let end = self.prev_span();
        ChildSpec {
            actor_type,
            restart_policy,
            max_restarts: None, // per-child max_restarts not implemented yet
            span: merge_spans(&start, &end),
        }
    }

    // -----------------------------------------------------------------------
    // Type expressions
    // -----------------------------------------------------------------------

    /// ```text
    /// type_param_list <- '[' IDENT (',' IDENT)* ','? ']'
    /// ```
    /// Parses a list of type parameter names in square brackets.
    fn parse_type_param_list(&mut self) -> Vec<TypeParam> {
        let mut params = Vec::new();
        self.advance(); // consume '['

        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                let bounds = self.parse_type_param_bounds();
                params.push(TypeParam { name, bounds });
            }
            _ => {
                self.error_expected(&["type parameter name"]);
            }
        }

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
            if matches!(self.peek(), TokenKind::RBracket) {
                break; // trailing comma
            }
            match self.peek().clone() {
                TokenKind::Ident(name) => {
                    self.advance();
                    let bounds = self.parse_type_param_bounds();
                    params.push(TypeParam { name, bounds });
                }
                _ => {
                    self.error_expected(&["type parameter name"]);
                    break;
                }
            }
        }

        if self.expect(TokenKind::RBracket).is_err() {
            // error already recorded
        }

        params
    }

    /// Parse optional trait bounds on a type parameter: `: TraitName`.
    /// Returns the list of bound names.
    fn parse_type_param_bounds(&mut self) -> Vec<String> {
        let mut bounds = Vec::new();
        if matches!(self.peek(), TokenKind::Colon) {
            self.advance(); // consume ':'
                            // Parse at least one bound.
            match self.peek().clone() {
                TokenKind::Ident(name) => {
                    bounds.push(name);
                    self.advance();
                }
                _ => {
                    self.error_expected(&["trait bound name"]);
                }
            }
            // Parse additional bounds separated by '+'.
            while matches!(self.peek(), TokenKind::Plus) {
                self.advance(); // consume '+'
                match self.peek().clone() {
                    TokenKind::Ident(name) => {
                        bounds.push(name);
                        self.advance();
                    }
                    _ => {
                        self.error_expected(&["trait bound name"]);
                        break;
                    }
                }
            }
        }
        bounds
    }

    /// Parse a list of type parameter names without bounds (for enum declarations).
    /// Returns `Vec<String>` since enums don't support trait bounds.
    fn parse_simple_type_param_list(&mut self) -> Vec<String> {
        let mut params = Vec::new();
        self.advance(); // consume '['

        match self.peek().clone() {
            TokenKind::Ident(name) => {
                params.push(name);
                self.advance();
            }
            _ => {
                self.error_expected(&["type parameter name"]);
            }
        }

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
            if matches!(self.peek(), TokenKind::RBracket) {
                break; // trailing comma
            }
            match self.peek().clone() {
                TokenKind::Ident(name) => {
                    params.push(name);
                    self.advance();
                }
                _ => {
                    self.error_expected(&["type parameter name"]);
                    break;
                }
            }
        }

        if self.expect(TokenKind::RBracket).is_err() {
            // error already recorded
        }

        params
    }

    /// ```text
    /// type_expr <- linear_type / fn_type / IDENT / 'type' / '(' ')'
    /// linear_type <- '!' 'linear' type_expr
    /// fn_type   <- '(' type_list ')' '->' effect_set? type_expr
    /// ```
    fn parse_type_expr(&mut self) -> Spanned<TypeExpr> {
        let start = self.current_span();

        match self.peek().clone() {
            // The `fn` keyword starts a function type: `fn(T, U) -> R`
            TokenKind::Fn => {
                self.advance(); // consume 'fn'
                if self.expect(TokenKind::LParen).is_err() {
                    return Spanned::new(TypeExpr::Unit, start);
                }

                // Parse parameter types
                let mut params = Vec::new();
                if !matches!(self.peek(), TokenKind::RParen) {
                    params.push(self.parse_type_expr());
                    while matches!(self.peek(), TokenKind::Comma) {
                        self.advance(); // consume ','
                        if matches!(self.peek(), TokenKind::RParen) {
                            break; // trailing comma
                        }
                        params.push(self.parse_type_expr());
                    }
                }

                if self.expect(TokenKind::RParen).is_err() {
                    return Spanned::new(TypeExpr::Unit, start);
                }

                // Expect '->' followed by return type
                if self.expect(TokenKind::Arrow).is_err() {
                    return Spanned::new(TypeExpr::Unit, start);
                }

                // Optional effect set: `!{IO}`
                let effects = if matches!(self.peek(), TokenKind::Bang) {
                    Some(self.parse_effect_set())
                } else {
                    None
                };

                let ret = self.parse_type_expr();
                let end = ret.span;
                Spanned::new(
                    TypeExpr::Fn {
                        params,
                        ret: Box::new(ret),
                        effects,
                    },
                    merge_spans(&start, &end),
                )
            }
            // The `type` keyword as a type expression (for comptime type parameters).
            TokenKind::Type => {
                self.advance(); // consume 'type'
                Spanned::new(TypeExpr::Type, start)
            }
            // Linear type: `!linear T`
            TokenKind::Bang => {
                self.advance(); // consume '!'

                // Check if this is `!linear` (linear type) vs `!{...}` (effect set)
                if matches!(self.peek(), TokenKind::Ident(name) if name == "linear") {
                    self.advance(); // consume 'linear'
                    let inner = self.parse_type_expr();
                    let end = inner.span;
                    Spanned::new(TypeExpr::Linear(Box::new(inner)), merge_spans(&start, &end))
                } else {
                    // This is an effect set, but we encountered it in an invalid position.
                    // Effect sets should only appear after `->` in function types.
                    self.error_expected(&["linear (for linear types)"]);
                    // Try to recover by parsing as effect set anyway
                    let effect_set = self.parse_effect_set();
                    let end = self.prev_span();
                    Spanned::new(
                        TypeExpr::Named {
                            name: format!("!{{{}}}", effect_set.effects.join(", ")),
                            cap: None,
                        },
                        merge_spans(&start, &end),
                    )
                }
            }
            TokenKind::Ident(name) => {
                self.advance();
                // Check for generic type arguments: `Name[Arg1, Arg2]`
                if matches!(self.peek(), TokenKind::LBracket) {
                    self.advance(); // consume '['
                    let mut args = Vec::new();
                    if !matches!(self.peek(), TokenKind::RBracket) {
                        args.push(self.parse_type_expr());
                        while matches!(self.peek(), TokenKind::Comma) {
                            self.advance(); // consume ','
                            if matches!(self.peek(), TokenKind::RBracket) {
                                break; // trailing comma
                            }
                            args.push(self.parse_type_expr());
                        }
                    }
                    if self.expect(TokenKind::RBracket).is_err() {
                        // error already recorded
                    }
                    let end = self.prev_span();
                    Spanned::new(
                        TypeExpr::Generic {
                            name,
                            args,
                            cap: None,
                        },
                        merge_spans(&start, &end),
                    )
                } else {
                    Spanned::new(TypeExpr::Named { name, cap: None }, start)
                }
            }
            TokenKind::LParen => {
                // Could be `()` (unit type) or `(T, U) -> R` (function type).
                // Peek ahead to decide: if after '(' we see ')' followed by
                // '->', that's a function type with no params: `() -> R`.
                // If after '(' we see ')' NOT followed by '->', that's unit.
                // If after '(' we see a type, that's a function type.
                self.advance(); // consume '('

                if matches!(self.peek(), TokenKind::RParen) {
                    // Either `()` unit type or `() -> R` nullary function type.
                    self.advance(); // consume ')'
                    if matches!(self.peek(), TokenKind::Arrow) {
                        // Function type: `() -> R`
                        self.advance(); // consume '->'
                        let effects = if matches!(self.peek(), TokenKind::Bang) {
                            Some(self.parse_effect_set())
                        } else {
                            None
                        };
                        let ret = self.parse_type_expr();
                        let end = ret.span;
                        Spanned::new(
                            TypeExpr::Fn {
                                params: vec![],
                                ret: Box::new(ret),
                                effects,
                            },
                            merge_spans(&start, &end),
                        )
                    } else {
                        // Unit type: `()`
                        let end = self.prev_span();
                        Spanned::new(TypeExpr::Unit, merge_spans(&start, &end))
                    }
                } else {
                    // Parse param types: `(T, U, ...)`
                    let mut params = vec![self.parse_type_expr()];
                    while matches!(self.peek(), TokenKind::Comma) {
                        self.advance(); // consume ','
                        if matches!(self.peek(), TokenKind::RParen) {
                            break; // trailing comma
                        }
                        params.push(self.parse_type_expr());
                    }
                    if self.expect(TokenKind::RParen).is_err() {
                        // error already recorded
                    }
                    // If followed by '->', it's a function type.
                    if matches!(self.peek(), TokenKind::Arrow) {
                        self.advance(); // consume '->'
                        let effects = if matches!(self.peek(), TokenKind::Bang) {
                            Some(self.parse_effect_set())
                        } else {
                            None
                        };
                        let ret = self.parse_type_expr();
                        let end = ret.span;
                        Spanned::new(
                            TypeExpr::Fn {
                                params,
                                ret: Box::new(ret),
                                effects,
                            },
                            merge_spans(&start, &end),
                        )
                    } else if params.len() >= 2 {
                        // Tuple type: `(T, U)` without `->`.
                        let end = self.prev_span();
                        Spanned::new(TypeExpr::Tuple(params), merge_spans(&start, &end))
                    } else {
                        // Error: `(T)` without `->` is not valid.
                        self.error_expected(&["`->`"]);
                        let end = self.prev_span();
                        Spanned::new(TypeExpr::Unit, merge_spans(&start, &end))
                    }
                }
            }
            _ => {
                self.error_expected(&["type"]);
                // Return a synthetic unit type so parsing can continue.
                Spanned::new(TypeExpr::Unit, start)
            }
        }
    }

    /// ```text
    /// effect_set <- '!' '{' effect_name (',' effect_name)* ','? '}'
    /// effect_name <- IDENT | 'Throws' '(' IDENT ')'
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

        if let Some(effect) = self.parse_effect_name() {
            effects.push(effect);
        }

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance(); // consume ','
                            // Allow trailing comma.
            if matches!(self.peek(), TokenKind::RBrace) {
                break;
            }
            if let Some(effect) = self.parse_effect_name() {
                effects.push(effect);
            } else {
                break;
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

    fn parse_effect_name(&mut self) -> Option<String> {
        let TokenKind::Ident(name) = self.peek().clone() else {
            self.error_expected(&["effect name"]);
            return None;
        };
        self.advance();

        if name == "Throws" && matches!(self.peek(), TokenKind::LParen) {
            self.advance(); // consume '('
            let inner = match self.peek().clone() {
                TokenKind::Ident(inner) => {
                    self.advance();
                    inner
                }
                _ => {
                    self.error_expected(&["effect type"]);
                    return None;
                }
            };
            if self.expect(TokenKind::RParen).is_err() {
                return None;
            }
            Some(format!("Throws({inner})"))
        } else {
            Some(name)
        }
    }

    // -----------------------------------------------------------------------
    // Interpolated string helpers
    // -----------------------------------------------------------------------

    /// Parse the parts of an interpolated string token into AST nodes.
    ///
    /// Literal parts become `StringInterpPart::Literal`. Expression parts
    /// are lexed and parsed as standalone expressions via a fresh
    /// lexer + parser pipeline.
    fn parse_interpolation_parts(
        &mut self,
        parts: &[InterpolationPart],
        span: Span,
    ) -> Vec<StringInterpPart> {
        let mut ast_parts = Vec::new();
        for part in parts {
            match part {
                InterpolationPart::Literal(s) => {
                    ast_parts.push(StringInterpPart::Literal(s.clone()));
                }
                InterpolationPart::Expr(src) => {
                    // Lex the expression source text.
                    let mut lexer = crate::lexer::Lexer::new(src, self.file_id);
                    let tokens = lexer.tokenize();
                    // Parse a single expression from the tokens.
                    let mut sub_parser = Parser {
                        tokens,
                        pos: 0,
                        errors: Vec::new(),
                        file_id: self.file_id,
                        expr_depth: 0,
                    };
                    sub_parser.record_lex_errors();
                    let expr = sub_parser.parse_expr();
                    // Propagate any errors from the sub-parser.
                    for err in sub_parser.errors {
                        self.errors.push(err);
                    }
                    ast_parts.push(StringInterpPart::Expr(Box::new(expr)));
                }
            }
        }
        // If there are no parts at all, insert an empty literal.
        if ast_parts.is_empty() {
            ast_parts.push(StringInterpPart::Literal(String::new()));
        }
        let _ = span; // used by caller for the Spanned wrapper
        ast_parts
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
