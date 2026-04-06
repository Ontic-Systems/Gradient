//! SMT solver wrapper for Gradient contract verification.
//!
//! This module provides a high-level interface to the Z3 SMT solver,
//! abstracting away the details of solver configuration and providing
//! convenient methods for checking validity of verification conditions.

use z3::{Context, Solver as Z3Solver};

use super::error::{Counterexample, SmtValue, VerificationError, VerificationResult};

/// Wrapper around the Z3 SMT solver.
///
/// This struct manages the Z3 context and solver instance, providing
/// a simplified API for checking validity of verification conditions.
pub struct Solver<'ctx> {
    /// The Z3 context (owns all Z3 objects).
    context: &'ctx Context,
    /// The Z3 solver instance.
    solver: Z3Solver<'ctx>,
}

impl<'ctx> Solver<'ctx> {
    /// Creates a new solver with the given context.
    pub fn new(context: &'ctx Context) -> Self {
        let solver = Z3Solver::new(context);
        Self { context, solver }
    }

    /// Returns a reference to the Z3 context.
    pub fn context(&self) -> &Context {
        self.context
    }

    /// Returns a reference to the Z3 solver.
    pub fn z3_solver(&self) -> &Z3Solver<'ctx> {
        &self.solver
    }

    /// Pushes a new scope onto the solver stack.
    ///
    /// This allows temporary assertions that can be removed with `pop`.
    pub fn push(&self) {
        self.solver.push();
    }

    /// Pops a scope from the solver stack.
    pub fn pop(&self) {
        self.solver.pop(1);
    }

    /// Resets the solver to its initial state.
    pub fn reset(&self) {
        self.solver.reset();
    }

    /// Checks if the current set of assertions is satisfiable.
    ///
    /// Returns:
    /// - `Ok(true)` if satisfiable
    /// - `Ok(false)` if unsatisfiable
    /// - `Err(...)` if there was a solver error
    pub fn check_sat(&self) -> VerificationResult<bool> {
        match self.solver.check() {
            z3::SatResult::Sat => Ok(true),
            z3::SatResult::Unsat => Ok(false),
            z3::SatResult::Unknown => Err(VerificationError::Inconclusive {
                reason: "Solver returned unknown".to_string(),
            }),
        }
    }

    /// Checks if a verification condition is valid.
    ///
    /// A condition is valid if it holds in all models (i.e., its negation
    /// is unsatisfiable).
    ///
    /// # Arguments
    ///
    /// * `condition` - The Z3 boolean expression to check for validity
    ///
    /// Returns:
    /// - `Ok(())` if the condition is valid
    /// - `Err(VerificationError::...)` if invalid or inconclusive
    pub fn check_valid(&self, condition: &z3::ast::Bool) -> VerificationResult<()> {
        // To check validity of P, we check unsatisfiability of ¬P
        let negation = condition.not();
        self.solver.assert(&negation);

        match self.solver.check() {
            z3::SatResult::Unsat => {
                // Negation is unsatisfiable, so original is valid
                Ok(())
            }
            z3::SatResult::Sat => {
                // Negation is satisfiable, so original is invalid
                // Try to get a counterexample
                let model = self.solver.get_model();
                let counterexample = model.map(|m| extract_counterexample(&m));

                Err(VerificationError::Inconclusive {
                    reason: format!(
                        "Condition is not valid. Counterexample: {:?}",
                        counterexample
                    ),
                })
            }
            z3::SatResult::Unknown => Err(VerificationError::Inconclusive {
                reason: "Solver could not determine validity".to_string(),
            }),
        }
    }

    /// Checks if an implication (premise => conclusion) is valid.
    ///
    /// This is useful for checking contract conditions where the premise
    /// represents the assumed preconditions and the conclusion represents
    /// the postcondition to prove.
    pub fn check_implication(
        &self,
        premise: &z3::ast::Bool,
        conclusion: &z3::ast::Bool,
    ) -> VerificationResult<()> {
        let implication = premise.implies(conclusion);
        self.check_valid(&implication)
    }
}

/// Extracts a counterexample from a Z3 model.
fn extract_counterexample(_model: &z3::Model) -> Counterexample {
    let assignments = Vec::new();

    // Note: In a full implementation, we would iterate over the model
    // and extract all variable assignments. For the foundation, we
    // provide a basic implementation.

    let summary = "Counterexample available in model".to_string();

    Counterexample {
        assignments,
        summary,
    }
}

/// Converts a Z3 AST value to an SmtValue.
fn z3_value_to_smt_value(value: &z3::ast::Dynamic) -> SmtValue {
    // Try to extract as different types
    if let Some(int_val) = value.as_int() {
        if let Some(i) = int_val.as_i64() {
            return SmtValue::Int(i);
        }
    }

    if let Some(_bool_val) = value.as_bool() {
        // We can't directly extract the boolean value from the AST
        // In a full implementation, we'd use model evaluation
        return SmtValue::Unknown("bool".to_string());
    }

    SmtValue::Unknown(format!("{:?}", value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use z3::ast::Ast;
    use z3::Config;

    fn test_context() -> Context {
        let config = Config::new();
        Context::new(&config)
    }

    #[test]
    fn test_solver_creation() {
        let ctx = test_context();
        let solver = Solver::new(&ctx);
        assert!(solver.check_sat().is_ok());
    }

    #[test]
    fn test_check_valid_tautology() {
        let ctx = test_context();
        let solver = Solver::new(&ctx);

        // x = x is always valid (tautology)
        let x = z3::ast::Int::new_const(&ctx, "x");
        let condition = x._eq(&x);

        let result = solver.check_valid(&condition);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_sat_with_constraint() {
        let ctx = test_context();
        let solver = Solver::new(&ctx);

        // x > 0 and x < 10 should be satisfiable
        let x = z3::ast::Int::new_const(&ctx, "x");
        let gt_zero = x.gt(&z3::ast::Int::from_i64(&ctx, 0));
        let lt_ten = x.lt(&z3::ast::Int::from_i64(&ctx, 10));

        solver.solver.assert(&gt_zero);
        solver.solver.assert(&lt_ten);

        let result = solver.check_sat();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), true);
    }
}
