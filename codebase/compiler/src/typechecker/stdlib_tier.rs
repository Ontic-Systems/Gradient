//! Stdlib tier classification (`core` / `alloc` / `std`) — ADR 0005.
//!
//! Per [ADR 0005](../../../../docs/adr/0005-stdlib-split.md), the Gradient
//! standard library is partitioned into three tiers — `core`, `alloc`, `std`
//! — and the boundary between tiers is **defined by the effect rows their
//! builtins (and user functions) carry**. There is no `--features` flag for
//! the stdlib; the tier is a derived property of the effect closure.
//!
//! This module is the **scaffold** that issue
//! [#345](https://github.com/Ontic-Systems/Gradient/issues/345) calls for. It
//! provides:
//!
//! 1. The canonical [`StdlibTier`] enum.
//! 2. The pure [`classify_effects`] function deriving a tier from any effect
//!    row.
//! 3. Constant effect-set members for each tier (so downstream code that
//!    needs to test "does this builtin belong to `alloc`?" has a single
//!    source of truth).
//! 4. A [`TypeEnv::lookup_fn_tier`] helper (registered via
//!    [`super::env::TypeEnv`]) that classifies a registered builtin by name.
//!
//! The user-visible `.gr` import root remains a single namespace for now;
//! the tier check happens through the effect contract on each call (per
//! ADR 0005's "tier is computed, not declared" principle). Follow-on
//! sub-issues consume this scaffold:
//!
//! - [#347](https://github.com/Ontic-Systems/Gradient/issues/347) — `no_std`
//!   test matrix asserts a known-pure module classifies at `Core`.
//! - [#348](https://github.com/Ontic-Systems/Gradient/issues/348) — checker
//!   rejects `import std::<x>` where `<x>` is `Std`-tier and the importing
//!   module's effect closure does not surface a `Std`-tier effect.
//! - Epic E5 ([#298](https://github.com/Ontic-Systems/Gradient/issues/298))
//!   — modular runtime DCE drops `alloc`/`std` runtime support when the
//!   linked program's tier is lower.

use std::fmt;

/// The three Gradient stdlib tiers.
///
/// Order is meaningful: `Core < Alloc < Std`. A function classifies into the
/// **smallest** tier whose effect contract covers its declared effect row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StdlibTier {
    /// Zero `!{Heap}`, zero `!{IO}`, zero `!{FS}`, zero `!{Net}`,
    /// zero `!{Time}`, zero `!{Mut}`. Pure data manipulation, integer/float
    /// math, Option/Result combinators, atomic primitives, comparison
    /// helpers.
    Core,
    /// `Core` + `!{Heap}`. Owned containers, heap-backed string formatting,
    /// list/map/set, refcounted handles, COW.
    Alloc,
    /// `Alloc` + `!{IO}` + `!{FS}` + `!{Net}` + `!{Time}` + `!{Mut}`.
    /// Anything that touches the operating system, the network, or the
    /// wall clock.
    Std,
}

impl StdlibTier {
    /// Return the tier's stable string slug.
    ///
    /// Matches the ADR 0005 / `docs/stdlib-migration.md` vocabulary.
    pub const fn as_str(self) -> &'static str {
        match self {
            StdlibTier::Core => "core",
            StdlibTier::Alloc => "alloc",
            StdlibTier::Std => "std",
        }
    }

    /// All three tiers in order, for iteration / catalog generation.
    pub const ALL: [StdlibTier; 3] = [StdlibTier::Core, StdlibTier::Alloc, StdlibTier::Std];
}

impl fmt::Display for StdlibTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Effects that **promote a function into the `Alloc` tier**.
///
/// Any function carrying one of these in its effect row needs a heap
/// allocator at runtime and therefore cannot live at the `Core` tier.
pub const ALLOC_TIER_EFFECTS: &[&str] = &["Heap"];

/// Effects that **promote a function into the `Std` tier**.
///
/// Any function carrying one of these in its effect row touches the
/// operating system, the network, or wall-clock state, and therefore needs
/// the `Std` runtime support.
///
/// `Mut` is included because process-tier mutable state (env vars, global
/// allocator state observable from outside the call) is a `Std`-tier
/// concern even when no I/O syscall is invoked directly.
pub const STD_TIER_EFFECTS: &[&str] = &["IO", "FS", "Net", "Time", "Mut"];

/// Classify an effect row into a [`StdlibTier`].
///
/// The classification rule (per ADR 0005):
///
/// 1. If any effect in the row is in [`STD_TIER_EFFECTS`], the tier is
///    [`StdlibTier::Std`].
/// 2. Else if any effect is in [`ALLOC_TIER_EFFECTS`], the tier is
///    [`StdlibTier::Alloc`].
/// 3. Else (including the empty effect row), the tier is
///    [`StdlibTier::Core`].
///
/// **Out-of-axis effects** — `Async`, `Send`, `Atomic`, `Volatile`,
/// `Stack`, `Static`, `Throws(_)`, `FFI(_)`, `Actor`, and any effect
/// variable — are deliberately ignored. They classify orthogonally to the
/// `core`/`alloc`/`std` axis (per ADR 0005 "Neutral / deferred" §:
/// `gradient bindgen`-generated externs default to `!{FFI(C), Unsafe}` and
/// a `core`-tier consumer can still call them).
///
/// # Examples
///
/// ```
/// use gradient_compiler::typechecker::stdlib_tier::{classify_effects, StdlibTier};
///
/// // Pure function: Core.
/// let empty: [String; 0] = [];
/// assert_eq!(classify_effects(&empty), StdlibTier::Core);
///
/// // Allocator: Alloc.
/// assert_eq!(
///     classify_effects(&["Heap".to_string()]),
///     StdlibTier::Alloc,
/// );
///
/// // I/O: Std.
/// assert_eq!(
///     classify_effects(&["IO".to_string(), "FS".to_string()]),
///     StdlibTier::Std,
/// );
///
/// // Std wins over Alloc when both are present.
/// assert_eq!(
///     classify_effects(&["Heap".to_string(), "IO".to_string()]),
///     StdlibTier::Std,
/// );
///
/// // Out-of-axis effects don't promote.
/// assert_eq!(
///     classify_effects(&["Atomic".to_string(), "Volatile".to_string()]),
///     StdlibTier::Core,
/// );
/// ```
pub fn classify_effects<S: AsRef<str>>(effects: &[S]) -> StdlibTier {
    let mut has_alloc = false;

    for eff in effects {
        let name = eff.as_ref();
        if STD_TIER_EFFECTS.contains(&name) {
            // Short-circuit: Std is the maximum tier.
            return StdlibTier::Std;
        }
        if ALLOC_TIER_EFFECTS.contains(&name) {
            has_alloc = true;
        }
    }

    if has_alloc {
        StdlibTier::Alloc
    } else {
        StdlibTier::Core
    }
}

/// Return `true` if `tier` is permitted under `module_tier`.
///
/// A module declared at tier `module_tier` may call any function whose
/// classified tier is **less than or equal to** `module_tier`. This is the
/// rule that the `import std in no_std module = compile error` rejection
/// (#348) will key off.
///
/// # Examples
///
/// ```
/// use gradient_compiler::typechecker::stdlib_tier::{permitted_under, StdlibTier};
///
/// // Std module can call anything.
/// assert!(permitted_under(StdlibTier::Core, StdlibTier::Std));
/// assert!(permitted_under(StdlibTier::Alloc, StdlibTier::Std));
/// assert!(permitted_under(StdlibTier::Std, StdlibTier::Std));
///
/// // Core module can only call Core.
/// assert!(permitted_under(StdlibTier::Core, StdlibTier::Core));
/// assert!(!permitted_under(StdlibTier::Alloc, StdlibTier::Core));
/// assert!(!permitted_under(StdlibTier::Std, StdlibTier::Core));
/// ```
pub fn permitted_under(callee_tier: StdlibTier, module_tier: StdlibTier) -> bool {
    callee_tier <= module_tier
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_effect_row_classifies_as_core() {
        let empty: [String; 0] = [];
        assert_eq!(classify_effects(&empty), StdlibTier::Core);
    }

    #[test]
    fn single_heap_classifies_as_alloc() {
        assert_eq!(classify_effects(&["Heap".to_string()]), StdlibTier::Alloc);
    }

    #[test]
    fn each_std_effect_classifies_as_std() {
        for eff in STD_TIER_EFFECTS {
            assert_eq!(
                classify_effects(&[(*eff).to_string()]),
                StdlibTier::Std,
                "{eff} should classify as Std",
            );
        }
    }

    #[test]
    fn std_dominates_alloc() {
        assert_eq!(
            classify_effects(&["Heap".to_string(), "IO".to_string()]),
            StdlibTier::Std,
        );
        assert_eq!(
            classify_effects(&["Net".to_string(), "Heap".to_string()]),
            StdlibTier::Std,
        );
    }

    #[test]
    fn out_of_axis_effects_do_not_promote() {
        // Async, Send, Atomic, Volatile, Stack, Static — none promote.
        for eff in ["Async", "Send", "Atomic", "Volatile", "Stack", "Static"] {
            assert_eq!(
                classify_effects(&[eff.to_string()]),
                StdlibTier::Core,
                "{eff} should not promote past Core",
            );
        }
    }

    #[test]
    fn parameterized_throws_does_not_promote() {
        // Throws(E) is orthogonal to the tier axis.
        assert_eq!(
            classify_effects(&["Throws(ParseError)".to_string()]),
            StdlibTier::Core,
        );
    }

    #[test]
    fn parameterized_ffi_does_not_promote() {
        // FFI(C) is orthogonal — bindgen externs at core consumer remain core.
        assert_eq!(classify_effects(&["FFI(C)".to_string()]), StdlibTier::Core);
        assert_eq!(
            classify_effects(&["FFI(Wasm)".to_string()]),
            StdlibTier::Core,
        );
    }

    #[test]
    fn alloc_with_orthogonal_stays_alloc() {
        assert_eq!(
            classify_effects(&[
                "Heap".to_string(),
                "Throws(ParseError)".to_string(),
                "Atomic".to_string(),
            ]),
            StdlibTier::Alloc,
        );
    }

    #[test]
    fn tier_ordering_matches_inclusion() {
        assert!(StdlibTier::Core < StdlibTier::Alloc);
        assert!(StdlibTier::Alloc < StdlibTier::Std);
        assert!(StdlibTier::Core < StdlibTier::Std);
    }

    #[test]
    fn tier_as_str_matches_adr_vocabulary() {
        assert_eq!(StdlibTier::Core.as_str(), "core");
        assert_eq!(StdlibTier::Alloc.as_str(), "alloc");
        assert_eq!(StdlibTier::Std.as_str(), "std");
    }

    #[test]
    fn tier_display_matches_as_str() {
        assert_eq!(format!("{}", StdlibTier::Core), "core");
        assert_eq!(format!("{}", StdlibTier::Alloc), "alloc");
        assert_eq!(format!("{}", StdlibTier::Std), "std");
    }

    #[test]
    fn permitted_under_respects_inclusion() {
        // Std module accepts everything.
        assert!(permitted_under(StdlibTier::Core, StdlibTier::Std));
        assert!(permitted_under(StdlibTier::Alloc, StdlibTier::Std));
        assert!(permitted_under(StdlibTier::Std, StdlibTier::Std));

        // Alloc module rejects Std but accepts Core/Alloc.
        assert!(permitted_under(StdlibTier::Core, StdlibTier::Alloc));
        assert!(permitted_under(StdlibTier::Alloc, StdlibTier::Alloc));
        assert!(!permitted_under(StdlibTier::Std, StdlibTier::Alloc));

        // Core module accepts only Core.
        assert!(permitted_under(StdlibTier::Core, StdlibTier::Core));
        assert!(!permitted_under(StdlibTier::Alloc, StdlibTier::Core));
        assert!(!permitted_under(StdlibTier::Std, StdlibTier::Core));
    }

    #[test]
    fn all_iter_covers_every_tier() {
        assert_eq!(StdlibTier::ALL.len(), 3);
        assert!(StdlibTier::ALL.contains(&StdlibTier::Core));
        assert!(StdlibTier::ALL.contains(&StdlibTier::Alloc));
        assert!(StdlibTier::ALL.contains(&StdlibTier::Std));
    }

    #[test]
    fn classify_accepts_str_slices_and_strings() {
        // Both &str and String slices should classify identically.
        let a: [&str; 1] = ["Heap"];
        let b: [String; 1] = ["Heap".to_string()];
        assert_eq!(classify_effects(&a), classify_effects(&b));
    }
}
