//! Structured error types for SMT verification.
//!
//! This module defines the error types returned when SMT verification fails,
//! providing detailed information about what contract condition failed and why.

use serde::{Deserialize, Serialize};

/// The result type for SMT verification operations.
pub type VerificationResult<T> = Result<T, VerificationError>;

/// A structured error indicating a failed contract verification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VerificationError {
    /// A precondition (requires clause) could not be proven.
    PreconditionFailed {
        /// The function name whose precondition failed.
        function: String,
        /// The index of the precondition clause that failed.
        clause_index: usize,
        /// The source text of the failed condition.
        condition: String,
        /// A human-readable explanation of why it failed.
        explanation: String,
    },

    /// A postcondition (ensures clause) could not be proven.
    PostconditionFailed {
        /// The function name whose postcondition failed.
        function: String,
        /// The index of the postcondition clause that failed.
        clause_index: usize,
        /// The source text of the failed condition.
        condition: String,
        /// A human-readable explanation of why it failed.
        explanation: String,
    },

    /// An error occurred while encoding a Gradient expression to SMT.
    EncodingError {
        /// Description of what went wrong during encoding.
        message: String,
        /// The source location if available.
        location: Option<String>,
    },

    /// The SMT solver returned an error or unexpected result.
    SolverError {
        /// The error message from the solver.
        message: String,
    },

    /// The verification was inconclusive (e.g., timeout or resource limit).
    Inconclusive {
        /// The reason why verification could not complete.
        reason: String,
    },
}

impl VerificationError {
    /// Returns a human-readable description of the error.
    pub fn message(&self) -> String {
        match self {
            VerificationError::PreconditionFailed {
                function,
                clause_index,
                condition,
                explanation,
            } => {
                format!(
                    "Precondition #{} of function '{}' failed: {}\n  Condition: {}\n  Explanation: {}",
                    clause_index + 1,
                    function,
                    condition,
                    condition,
                    explanation
                )
            }
            VerificationError::PostconditionFailed {
                function,
                clause_index,
                condition,
                explanation,
            } => {
                format!(
                    "Postcondition #{} of function '{}' failed: {}\n  Condition: {}\n  Explanation: {}",
                    clause_index + 1,
                    function,
                    condition,
                    condition,
                    explanation
                )
            }
            VerificationError::EncodingError { message, location } => {
                if let Some(loc) = location {
                    format!("SMT encoding error at {}: {}", loc, message)
                } else {
                    format!("SMT encoding error: {}", message)
                }
            }
            VerificationError::SolverError { message } => {
                format!("SMT solver error: {}", message)
            }
            VerificationError::Inconclusive { reason } => {
                format!("Verification inconclusive: {}", reason)
            }
        }
    }

    /// Returns true if this error represents a failed contract (pre/post condition).
    pub fn is_contract_failure(&self) -> bool {
        matches!(
            self,
            VerificationError::PreconditionFailed { .. }
                | VerificationError::PostconditionFailed { .. }
        )
    }

    /// Returns true if this error represents an internal error (encoding/solver).
    pub fn is_internal_error(&self) -> bool {
        matches!(
            self,
            VerificationError::EncodingError { .. } | VerificationError::SolverError { .. }
        )
    }
}

impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for VerificationError {}

/// A counterexample produced when a verification condition is falsifiable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Counterexample {
    /// The values assigned to variables in the counterexample.
    pub assignments: Vec<(String, SmtValue)>,
    /// A human-readable summary of the counterexample.
    pub summary: String,
}

/// A value in the SMT model (counterexample).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SmtValue {
    /// An integer value.
    Int(i64),
    /// A boolean value.
    Bool(bool),
    /// A string value (for debugging).
    String(String),
    /// An unknown/unsupported value.
    Unknown(String),
}

impl std::fmt::Display for SmtValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SmtValue::Int(i) => write!(f, "{}", i),
            SmtValue::Bool(b) => write!(f, "{}", b),
            SmtValue::String(s) => write!(f, "\"{}\"", s),
            SmtValue::Unknown(s) => write!(f, "<unknown: {}>", s),
        }
    }
}
