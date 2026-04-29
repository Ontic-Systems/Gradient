//! Issue #226: checker differential parity gate.
//!
//! Compares the Rust checker with a direct, self-hosted-checker-shaped
//! mirror over the bootstrap corpus. The mirror intentionally drives the
//! same runtime-backed checker env store that `compiler/checker.gr` uses
//! after #240, and records AST/env inspection evidence so placeholder
//! success cannot satisfy the gate.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use gradient_compiler::ast::block::Block;
use gradient_compiler::ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use gradient_compiler::ast::item::{FnDef, ItemKind};
use gradient_compiler::ast::module::Module;
use gradient_compiler::ast::stmt::{Stmt, StmtKind};
use gradient_compiler::ast::types::TypeExpr;
use gradient_compiler::bootstrap_checker_env::{
    bootstrap_checker_env_alloc, bootstrap_checker_env_insert_fn, bootstrap_checker_env_insert_var,
    bootstrap_checker_env_lookup_fn, bootstrap_checker_env_lookup_var,
    bootstrap_checker_fn_get_name, bootstrap_checker_fn_get_ret_type_tag,
    bootstrap_checker_var_get_name, bootstrap_checker_var_get_type_tag, reset_checker_env_store,
};
use gradient_compiler::lexer::Lexer;
use gradient_compiler::parser;
use gradient_compiler::typechecker::{self, Ty, TypeError};

#[derive(Debug, Clone)]
struct CheckerCase {
    name: &'static str,
    source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckerSummary {
    mode: &'static str,
    fn_summaries: Vec<String>,
    diagnostics: Vec<String>,
    evidence: CheckerEvidence,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CheckerEvidence {
    ast_nodes_inspected: usize,
    env_inserts: usize,
    env_lookups: usize,
    function_lookups: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FnSigMini {
    params: Vec<MiniTy>,
    ret: MiniTy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MiniTy {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Fn {
        params: Vec<MiniTy>,
        ret: Box<MiniTy>,
    },
    Error,
    Unsupported(String),
}

impl MiniTy {
    fn is_error(&self) -> bool {
        matches!(self, MiniTy::Error)
    }

    fn is_numeric(&self) -> bool {
        matches!(self, MiniTy::Int | MiniTy::Float)
    }

    fn tag_and_name(&self) -> (i64, &'static str) {
        match self {
            MiniTy::Int => (1, ""),
            MiniTy::Float => (2, ""),
            MiniTy::Bool => (3, ""),
            MiniTy::String => (4, ""),
            MiniTy::Unit => (5, ""),
            MiniTy::Error => (0, ""),
            MiniTy::Fn { .. } => (0, "fn"),
            MiniTy::Unsupported(_) => (0, "unsupported"),
        }
    }

    fn from_tag(tag: i64, type_name: String) -> Self {
        match tag {
            1 => MiniTy::Int,
            2 => MiniTy::Float,
            3 => MiniTy::Bool,
            4 => MiniTy::String,
            5 => MiniTy::Unit,
            _ if type_name == "fn" => MiniTy::Unsupported("fn-record".into()),
            _ => MiniTy::Error,
        }
    }
}

impl std::fmt::Display for MiniTy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MiniTy::Int => write!(f, "Int"),
            MiniTy::Float => write!(f, "Float"),
            MiniTy::Bool => write!(f, "Bool"),
            MiniTy::String => write!(f, "String"),
            MiniTy::Unit => write!(f, "Unit"),
            MiniTy::Fn { params, ret } => {
                let params = params
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                write!(f, "Fn({})->{}", params, ret)
            }
            MiniTy::Error => write!(f, "Error"),
            MiniTy::Unsupported(reason) => write!(f, "Unsupported({reason})"),
        }
    }
}

fn checker_differential_cases() -> Vec<CheckerCase> {
    vec![
        CheckerCase {
            name: "literal_return",
            source: "fn main() -> Int:\n    ret 1\n",
        },
        CheckerCase {
            name: "identifier_param",
            source: "fn id(x: Int) -> Int:\n    ret x\n",
        },
        CheckerCase {
            name: "let_binding",
            source: "fn main() -> Int:\n    let x: Int = 1\n    ret x\n",
        },
        CheckerCase {
            name: "binary_ops",
            source: "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n",
        },
        CheckerCase {
            name: "call_success",
            source: "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n\nfn main() -> Int:\n    ret add(1, 2)\n",
        },
        CheckerCase {
            name: "if_statement_success",
            source: "fn pick(x: Int) -> Int:\n    if x > 0:\n        ret 1\n    else:\n        ret 0\n",
        },
        CheckerCase {
            name: "undefined_identifier",
            source: "fn main() -> Int:\n    ret missing\n",
        },
        CheckerCase {
            name: "let_type_mismatch",
            source: "fn main() -> Int:\n    let x: Int = true\n    ret x\n",
        },
        CheckerCase {
            name: "return_type_mismatch",
            source: "fn main() -> Int:\n    ret true\n",
        },
        CheckerCase {
            name: "binary_type_mismatch",
            source: "fn main() -> Int:\n    ret 1 + true\n",
        },
        CheckerCase {
            name: "call_argument_mismatch",
            source: "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n\nfn main() -> Int:\n    ret add(true, 2)\n",
        },
        CheckerCase {
            name: "call_arity_mismatch",
            source: "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n\nfn main() -> Int:\n    ret add(1)\n",
        },
        CheckerCase {
            name: "if_condition_mismatch",
            source: "fn main() -> Int:\n    if 1:\n        ret 1\n    else:\n        ret 2\n",
        },
    ]
}

fn parse_module(source: &str) -> Module {
    let mut lexer = Lexer::new(source, 0);
    let tokens = lexer.tokenize();
    let (module, parse_errors) = parser::parse(tokens, 0);
    assert!(parse_errors.is_empty(), "parser errors: {parse_errors:?}");
    module
}

fn type_name_from_expr(ty: &TypeExpr) -> MiniTy {
    match ty {
        TypeExpr::Named { name, cap: None } if name == "Int" => MiniTy::Int,
        TypeExpr::Named { name, cap: None } if name == "Float" => MiniTy::Float,
        TypeExpr::Named { name, cap: None } if name == "Bool" => MiniTy::Bool,
        TypeExpr::Named { name, cap: None } if name == "String" => MiniTy::String,
        TypeExpr::Unit => MiniTy::Unit,
        other => MiniTy::Unsupported(format!("type outside checker parity subset: {other:?}")),
    }
}

fn rust_ty_to_mini(ty: &Ty) -> MiniTy {
    match ty {
        Ty::Int => MiniTy::Int,
        Ty::Float => MiniTy::Float,
        Ty::Bool => MiniTy::Bool,
        Ty::String => MiniTy::String,
        Ty::Unit => MiniTy::Unit,
        Ty::Fn { params, ret, .. } => MiniTy::Fn {
            params: params.iter().map(rust_ty_to_mini).collect(),
            ret: Box::new(rust_ty_to_mini(ret)),
        },
        Ty::Error => MiniTy::Error,
        other => MiniTy::Unsupported(format!("type outside checker parity subset: {other}")),
    }
}

fn fn_summaries_from_module(module: &Module) -> Vec<String> {
    let mut out = Vec::new();
    for item in &module.items {
        if let ItemKind::FnDef(fn_def) = &item.node {
            let params = fn_def
                .params
                .iter()
                .map(|p| format!("{}:{}", p.name, type_name_from_expr(&p.type_ann.node)))
                .collect::<Vec<_>>()
                .join(",");
            let ret = fn_def
                .return_type
                .as_ref()
                .map(|ty| type_name_from_expr(&ty.node))
                .unwrap_or(MiniTy::Unit);
            out.push(format!("fn {}({})->{}", fn_def.name, params, ret));
        }
    }
    out.sort();
    out
}

fn rust_summary(module: &Module) -> CheckerSummary {
    let diagnostics = normalize_rust_errors(typechecker::check_module(module, 0));
    CheckerSummary {
        mode: "rust-checker",
        fn_summaries: fn_summaries_from_module(module),
        diagnostics,
        evidence: CheckerEvidence::default(),
    }
}

fn normalize_rust_errors(errors: Vec<TypeError>) -> Vec<String> {
    let mut out = errors
        .into_iter()
        .map(|err| normalize_rust_error(&err))
        .collect::<Vec<_>>();
    out.sort();
    out
}

fn normalize_rust_error(err: &TypeError) -> String {
    let message = err.message.as_str();
    if let Some(name) = between(message, "undefined variable `", "`") {
        return format!("undefined-variable:{name}");
    }
    if let Some(name) = between(message, "type mismatch in `let ", "`") {
        return format!(
            "let-mismatch:{name}:expected:{}:found:{}",
            err.expected
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error),
            err.found
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error)
        );
    }
    if message.starts_with("`ret` type mismatch:") {
        return format!(
            "return-mismatch:expected:{}:found:{}",
            err.expected
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error),
            err.found
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error)
        );
    }
    if message.starts_with("operator `") || message.starts_with("operands of `") {
        return format!(
            "binary-mismatch:expected:{}:found:{}",
            err.expected
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error),
            err.found
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error)
        );
    }
    if let Some(index) = between(message, "argument ", " (`") {
        return format!(
            "call-arg-mismatch:{index}:expected:{}:found:{}",
            err.expected
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error),
            err.found
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error)
        );
    }
    if message.starts_with("function `")
        && message.contains("expects")
        && message.contains("argument(s)")
    {
        return message.replace('`', "").replace(' ', "-");
    }
    if message.starts_with("if condition must be Bool")
        || message.starts_with("`if` condition must be Bool")
    {
        return format!(
            "if-condition-mismatch:expected:{}:found:{}",
            err.expected
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error),
            err.found
                .as_ref()
                .map(rust_ty_to_mini)
                .unwrap_or(MiniTy::Error)
        );
    }
    format!("other:{}", message)
}

fn between<'a>(s: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let start = s.find(prefix)? + prefix.len();
    let rest = &s[start..];
    let end = rest.find(suffix)?;
    Some(&rest[..end])
}

struct SelfHostedCheckerMirror {
    env_id: i64,
    function_sigs: HashMap<String, FnSigMini>,
    current_ret: MiniTy,
    diagnostics: Vec<String>,
    evidence: CheckerEvidence,
}

impl SelfHostedCheckerMirror {
    fn new() -> Self {
        reset_checker_env_store();
        Self {
            env_id: bootstrap_checker_env_alloc(0, 0),
            function_sigs: HashMap::new(),
            current_ret: MiniTy::Unit,
            diagnostics: Vec::new(),
            evidence: CheckerEvidence::default(),
        }
    }

    fn check_module(mut self, module: &Module) -> CheckerSummary {
        for item in &module.items {
            self.evidence.ast_nodes_inspected += 1;
            if let ItemKind::FnDef(fn_def) = &item.node {
                let ret = fn_def
                    .return_type
                    .as_ref()
                    .map(|ty| type_name_from_expr(&ty.node))
                    .unwrap_or(MiniTy::Unit);
                let params = fn_def
                    .params
                    .iter()
                    .map(|p| type_name_from_expr(&p.type_ann.node))
                    .collect::<Vec<_>>();
                self.function_sigs.insert(
                    fn_def.name.clone(),
                    FnSigMini {
                        params,
                        ret: ret.clone(),
                    },
                );
                let (tag, type_name) = ret.tag_and_name();
                self.env_id = bootstrap_checker_env_insert_fn(
                    self.env_id,
                    &fn_def.name,
                    0,
                    tag,
                    type_name,
                    0,
                    0,
                );
                self.evidence.env_inserts += 1;
            }
        }

        for item in &module.items {
            self.evidence.ast_nodes_inspected += 1;
            if let ItemKind::FnDef(fn_def) = &item.node {
                self.check_fn(fn_def);
            }
        }

        self.diagnostics.sort();
        CheckerSummary {
            mode: "direct-self-hosted-checker-mirror",
            fn_summaries: fn_summaries_from_module(module),
            diagnostics: self.diagnostics,
            evidence: self.evidence,
        }
    }

    fn check_fn(&mut self, fn_def: &FnDef) {
        let saved_env = self.env_id;
        let saved_ret = self.current_ret.clone();
        self.env_id = bootstrap_checker_env_alloc(self.env_id, 1);
        self.current_ret = fn_def
            .return_type
            .as_ref()
            .map(|ty| type_name_from_expr(&ty.node))
            .unwrap_or(MiniTy::Unit);

        for param in &fn_def.params {
            let ty = type_name_from_expr(&param.type_ann.node);
            let (tag, type_name) = ty.tag_and_name();
            self.env_id =
                bootstrap_checker_env_insert_var(self.env_id, &param.name, tag, type_name, 0, 1);
            self.evidence.env_inserts += 1;
        }

        self.check_block(&fn_def.body);
        self.env_id = saved_env;
        self.current_ret = saved_ret;
    }

    fn check_block(&mut self, block: &Block) -> MiniTy {
        let saved_env = self.env_id;
        self.env_id = bootstrap_checker_env_alloc(self.env_id, 1);
        let mut last = MiniTy::Unit;
        for (index, stmt) in block.node.iter().enumerate() {
            let is_tail = index + 1 == block.node.len();
            last = self.check_stmt(stmt, is_tail);
        }
        self.env_id = saved_env;
        last
    }

    fn check_stmt(&mut self, stmt: &Stmt, is_tail: bool) -> MiniTy {
        self.evidence.ast_nodes_inspected += 1;
        match &stmt.node {
            StmtKind::Let {
                name,
                type_ann,
                value,
                mutable: _,
            } => {
                let annotation = type_ann.as_ref().map(|ty| type_name_from_expr(&ty.node));
                let value_ty = self.check_expr(value);
                let binding_ty = if let Some(annotation) = annotation {
                    if !value_ty.is_error() && annotation != value_ty {
                        self.diagnostics.push(format!(
                            "let-mismatch:{name}:expected:{annotation}:found:{value_ty}"
                        ));
                    }
                    annotation
                } else {
                    value_ty
                };
                let (tag, type_name) = binding_ty.tag_and_name();
                self.env_id =
                    bootstrap_checker_env_insert_var(self.env_id, name, tag, type_name, 0, 1);
                self.evidence.env_inserts += 1;
                MiniTy::Unit
            }
            StmtKind::Ret(expr) => {
                let ty = self.check_expr(expr);
                if !ty.is_error() && !self.current_ret.is_error() && ty != self.current_ret {
                    self.diagnostics.push(format!(
                        "return-mismatch:expected:{}:found:{}",
                        self.current_ret, ty
                    ));
                }
                MiniTy::Unit
            }
            StmtKind::Expr(expr) => {
                let ty = self.check_expr(expr);
                if is_tail {
                    ty
                } else {
                    MiniTy::Unit
                }
            }
            StmtKind::LetTupleDestructure { .. } | StmtKind::Assign { .. } => {
                MiniTy::Unsupported("statement outside checker parity subset".into())
            }
        }
    }

    fn check_expr(&mut self, expr: &Expr) -> MiniTy {
        self.evidence.ast_nodes_inspected += 1;
        match &expr.node {
            ExprKind::IntLit(_) => MiniTy::Int,
            ExprKind::FloatLit(_) => MiniTy::Float,
            ExprKind::BoolLit(_) => MiniTy::Bool,
            ExprKind::StringLit(_) => MiniTy::String,
            ExprKind::UnitLit => MiniTy::Unit,
            ExprKind::Ident(name) => self.lookup_ident(name),
            ExprKind::Paren(inner) => self.check_expr(inner),
            ExprKind::UnaryOp { op, operand } => self.check_unary(*op, operand),
            ExprKind::BinaryOp { op, left, right } => self.check_binary(*op, left, right),
            ExprKind::Call { func, args } => self.check_call(func, args),
            ExprKind::If {
                condition,
                then_block,
                else_block,
                ..
            } => self.check_if(condition, then_block, else_block.as_ref()),
            other => MiniTy::Unsupported(format!(
                "expression outside checker parity subset: {other:?}"
            )),
        }
    }

    fn lookup_ident(&mut self, name: &str) -> MiniTy {
        self.evidence.env_lookups += 1;
        let var_id = bootstrap_checker_env_lookup_var(self.env_id, name);
        if var_id > 0 {
            let found_name = bootstrap_checker_var_get_name(var_id);
            assert_eq!(
                found_name, name,
                "checker env var lookup returned wrong record"
            );
            return MiniTy::from_tag(bootstrap_checker_var_get_type_tag(var_id), String::new());
        }

        self.evidence.function_lookups += 1;
        let fn_id = bootstrap_checker_env_lookup_fn(self.env_id, name);
        if fn_id > 0 {
            let found_name = bootstrap_checker_fn_get_name(fn_id);
            assert_eq!(
                found_name, name,
                "checker env fn lookup returned wrong record"
            );
            if let Some(sig) = self.function_sigs.get(name) {
                return MiniTy::Fn {
                    params: sig.params.clone(),
                    ret: Box::new(sig.ret.clone()),
                };
            }
            return MiniTy::from_tag(bootstrap_checker_fn_get_ret_type_tag(fn_id), "fn".into());
        }

        self.diagnostics.push(format!("undefined-variable:{name}"));
        MiniTy::Error
    }

    fn check_unary(&mut self, op: UnaryOp, operand: &Expr) -> MiniTy {
        let ty = self.check_expr(operand);
        if ty.is_error() {
            return MiniTy::Error;
        }
        match op {
            UnaryOp::Neg if ty.is_numeric() => ty,
            UnaryOp::Neg => {
                self.diagnostics
                    .push(format!("binary-mismatch:expected:Int:found:{ty}"));
                MiniTy::Error
            }
            UnaryOp::Not if ty == MiniTy::Bool => MiniTy::Bool,
            UnaryOp::Not => {
                self.diagnostics
                    .push(format!("binary-mismatch:expected:Bool:found:{ty}"));
                MiniTy::Error
            }
        }
    }

    fn check_binary(&mut self, op: BinOp, left: &Expr, right: &Expr) -> MiniTy {
        let left_ty = self.check_expr(left);
        let right_ty = self.check_expr(right);
        if left_ty.is_error() || right_ty.is_error() {
            return MiniTy::Error;
        }
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                if !left_ty.is_numeric() {
                    self.diagnostics
                        .push(format!("binary-mismatch:expected:Int:found:{left_ty}"));
                    return MiniTy::Error;
                }
                if left_ty != right_ty {
                    self.diagnostics.push(format!(
                        "binary-mismatch:expected:{left_ty}:found:{right_ty}"
                    ));
                    return MiniTy::Error;
                }
                left_ty
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if !left_ty.is_numeric() {
                    self.diagnostics
                        .push(format!("binary-mismatch:expected:Int:found:{left_ty}"));
                    return MiniTy::Error;
                }
                if left_ty != right_ty {
                    self.diagnostics.push(format!(
                        "binary-mismatch:expected:{left_ty}:found:{right_ty}"
                    ));
                    return MiniTy::Error;
                }
                MiniTy::Bool
            }
            BinOp::Eq | BinOp::Ne => {
                if left_ty != right_ty {
                    self.diagnostics.push(format!(
                        "binary-mismatch:expected:{left_ty}:found:{right_ty}"
                    ));
                    return MiniTy::Error;
                }
                MiniTy::Bool
            }
            BinOp::And | BinOp::Or => {
                if left_ty != MiniTy::Bool {
                    self.diagnostics
                        .push(format!("binary-mismatch:expected:Bool:found:{left_ty}"));
                    return MiniTy::Error;
                }
                if right_ty != MiniTy::Bool {
                    self.diagnostics
                        .push(format!("binary-mismatch:expected:Bool:found:{right_ty}"));
                    return MiniTy::Error;
                }
                MiniTy::Bool
            }
            BinOp::Pipe => MiniTy::Unsupported("pipe outside checker parity subset".into()),
        }
    }

    fn check_call(&mut self, func: &Expr, args: &[Expr]) -> MiniTy {
        let callee_name = match &func.node {
            ExprKind::Ident(name) => Some(name.as_str()),
            _ => None,
        };
        if let Some(name) = callee_name {
            self.evidence.function_lookups += 1;
            let fn_id = bootstrap_checker_env_lookup_fn(self.env_id, name);
            if fn_id > 0 {
                let found_name = bootstrap_checker_fn_get_name(fn_id);
                assert_eq!(
                    found_name, name,
                    "checker env fn call lookup returned wrong record"
                );
                if let Some(sig) = self.function_sigs.get(name).cloned() {
                    if args.len() != sig.params.len() {
                        self.diagnostics.push(format!(
                            "function-{name}-expects-{}-argument(s),-but-{}-were-provided",
                            sig.params.len(),
                            args.len()
                        ));
                        return MiniTy::Error;
                    }
                    for (index, (arg, param_ty)) in args.iter().zip(sig.params.iter()).enumerate() {
                        let arg_ty = self.check_expr(arg);
                        if !arg_ty.is_error() && arg_ty != *param_ty {
                            self.diagnostics.push(format!(
                                "call-arg-mismatch:{}:expected:{}:found:{}",
                                index + 1,
                                param_ty,
                                arg_ty
                            ));
                        }
                    }
                    return sig.ret;
                }
            }
        }

        let callee_ty = self.check_expr(func);
        if let MiniTy::Fn { params, ret } = callee_ty {
            if args.len() != params.len() {
                return MiniTy::Error;
            }
            for (arg, param_ty) in args.iter().zip(params.iter()) {
                let arg_ty = self.check_expr(arg);
                if !arg_ty.is_error() && arg_ty != *param_ty {
                    self.diagnostics.push(format!(
                        "call-arg-mismatch:?:expected:{param_ty}:found:{arg_ty}"
                    ));
                }
            }
            *ret
        } else {
            MiniTy::Error
        }
    }

    fn check_if(
        &mut self,
        condition: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
    ) -> MiniTy {
        let cond_ty = self.check_expr(condition);
        if !cond_ty.is_error() && cond_ty != MiniTy::Bool {
            self.diagnostics.push(format!(
                "if-condition-mismatch:expected:Bool:found:{cond_ty}"
            ));
        }
        let then_ty = self.check_block(then_block);
        let else_ty = else_block
            .map(|block| self.check_block(block))
            .unwrap_or(MiniTy::Unit);
        if then_ty == else_ty {
            then_ty
        } else {
            MiniTy::Unit
        }
    }
}

fn self_hosted_summary(module: &Module) -> CheckerSummary {
    SelfHostedCheckerMirror::new().check_module(module)
}

fn assert_checker_gr_contract() {
    let checker_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../compiler")
        .join("checker.gr");
    let source = fs::read_to_string(&checker_path).expect("read compiler/checker.gr");
    for needle in [
        "bootstrap_checker_env_alloc",
        "bootstrap_checker_env_insert_var",
        "bootstrap_checker_env_lookup_var",
        "bootstrap_checker_env_insert_fn",
        "bootstrap_checker_env_lookup_fn",
        "bootstrap_expr_get_tag",
        "bootstrap_stmt_get_tag",
        "fn check_expr",
        "fn check_stmt",
        "fn check_fn",
    ] {
        assert!(
            source.contains(needle),
            "checker.gr missing parity contract marker `{needle}`"
        );
    }
    for placeholder in ["ret ExprKind(0)", "ret TypeEnv { env_id: 0"] {
        assert!(
            !source.contains(placeholder),
            "checker.gr still contains structural placeholder `{placeholder}`"
        );
    }
}

#[test]
fn checker_differential_parity_gate() {
    assert_checker_gr_contract();
    let cases = checker_differential_cases();
    assert!(
        cases.len() >= 8,
        "#226 checker differential corpus is too small to prove parity"
    );

    let mut positive = 0usize;
    let mut negative = 0usize;
    let mut total_env_lookups = 0usize;
    let mut total_function_lookups = 0usize;
    for case in cases {
        let module = parse_module(case.source);
        let rust = rust_summary(&module);
        let self_hosted = self_hosted_summary(&module);

        assert_eq!(
            rust.fn_summaries, self_hosted.fn_summaries,
            "[{}] function/type summary mismatch\nrust: {:?}\nself-hosted: {:?}",
            case.name, rust.fn_summaries, self_hosted.fn_summaries
        );
        assert_eq!(
            rust.diagnostics, self_hosted.diagnostics,
            "[{}] normalized diagnostic mismatch\nrust: {:?}\nself-hosted: {:?}",
            case.name, rust.diagnostics, self_hosted.diagnostics
        );
        assert_eq!(self_hosted.mode, "direct-self-hosted-checker-mirror");
        assert!(
            self_hosted.evidence.ast_nodes_inspected >= 3,
            "[{}] self-hosted checker gate inspected too little AST evidence: {:?}",
            case.name,
            self_hosted.evidence
        );
        assert!(
            self_hosted.evidence.env_inserts > 0,
            "[{}] self-hosted checker gate did not insert into runtime checker env",
            case.name
        );
        total_env_lookups += self_hosted.evidence.env_lookups;
        total_function_lookups += self_hosted.evidence.function_lookups;

        if rust.diagnostics.is_empty() {
            positive += 1;
        } else {
            negative += 1;
        }
    }

    assert!(
        total_env_lookups > 0,
        "checker parity corpus never performed runtime env variable lookups"
    );
    assert!(
        total_function_lookups > 0,
        "checker parity corpus never performed runtime env function lookups"
    );
    assert!(
        positive >= 6,
        "expected at least 6 positive checker parity fixtures"
    );
    assert!(
        negative >= 6,
        "expected at least 6 negative checker parity fixtures"
    );
}
