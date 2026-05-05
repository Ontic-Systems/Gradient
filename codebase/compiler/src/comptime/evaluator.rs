use crate::ast::{
    block::Block,
    expr::{BinOp, Expr, ExprKind, UnaryOp},
    item::FnDef,
    span::Spanned,
    stmt::StmtKind,
};
use std::collections::HashMap;

use super::value::ComptimeValue;

/// Errors that can occur during compile-time evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeError {
    /// Recursion depth limit exceeded.
    RecursionLimit { name: String },
    /// Unknown variable referenced.
    UnknownVariable { name: String },
    /// Expression cannot be evaluated at compile time.
    NotComptime { expr: String },
    /// Unsupported operation attempted.
    UnsupportedOperation { op: String },
    /// Type error during evaluation.
    TypeError { expected: String, got: String },
    /// Division by zero.
    DivisionByZero,
    /// Invalid pattern in match expression.
    InvalidPattern,
}

impl std::fmt::Display for ComptimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComptimeError::RecursionLimit { name } => {
                write!(f, "Recursion limit exceeded in function '{}'", name)
            }
            ComptimeError::UnknownVariable { name } => {
                write!(f, "Unknown variable '{}'", name)
            }
            ComptimeError::NotComptime { expr } => {
                write!(f, "Expression not evaluable at compile time: {}", expr)
            }
            ComptimeError::UnsupportedOperation { op } => {
                write!(f, "Unsupported operation: {}", op)
            }
            ComptimeError::TypeError { expected, got } => {
                write!(f, "Type error: expected {}, got {}", expected, got)
            }
            ComptimeError::DivisionByZero => {
                write!(f, "Division by zero")
            }
            ComptimeError::InvalidPattern => {
                write!(f, "Invalid pattern in match expression")
            }
        }
    }
}

impl std::error::Error for ComptimeError {}

/// Compile-time expression evaluator.
///
/// This struct provides an environment for evaluating Gradient expressions
/// at compile time. It maintains a stack of variable bindings and tracks
/// recursion depth to prevent infinite loops.
#[derive(Debug)]
pub struct ComptimeEvaluator {
    /// Variable environment: maps variable names to their values.
    env: HashMap<String, ComptimeValue>,
    /// Call stack for function calls being evaluated.
    call_stack: Vec<String>,
    /// Current recursion depth.
    call_depth: usize,
    /// Maximum allowed recursion depth.
    max_depth: usize,
    /// Available function definitions for comptime calls.
    functions: HashMap<String, FnDef>,
}

impl ComptimeEvaluator {
    /// Creates a new evaluator with default settings.
    ///
    /// The default maximum recursion depth is 1000.
    pub fn new() -> Self {
        Self {
            env: HashMap::new(),
            call_stack: Vec::new(),
            call_depth: 0,
            max_depth: 1000,
            functions: HashMap::new(),
        }
    }

    /// Creates a new evaluator with a custom maximum recursion depth.
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            env: HashMap::new(),
            call_stack: Vec::new(),
            call_depth: 0,
            max_depth,
            functions: HashMap::new(),
        }
    }

    /// Registers a function definition for comptime calls.
    pub fn register_function(&mut self, fn_def: FnDef) {
        self.functions.insert(fn_def.name.clone(), fn_def);
    }

    /// Registers multiple function definitions.
    pub fn register_functions(&mut self, fn_defs: Vec<FnDef>) {
        for fn_def in fn_defs {
            self.register_function(fn_def);
        }
    }

    /// Looks up a registered function by name.
    pub fn get_function(&self, name: &str) -> Option<&FnDef> {
        self.functions.get(name)
    }

    /// Evaluates a function at compile time.
    ///
    /// # Arguments
    ///
    /// * `fn_def` - The function definition to evaluate
    /// * `comptime_args` - Map of argument names to their compile-time values
    ///
    /// # Returns
    ///
    /// The result of evaluating the function body, or an error if evaluation fails.
    pub fn eval_fn(
        &mut self,
        fn_def: &FnDef,
        comptime_args: HashMap<String, ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        // Check recursion limit
        if self.call_depth >= self.max_depth {
            return Err(ComptimeError::RecursionLimit {
                name: fn_def.name.clone(),
            });
        }

        // Increment depth and track this call
        self.call_depth += 1;
        self.call_stack.push(fn_def.name.clone());

        // Save current environment
        let saved_env = self.env.clone();

        // Set up new environment with function arguments
        for (name, value) in comptime_args {
            self.env.insert(name, value);
        }

        // Evaluate the function body
        let result = self.eval_block(&fn_def.body);

        // Restore environment and depth
        self.env = saved_env;
        self.call_stack.pop();
        self.call_depth -= 1;

        result
    }

    /// Evaluates a block expression.
    ///
    /// A block is a sequence of statements. The value of the block is the
    /// value of the final expression, or unit if there is no final expression.
    pub fn eval_block(&mut self, block: &Block) -> Result<ComptimeValue, ComptimeError> {
        self.eval_block_with_env(block, true)
    }

    /// Evaluates a block with optional environment scoping.
    fn eval_block_with_env(
        &mut self,
        block: &Block,
        new_scope: bool,
    ) -> Result<ComptimeValue, ComptimeError> {
        // Save environment if creating a new scope
        let saved_env = if new_scope {
            Some(self.env.clone())
        } else {
            None
        };

        let stmts = &block.node;
        let mut last_value = ComptimeValue::Unit;

        for (i, stmt) in stmts.iter().enumerate() {
            let is_last = i == stmts.len() - 1;
            let value = self.eval_stmt(stmt, is_last)?;
            if is_last {
                last_value = value;
            }
        }

        // Restore environment if we created a new scope
        if let Some(saved) = saved_env {
            self.env = saved;
        }

        Ok(last_value)
    }

    /// Evaluates a single statement.
    fn eval_stmt(
        &mut self,
        stmt: &Spanned<StmtKind>,
        is_last: bool,
    ) -> Result<ComptimeValue, ComptimeError> {
        match &stmt.node {
            StmtKind::Let {
                name,
                value,
                mutable: _,
                ..
            } => {
                let val = self.eval_expr(value)?;
                self.env.insert(name.clone(), val);
                Ok(ComptimeValue::Unit)
            }
            StmtKind::LetTupleDestructure { .. } => {
                // Tuple destructuring not yet supported in comptime
                Err(ComptimeError::NotComptime {
                    expr: "tuple destructure".to_string(),
                })
            }
            StmtKind::Assign { .. } => {
                // Assignment not supported in comptime (pure evaluation)
                Err(ComptimeError::NotComptime {
                    expr: "assignment".to_string(),
                })
            }
            StmtKind::Ret(expr) => self.eval_expr(expr),
            StmtKind::Expr(expr) => {
                if is_last {
                    self.eval_expr(expr)
                } else {
                    self.eval_expr(expr)?;
                    Ok(ComptimeValue::Unit)
                }
            }
        }
    }

    /// Evaluates an expression at compile time.
    ///
    /// This is the core method for compile-time evaluation. It matches on
    /// the expression kind and recursively evaluates sub-expressions.
    pub fn eval_expr(&mut self, expr: &Expr) -> Result<ComptimeValue, ComptimeError> {
        match &expr.node {
            ExprKind::IntLit(n) => Ok(ComptimeValue::Int(*n)),
            ExprKind::FloatLit(n) => Ok(ComptimeValue::Float(*n)),
            ExprKind::BoolLit(b) => Ok(ComptimeValue::Bool(*b)),
            ExprKind::StringLit(s) => Ok(ComptimeValue::String(s.clone())),
            ExprKind::UnitLit => Ok(ComptimeValue::Unit),
            ExprKind::Ident(name) => self.eval_variable(name),
            ExprKind::BinaryOp { op, left, right } => {
                self.eval_binary_op(*op, left.as_ref(), right.as_ref())
            }
            ExprKind::UnaryOp { op, operand } => self.eval_unary_op(*op, operand.as_ref()),
            ExprKind::If {
                condition,
                then_block,
                else_ifs,
                else_block,
            } => self.eval_if(
                condition.as_ref(),
                then_block,
                else_ifs,
                else_block.as_ref(),
            ),
            ExprKind::Call { func, args } => self.eval_call(func.as_ref(), args),
            ExprKind::FieldAccess { object, field } => {
                self.eval_field_access(object.as_ref(), field)
            }
            ExprKind::RecordLit {
                type_name,
                base,
                fields,
            } => self.eval_record_lit(type_name, base.as_deref(), fields),
            ExprKind::Construct { name, fields } => self.eval_construct(name, fields),
            ExprKind::Tuple(items) => self.eval_tuple(items),
            ExprKind::TupleField { tuple, index } => self.eval_tuple_field(tuple.as_ref(), *index),
            ExprKind::ListLit(items) => self.eval_list_lit(items),
            ExprKind::Paren(expr) => self.eval_expr(expr.as_ref()),
            ExprKind::Match { scrutinee, arms } => self.eval_match(scrutinee.as_ref(), arms),
            _ => Err(ComptimeError::NotComptime {
                expr: format!("{:?}", expr.node),
            }),
        }
    }

    /// Evaluates a variable reference.
    fn eval_variable(&self, name: &str) -> Result<ComptimeValue, ComptimeError> {
        self.env
            .get(name)
            .cloned()
            .ok_or_else(|| ComptimeError::UnknownVariable {
                name: name.to_string(),
            })
    }

    /// Evaluates a field access on a compile-time record or enum variant value.
    fn eval_field_access(
        &mut self,
        object: &Expr,
        field: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        let object_val = self.eval_expr(object)?;
        match object_val {
            ComptimeValue::Record { fields, .. } => {
                fields
                    .get(field)
                    .cloned()
                    .ok_or_else(|| ComptimeError::UnknownVariable {
                        name: field.to_string(),
                    })
            }
            ComptimeValue::Variant { fields, .. } => fields
                .into_iter()
                .find_map(|(name, value)| if name == field { Some(value) } else { None })
                .ok_or_else(|| ComptimeError::UnknownVariable {
                    name: field.to_string(),
                }),
            other => Err(ComptimeError::TypeError {
                expected: "record or variant".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    /// Evaluates a record literal, including same-type record spread.
    fn eval_record_lit(
        &mut self,
        type_name: &str,
        base: Option<&Expr>,
        fields: &[(String, Expr)],
    ) -> Result<ComptimeValue, ComptimeError> {
        let mut values = match base {
            Some(base_expr) => match self.eval_expr(base_expr)? {
                ComptimeValue::Record {
                    type_name: base_type,
                    fields,
                } if base_type == type_name => fields,
                ComptimeValue::Record { type_name: got, .. } => {
                    return Err(ComptimeError::TypeError {
                        expected: type_name.to_string(),
                        got,
                    })
                }
                other => {
                    return Err(ComptimeError::TypeError {
                        expected: "record".to_string(),
                        got: other.type_name().to_string(),
                    })
                }
            },
            None => HashMap::new(),
        };

        for (name, expr) in fields {
            values.insert(name.clone(), self.eval_expr(expr)?);
        }

        Ok(ComptimeValue::Record {
            type_name: type_name.to_string(),
            fields: values,
        })
    }

    /// Evaluates an enum variant/constructor with named payload fields.
    fn eval_construct(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
    ) -> Result<ComptimeValue, ComptimeError> {
        let mut values = Vec::with_capacity(fields.len());
        for (field, expr) in fields {
            values.push((field.clone(), self.eval_expr(expr)?));
        }

        Ok(ComptimeValue::Variant {
            name: name.to_string(),
            fields: values,
        })
    }

    /// Evaluates a tuple literal by preserving element order.
    fn eval_tuple(&mut self, items: &[Expr]) -> Result<ComptimeValue, ComptimeError> {
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            values.push(self.eval_expr(item)?);
        }
        Ok(ComptimeValue::Tuple(values))
    }

    /// Evaluates tuple field access (`pair.0`) on compile-time tuple values.
    fn eval_tuple_field(
        &mut self,
        tuple: &Expr,
        index: usize,
    ) -> Result<ComptimeValue, ComptimeError> {
        match self.eval_expr(tuple)? {
            ComptimeValue::Tuple(values) => {
                values
                    .get(index)
                    .cloned()
                    .ok_or_else(|| ComptimeError::UnknownVariable {
                        name: index.to_string(),
                    })
            }
            other => Err(ComptimeError::TypeError {
                expected: "tuple".to_string(),
                got: other.type_name().to_string(),
            }),
        }
    }

    /// Evaluates a list literal by preserving element order.
    fn eval_list_lit(&mut self, items: &[Expr]) -> Result<ComptimeValue, ComptimeError> {
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            values.push(self.eval_expr(item)?);
        }
        Ok(ComptimeValue::List(values))
    }

    /// Evaluates a binary operation.
    fn eval_binary_op(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<ComptimeValue, ComptimeError> {
        let left_val = self.eval_expr(left)?;
        let right_val = self.eval_expr(right)?;

        match op {
            BinOp::Add => self.eval_add(left_val, right_val),
            BinOp::Sub => self.eval_sub(left_val, right_val),
            BinOp::Mul => self.eval_mul(left_val, right_val),
            BinOp::Div => self.eval_div(left_val, right_val),
            BinOp::Mod => self.eval_mod(left_val, right_val),
            BinOp::Eq => self.eval_eq(left_val, right_val),
            BinOp::Ne => self.eval_ne(left_val, right_val),
            BinOp::Lt => self.eval_lt(left_val, right_val),
            BinOp::Le => self.eval_le(left_val, right_val),
            BinOp::Gt => self.eval_gt(left_val, right_val),
            BinOp::Ge => self.eval_ge(left_val, right_val),
            BinOp::And => self.eval_and(left_val, right_val),
            BinOp::Or => self.eval_or(left_val, right_val),
            BinOp::Pipe => {
                // Pipe operator: left |> right means right(left)
                // For comptime, we need the right side to be a function
                Err(ComptimeError::NotComptime {
                    expr: "pipe operator".to_string(),
                })
            }
        }
    }

    fn eval_add(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => a
                .checked_add(b)
                .map(ComptimeValue::Int)
                .ok_or(ComptimeError::UnsupportedOperation {
                    op: "integer overflow in addition".to_string(),
                }),
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => Ok(ComptimeValue::Float(a + b)),
            (ComptimeValue::String(a), ComptimeValue::String(b)) => {
                Ok(ComptimeValue::String(a + &b))
            }
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int, Float, or String".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_sub(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => a
                .checked_sub(b)
                .map(ComptimeValue::Int)
                .ok_or(ComptimeError::UnsupportedOperation {
                    op: "integer overflow in subtraction".to_string(),
                }),
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => Ok(ComptimeValue::Float(a - b)),
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int or Float".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_mul(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => a
                .checked_mul(b)
                .map(ComptimeValue::Int)
                .ok_or(ComptimeError::UnsupportedOperation {
                    op: "integer overflow in multiplication".to_string(),
                }),
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => Ok(ComptimeValue::Float(a * b)),
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int or Float".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_div(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => {
                if b == 0 {
                    Err(ComptimeError::DivisionByZero)
                } else {
                    Ok(ComptimeValue::Int(a / b))
                }
            }
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => {
                if b == 0.0 {
                    Err(ComptimeError::DivisionByZero)
                } else {
                    Ok(ComptimeValue::Float(a / b))
                }
            }
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int or Float".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_mod(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => {
                if b == 0 {
                    Err(ComptimeError::DivisionByZero)
                } else {
                    Ok(ComptimeValue::Int(a % b))
                }
            }
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_eq(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (&left, &right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => Ok(ComptimeValue::Bool(a == b)),
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => Ok(ComptimeValue::Bool(a == b)),
            (ComptimeValue::Bool(a), ComptimeValue::Bool(b)) => Ok(ComptimeValue::Bool(a == b)),
            (ComptimeValue::String(a), ComptimeValue::String(b)) => Ok(ComptimeValue::Bool(a == b)),
            (ComptimeValue::Tuple(a), ComptimeValue::Tuple(b)) => Ok(ComptimeValue::Bool(a == b)),
            (ComptimeValue::List(a), ComptimeValue::List(b)) => Ok(ComptimeValue::Bool(a == b)),
            (ComptimeValue::Unit, ComptimeValue::Unit) => Ok(ComptimeValue::Bool(true)),
            _ => Ok(ComptimeValue::Bool(false)),
        }
    }

    fn eval_ne(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        let eq_result = self.eval_eq(left, right)?;
        match eq_result {
            ComptimeValue::Bool(b) => Ok(ComptimeValue::Bool(!b)),
            _ => Err(ComptimeError::TypeError {
                expected: "Bool".to_string(),
                got: eq_result.type_name().to_string(),
            }),
        }
    }

    fn eval_lt(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => Ok(ComptimeValue::Bool(a < b)),
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => Ok(ComptimeValue::Bool(a < b)),
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int or Float".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_le(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => Ok(ComptimeValue::Bool(a <= b)),
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => Ok(ComptimeValue::Bool(a <= b)),
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int or Float".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_gt(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => Ok(ComptimeValue::Bool(a > b)),
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => Ok(ComptimeValue::Bool(a > b)),
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int or Float".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_ge(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Int(a), ComptimeValue::Int(b)) => Ok(ComptimeValue::Bool(a >= b)),
            (ComptimeValue::Float(a), ComptimeValue::Float(b)) => Ok(ComptimeValue::Bool(a >= b)),
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Int or Float".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_and(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Bool(a), ComptimeValue::Bool(b)) => Ok(ComptimeValue::Bool(a && b)),
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Bool".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    fn eval_or(
        &self,
        left: ComptimeValue,
        right: ComptimeValue,
    ) -> Result<ComptimeValue, ComptimeError> {
        match (left, right) {
            (ComptimeValue::Bool(a), ComptimeValue::Bool(b)) => Ok(ComptimeValue::Bool(a || b)),
            (l, r) => Err(ComptimeError::TypeError {
                expected: "Bool".to_string(),
                got: format!("{} and {}", l.type_name(), r.type_name()),
            }),
        }
    }

    /// Evaluates a unary operation.
    fn eval_unary_op(
        &mut self,
        op: UnaryOp,
        operand: &Expr,
    ) -> Result<ComptimeValue, ComptimeError> {
        let val = self.eval_expr(operand)?;

        match op {
            UnaryOp::Neg => match val {
                ComptimeValue::Int(n) => Ok(ComptimeValue::Int(-n)),
                ComptimeValue::Float(n) => Ok(ComptimeValue::Float(-n)),
                _ => Err(ComptimeError::TypeError {
                    expected: "Int or Float".to_string(),
                    got: val.type_name().to_string(),
                }),
            },
            UnaryOp::Not => match val {
                ComptimeValue::Bool(b) => Ok(ComptimeValue::Bool(!b)),
                _ => Err(ComptimeError::TypeError {
                    expected: "Bool".to_string(),
                    got: val.type_name().to_string(),
                }),
            },
        }
    }

    /// Evaluates an if expression.
    fn eval_if(
        &mut self,
        condition: &Expr,
        then_block: &Block,
        else_ifs: &[(Expr, Block)],
        else_block: Option<&Block>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let cond_val = self.eval_expr(condition)?;

        match cond_val {
            ComptimeValue::Bool(true) => self.eval_block(then_block),
            ComptimeValue::Bool(false) => {
                // Check else if branches
                for (cond, block) in else_ifs {
                    let elif_cond = self.eval_expr(cond)?;
                    match elif_cond {
                        ComptimeValue::Bool(true) => return self.eval_block(block),
                        ComptimeValue::Bool(false) => continue,
                        _ => {
                            return Err(ComptimeError::TypeError {
                                expected: "Bool".to_string(),
                                got: elif_cond.type_name().to_string(),
                            })
                        }
                    }
                }

                // Check else block
                if let Some(block) = else_block {
                    self.eval_block(block)
                } else {
                    Ok(ComptimeValue::Unit)
                }
            }
            _ => Err(ComptimeError::TypeError {
                expected: "Bool".to_string(),
                got: cond_val.type_name().to_string(),
            }),
        }
    }

    /// Evaluates a function call.
    fn eval_call(&mut self, func: &Expr, args: &[Expr]) -> Result<ComptimeValue, ComptimeError> {
        // For now, only support calling identifiers (named functions)
        let func_name = match &func.node {
            ExprKind::Ident(name) => name.clone(),
            _ => {
                return Err(ComptimeError::NotComptime {
                    expr: "complex function call".to_string(),
                })
            }
        };

        // Look up the function definition
        let fn_def =
            self.functions
                .get(&func_name)
                .cloned()
                .ok_or_else(|| ComptimeError::NotComptime {
                    expr: format!("call to unknown function '{}'", func_name),
                })?;

        // Evaluate arguments
        let mut arg_values: Vec<ComptimeValue> = Vec::new();
        for arg in args {
            arg_values.push(self.eval_expr(arg)?);
        }

        // Map arguments to parameter names
        let mut comptime_args = HashMap::new();
        for (i, param) in fn_def.params.iter().enumerate() {
            if i < arg_values.len() {
                comptime_args.insert(param.name.clone(), arg_values[i].clone());
            }
        }

        // Evaluate the function
        self.eval_fn(&fn_def, comptime_args)
    }

    /// Evaluates a match expression.
    fn eval_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[crate::ast::expr::MatchArm],
    ) -> Result<ComptimeValue, ComptimeError> {
        let scrut_val = self.eval_expr(scrutinee)?;

        for arm in arms {
            if self.match_pattern(&scrut_val, &arm.pattern)? {
                // Check guard if present
                if let Some(ref guard) = arm.guard {
                    let guard_val = self.eval_expr(guard)?;
                    match guard_val {
                        ComptimeValue::Bool(true) => return self.eval_block(&arm.body),
                        ComptimeValue::Bool(false) => continue,
                        _ => {
                            return Err(ComptimeError::TypeError {
                                expected: "Bool".to_string(),
                                got: guard_val.type_name().to_string(),
                            })
                        }
                    }
                } else {
                    return self.eval_block(&arm.body);
                }
            }
        }

        // No pattern matched - this is an error at comptime
        Err(ComptimeError::InvalidPattern)
    }

    /// Matches a value against a pattern.
    fn match_pattern(
        &mut self,
        value: &ComptimeValue,
        pattern: &crate::ast::expr::Pattern,
    ) -> Result<bool, ComptimeError> {
        use crate::ast::expr::Pattern;

        match (value, pattern) {
            (ComptimeValue::Int(n), Pattern::IntLit(m)) => Ok(*n == *m),
            (ComptimeValue::Bool(b), Pattern::BoolLit(c)) => Ok(*b == *c),
            (ComptimeValue::String(s), Pattern::StringLit(t)) => Ok(s == t),
            (_, Pattern::Wildcard) => Ok(true),
            (ComptimeValue::Variant { name, fields }, Pattern::Variant { variant, bindings }) => {
                if name != variant || fields.len() != bindings.len() {
                    return Ok(false);
                }

                for (binding, (_, field_value)) in bindings.iter().zip(fields.iter()) {
                    self.env.insert(binding.clone(), field_value.clone());
                }
                Ok(true)
            }
            (_, Pattern::Variable(name)) => {
                self.env.insert(name.clone(), value.clone());
                Ok(true)
            }
            (_, Pattern::Or(patterns)) => {
                for alternative in patterns {
                    let saved_env = self.env.clone();
                    if self.match_pattern(value, alternative)? {
                        return Ok(true);
                    }
                    self.env = saved_env;
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }
}

impl Default for ComptimeEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::block::Block;
    use crate::ast::expr::{ExprKind, Pattern};
    use crate::ast::span::{Span, Spanned};
    use crate::ast::stmt::{Stmt, StmtKind};

    fn make_expr(kind: ExprKind) -> Expr {
        Spanned::new(kind, Span::empty())
    }

    fn make_stmt(kind: StmtKind) -> Stmt {
        Spanned::new(kind, Span::empty())
    }

    fn make_block(stmts: Vec<Stmt>) -> Block {
        Spanned::new(stmts, Span::empty())
    }

    #[test]
    fn test_eval_literal() {
        let mut eval = ComptimeEvaluator::new();

        // Test integer literal
        let expr = make_expr(ExprKind::IntLit(42));
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(42));

        // Test float literal
        let expr = make_expr(ExprKind::FloatLit(3.14));
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Float(3.14));

        // Test bool literal
        let expr = make_expr(ExprKind::BoolLit(true));
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(true));

        // Test string literal
        let expr = make_expr(ExprKind::StringLit("hello".to_string()));
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::String("hello".to_string()));

        // Test unit literal
        let expr = make_expr(ExprKind::UnitLit);
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Unit);
    }

    #[test]
    fn test_eval_arithmetic() {
        let mut eval = ComptimeEvaluator::new();

        // Test 1 + 2 = 3
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Add,
            left: Box::new(make_expr(ExprKind::IntLit(1))),
            right: Box::new(make_expr(ExprKind::IntLit(2))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(3));

        // Test 10 - 3 = 7
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Sub,
            left: Box::new(make_expr(ExprKind::IntLit(10))),
            right: Box::new(make_expr(ExprKind::IntLit(3))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(7));

        // Test 4 * 5 = 20
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Mul,
            left: Box::new(make_expr(ExprKind::IntLit(4))),
            right: Box::new(make_expr(ExprKind::IntLit(5))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(20));

        // Test 20 / 4 = 5
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Div,
            left: Box::new(make_expr(ExprKind::IntLit(20))),
            right: Box::new(make_expr(ExprKind::IntLit(4))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(5));

        // Test float arithmetic
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Add,
            left: Box::new(make_expr(ExprKind::FloatLit(1.5))),
            right: Box::new(make_expr(ExprKind::FloatLit(2.5))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Float(4.0));
    }

    #[test]
    fn test_eval_comparison() {
        let mut eval = ComptimeEvaluator::new();

        // Test 5 < 10 = true
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Lt,
            left: Box::new(make_expr(ExprKind::IntLit(5))),
            right: Box::new(make_expr(ExprKind::IntLit(10))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(true));

        // Test 5 > 10 = false
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Gt,
            left: Box::new(make_expr(ExprKind::IntLit(5))),
            right: Box::new(make_expr(ExprKind::IntLit(10))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(false));

        // Test 5 == 5 = true
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Eq,
            left: Box::new(make_expr(ExprKind::IntLit(5))),
            right: Box::new(make_expr(ExprKind::IntLit(5))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(true));

        // Test 5 != 5 = false
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Ne,
            left: Box::new(make_expr(ExprKind::IntLit(5))),
            right: Box::new(make_expr(ExprKind::IntLit(5))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(false));
    }

    #[test]
    fn test_eval_logical() {
        let mut eval = ComptimeEvaluator::new();

        // Test true and false = false
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::And,
            left: Box::new(make_expr(ExprKind::BoolLit(true))),
            right: Box::new(make_expr(ExprKind::BoolLit(false))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(false));

        // Test true or false = true
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Or,
            left: Box::new(make_expr(ExprKind::BoolLit(true))),
            right: Box::new(make_expr(ExprKind::BoolLit(false))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(true));
    }

    #[test]
    fn test_eval_if() {
        let mut eval = ComptimeEvaluator::new();

        // Test if true: 1 else: 0
        let then_block = make_block(vec![make_stmt(StmtKind::Expr(make_expr(
            ExprKind::IntLit(1),
        )))]);
        let else_block = make_block(vec![make_stmt(StmtKind::Expr(make_expr(
            ExprKind::IntLit(0),
        )))]);

        let expr = make_expr(ExprKind::If {
            condition: Box::new(make_expr(ExprKind::BoolLit(true))),
            then_block,
            else_ifs: vec![],
            else_block: Some(else_block),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(1));

        // Test if false: 1 else: 0
        let then_block = make_block(vec![make_stmt(StmtKind::Expr(make_expr(
            ExprKind::IntLit(1),
        )))]);
        let else_block = make_block(vec![make_stmt(StmtKind::Expr(make_expr(
            ExprKind::IntLit(0),
        )))]);

        let expr = make_expr(ExprKind::If {
            condition: Box::new(make_expr(ExprKind::BoolLit(false))),
            then_block,
            else_ifs: vec![],
            else_block: Some(else_block),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(0));
    }

    #[test]
    fn test_eval_variable() {
        let mut eval = ComptimeEvaluator::new();

        // Set up a variable in the environment
        eval.env.insert("x".to_string(), ComptimeValue::Int(42));

        // Evaluate the variable
        let expr = make_expr(ExprKind::Ident("x".to_string()));
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(42));

        // Unknown variable should error
        let expr = make_expr(ExprKind::Ident("unknown".to_string()));
        let result = eval.eval_expr(&expr);
        assert!(result.is_err());
    }

    #[test]
    fn test_eval_block() {
        let mut eval = ComptimeEvaluator::new();

        // Create a block with let and expression
        let stmts = vec![
            make_stmt(StmtKind::Let {
                name: "x".to_string(),
                type_ann: None,
                value: make_expr(ExprKind::IntLit(10)),
                mutable: false,
            }),
            make_stmt(StmtKind::Expr(make_expr(ExprKind::BinaryOp {
                op: BinOp::Add,
                left: Box::new(make_expr(ExprKind::Ident("x".to_string()))),
                right: Box::new(make_expr(ExprKind::IntLit(5))),
            }))),
        ];
        let block = make_block(stmts);

        // Test block by wrapping in an if-true expression
        let expr = make_expr(ExprKind::If {
            condition: Box::new(make_expr(ExprKind::BoolLit(true))),
            then_block: block,
            else_ifs: vec![],
            else_block: None,
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(15));
    }

    #[test]
    fn test_eval_unary() {
        let mut eval = ComptimeEvaluator::new();

        // Test -5 = -5
        let expr = make_expr(ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand: Box::new(make_expr(ExprKind::IntLit(5))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(-5));

        // Test not true = false
        let expr = make_expr(ExprKind::UnaryOp {
            op: UnaryOp::Not,
            operand: Box::new(make_expr(ExprKind::BoolLit(true))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(false));
    }

    #[test]
    fn test_division_by_zero() {
        let mut eval = ComptimeEvaluator::new();

        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Div,
            left: Box::new(make_expr(ExprKind::IntLit(10))),
            right: Box::new(make_expr(ExprKind::IntLit(0))),
        });
        let result = eval.eval_expr(&expr);
        assert!(matches!(result, Err(ComptimeError::DivisionByZero)));
    }

    #[test]
    fn test_recursion_limit() {
        let mut eval = ComptimeEvaluator::with_max_depth(5);

        // Create a recursive function
        let body = make_block(vec![make_stmt(StmtKind::Expr(make_expr(ExprKind::Call {
            func: Box::new(make_expr(ExprKind::Ident("recurse".to_string()))),
            args: vec![],
        })))]);

        let fn_def = FnDef {
            name: "recurse".to_string(),
            type_params: vec![],
            params: vec![],
            return_type: None,
            effects: None,
            body,
            annotations: vec![],
            contracts: vec![],
            budget: None,
            is_export: false,
            is_test: false,
            is_verified: false,
            doc_comment: None,
        };

        eval.register_function(fn_def.clone());

        // This should hit the recursion limit
        let result = eval.eval_fn(&fn_def, HashMap::new());
        assert!(matches!(
            result,
            Err(ComptimeError::RecursionLimit { name }) if name == "recurse"
        ));
    }

    #[test]
    fn test_eval_string_concat() {
        let mut eval = ComptimeEvaluator::new();

        // Test "hello" + " " + "world" = "hello world"
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Add,
            left: Box::new(make_expr(ExprKind::StringLit("hello".to_string()))),
            right: Box::new(make_expr(ExprKind::StringLit(" world".to_string()))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::String("hello world".to_string()));
    }

    #[test]
    fn test_eval_modulo() {
        let mut eval = ComptimeEvaluator::new();

        // Test 17 % 5 = 2
        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Mod,
            left: Box::new(make_expr(ExprKind::IntLit(17))),
            right: Box::new(make_expr(ExprKind::IntLit(5))),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(2));
    }

    #[test]
    fn test_eval_paren() {
        let mut eval = ComptimeEvaluator::new();

        // Test (1 + 2) * 3 = 9
        let inner = make_expr(ExprKind::BinaryOp {
            op: BinOp::Add,
            left: Box::new(make_expr(ExprKind::IntLit(1))),
            right: Box::new(make_expr(ExprKind::IntLit(2))),
        });
        let expr = make_expr(ExprKind::Paren(Box::new(inner)));

        // Evaluate just the paren (should be 3)
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(3));
    }

    #[test]
    fn test_eval_tuple_and_tuple_field() {
        let mut eval = ComptimeEvaluator::new();

        let tuple = make_expr(ExprKind::Tuple(vec![
            make_expr(ExprKind::IntLit(7)),
            make_expr(ExprKind::StringLit("token".to_string())),
        ]));
        let result = eval.eval_expr(&tuple).unwrap();
        assert_eq!(
            result,
            ComptimeValue::Tuple(vec![
                ComptimeValue::Int(7),
                ComptimeValue::String("token".to_string()),
            ])
        );

        let expr = make_expr(ExprKind::TupleField {
            tuple: Box::new(tuple),
            index: 1,
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::String("token".to_string()));
    }

    #[test]
    fn test_eval_list_literal_and_equality() {
        let mut eval = ComptimeEvaluator::new();

        let list = make_expr(ExprKind::ListLit(vec![
            make_expr(ExprKind::IntLit(1)),
            make_expr(ExprKind::IntLit(2)),
            make_expr(ExprKind::IntLit(3)),
        ]));
        let result = eval.eval_expr(&list).unwrap();
        assert_eq!(
            result,
            ComptimeValue::List(vec![
                ComptimeValue::Int(1),
                ComptimeValue::Int(2),
                ComptimeValue::Int(3),
            ])
        );

        let expr = make_expr(ExprKind::BinaryOp {
            op: BinOp::Eq,
            left: Box::new(list.clone()),
            right: Box::new(list),
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Bool(true));
    }

    #[test]
    fn test_eval_record_field_access() {
        let mut eval = ComptimeEvaluator::new();

        let record = make_expr(ExprKind::RecordLit {
            type_name: "Position".to_string(),
            base: None,
            fields: vec![
                ("line".to_string(), make_expr(ExprKind::IntLit(7))),
                ("col".to_string(), make_expr(ExprKind::IntLit(3))),
            ],
        });
        let expr = make_expr(ExprKind::FieldAccess {
            object: Box::new(record),
            field: "line".to_string(),
        });

        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(7));
    }

    #[test]
    fn test_eval_variant_field_access() {
        let mut eval = ComptimeEvaluator::new();

        let variant = make_expr(ExprKind::Construct {
            name: "Ident".to_string(),
            fields: vec![(
                "name".to_string(),
                make_expr(ExprKind::StringLit("parse_module".to_string())),
            )],
        });
        let expr = make_expr(ExprKind::FieldAccess {
            object: Box::new(variant),
            field: "name".to_string(),
        });

        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::String("parse_module".to_string()));
    }

    #[test]
    fn test_eval_variant_match_binds_payload() {
        let mut eval = ComptimeEvaluator::new();

        let arms = vec![crate::ast::expr::MatchArm {
            pattern: Pattern::Variant {
                variant: "IntLit".to_string(),
                bindings: vec!["value".to_string()],
            },
            guard: None,
            body: make_block(vec![make_stmt(StmtKind::Expr(make_expr(ExprKind::Ident(
                "value".to_string(),
            ))))]),
            span: Span::empty(),
        }];
        let expr = make_expr(ExprKind::Match {
            scrutinee: Box::new(make_expr(ExprKind::Construct {
                name: "IntLit".to_string(),
                fields: vec![("value".to_string(), make_expr(ExprKind::IntLit(42)))],
            })),
            arms,
        });

        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(42));
    }

    #[test]
    fn test_eval_match() {
        let mut eval = ComptimeEvaluator::new();

        // match 42: 0: 100 42: 200 _: 300
        let arms = vec![
            crate::ast::expr::MatchArm {
                pattern: Pattern::IntLit(0),
                guard: None,
                body: make_block(vec![make_stmt(StmtKind::Expr(make_expr(
                    ExprKind::IntLit(100),
                )))]),
                span: Span::empty(),
            },
            crate::ast::expr::MatchArm {
                pattern: Pattern::IntLit(42),
                guard: None,
                body: make_block(vec![make_stmt(StmtKind::Expr(make_expr(
                    ExprKind::IntLit(200),
                )))]),
                span: Span::empty(),
            },
            crate::ast::expr::MatchArm {
                pattern: Pattern::Wildcard,
                guard: None,
                body: make_block(vec![make_stmt(StmtKind::Expr(make_expr(
                    ExprKind::IntLit(300),
                )))]),
                span: Span::empty(),
            },
        ];

        let expr = make_expr(ExprKind::Match {
            scrutinee: Box::new(make_expr(ExprKind::IntLit(42))),
            arms,
        });
        let result = eval.eval_expr(&expr).unwrap();
        assert_eq!(result, ComptimeValue::Int(200));
    }
}
