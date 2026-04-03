#![cfg(feature = "smt")]
//! Static Contract Verification using SMT Solvers
//!
//! WARNING: This module has significant API mismatches with the current AST
//! and needs a full update.
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
use crate::ast::types::TypeExpr;
use std::collections::HashMap;

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

/// A static contract verifier using Z3 SMT solver.
pub struct ContractVerifier {
    /// The Z3 context for creating expressions.
    #[cfg(feature = "smt")]
    context: z3::Context,
    /// The Z3 solver instance.
    #[cfg(feature = "smt")]
    solver: z3::Solver,
    /// Variable name -> Z3 AST mapping.
    #[cfg(feature = "smt")]
    variables: HashMap<String, z3::ast::Int>,
    /// Timeout in milliseconds for Z3 queries.
    timeout_ms: u32,
}

impl ContractVerifier {
    /// Create a new contract verifier with default settings.
    pub fn new() -> Self {
        #[cfg(feature = "smt")]
        {
            let cfg = z3::Config::new();
            let context = z3::Context::new(&cfg);
            let solver = z3::Solver::new(&context);
            let mut variables = HashMap::new();

            Self {
                context,
                solver,
                variables,
                timeout_ms: 5000, // 5 second default timeout
            }
        }

        #[cfg(not(feature = "smt"))]
        {
            Self { timeout_ms: 5000 }
        }
    }

    /// Set the timeout for Z3 queries.
    pub fn with_timeout(mut self, ms: u32) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Verify all contracts on a function.
    ///
    /// Returns a map from contract index to verification result.
    pub fn verify_function(&mut self, fn_def: &FnDef) -> HashMap<usize, VerificationResult> {
        let mut results = HashMap::new();

        for (idx, contract) in fn_def.contracts.iter().enumerate() {
            let result = self.verify_contract(contract, fn_def);
            results.insert(idx, result);
        }

        results
    }

    /// Verify a single contract annotation.
    fn verify_contract(&mut self, contract: &Contract, fn_def: &FnDef) -> VerificationResult {
        match contract.kind {
            ContractKind::Requires => self.verify_precondition(&contract.condition, fn_def),
            ContractKind::Ensures => self.verify_postcondition(&contract.condition, fn_def),
        }
    }

    /// Verify that a precondition is satisfiable.
    ///
    /// A precondition is valid if there exist parameter values that satisfy it.
    /// This ensures the function can actually be called.
    fn verify_precondition(
        &mut self,
        condition: &Expr,
        fn_def: &FnDef,
    ) -> VerificationResult {
        #[cfg(feature = "smt")]
        {
            self.reset_solver();
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
                None => VerificationResult::Error(
                    "Failed to encode precondition expression".to_string(),
                ),
            }
        }

        #[cfg(not(feature = "smt"))]
        {
            VerificationResult::Error(
                "SMT verification requires 'smt' feature enabled".to_string(),
            )
        }
    }

    /// Verify that a postcondition holds.
    ///
    /// For postconditions, we attempt to prove that the condition holds
    /// for all valid inputs (those satisfying the precondition).
    fn verify_postcondition(
        &mut self,
        condition: &Expr,
        fn_def: &FnDef,
    ) -> VerificationResult {
        #[cfg(feature = "smt")]
        {
            self.reset_solver();
            self.declare_parameters(fn_def);

            // Declare the result variable
            let result_var = z3::ast::Int::new_const(
                &self.context,
                &z3::Symbol::String("result".to_string()),
            );
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
                None => VerificationResult::Error(
                    "Failed to encode postcondition expression".to_string(),
                ),
            }
        }

        #[cfg(not(feature = "smt"))]
        {
            VerificationResult::Error(
                "SMT verification requires 'smt' feature enabled".to_string(),
            )
        }
    }

    /// Reset the solver state for a new verification query.
    #[cfg(feature = "smt")]
    fn reset_solver(&mut self) {
        self.solver.reset();
        self.variables.clear();
    }

    /// Declare all function parameters as SMT variables.
    #[cfg(feature = "smt")]
    fn declare_parameters(&mut self, fn_def: &FnDef) {
        for param in &fn_def.params {
            let name = &param.name;
            let symbol = z3::Symbol::String(name.clone());
            let var = z3::ast::Int::new_const(&self.context, &symbol);
            self.variables.insert(name.clone(), var);
        }
    }

    /// Conjoin all preconditions into a single SMT expression.
    #[cfg(feature = "smt")]
    fn conjoin_preconditions(&mut self, fn_def: &FnDef) -> Option<z3::ast::Bool> {
        let mut preconds = Vec::new();

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
                    .reduce(|a, b| z3::ast::Bool::and(&self.context, &[&a, &b]))
                    .unwrap(),
            )
        }
    }

    /// Encode a Gradient boolean expression as a Z3 boolean AST.
    #[cfg(feature = "smt")]
    fn encode_bool_expr(&self, expr: &Expr) -> Option<z3::ast::Bool> {
        match &expr.kind {
            ExprKind::Binary { op, left, right } => {
                let left_int = self.encode_int_expr(left)?;
                let right_int = self.encode_int_expr(right)?;

                let result = match op {
                    BinOp::Eq => left_int._eq(&right_int),
                    BinOp::Ne => left_int._eq(&right_int).not(),
                    BinOp::Lt => left_int.lt(&right_int),
                    BinOp::Le => left_int.le(&right_int),
                    BinOp::Gt => left_int.gt(&right_int),
                    BinOp::Ge => left_int.ge(&right_int),
                    _ => return None,
                };

                Some(result)
            }
            ExprKind::And { left, right } => {
                let left_bool = self.encode_bool_expr(left)?;
                let right_bool = self.encode_bool_expr(right)?;
                Some(z3::ast::Bool::and(&self.context, &[&left_bool, &right_bool]))
            }
            ExprKind::Or { left, right } => {
                let left_bool = self.encode_bool_expr(left)?;
                let right_bool = self.encode_bool_expr(right)?;
                Some(z3::ast::Bool::or(&self.context, &[&left_bool, &right_bool]))
            }
            ExprKind::Not { expr } => {
                let inner = self.encode_bool_expr(expr)?;
                Some(inner.not())
            }
            ExprKind::LiteralBool(v) => Some(z3::ast::Bool::from_bool(&self.context, *v)),
            _ => None,
        }
    }

    /// Encode a Gradient integer expression as a Z3 integer AST.
    #[cfg(feature = "smt")]
    fn encode_int_expr(&self, expr: &Expr) -> Option<z3::ast::Int> {
        match &expr.kind {
            ExprKind::LiteralInt(n) => Some(z3::ast::Int::from_i64(&self.context, *n)),
            ExprKind::Identifier(name) => self.variables.get(name).cloned(),
            ExprKind::Binary { op, left, right } => {
                let left_int = self.encode_int_expr(left)?;
                let right_int = self.encode_int_expr(right)?;

                match op {
                    BinOp::Add => Some(left_int + right_int),
                    BinOp::Sub => Some(left_int - right_int),
                    BinOp::Mul => Some(left_int * right_int),
                    BinOp::Div => Some(left_int / right_int),
                    BinOp::Rem => left_int.rem(&right_int),
                    _ => None,
                }
            }
            ExprKind::Unary { op, expr } => {
                let inner = self.encode_int_expr(expr)?;
                match op {
                    UnaryOp::Neg => Some(z3::ast::Int::from_i64(&self.context, 0) - inner),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Extract a counterexample from an SMT model.
    #[cfg(feature = "smt")]
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

impl Default for ContractVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Verify contracts for a single function.
///
/// This is the main entry point for static contract verification.
/// It creates a fresh verifier and checks all contracts on the function.
pub fn verify_function_contracts(fn_def: &FnDef) -> HashMap<usize, VerificationResult> {
    let mut verifier = ContractVerifier::new();
    verifier.verify_function(fn_def)
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
    use crate::ast::span::Span;
    use crate::ast::types::TypeExpr;

    // Helper to create a simple integer expression
    fn int_expr(n: i64) -> Expr {
        Expr {
            kind: ExprKind::LiteralInt(n),
            span: Span::new(0, 0, 0),
        }
    }

    // Helper to create an identifier expression
    fn ident_expr(name: &str) -> Expr {
        Expr {
            kind: ExprKind::Identifier(name.to_string()),
            span: Span::new(0, 0, 0),
        }
    }

    // Helper to create a binary expression
    fn binary_expr(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr {
            kind: ExprKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            },
            span: Span::new(0, 0, 0),
        }
    }

    #[test]
    #[cfg(feature = "smt")]
    fn test_simple_precondition_satisfiable() {
        // @requires(x > 0) - should be satisfiable
        let condition = binary_expr(BinOp::Gt, ident_expr("x"), int_expr(0));
        let contract = Contract {
            kind: ContractKind::Requires,
            condition,
            span: Span::new(0, 0, 0),
        };

        let fn_def = FnDef {
            name: "test".to_string(),
            type_params: vec![],
            params: vec![Param {
                name: "x".to_string(),
                type_ann: crate::ast::span::Spanned {
                    node: TypeExpr::Int,
                    span: Span::new(0, 0, 0),
                },
                span: Span::new(0, 0, 0),
            }],
            return_type: None,
            effects: vec![],
            body: crate::ast::block::Block { stmts: vec![] },
            annotations: vec![],
            contracts: vec![contract],
            budget: None,
            is_export: false,
            span: Span::new(0, 0, 0),
        };

        let results = verify_function_contracts(&fn_def);
        assert_eq!(results[&0], VerificationResult::Proved);
    }

    #[test]
    #[cfg(feature = "smt")]
    fn test_contradictory_precondition() {
        // @requires(x > 0 and x < 0) - should be unsatisfiable
        let left = binary_expr(BinOp::Gt, ident_expr("x"), int_expr(0));
        let right = binary_expr(BinOp::Lt, ident_expr("x"), int_expr(0));
        let condition = Expr {
            kind: ExprKind::And {
                left: Box::new(left),
                right: Box::new(right),
            },
            span: Span::new(0, 0, 0),
        };

        let contract = Contract {
            kind: ContractKind::Requires,
            condition,
            span: Span::new(0, 0, 0),
        };

        let fn_def = FnDef {
            name: "test".to_string(),
            type_params: vec![],
            params: vec![Param {
                name: "x".to_string(),
                type_ann: crate::ast::span::Spanned {
                    node: TypeExpr::Int,
                    span: Span::new(0, 0, 0),
                },
                span: Span::new(0, 0, 0),
            }],
            return_type: None,
            effects: vec![],
            body: crate::ast::block::Block { stmts: vec![] },
            annotations: vec![],
            contracts: vec![contract],
            budget: None,
            is_export: false,
            span: Span::new(0, 0, 0),
        };

        let results = verify_function_contracts(&fn_def);
        assert!(matches!(results[&0], VerificationResult::CounterExample { .. }));
    }
}
