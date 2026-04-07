//! SMT encoder for Gradient types and expressions.
//!
//! This module provides the core encoding logic for converting Gradient AST
//! expressions into Z3 SMT expressions. It handles:
//!
//! - Type mappings (Int → Int sort, Bool → Bool sort)
//! - Comparison operators (=, <, >, ≤, ≥)
//! - Boolean operations (and, or, not)
//! - Arithmetic operations (+, -, *, /)
//!
//! # Example
//!
//! ```rust,ignore
//! use gradient_compiler::smt::{Encoder, Solver};
//!
//! let solver = Solver::new();
//! let encoder = Encoder::new(&solver);
//!
//! // Encode a simple integer comparison: x > 0
//! let x = encoder.fresh_int("x");
//! let zero = encoder.int(0);
//! let condition = encoder.gt(&x, &zero);
//! ```

use z3::ast::Ast;
use z3::{ast, Context, Sort};

use crate::ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use crate::typechecker::types::Ty;

use super::error::{VerificationError, VerificationResult};

/// Encoder for converting Gradient expressions to Z3 SMT expressions.
pub struct Encoder<'ctx> {
    /// Reference to the Z3 context.
    context: &'ctx Context,
}

impl<'ctx> Encoder<'ctx> {
    /// Creates a new encoder for the given Z3 context.
    pub fn new(context: &'ctx Context) -> Self {
        Self { context }
    }

    // ==================== Sort Creation ====================

    /// Returns the Z3 Int sort.
    pub fn int_sort(&self) -> Sort<'ctx> {
        Sort::int(self.context)
    }

    /// Returns the Z3 Bool sort.
    pub fn bool_sort(&self) -> Sort<'ctx> {
        Sort::bool(self.context)
    }

    // ==================== Constant Creation ====================

    /// Creates an integer constant with the given value.
    pub fn int(&self, value: i64) -> ast::Int<'ctx> {
        ast::Int::from_i64(self.context, value)
    }

    /// Creates a boolean constant with the given value.
    pub fn bool(&self, value: bool) -> ast::Bool<'ctx> {
        ast::Bool::from_bool(self.context, value)
    }

    // ==================== Variable Creation ====================

    /// Creates a fresh integer variable with the given name.
    pub fn fresh_int(&self, name: &str) -> ast::Int<'ctx> {
        ast::Int::new_const(self.context, name)
    }

    /// Creates a fresh boolean variable with the given name.
    pub fn fresh_bool(&self, name: &str) -> ast::Bool<'ctx> {
        ast::Bool::new_const(self.context, name)
    }

    /// Creates a fresh constant of the given type.
    pub fn fresh_var(&self, name: &str, ty: &Ty) -> VerificationResult<ast::Dynamic<'ctx>> {
        match ty {
            Ty::Int => Ok(self.fresh_int(name).into()),
            Ty::Bool => Ok(self.fresh_bool(name).into()),
            _ => Err(VerificationError::EncodingError {
                message: format!("Unsupported type for SMT encoding: {:?}", ty),
                location: None,
            }),
        }
    }

    // ==================== Comparison Operations ====================

    /// Encodes equality (a = b).
    pub fn eq<A: Into<ast::Dynamic<'ctx>>, B: Into<ast::Dynamic<'ctx>>>(
        &self,
        a: A,
        b: B,
    ) -> ast::Bool<'ctx> {
        let a_dyn: ast::Dynamic = a.into();
        let b_dyn: ast::Dynamic = b.into();
        a_dyn._eq(&b_dyn)
    }

    /// Encodes less than (a < b) for integers.
    pub fn lt(&self, a: &ast::Int<'ctx>, b: &ast::Int<'ctx>) -> ast::Bool<'ctx> {
        a.lt(b)
    }

    /// Encodes greater than (a > b) for integers.
    pub fn gt(&self, a: &ast::Int<'ctx>, b: &ast::Int<'ctx>) -> ast::Bool<'ctx> {
        a.gt(b)
    }

    /// Encodes less than or equal (a ≤ b) for integers.
    pub fn le(&self, a: &ast::Int<'ctx>, b: &ast::Int<'ctx>) -> ast::Bool<'ctx> {
        a.le(b)
    }

    /// Encodes greater than or equal (a ≥ b) for integers.
    pub fn ge(&self, a: &ast::Int<'ctx>, b: &ast::Int<'ctx>) -> ast::Bool<'ctx> {
        a.ge(b)
    }

    // ==================== Boolean Operations ====================

    /// Encodes logical AND.
    pub fn and(&self, a: &ast::Bool<'ctx>, b: &ast::Bool<'ctx>) -> ast::Bool<'ctx> {
        ast::Bool::and(self.context, &[a, b])
    }

    /// Encodes logical OR.
    pub fn or(&self, a: &ast::Bool<'ctx>, b: &ast::Bool<'ctx>) -> ast::Bool<'ctx> {
        ast::Bool::or(self.context, &[a, b])
    }

    /// Encodes logical NOT.
    pub fn not(&self, a: &ast::Bool<'ctx>) -> ast::Bool<'ctx> {
        a.not()
    }

    /// Encodes logical implication (a => b).
    pub fn implies(&self, a: &ast::Bool<'ctx>, b: &ast::Bool<'ctx>) -> ast::Bool<'ctx> {
        a.implies(b)
    }

    // ==================== Arithmetic Operations ====================

    /// Encodes addition (a + b).
    pub fn add(&self, a: &ast::Int<'ctx>, b: &ast::Int<'ctx>) -> ast::Int<'ctx> {
        ast::Int::add(self.context, &[a, b])
    }

    /// Encodes subtraction (a - b).
    pub fn sub(&self, a: &ast::Int<'ctx>, b: &ast::Int<'ctx>) -> ast::Int<'ctx> {
        ast::Int::sub(self.context, &[a, b])
    }

    /// Encodes multiplication (a * b).
    pub fn mul(&self, a: &ast::Int<'ctx>, b: &ast::Int<'ctx>) -> ast::Int<'ctx> {
        ast::Int::mul(self.context, &[a, b])
    }

    /// Encodes division (a / b).
    pub fn div(&self, a: &ast::Int<'ctx>, b: &ast::Int<'ctx>) -> ast::Int<'ctx> {
        a.div(b)
    }

    // ==================== Expression Encoding ====================

    /// Encodes a Gradient expression to a Z3 expression.
    ///
    /// This is a basic implementation that handles:
    /// - Integer literals
    /// - Boolean literals
    /// - Integer comparisons (<, >, <=, >=, ==)
    /// - Boolean operations (and, or, not)
    /// - Arithmetic operations (+, -, *, /)
    ///
    /// # Arguments
    ///
    /// * `expr` - The Gradient expression to encode
    /// * `var_lookup` - A function to resolve variable names to Z3 expressions
    pub fn encode_expr<F>(
        &self,
        expr: &Expr,
        var_lookup: &mut F,
    ) -> VerificationResult<ast::Dynamic<'ctx>>
    where
        F: FnMut(&str) -> Option<ast::Dynamic<'ctx>>,
    {
        match &expr.node {
            // Integer literals
            ExprKind::IntLit(value) => Ok(ast::Int::from_i64(self.context, *value).into()),

            // Boolean literals
            ExprKind::BoolLit(value) => Ok(ast::Bool::from_bool(self.context, *value).into()),

            // Variable references
            ExprKind::Ident(name) => {
                if let Some(val) = var_lookup(name) {
                    Ok(val)
                } else {
                    Err(VerificationError::EncodingError {
                        message: format!("Unknown variable: {}", name),
                        location: None,
                    })
                }
            }

            // Binary operations
            ExprKind::BinaryOp { op, left, right } => {
                self.encode_binop(op, left, right, var_lookup)
            }

            // Unary operations
            ExprKind::UnaryOp { op, operand } => self.encode_unop(op, operand, var_lookup),

            // Unsupported expressions
            _ => Err(VerificationError::EncodingError {
                message: format!("Unsupported expression type for SMT encoding"),
                location: None,
            }),
        }
    }

    fn encode_binop<F>(
        &self,
        op: &BinOp,
        lhs: &Expr,
        rhs: &Expr,
        var_lookup: &mut F,
    ) -> VerificationResult<ast::Dynamic<'ctx>>
    where
        F: FnMut(&str) -> Option<ast::Dynamic<'ctx>>,
    {
        let lhs_encoded = self.encode_expr(lhs, var_lookup)?;
        let rhs_encoded = self.encode_expr(rhs, var_lookup)?;

        match op {
            // Arithmetic operations
            BinOp::Add => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_int(), rhs_encoded.as_int()) {
                    Ok(ast::Int::add(self.context, &[&a, &b]).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Addition requires integer operands".to_string(),
                        location: None,
                    })
                }
            }
            BinOp::Sub => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_int(), rhs_encoded.as_int()) {
                    Ok(ast::Int::sub(self.context, &[&a, &b]).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Subtraction requires integer operands".to_string(),
                        location: None,
                    })
                }
            }
            BinOp::Mul => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_int(), rhs_encoded.as_int()) {
                    Ok(ast::Int::mul(self.context, &[&a, &b]).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Multiplication requires integer operands".to_string(),
                        location: None,
                    })
                }
            }
            BinOp::Div => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_int(), rhs_encoded.as_int()) {
                    Ok(a.div(&b).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Division requires integer operands".to_string(),
                        location: None,
                    })
                }
            }

            // Comparison operations
            BinOp::Eq => Ok(lhs_encoded._eq(&rhs_encoded).into()),
            BinOp::Lt => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_int(), rhs_encoded.as_int()) {
                    Ok(a.lt(&b).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Less-than comparison requires integer operands".to_string(),
                        location: None,
                    })
                }
            }
            BinOp::Gt => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_int(), rhs_encoded.as_int()) {
                    Ok(a.gt(&b).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Greater-than comparison requires integer operands".to_string(),
                        location: None,
                    })
                }
            }
            BinOp::Le => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_int(), rhs_encoded.as_int()) {
                    Ok(a.le(&b).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Less-than-or-equal comparison requires integer operands"
                            .to_string(),
                        location: None,
                    })
                }
            }
            BinOp::Ge => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_int(), rhs_encoded.as_int()) {
                    Ok(a.ge(&b).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Greater-than-or-equal comparison requires integer operands"
                            .to_string(),
                        location: None,
                    })
                }
            }

            // Logical operations
            BinOp::And => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_bool(), rhs_encoded.as_bool()) {
                    Ok(ast::Bool::and(self.context, &[&a, &b]).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Logical AND requires boolean operands".to_string(),
                        location: None,
                    })
                }
            }
            BinOp::Or => {
                if let (Some(a), Some(b)) = (lhs_encoded.as_bool(), rhs_encoded.as_bool()) {
                    Ok(ast::Bool::or(self.context, &[&a, &b]).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Logical OR requires boolean operands".to_string(),
                        location: None,
                    })
                }
            }

            // Unsupported operators
            _ => Err(VerificationError::EncodingError {
                message: format!("Unsupported binary operator: {:?}", op),
                location: None,
            }),
        }
    }

    fn encode_unop<F>(
        &self,
        op: &UnaryOp,
        operand: &Expr,
        var_lookup: &mut F,
    ) -> VerificationResult<ast::Dynamic<'ctx>>
    where
        F: FnMut(&str) -> Option<ast::Dynamic<'ctx>>,
    {
        let operand_encoded = self.encode_expr(operand, var_lookup)?;

        match op {
            UnaryOp::Not => {
                if let Some(b) = operand_encoded.as_bool() {
                    Ok(b.not().into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Logical NOT requires boolean operand".to_string(),
                        location: None,
                    })
                }
            }
            UnaryOp::Neg => {
                if let Some(a) = operand_encoded.as_int() {
                    // Negation: -a = 0 - a
                    let zero = ast::Int::from_i64(self.context, 0);
                    Ok(ast::Int::sub(self.context, &[&zero, &a]).into())
                } else {
                    Err(VerificationError::EncodingError {
                        message: "Arithmetic negation requires integer operand".to_string(),
                        location: None,
                    })
                }
            }
        }
    }

    /// Encodes a Gradient type to a Z3 sort.
    pub fn encode_type(&self, ty: &Ty) -> VerificationResult<Sort<'ctx>> {
        match ty {
            Ty::Int => Ok(Sort::int(self.context)),
            Ty::Bool => Ok(Sort::bool(self.context)),
            _ => Err(VerificationError::EncodingError {
                message: format!("Unsupported type for SMT encoding: {:?}", ty),
                location: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z3::Config;

    fn test_context() -> Context {
        let config = Config::new();
        Context::new(&config)
    }

    #[test]
    fn test_int_encoding() {
        let ctx = test_context();
        let encoder = Encoder::new(&ctx);

        let x = encoder.fresh_int("x");
        let five = encoder.int(5);
        let condition = encoder.gt(&x, &five);

        // Just verify we can create the expression without panic
        assert!(!condition.to_string().is_empty());
    }

    #[test]
    fn test_bool_encoding() {
        let ctx = test_context();
        let encoder = Encoder::new(&ctx);

        let p = encoder.fresh_bool("p");
        let q = encoder.fresh_bool("q");
        let condition = encoder.and(&p, &q);

        // Just verify we can create the expression without panic
        assert!(!condition.to_string().is_empty());
    }

    #[test]
    fn test_comparison_operators() {
        let ctx = test_context();
        let encoder = Encoder::new(&ctx);

        let a = encoder.fresh_int("a");
        let b = encoder.fresh_int("b");

        // Test all comparison operators
        let lt = encoder.lt(&a, &b);
        let gt = encoder.gt(&a, &b);
        let le = encoder.le(&a, &b);
        let ge = encoder.ge(&a, &b);
        let eq = encoder.eq(&a, &b);

        // Verify expressions are created
        assert!(!lt.to_string().is_empty());
        assert!(!gt.to_string().is_empty());
        assert!(!le.to_string().is_empty());
        assert!(!ge.to_string().is_empty());
        assert!(!eq.to_string().is_empty());
    }

    #[test]
    fn test_arithmetic_encoding() {
        let ctx = test_context();
        let encoder = Encoder::new(&ctx);

        let x = encoder.fresh_int("x");
        let y = encoder.fresh_int("y");

        let sum = encoder.add(&x, &y);
        let diff = encoder.sub(&x, &y);
        let prod = encoder.mul(&x, &y);

        assert!(!sum.to_string().is_empty());
        assert!(!diff.to_string().is_empty());
        assert!(!prod.to_string().is_empty());
    }

    #[test]
    fn test_encode_int_literal() {
        let ctx = test_context();
        let encoder = Encoder::new(&ctx);

        // Create a simple integer literal expression
        let expr = Expr {
            node: ExprKind::IntLit(42),
            span: crate::ast::span::Span::empty(),
        };

        let mut lookup = |_name: &str| -> Option<ast::Dynamic> { None };
        let result = encoder.encode_expr(&expr, &mut lookup);

        assert!(result.is_ok());
        let encoded = result.unwrap();
        assert!(encoded.as_int().is_some());
    }

    #[test]
    fn test_encode_bool_literal() {
        let ctx = test_context();
        let encoder = Encoder::new(&ctx);

        let expr = Expr {
            node: ExprKind::BoolLit(true),
            span: crate::ast::span::Span::empty(),
        };

        let mut lookup = |_name: &str| -> Option<ast::Dynamic> { None };
        let result = encoder.encode_expr(&expr, &mut lookup);

        assert!(result.is_ok());
        let encoded = result.unwrap();
        assert!(encoded.as_bool().is_some());
    }

    #[test]
    fn test_encode_int_comparison() {
        let ctx = test_context();
        let encoder = Encoder::new(&ctx);

        // x > 0
        let expr = Expr {
            node: ExprKind::BinaryOp {
                op: BinOp::Gt,
                left: Box::new(Expr {
                    node: ExprKind::Ident("x".to_string()),
                    span: crate::ast::span::Span::empty(),
                }),
                right: Box::new(Expr {
                    node: ExprKind::IntLit(0),
                    span: crate::ast::span::Span::empty(),
                }),
            },
            span: crate::ast::span::Span::empty(),
        };

        let mut lookup = |name: &str| -> Option<ast::Dynamic> {
            if name == "x" {
                Some(ast::Int::new_const(&ctx, "x").into())
            } else {
                None
            }
        };

        let result = encoder.encode_expr(&expr, &mut lookup);
        assert!(result.is_ok());
        let encoded = result.unwrap();
        assert!(encoded.as_bool().is_some()); // Comparisons return bool
    }
}
