//! Verification Condition (VC) intermediate representation.
//!
//! This module defines the launch-tier data structures for the static
//! contract verification pipeline established by ADR 0003 (tiered
//! contracts). The VC IR is the bridge between the typechecked AST and
//! an SMT solver: a `VerificationCondition` packages a single proof
//! obligation with the source-level metadata needed to translate a
//! solver counterexample back into a Gradient diagnostic.
//!
//! Scope at this milestone (sub-issue #327):
//!
//! - Define the data structures so downstream issues (#328 VC generator,
//!   #329 Z3 integration + counterexample translation) have a fixed
//!   surface to target.
//! - Construct empty `VerificationConditionSet`s for `@verified`
//!   functions during checker traversal so the checker can record which
//!   functions opted into the verified tier.
//! - Do NOT yet translate function bodies to SMT-LIB (#328) or invoke
//!   Z3 (#329). The launch tier is "annotation parses + recognized; VC
//!   structures exist; pipeline emits an unimplemented warning".
//!
//! See ADR 0003 § "VC generator (sub-issue #328)" for the launch-tier
//! semantics this stub anchors.

use crate::ast::item::ContractKind;
use crate::ast::span::Span;

/// A single proof obligation derived from one function contract.
///
/// In the launch tier (this PR), `VerificationCondition` carries the
/// minimum metadata the checker needs to record that an obligation
/// would be emitted; the SMT-LIB translation lives in #328.
#[derive(Debug, Clone, PartialEq)]
pub struct VerificationCondition {
    /// Which contract this obligation derives from (precondition vs
    /// postcondition).
    pub kind: ContractKind,
    /// The source span of the originating `@requires` or `@ensures`
    /// annotation. Used for counterexample diagnostics in #329.
    pub origin_span: Span,
    /// Whether the VC translation pipeline (#328) is wired for this
    /// obligation. `false` at the launch tier; flips to `true` when
    /// the body-to-SMT translator is in place.
    pub translated: bool,
}

/// All proof obligations derived from a single `@verified` function.
///
/// Wraps `Vec<VerificationCondition>` with the function name so that
/// downstream diagnostics can reference the function unambiguously.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VerificationConditionSet {
    /// The function this set of obligations belongs to.
    pub fn_name: String,
    /// One `VerificationCondition` per `@requires` / `@ensures` on the
    /// function. Empty for a `@verified` function with no contracts —
    /// which is itself a checker error per ADR 0003.
    pub conditions: Vec<VerificationCondition>,
}

impl VerificationConditionSet {
    /// Construct a new (empty) set for the named function.
    pub fn new(fn_name: impl Into<String>) -> Self {
        Self {
            fn_name: fn_name.into(),
            conditions: Vec::new(),
        }
    }

    /// Append a stub VC referencing the originating annotation span.
    ///
    /// `translated` defaults to `false` because the body-to-SMT
    /// translator (#328) is not yet implemented; the obligation exists
    /// only as a placeholder in the launch tier.
    pub fn add_stub(&mut self, kind: ContractKind, origin_span: Span) {
        self.conditions.push(VerificationCondition {
            kind,
            origin_span,
            translated: false,
        });
    }

    /// Number of obligations recorded.
    pub fn len(&self) -> usize {
        self.conditions.len()
    }

    /// Whether this set carries no obligations.
    pub fn is_empty(&self) -> bool {
        self.conditions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::span::Position;

    fn dummy_span() -> Span {
        Span::new(0, Position::new(1, 1, 0), Position::new(1, 1, 0))
    }

    #[test]
    fn empty_set_is_empty() {
        let set = VerificationConditionSet::new("clamp_nonneg");
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert_eq!(set.fn_name, "clamp_nonneg");
    }

    #[test]
    fn add_stub_records_kind_and_span() {
        let mut set = VerificationConditionSet::new("clamp_nonneg");
        set.add_stub(ContractKind::Requires, dummy_span());
        set.add_stub(ContractKind::Ensures, dummy_span());
        assert_eq!(set.len(), 2);
        assert_eq!(set.conditions[0].kind, ContractKind::Requires);
        assert_eq!(set.conditions[1].kind, ContractKind::Ensures);
        // Launch-tier stubs are NOT yet translated to SMT-LIB.
        assert!(!set.conditions[0].translated);
        assert!(!set.conditions[1].translated);
    }
}
