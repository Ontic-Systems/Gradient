//! Canonical code formatter for the Gradient programming language.
//!
//! This module implements an AST pretty-printer: parse the source, walk the
//! AST, and emit canonically formatted text. This guarantees:
//!
//! - Consistent 4-space indentation
//! - Consistent spacing around operators
//! - Normalized line breaks
//! - One canonical form for every program
//!
//! # Limitations
//!
//! Comments are not preserved. The lexer strips comments from the token
//! stream before the parser sees them, so they do not appear in the AST.
//! A future version could preserve comments by attaching them to AST nodes
//! during lexing.
//!
//! # Usage
//!
//! ```ignore
//! use gradient_compiler::fmt::format_source;
//!
//! let source = "fn   add(a:Int,b:Int)->Int:\n    ret a+b\n";
//! let formatted = format_source(source).unwrap();
//! assert_eq!(formatted, "fn add(a: Int, b: Int) -> Int:\n    ret a + b\n");
//! ```

use crate::ast::expr::{BinOp, Expr, ExprKind, MatchArm, Pattern, StringInterpPart, UnaryOp};
use crate::ast::item::{Annotation, EnumVariant, ExternFnDecl, FnDef, ItemKind, Param};
use crate::ast::module::Module;
use crate::ast::stmt::StmtKind;
use crate::ast::types::{EffectSet, TypeExpr};
use crate::ast::Spanned;
use crate::lexer::Lexer;
use crate::parser::{self, ParseError};

/// Format Gradient source code into its canonical form.
///
/// Parses the input, and if successful, walks the AST to emit formatted
/// output. Returns `Err` with the list of parse errors if the source
/// cannot be parsed.
pub fn format_source(source: &str) -> Result<String, Vec<ParseError>> {
    let mut lexer = Lexer::new(source, 0);
    let tokens = lexer.tokenize();
    let (module, errors) = parser::parse(tokens, 0);
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut f = Formatter::new();
    f.format_module(&module);
    Ok(f.output)
}

// ---------------------------------------------------------------------------
// Formatter state
// ---------------------------------------------------------------------------

/// Internal formatter state that accumulates output text.
struct Formatter {
    /// The accumulated output string.
    output: String,
    /// Current indentation depth (number of 4-space levels).
    indent: usize,
}

impl Formatter {
    fn new() -> Self {
        Self {
            output: String::new(),
            indent: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Output helpers
    // -----------------------------------------------------------------------

    /// Write a string to the output without any indentation prefix.
    fn write(&mut self, s: &str) {
        self.output.push_str(s);
    }

    /// Write an indentation prefix (4 spaces per level) to the output.
    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
    }

    /// Write an indented line: indentation prefix + text + newline.
    fn write_line(&mut self, s: &str) {
        self.write_indent();
        self.output.push_str(s);
        self.output.push('\n');
    }

    /// Write a newline character.
    fn newline(&mut self) {
        self.output.push('\n');
    }

    // -----------------------------------------------------------------------
    // Module formatting
    // -----------------------------------------------------------------------

    /// Format an entire module (the root AST node).
    fn format_module(&mut self, module: &Module) {
        // Module declaration.
        if let Some(ref md) = module.module_decl {
            self.write_line(&format!("mod {}", md.path.join(".")));
        }

        // Use declarations.
        if !module.uses.is_empty() {
            // Blank line after mod decl (if present) before uses.
            if module.module_decl.is_some() {
                self.newline();
            }
            for use_decl in &module.uses {
                let path_str = use_decl.path.join(".");
                if let Some(ref imports) = use_decl.specific_imports {
                    self.write_line(&format!("use {}.{{{}}}", path_str, imports.join(", ")));
                } else {
                    self.write_line(&format!("use {}", path_str));
                }
            }
        }

        // Top-level items, separated by blank lines.
        let need_blank_before_items = module.module_decl.is_some() || !module.uses.is_empty();
        for (i, item) in module.items.iter().enumerate() {
            // Blank line before each item (and between items).
            if (i == 0 && need_blank_before_items) || i > 0 {
                self.newline();
            }
            self.format_item(&item.node);
        }
    }

    // -----------------------------------------------------------------------
    // Item formatting
    // -----------------------------------------------------------------------

    /// Format a top-level item.
    fn format_item(&mut self, item: &ItemKind) {
        match item {
            ItemKind::FnDef(fn_def) => self.format_fn_def(fn_def),
            ItemKind::ExternFn(decl) => self.format_extern_fn(decl),
            ItemKind::Let {
                name,
                type_ann,
                value,
                mutable,
            } => {
                let mut line = String::from("let ");
                if *mutable {
                    line.push_str("mut ");
                }
                line.push_str(name);
                if let Some(ref ta) = type_ann {
                    line.push_str(": ");
                    line.push_str(&self.format_type_expr(&ta.node));
                }
                line.push_str(" = ");
                line.push_str(&self.format_expr(value));
                self.write_line(&line);
            }
            ItemKind::TypeDecl {
                name, type_expr, ..
            } => {
                self.write_line(&format!(
                    "type {} = {}",
                    name,
                    self.format_type_expr(&type_expr.node)
                ));
            }
            ItemKind::CapDecl { allowed_effects } => {
                self.write_line(&format!("@cap({})", allowed_effects.join(", ")));
            }
            ItemKind::EnumDecl {
                name,
                type_params,
                variants,
                ..
            } => {
                self.format_enum_decl(name, type_params, variants);
            }
            ItemKind::LetTupleDestructure {
                names,
                type_ann,
                value,
            } => {
                let mut line = String::from("let (");
                line.push_str(&names.join(", "));
                line.push(')');
                if let Some(ref ta) = type_ann {
                    line.push_str(": ");
                    line.push_str(&self.format_type_expr(&ta.node));
                }
                line.push_str(" = ");
                line.push_str(&self.format_expr(value));
                self.write_line(&line);
            }
            ItemKind::ActorDecl { name, .. } => {
                // Actors are formatted as-is (placeholder until full actor support).
                self.write_line(&format!("actor {}:", name));
            }
            ItemKind::TraitDecl { name, .. } => {
                self.write_line(&format!("trait {}:", name));
            }
            ItemKind::ImplBlock {
                trait_name,
                target_type,
                ..
            } => {
                self.write_line(&format!("impl {} for {}:", trait_name, target_type));
            }
            ItemKind::ModBlock { name, items, .. } => {
                self.write_line(&format!("mod {}:", name));
                self.indent += 1;
                for item in items {
                    self.format_item(&item.node);
                }
                self.indent -= 1;
            }
        }
    }

    /// Format a function definition.
    fn format_fn_def(&mut self, fn_def: &FnDef) {
        // Budget annotation.
        if let Some(ref budget) = fn_def.budget {
            self.write_indent();
            let mut parts = Vec::new();
            if let Some(ref cpu) = budget.cpu {
                parts.push(format!("cpu: {}", cpu));
            }
            if let Some(ref mem) = budget.mem {
                parts.push(format!("mem: {}", mem));
            }
            self.write(&format!("@budget({})\n", parts.join(", ")));
        }

        // Annotations.
        for ann in &fn_def.annotations {
            self.format_annotation(ann);
        }

        // Signature line.
        let type_params_str = if fn_def.type_params.is_empty() {
            String::new()
        } else {
            let tp_strs: Vec<String> = fn_def
                .type_params
                .iter()
                .map(|tp| {
                    if tp.bounds.is_empty() {
                        tp.name.clone()
                    } else {
                        format!("{}: {}", tp.name, tp.bounds.join(" + "))
                    }
                })
                .collect();
            format!("[{}]", tp_strs.join(", "))
        };
        self.write_indent();
        self.write(&format!(
            "fn {}{}({}){}:\n",
            fn_def.name,
            type_params_str,
            self.format_params(&fn_def.params),
            self.format_return_clause(&fn_def.effects, &fn_def.return_type),
        ));

        // Body.
        self.indent += 1;
        self.format_block(&fn_def.body);
        self.indent -= 1;
    }

    /// Format an extern function declaration (no body).
    fn format_extern_fn(&mut self, decl: &ExternFnDecl) {
        // Annotations.
        for ann in &decl.annotations {
            self.format_annotation(ann);
        }

        self.write_line(&format!(
            "fn {}({}){}",
            decl.name,
            self.format_params(&decl.params),
            self.format_return_clause(&decl.effects, &decl.return_type),
        ));
    }

    /// Format an annotation.
    fn format_annotation(&mut self, ann: &Annotation) {
        if ann.args.is_empty() {
            self.write_line(&format!("@{}", ann.name));
        } else {
            let args: Vec<String> = ann.args.iter().map(|a| self.format_expr(a)).collect();
            self.write_line(&format!("@{}({})", ann.name, args.join(", ")));
        }
    }

    /// Format an enum declaration.
    fn format_enum_decl(&mut self, name: &str, type_params: &[String], variants: &[EnumVariant]) {
        let mut parts: Vec<String> = Vec::new();
        for v in variants {
            if let Some(ref fields) = v.fields {
                if fields.is_empty() {
                    parts.push(v.name.clone());
                } else {
                    // Format fields - handle both named and anonymous
                    let field_strs: Vec<String> = fields
                        .iter()
                        .map(|f| match f {
                            crate::ast::item::VariantField::Named { name, type_expr } => {
                                format!("{}: {}", name, self.format_type_expr(&type_expr.node))
                            }
                            crate::ast::item::VariantField::Anonymous(type_expr) => {
                                self.format_type_expr(&type_expr.node)
                            }
                        })
                        .collect();
                    parts.push(format!("{}({})", v.name, field_strs.join(", ")));
                }
            } else {
                parts.push(v.name.clone());
            }
        }
        let tp_str = if type_params.is_empty() {
            String::new()
        } else {
            format!("[{}]", type_params.join(", "))
        };
        self.write_line(&format!("type {}{} = {}", name, tp_str, parts.join(" | ")));
    }

    // -----------------------------------------------------------------------
    // Signature helpers
    // -----------------------------------------------------------------------

    /// Format a parameter list (without the surrounding parentheses).
    fn format_params(&self, params: &[Param]) -> String {
        params
            .iter()
            .map(|p| format!("{}: {}", p.name, self.format_type_expr(&p.type_ann.node)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Format a return clause: ` -> Type` or ` -> !{Effects} Type` or empty.
    fn format_return_clause(
        &self,
        effects: &Option<EffectSet>,
        return_type: &Option<Spanned<TypeExpr>>,
    ) -> String {
        match return_type {
            None => String::new(),
            Some(ref rt) => {
                let effects_str = match effects {
                    Some(ref es) => format!("!{{{}}} ", es.effects.join(", ")),
                    None => String::new(),
                };
                format!(" -> {}{}", effects_str, self.format_type_expr(&rt.node))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Type expression formatting
    // -----------------------------------------------------------------------

    /// Format a type expression.
    fn format_type_expr(&self, ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Named { name, cap } => {
                let cap_str = match cap {
                    Some(c) => format!(" {}", c),
                    None => String::new(),
                };
                format!("{}{}", name, cap_str)
            }
            TypeExpr::Unit => "()".to_string(),
            TypeExpr::Fn {
                params,
                ret,
                effects,
            } => {
                let param_strs: Vec<String> = params
                    .iter()
                    .map(|p| self.format_type_expr(&p.node))
                    .collect();
                let eff_str = match effects {
                    Some(eff) if !eff.effects.is_empty() => {
                        format!(" !{{{}}}", eff.effects.join(", "))
                    }
                    _ => String::new(),
                };
                format!(
                    "({}) ->{} {}",
                    param_strs.join(", "),
                    eff_str,
                    self.format_type_expr(&ret.node)
                )
            }
            TypeExpr::Generic { name, args, cap } => {
                let arg_strs: Vec<String> = args
                    .iter()
                    .map(|a| self.format_type_expr(&a.node))
                    .collect();
                let cap_str = match cap {
                    Some(c) => format!(" {}", c),
                    None => String::new(),
                };
                format!("{}[{}]{}", name, arg_strs.join(", "), cap_str)
            }
            TypeExpr::Tuple(elems) => {
                let elem_strs: Vec<String> = elems
                    .iter()
                    .map(|e| self.format_type_expr(&e.node))
                    .collect();
                format!("({})", elem_strs.join(", "))
            }
            TypeExpr::Linear(inner) => {
                format!("@linear {}", self.format_type_expr(&inner.node))
            }
            TypeExpr::Type => "type".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Block and statement formatting
    // -----------------------------------------------------------------------

    /// Format a block (a sequence of statements at the current indentation).
    fn format_block(&mut self, block: &Spanned<Vec<crate::ast::stmt::Stmt>>) {
        for stmt in &block.node {
            self.format_stmt(&stmt.node);
        }
    }

    /// Format a single statement.
    fn format_stmt(&mut self, stmt: &StmtKind) {
        match stmt {
            StmtKind::Let {
                name,
                type_ann,
                value,
                mutable,
            } => {
                let mut line = String::from("let ");
                if *mutable {
                    line.push_str("mut ");
                }
                line.push_str(name);
                if let Some(ref ta) = type_ann {
                    line.push_str(": ");
                    line.push_str(&self.format_type_expr(&ta.node));
                }
                line.push_str(" = ");
                line.push_str(&self.format_expr(value));
                self.write_line(&line);
            }
            StmtKind::LetTupleDestructure {
                names,
                type_ann,
                value,
            } => {
                let mut line = String::from("let (");
                line.push_str(&names.join(", "));
                line.push(')');
                if let Some(ref ta) = type_ann {
                    line.push_str(": ");
                    line.push_str(&self.format_type_expr(&ta.node));
                }
                line.push_str(" = ");
                line.push_str(&self.format_expr(value));
                self.write_line(&line);
            }
            StmtKind::Assign { name, value } => {
                self.write_line(&format!("{} = {}", name, self.format_expr(value)));
            }
            StmtKind::Ret(expr) => {
                self.write_line(&format!("ret {}", self.format_expr(expr)));
            }
            StmtKind::Expr(expr) => {
                // Expression-level if/while/for/match need special handling
                // because they contain blocks that must be indented.
                match &expr.node {
                    ExprKind::If { .. } => {
                        self.format_if_stmt(expr);
                    }
                    ExprKind::While { .. } => {
                        self.format_while_stmt(expr);
                    }
                    ExprKind::For { .. } => {
                        self.format_for_stmt(expr);
                    }
                    ExprKind::Match { .. } => {
                        self.format_match_stmt(expr);
                    }
                    _ => {
                        self.write_line(&self.format_expr(expr));
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Block-level control flow formatting (as statements)
    // -----------------------------------------------------------------------

    /// Format an `if` expression used as a statement (with indented blocks).
    fn format_if_stmt(&mut self, expr: &Expr) {
        if let ExprKind::If {
            condition,
            then_block,
            else_ifs,
            else_block,
        } = &expr.node
        {
            self.write_indent();
            self.write(&format!("if {}:\n", self.format_expr(condition)));
            self.indent += 1;
            self.format_block(then_block);
            self.indent -= 1;

            for (ei_cond, ei_block) in else_ifs {
                self.write_indent();
                self.write(&format!("else if {}:\n", self.format_expr(ei_cond)));
                self.indent += 1;
                self.format_block(ei_block);
                self.indent -= 1;
            }

            if let Some(ref eb) = else_block {
                self.write_indent();
                self.write("else:\n");
                self.indent += 1;
                self.format_block(eb);
                self.indent -= 1;
            }
        }
    }

    /// Format a `while` expression used as a statement.
    fn format_while_stmt(&mut self, expr: &Expr) {
        if let ExprKind::While { condition, body } = &expr.node {
            self.write_indent();
            self.write(&format!("while {}:\n", self.format_expr(condition)));
            self.indent += 1;
            self.format_block(body);
            self.indent -= 1;
        }
    }

    /// Format a `for` expression used as a statement.
    fn format_for_stmt(&mut self, expr: &Expr) {
        if let ExprKind::For { var, iter, body } = &expr.node {
            self.write_indent();
            self.write(&format!("for {} in {}:\n", var, self.format_expr(iter)));
            self.indent += 1;
            self.format_block(body);
            self.indent -= 1;
        }
    }

    /// Format a `match` expression used as a statement.
    fn format_match_stmt(&mut self, expr: &Expr) {
        if let ExprKind::Match { scrutinee, arms } = &expr.node {
            self.write_indent();
            self.write(&format!("match {}:\n", self.format_expr(scrutinee)));
            self.indent += 1;
            for arm in arms {
                self.format_match_arm(arm);
            }
            self.indent -= 1;
        }
    }

    /// Format a single match arm.
    fn format_match_arm(&mut self, arm: &MatchArm) {
        self.write_indent();
        let pat_str = self.format_pattern(&arm.pattern);
        if let Some(ref guard) = arm.guard {
            self.write(&format!("{} if {}:\n", pat_str, self.format_expr(guard)));
        } else {
            self.write(&format!("{}:\n", pat_str));
        }
        self.indent += 1;
        self.format_block(&arm.body);
        self.indent -= 1;
    }

    // -----------------------------------------------------------------------
    // Pattern formatting
    // -----------------------------------------------------------------------

    /// Format a match pattern.
    fn format_pattern(&self, pattern: &Pattern) -> String {
        match pattern {
            Pattern::IntLit(n) => format!("{}", n),
            Pattern::BoolLit(b) => format!("{}", b),
            Pattern::Wildcard => "_".to_string(),
            Pattern::Variant { variant, bindings } => {
                if bindings.is_empty() {
                    variant.clone()
                } else {
                    format!("{}({})", variant, bindings.join(", "))
                }
            }
            Pattern::Tuple(pats) => {
                let pat_strs: Vec<String> = pats.iter().map(|p| self.format_pattern(p)).collect();
                format!("({})", pat_strs.join(", "))
            }
            Pattern::StringLit(s) => format!("\"{}\"", s),
            Pattern::Variable(name) => name.clone(),
            Pattern::Or(alternatives) => {
                let alt_strs: Vec<String> = alternatives
                    .iter()
                    .map(|p| self.format_pattern(p))
                    .collect();
                alt_strs.join(" | ")
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expression formatting (produces a string, not written to output)
    // -----------------------------------------------------------------------

    /// Format an expression to a string.
    ///
    /// Most expressions are inline (produce no newlines). Control-flow
    /// expressions (if, while, for, match) can appear in expression position
    /// but when used as statements are handled by the `format_*_stmt` methods
    /// above. In inline contexts we produce a compact single-line form.
    fn format_expr(&self, expr: &Expr) -> String {
        match &expr.node {
            ExprKind::IntLit(n) => format!("{}", n),
            ExprKind::FloatLit(n) => format_float(*n),
            ExprKind::StringLit(s) => format!("\"{}\"", escape_string(s)),
            ExprKind::StringInterp { parts } => {
                let mut s = String::from("f\"");
                for part in parts {
                    match part {
                        StringInterpPart::Literal(lit) => s.push_str(&escape_string(lit)),
                        StringInterpPart::Expr(expr) => {
                            s.push('{');
                            s.push_str(&self.format_expr(expr));
                            s.push('}');
                        }
                    }
                }
                s.push('"');
                s
            }
            ExprKind::BoolLit(b) => format!("{}", b),
            ExprKind::UnitLit => "()".to_string(),
            ExprKind::Ident(name) => name.clone(),
            ExprKind::TypedHole(label) => {
                if let Some(ref l) = label {
                    format!("?{}", l)
                } else {
                    "?".to_string()
                }
            }
            ExprKind::BinaryOp { op, left, right } => {
                let left_str = self.format_expr_with_parens(left, Some(*op), true);
                let right_str = self.format_expr_with_parens(right, Some(*op), false);
                format!("{} {} {}", left_str, format_binop(*op), right_str)
            }
            ExprKind::UnaryOp { op, operand } => {
                let operand_str = self.format_expr(operand);
                match op {
                    UnaryOp::Neg => {
                        // Add parens around binary ops to avoid ambiguity.
                        if matches!(operand.node, ExprKind::BinaryOp { .. }) {
                            format!("-({})", operand_str)
                        } else {
                            format!("-{}", operand_str)
                        }
                    }
                    UnaryOp::Not => {
                        format!("not {}", operand_str)
                    }
                }
            }
            ExprKind::Call { func, args } => {
                let func_str = self.format_expr(func);
                let arg_strs: Vec<String> = args.iter().map(|a| self.format_expr(a)).collect();
                format!("{}({})", func_str, arg_strs.join(", "))
            }
            ExprKind::FieldAccess { object, field } => {
                format!("{}.{}", self.format_expr(object), field)
            }
            ExprKind::Paren(inner) => {
                format!("({})", self.format_expr(inner))
            }
            ExprKind::ListLit(elems) => {
                let elem_strs: Vec<String> = elems.iter().map(|e| self.format_expr(e)).collect();
                format!("[{}]", elem_strs.join(", "))
            }
            ExprKind::Tuple(elems) => {
                let elem_strs: Vec<String> = elems.iter().map(|e| self.format_expr(e)).collect();
                format!("({})", elem_strs.join(", "))
            }
            ExprKind::RecordLit { type_name, fields } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(name, val)| format!("{}: {}", name, self.format_expr(val)))
                    .collect();
                format!("{}: {}", type_name, field_strs.join(" "))
            }
            ExprKind::Construct { name, fields } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(fname, val)| format!("{}: {}", fname, self.format_expr(val)))
                    .collect();
                format!("{}({})", name, field_strs.join(", "))
            }
            ExprKind::TupleField { tuple, index } => {
                format!("{}.{}", self.format_expr(tuple), index)
            }
            // Control-flow expressions in inline position: produce compact form.
            // These should be rare in expression position; the statement formatters
            // handle them with proper indentation when used as statements.
            ExprKind::If {
                condition,
                then_block,
                else_ifs,
                else_block,
            } => {
                // In expression position, if/else can still appear.
                // We produce a multi-line string if needed. But since format_expr
                // returns a String for inline use, this is a simplified representation.
                let mut s = format!("if {}", self.format_expr(condition));
                if !then_block.node.is_empty() {
                    // Just show the last expression in the block for inline.
                    s.push_str(": ...");
                }
                for (cond, _) in else_ifs {
                    s.push_str(&format!(" else if {}: ...", self.format_expr(cond)));
                }
                if else_block.is_some() {
                    s.push_str(" else: ...");
                }
                s
            }
            ExprKind::While { condition, .. } => {
                format!("while {}: ...", self.format_expr(condition))
            }
            ExprKind::For { var, iter, .. } => {
                format!("for {} in {}: ...", var, self.format_expr(iter))
            }
            ExprKind::Match { scrutinee, .. } => {
                format!("match {}: ...", self.format_expr(scrutinee))
            }
            ExprKind::Spawn { actor_name, .. } => {
                format!("spawn {}", actor_name)
            }
            ExprKind::Send {
                target, message, ..
            } => {
                format!("send {} <- {}", self.format_expr(target), message)
            }
            ExprKind::Ask {
                target, message, ..
            } => {
                format!("ask {} <- {}", self.format_expr(target), message)
            }
            ExprKind::Range { start, end } => {
                format!("{}..{}", self.format_expr(start), self.format_expr(end))
            }
            ExprKind::Try(inner) => {
                format!("{}?", self.format_expr(inner))
            }
            ExprKind::Closure {
                params,
                return_type,
                body,
            } => {
                let param_strs: Vec<String> = params
                    .iter()
                    .map(|p| {
                        if let Some(ref ty) = p.type_ann {
                            format!("{}: {}", p.name, self.format_type_expr(&ty.node))
                        } else {
                            p.name.clone()
                        }
                    })
                    .collect();
                let ret_str = if let Some(ref rt) = return_type {
                    format!(" -> {}", self.format_type_expr(&rt.node))
                } else {
                    String::new()
                };
                format!(
                    "|{}|{} {}",
                    param_strs.join(", "),
                    ret_str,
                    self.format_expr(body)
                )
            }
            ExprKind::Defer { body } => {
                format!("defer {}", self.format_expr(body))
            }
            ExprKind::ConcurrentScope { .. } => "concurrent_scope { ... }".to_string(),
            ExprKind::Supervisor { .. } => "supervisor { ... }".to_string(),
        }
    }

    /// Format an expression, adding parentheses if needed for operator
    /// precedence clarity.
    fn format_expr_with_parens(
        &self,
        expr: &Expr,
        parent_op: Option<BinOp>,
        is_left: bool,
    ) -> String {
        // If the child is a binary op with lower precedence than the parent,
        // wrap in parens. Also handle right-associativity for same-precedence.
        if let ExprKind::BinaryOp { op: child_op, .. } = &expr.node {
            if let Some(parent) = parent_op {
                let child_prec = precedence(*child_op);
                let parent_prec = precedence(parent);

                if child_prec < parent_prec || (child_prec == parent_prec && !is_left) {
                    return format!("({})", self.format_expr(expr));
                }
            }
        }
        self.format_expr(expr)
    }
}

// ---------------------------------------------------------------------------
// Operator formatting helpers
// ---------------------------------------------------------------------------

/// Format a binary operator as its source text.
fn format_binop(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Pipe => "|>",
    }
}

/// Return the precedence level for an operator (higher = tighter binding).
fn precedence(op: BinOp) -> u8 {
    match op {
        BinOp::Pipe => 0,
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 3,
        BinOp::Add | BinOp::Sub => 4,
        BinOp::Mul | BinOp::Div | BinOp::Mod => 5,
    }
}

/// Format a float value, ensuring it has a decimal point.
fn format_float(n: f64) -> String {
    let s = format!("{}", n);
    if s.contains('.') {
        s
    } else {
        format!("{}.0", s)
    }
}

/// Escape a string for output in a Gradient string literal.
fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: format source and unwrap, panicking on parse errors.
    fn fmt(source: &str) -> String {
        format_source(source).unwrap_or_else(|errs| {
            panic!(
                "Parse errors while formatting:\n{}",
                errs.iter()
                    .map(|e| format!("  {}", e))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })
    }

    #[test]
    fn format_simple_function() {
        let source = "fn   add(a:Int,b:Int)->Int:\n    ret a+b\n";
        let result = fmt(source);
        assert_eq!(result, "fn add(a: Int, b: Int) -> Int:\n    ret a + b\n");
    }

    #[test]
    fn format_if_else() {
        let source = concat!(
            "fn check(x:Int)->String:\n",
            "    if x>0:\n",
            "        ret \"positive\"\n",
            "    else:\n",
            "        ret \"non-positive\"\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "fn check(x: Int) -> String:\n",
            "    if x > 0:\n",
            "        ret \"positive\"\n",
            "    else:\n",
            "        ret \"non-positive\"\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_while_loop() {
        let source = concat!(
            "fn countdown(n:Int)->!{IO} ():\n",
            "    let mut i:Int=n\n",
            "    while i>0:\n",
            "        print_int(i)\n",
            "        i=i-1\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "fn countdown(n: Int) -> !{IO} ():\n",
            "    let mut i: Int = n\n",
            "    while i > 0:\n",
            "        print_int(i)\n",
            "        i = i - 1\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_match_expression() {
        let source = concat!(
            "fn describe(n:Int)->String:\n",
            "    match n:\n",
            "        0:\n",
            "            ret \"zero\"\n",
            "        1:\n",
            "            ret \"one\"\n",
            "        _:\n",
            "            ret \"other\"\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "fn describe(n: Int) -> String:\n",
            "    match n:\n",
            "        0:\n",
            "            ret \"zero\"\n",
            "        1:\n",
            "            ret \"one\"\n",
            "        _:\n",
            "            ret \"other\"\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_enum_declaration() {
        let source = "type Direction=North|South|East|West\n";
        let result = fmt(source);
        assert_eq!(result, "type Direction = North | South | East | West\n");
    }

    #[test]
    fn format_multi_function_program() {
        let source = concat!(
            "mod pure_functions\n",
            "\n",
            "fn double(x:Int)->Int:\n",
            "    ret x*2\n",
            "\n",
            "fn add_one(x:Int)->Int:\n",
            "    ret x+1\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "mod pure_functions\n",
            "\n",
            "fn double(x: Int) -> Int:\n",
            "    ret x * 2\n",
            "\n",
            "fn add_one(x: Int) -> Int:\n",
            "    ret x + 1\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_idempotent() {
        let source = concat!(
            "mod test\n",
            "\n",
            "fn fib(n: Int) -> Int:\n",
            "    if n <= 0:\n",
            "        ret 0\n",
            "    else if n == 1:\n",
            "        ret 1\n",
            "    else:\n",
            "        let a: Int = fib(n - 1)\n",
            "        let b: Int = fib(n - 2)\n",
            "        ret a + b\n",
            "\n",
            "fn main() -> !{IO} ():\n",
            "    let result: Int = fib(10)\n",
            "    print_int(result)\n",
        );
        let first = fmt(source);
        let second = fmt(&first);
        assert_eq!(first, second, "Formatter is not idempotent!");
    }

    #[test]
    fn format_module_with_uses() {
        let source = concat!(
            "mod main\n",
            "\n",
            "use helper\n",
            "\n",
            "fn compute() -> Int:\n",
            "    let sum: Int = helper.add(3, 4)\n",
            "    ret sum\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "mod main\n",
            "\n",
            "use helper\n",
            "\n",
            "fn compute() -> Int:\n",
            "    let sum: Int = helper.add(3, 4)\n",
            "    ret sum\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_type_alias() {
        let source = concat!(
            "mod type_alias\n",
            "\n",
            "type Count=Int\n",
            "type Name=String\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "mod type_alias\n",
            "\n",
            "type Count = Int\n",
            "\n",
            "type Name = String\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_cap_declaration() {
        let source = "@cap(IO, Net)\nfn main() -> ():\n    ret ()\n";
        let result = fmt(source);
        assert!(result.contains("@cap(IO, Net)"));
    }

    #[test]
    fn format_for_loop() {
        let source = concat!(
            "fn process(xs: Int) -> !{IO} ():\n",
            "    for x in range(10):\n",
            "        print_int(x)\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "fn process(xs: Int) -> !{IO} ():\n",
            "    for x in range(10):\n",
            "        print_int(x)\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_nested_if() {
        let source = concat!(
            "fn classify(n:Int)->String:\n",
            "    if n>0:\n",
            "        if n>100:\n",
            "            ret \"large\"\n",
            "        else:\n",
            "            ret \"small\"\n",
            "    else:\n",
            "        ret \"negative\"\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "fn classify(n: Int) -> String:\n",
            "    if n > 0:\n",
            "        if n > 100:\n",
            "            ret \"large\"\n",
            "        else:\n",
            "            ret \"small\"\n",
            "    else:\n",
            "        ret \"negative\"\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_operator_spacing() {
        let source = "fn calc(a:Int,b:Int)->Int:\n    ret a*b+a/b-a%b\n";
        let result = fmt(source);
        assert!(result.contains("a * b + a / b - a % b"));
    }

    #[test]
    fn format_boolean_operators() {
        let source = "fn test(a:Bool,b:Bool)->Bool:\n    ret a and b or not a\n";
        let result = fmt(source);
        assert!(result.contains("a and b or not a"));
    }

    #[test]
    fn format_string_escapes() {
        let source = "fn greet() -> String:\n    ret \"hello\\nworld\"\n";
        let result = fmt(source);
        assert!(result.contains("\"hello\\nworld\""));
    }

    #[test]
    fn format_extern_fn() {
        let source = "fn print_int(n: Int) -> !{IO} ()\n";
        let result = fmt(source);
        assert_eq!(result, "fn print_int(n: Int) -> !{IO} ()\n");
    }

    #[test]
    fn format_mutable_let() {
        let source = "fn test() -> Int:\n    let mut x: Int = 0\n    x = 42\n    ret x\n";
        let result = fmt(source);
        assert!(result.contains("let mut x: Int = 0"));
        assert!(result.contains("x = 42"));
    }

    #[test]
    fn format_enum_with_fields() {
        let source = "type Option = None | Some(Int)\n";
        let result = fmt(source);
        assert_eq!(result, "type Option = None | Some(Int)\n");
    }

    #[test]
    fn format_match_variant_patterns() {
        let source = concat!(
            "fn unwrap(o: Option) -> Int:\n",
            "    match o:\n",
            "        Some(x):\n",
            "            ret x\n",
            "        None:\n",
            "            ret 0\n",
        );
        let result = fmt(source);
        let expected = concat!(
            "fn unwrap(o: Option) -> Int:\n",
            "    match o:\n",
            "        Some(x):\n",
            "            ret x\n",
            "        None:\n",
            "            ret 0\n",
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn format_use_with_specific_imports() {
        let source = "use std.io.{read, write}\n\nfn main() -> ():\n    ret ()\n";
        let result = fmt(source);
        assert!(result.contains("use std.io.{read, write}"));
    }

    #[test]
    fn format_parse_error_returns_err() {
        let source = "fn broken(\n";
        let result = format_source(source);
        assert!(result.is_err());
    }

    #[test]
    fn format_unary_neg() {
        let source = "fn neg(x: Int) -> Int:\n    ret -x\n";
        let result = fmt(source);
        assert!(result.contains("ret -x"));
    }

    #[test]
    fn format_try_operator() {
        let source = "fn f() -> Result:\n    ret get()?\n";
        let result = fmt(source);
        assert!(
            result.contains("get()?"),
            "expected get()? in output, got: {}",
            result
        );
    }

    #[test]
    fn format_field_access() {
        let source = "fn get(obj: Foo) -> Int:\n    ret obj.field\n";
        let result = fmt(source);
        assert!(result.contains("obj.field"));
    }

    #[test]
    fn format_paren_expr() {
        let source = "fn calc(a: Int, b: Int) -> Int:\n    ret (a + b) * 2\n";
        let result = fmt(source);
        assert!(result.contains("(a + b) * 2"));
    }

    #[test]
    fn idempotent_on_all_e2e_files() {
        // Read and format every .gr file in the compiler/tests directory.
        // Verify formatting is idempotent: format(format(x)) == format(x).
        let test_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        if !test_dir.exists() {
            return; // Skip if tests dir is missing.
        }
        for entry in std::fs::read_dir(&test_dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("gr") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let first = match format_source(&source) {
                Ok(f) => f,
                Err(_) => continue, // Skip files that don't parse.
            };
            let second = format_source(&first).unwrap_or_else(|errs| {
                panic!(
                    "File {:?}: formatted output does not re-parse:\n{}",
                    path,
                    errs.iter()
                        .map(|e| format!("  {}", e))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            });
            assert_eq!(
                first, second,
                "File {:?}: formatter is not idempotent.\nFirst:\n{}\nSecond:\n{}",
                path, first, second
            );
        }
    }
}
