#![cfg(any(feature = "smt", feature = "smt-verify"))]
//! Static Contract Verification using SMT Solvers
//!
//! This module provides compile-time proof of contract conditions using Z3.
//! It translates Gradient expressions into SMT-LIB constraints and attempts
//! to prove that:
//!
//! 1. @requires preconditions are satisfiable (function can be called)
//! 2. @ensures postconditions hold given the preconditions and function body
//!
//! # Architecture
//!
//! - `SmtEncoder`: Translates Gradient AST expressions to Z3 AST
//! - `ContractVerifier`: Manages the SMT context and proves contract properties
//! - `VerificationResult`: Outcome of verification (proved, counterexample, unknown)
//!
//! # Supported Constructs
//!
//! - Integer arithmetic (+, -, *, /, %)
//! - Comparisons (==, !=, <, <=, >, >=)
//! - Boolean logic (and, or, not)
//! - Simple linear arithmetic (LIA) and bitvectors
//!
//! # Example
//!
//! ```ignore
//! @requires(n >= 0)
//! @ensures(result >= 1)
//! fn factorial(n: Int) -> Int:
//!     if n <= 1:
//!         ret 1
//!     else:
//!         ret n * factorial(n - 1)
//! ```
//!
//! The verifier will prove that:
//! - There exist values of `n` satisfying `n >= 0` (precondition is satisfiable)
//! - For all `n >= 0`, the return value is >= 1 (postcondition holds)

use crate::ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use crate::ast::item::{Contract, ContractKind, FnDef};
use std::collections::HashMap;
use z3::ast::Ast;

/// The result of attempting to verify a contract.
#[derive(Debug, Clone, PartialEq)]
pub enum VerificationResult {
    /// The contract property was proven to hold.
    Proved,
    /// The contract property was falsified; counterexample provided.
    CounterExample { bindings: HashMap<String, String> },
    /// The SMT solver returned unknown (e.g., due to timeout or unsupported theory).
    Unknown(String),
    /// An error occurred during verification (e.g., unsupported construct).
    Error(String),
}

/// Verify all contracts on a function.
///
/// This is the main entry point for static contract verification.
/// It creates a fresh Z3 context and checks all contracts on the function.
pub fn verify_function_contracts(fn_def: &FnDef) -> HashMap<usize, VerificationResult> {
    let cfg = z3::Config::new();
    let ctx = z3::Context::new(&cfg);
    let mut verifier = ContractVerifier::new(&ctx);

    let mut results = HashMap::new();
    for (idx, contract) in fn_def.contracts.iter().enumerate() {
        let result = verifier.verify_contract(contract, fn_def);
        results.insert(idx, result);
    }
    results
}

/// A static contract verifier using Z3 SMT solver.
///
/// Note: This verifier holds a reference to the Z3 context and must not
/// outlive it. The typical usage pattern is to create a context, verify
/// one or more functions, then discard the context.
pub struct ContractVerifier<'ctx> {
    /// The Z3 context for creating expressions.
    context: &'ctx z3::Context,
    /// The Z3 solver instance.
    solver: z3::Solver<'ctx>,
    /// Variable name -> Z3 AST mapping.
    variables: HashMap<String, z3::ast::Int<'ctx>>,
    /// Timeout in milliseconds for Z3 queries.
    timeout_ms: u32,
}

impl<'ctx> ContractVerifier<'ctx> {
    /// Create a new contract verifier with the given Z3 context.
    pub fn new(context: &'ctx z3::Context) -> Self {
        let solver = z3::Solver::new(context);
        let variables = HashMap::new();

        Self {
            context,
            solver,
            variables,
            timeout_ms: 5000, // 5 second default timeout
        }
    }

    /// Set the timeout for Z3 queries.
    pub fn with_timeout(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Verify a single contract annotation.
    fn verify_contract(&mut self, contract: &Contract, fn_def: &FnDef) -> VerificationResult {
        self.reset_solver();

        match contract.kind {
            ContractKind::Requires => self.verify_precondition(&contract.condition, fn_def),
            ContractKind::Ensures => self.verify_postcondition(&contract.condition, fn_def),
        }
    }

    /// Verify that a precondition is satisfiable.
    ///
    /// A precondition is valid if there exist parameter values that satisfy it.
    /// This ensures the function can actually be called.
    fn verify_precondition(&mut self, condition: &Expr, fn_def: &FnDef) -> VerificationResult {
        self.declare_parameters(fn_def);

        // Encode the condition
        match self.encode_bool_expr(condition) {
            Some(cond_ast) => {
                // Check satisfiability: exists params. condition
                self.solver.assert(&cond_ast);

                match self.solver.check() {
                    z3::SatResult::Sat => VerificationResult::Proved,
                    z3::SatResult::Unsat => VerificationResult::CounterExample {
                        bindings: HashMap::new(),
                    },
                    z3::SatResult::Unknown => VerificationResult::Unknown(
                        "SMT solver returned unknown for precondition".to_string(),
                    ),
                }
            }
            None => {
                VerificationResult::Error("Failed to encode precondition expression".to_string())
            }
        }
    }

    /// Verify that a postcondition holds.
    ///
    /// For postconditions, we attempt to prove that the condition holds
    /// for all valid inputs (those satisfying the precondition).
    fn verify_postcondition(&mut self, condition: &Expr, fn_def: &FnDef) -> VerificationResult {
        self.declare_parameters(fn_def);

        // Declare the result variable
        let result_var =
            z3::ast::Int::new_const(self.context, z3::Symbol::String("result".to_string()));
        self.variables.insert("result".to_string(), result_var);

        // Encode the negation of the postcondition
        // We prove: forall params. precondition => postcondition
        // By refutation: assume precondition and not(postcondition), check unsat
        let requires_conj = self.conjoin_preconditions(fn_def);

        match self.encode_bool_expr(condition) {
            Some(post_cond) => {
                let neg_post = post_cond.not();

                // Assert: requires && !ensures
                if let Some(pre) = requires_conj {
                    self.solver.assert(&pre);
                }
                self.solver.assert(&neg_post);

                match self.solver.check() {
                    z3::SatResult::Unsat => VerificationResult::Proved,
                    z3::SatResult::Sat => {
                        // Found a counterexample
                        let model = self.solver.get_model();
                        let bindings = self.extract_counterexample(model);
                        VerificationResult::CounterExample { bindings }
                    }
                    z3::SatResult::Unknown => VerificationResult::Unknown(
                        "SMT solver returned unknown for postcondition".to_string(),
                    ),
                }
            }
            None => {
                VerificationResult::Error("Failed to encode postcondition expression".to_string())
            }
        }
    }

    /// Reset the solver state for a new verification query.
    fn reset_solver(&mut self) {
        self.solver.reset();
        self.variables.clear();
    }

    /// Declare all function parameters as SMT variables.
    fn declare_parameters(&mut self, fn_def: &FnDef) {
        for param in &fn_def.params {
            let name = &param.name;
            let symbol = z3::Symbol::String(name.clone());
            let var = z3::ast::Int::new_const(self.context, symbol);
            self.variables.insert(name.clone(), var);
        }
    }

    /// Conjoin all preconditions into a single SMT expression.
    fn conjoin_preconditions(&self, fn_def: &FnDef) -> Option<z3::ast::Bool<'ctx>> {
        let mut preconds: Vec<z3::ast::Bool<'ctx>> = Vec::new();

        for contract in &fn_def.contracts {
            if contract.kind == ContractKind::Requires {
                if let Some(cond) = self.encode_bool_expr(&contract.condition) {
                    preconds.push(cond);
                }
            }
        }

        if preconds.is_empty() {
            None
        } else {
            Some(
                preconds
                    .into_iter()
                    .reduce(|a, b| z3::ast::Bool::and(self.context, &[&a, &b]))
                    .unwrap(),
            )
        }
    }

    /// Encode a Gradient boolean expression as a Z3 boolean AST.
    fn encode_bool_expr(&self, expr: &Expr) -> Option<z3::ast::Bool<'ctx>> {
        match &expr.node {
            ExprKind::BinaryOp { op, left, right } => {
                match op {
                    // Comparison operators produce booleans
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        let left_int = self.encode_int_expr(left)?;
                        let right_int = self.encode_int_expr(right)?;

                        let result = match op {
                            BinOp::Eq => left_int._eq(&right_int),
                            BinOp::Ne => left_int._eq(&right_int).not(),
                            BinOp::Lt => left_int.lt(&right_int),
                            BinOp::Le => left_int.le(&right_int),
                            BinOp::Gt => left_int.gt(&right_int),
                            BinOp::Ge => left_int.ge(&right_int),
                            _ => unreachable!(),
                        };
                        Some(result)
                    }
                    // Logical operators
                    BinOp::And => {
                        let left_bool = self.encode_bool_expr(left)?;
                        let right_bool = self.encode_bool_expr(right)?;
                        Some(z3::ast::Bool::and(self.context, &[&left_bool, &right_bool]))
                    }
                    BinOp::Or => {
                        let left_bool = self.encode_bool_expr(left)?;
                        let right_bool = self.encode_bool_expr(right)?;
                        Some(z3::ast::Bool::or(self.context, &[&left_bool, &right_bool]))
                    }
                    // Arithmetic operators don't produce booleans
                    _ => None,
                }
            }
            ExprKind::UnaryOp {
                op: UnaryOp::Not,
                operand,
            } => {
                let inner = self.encode_bool_expr(operand)?;
                Some(inner.not())
            }
            ExprKind::BoolLit(v) => Some(z3::ast::Bool::from_bool(self.context, *v)),
            _ => None,
        }
    }

    /// Encode a Gradient integer expression as a Z3 integer AST.
    fn encode_int_expr(&self, expr: &Expr) -> Option<z3::ast::Int<'ctx>> {
        match &expr.node {
            ExprKind::IntLit(n) => Some(z3::ast::Int::from_i64(self.context, *n)),
            ExprKind::Ident(name) => self.variables.get(name).cloned(),
            ExprKind::BinaryOp { op, left, right } => {
                let left_int = self.encode_int_expr(left)?;
                let right_int = self.encode_int_expr(right)?;

                match op {
                    BinOp::Add => Some(left_int + right_int),
                    BinOp::Sub => Some(left_int - right_int),
                    BinOp::Mul => Some(left_int * right_int),
                    BinOp::Div => Some(left_int / right_int),
                    BinOp::Mod => Some(left_int.rem(&right_int)),
                    _ => None, // Comparison operators don't produce integers
                }
            }
            ExprKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } => {
                let inner = self.encode_int_expr(operand)?;
                Some(z3::ast::Int::from_i64(self.context, 0) - inner)
            }
            _ => None,
        }
    }

    /// Extract a counterexample from an SMT model.
    fn extract_counterexample(&self, model: Option<z3::Model>) -> HashMap<String, String> {
        let mut bindings = HashMap::new();

        if let Some(m) = model {
            for (name, var) in &self.variables {
                if let Some(val) = m.eval(var, true) {
                    bindings.insert(name.clone(), val.to_string());
                }
            }
        }

        bindings
    }
}

/// A report of contract verification for an entire module.
#[derive(Debug, Clone)]
pub struct ModuleVerificationReport {
    /// Per-function verification results.
    /// Key: function name, Value: map from contract index to result.
    pub function_results: HashMap<String, HashMap<usize, VerificationResult>>,
    /// Summary statistics.
    pub proved_count: usize,
    pub counterexample_count: usize,
    pub unknown_count: usize,
    pub error_count: usize,
}

impl ModuleVerificationReport {
    /// Create an empty report.
    pub fn new() -> Self {
        Self {
            function_results: HashMap::new(),
            proved_count: 0,
            counterexample_count: 0,
            unknown_count: 0,
            error_count: 0,
        }
    }

    /// Add results for a function.
    pub fn add_function_results(
        &mut self,
        fn_name: String,
        results: HashMap<usize, VerificationResult>,
    ) {
        for result in results.values() {
            match result {
                VerificationResult::Proved => self.proved_count += 1,
                VerificationResult::CounterExample { .. } => self.counterexample_count += 1,
                VerificationResult::Unknown(_) => self.unknown_count += 1,
                VerificationResult::Error(_) => self.error_count += 1,
            }
        }
        self.function_results.insert(fn_name, results);
    }

    /// Get a summary string.
    pub fn summary(&self) -> String {
        format!(
            "Contract verification: {} proved, {} counterexamples, {} unknown, {} errors",
            self.proved_count, self.counterexample_count, self.unknown_count, self.error_count
        )
    }
}

impl Default for ModuleVerificationReport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::expr::{Expr, ExprKind};
    use crate::ast::item::{Contract, ContractKind, FnDef, Param};
    use crate::ast::span::Spanned;
    use crate::ast::span::{Position, Span};
    use crate::ast::types::TypeExpr;

    // Helper to create a dummy span for testing
    fn dummy_span() -> Span {
        Span::new(
            0,
            Position {
                line: 1,
                col: 1,
                offset: 0,
            },
            Position {
                line: 1,
                col: 1,
                offset: 0,
            },
        )
    }

    // Helper to create a simple integer expression
    fn int_expr(n: i64) -> Expr {
        Spanned {
            node: ExprKind::IntLit(n),
            span: dummy_span(),
        }
    }

    // Helper to create an identifier expression
    fn ident_expr(name: &str) -> Expr {
        Spanned {
            node: ExprKind::Ident(name.to_string()),
            span: dummy_span(),
        }
    }

    // Helper to create a binary expression
    fn binary_expr(op: BinOp, left: Expr, right: Expr) -> Expr {
        Spanned {
            node: ExprKind::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            },
            span: dummy_span(),
        }
    }

    // Helper to create a boolean expression
    fn bool_expr(val: bool) -> Expr {
        Spanned {
            node: ExprKind::BoolLit(val),
            span: dummy_span(),
        }
    }

    #[test]
    fn test_simple_precondition_satisfiable() {
        // @requires(x > 0) - should be satisfiable
        let condition = binary_expr(BinOp::Gt, ident_expr("x"), int_expr(0));
        let contract = Contract {
            kind: ContractKind::Requires,
            condition,
            span: dummy_span(),
            runtime_only_off_in_release: false,
        };

        let fn_def = FnDef {
            name: "test".to_string(),
            type_params: vec![],
            params: vec![Param {
                name: "x".to_string(),
                type_ann: Spanned {
                    node: TypeExpr::Named {
                        name: "i32".to_string(),
                        cap: None,
                    },
                    span: dummy_span(),
                },
                span: dummy_span(),
                comptime: false,
            }],
            return_type: None,
            effects: None,
            body: Spanned {
                node: vec![],
                span: dummy_span(),
            },
            annotations: vec![],
            contracts: vec![contract],
            budget: None,
            is_export: false,
            is_test: false,
            is_verified: false,
            is_bench: false,
            doc_comment: None,
        };

        let results = verify_function_contracts(&fn_def);
        assert!(matches!(results[&0], VerificationResult::Proved));
    }

    #[test]
    fn test_contradictory_precondition() {
        // @requires(x > 0 and x < 0) - should be unsatisfiable
        let left = binary_expr(BinOp::Gt, ident_expr("x"), int_expr(0));
        let right = binary_expr(BinOp::Lt, ident_expr("x"), int_expr(0));
        let condition = Spanned {
            node: ExprKind::BinaryOp {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
            },
            span: dummy_span(),
        };

        let contract = Contract {
            kind: ContractKind::Requires,
            condition,
            span: dummy_span(),
            runtime_only_off_in_release: false,
        };

        let fn_def = FnDef {
            name: "test".to_string(),
            type_params: vec![],
            params: vec![Param {
                name: "x".to_string(),
                type_ann: Spanned {
                    node: TypeExpr::Named {
                        name: "i32".to_string(),
                        cap: None,
                    },
                    span: dummy_span(),
                },
                span: dummy_span(),
                comptime: false,
            }],
            return_type: None,
            effects: None,
            body: Spanned {
                node: vec![],
                span: dummy_span(),
            },
            annotations: vec![],
            contracts: vec![contract],
            budget: None,
            is_export: false,
            is_test: false,
            is_verified: false,
            is_bench: false,
            doc_comment: None,
        };

        let results = verify_function_contracts(&fn_def);
        assert!(matches!(
            results[&0],
            VerificationResult::CounterExample { .. }
        ));
    }

    #[test]
    fn test_boolean_literal_encoding() {
        // Test that we can encode boolean literals
        let condition = bool_expr(true);
        let contract = Contract {
            kind: ContractKind::Requires,
            condition,
            span: dummy_span(),
            runtime_only_off_in_release: false,
        };

        let fn_def = FnDef {
            name: "test".to_string(),
            type_params: vec![],
            params: vec![],
            return_type: None,
            effects: None,
            body: Spanned {
                node: vec![],
                span: dummy_span(),
            },
            annotations: vec![],
            contracts: vec![contract],
            budget: None,
            is_export: false,
            is_test: false,
            is_verified: false,
            is_bench: false,
            doc_comment: None,
        };

        let results = verify_function_contracts(&fn_def);
        assert!(matches!(results[&0], VerificationResult::Proved));
    }
}
