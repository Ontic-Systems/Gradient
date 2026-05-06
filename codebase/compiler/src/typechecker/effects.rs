//! Effect system for the Gradient type checker.
//!
//! Effects are the core agent-first feature of Gradient. A function's effect
//! annotation is a **compiler-verified contract**: if a function declares no
//! effects, the compiler proves it cannot perform IO, mutate state, access
//! the network, or touch the filesystem. Agents can trust signatures.
//!
//! # Known effects
//!
//! | Effect | Meaning |
//! |--------|---------|
//! | `IO`   | Console/terminal I/O (print, read) |
//! | `Net`  | Network access (HTTP, sockets) |
//! | `FS`   | Filesystem access (read/write files) |
//! | `Mut`  | Observable mutation of shared state |
//! | `Time` | System clock access |
//! | `Actor` | Actor spawning/message-passing |
//! | `Heap` | Heap allocation (lists, records, alloc-backed collections) |
//! | `Stack` | Stack-only storage / frame-local memory tier marker |
//! | `Static` | Static storage / data-section memory tier marker |
//!
//! # Purity
//!
//! A function with no effect annotation is **pure by default**. The compiler
//! enforces this: calling an effectful function from a pure context is an error.
//! This means agents can read `fn compute(x: Int) -> Int` and know it has no
//! side effects — guaranteed by the compiler, not by convention.

use serde::Serialize;

/// The canonical set of effects recognized by the Gradient compiler.
///
/// Unknown effect names produce a compiler warning, encouraging users to
/// stick to the standard vocabulary so that agents can reason about code.
pub const KNOWN_EFFECTS: &[&str] = &[
    "IO", "Net", "FS", "Mut", "Time", "Actor", "Heap", "Stack", "Static",
];
/// Conservative default for `@extern` declarations that omit explicit effects.
pub const EXTERN_DEFAULT_EFFECTS: &[&str] = &["IO", "Net", "FS", "Mut", "Time"];

/// Check whether an effect name is recognized.
pub fn is_known_effect(name: &str) -> bool {
    KNOWN_EFFECTS.contains(&name)
}

/// Return the conservative default effect set for effect-omitted `@extern`s.
pub fn extern_default_effects() -> Vec<String> {
    EXTERN_DEFAULT_EFFECTS
        .iter()
        .map(|effect| (*effect).to_string())
        .collect()
}

/// Check whether a name in an effect set is an effect variable.
///
/// Effect variables are lowercase identifiers (e.g., `e`, `eff`).
/// Concrete effects are uppercase (e.g., `IO`, `Net`, `FS`).
pub fn is_effect_variable(name: &str) -> bool {
    name.chars()
        .next()
        .map(|c| c.is_ascii_lowercase())
        .unwrap_or(false)
}

/// Summary of effect analysis for a single function.
#[derive(Debug, Clone, Serialize)]
pub struct EffectInfo {
    /// The function's name.
    pub function: String,
    /// Effects declared in the function signature.
    pub declared: Vec<String>,
    /// Effects actually required by the function body (inferred).
    pub inferred: Vec<String>,
    /// Whether the function is provably pure (no inferred effects).
    pub is_pure: bool,
    /// Effects declared but not used (candidates for removal).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unused: Vec<String>,
    /// Effects used but not declared (would be errors caught by the checker).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

/// Summary of effect analysis for an entire module.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleEffectSummary {
    /// Per-function effect analysis.
    pub functions: Vec<EffectInfo>,
    /// Total number of provably pure functions.
    pub pure_count: usize,
    /// Total number of effectful functions.
    pub effectful_count: usize,
    /// All effects used anywhere in the module.
    pub effects_used: Vec<String>,
    /// Module-level capability ceiling (from `@cap(...)` declaration).
    /// If present, no function in this module may use effects outside this set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_ceiling: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_effects_recognized() {
        assert!(is_known_effect("IO"));
        assert!(is_known_effect("Net"));
        assert!(is_known_effect("FS"));
        assert!(is_known_effect("Mut"));
        assert!(is_known_effect("Time"));
        assert!(is_known_effect("Actor"));
        assert!(is_known_effect("Heap"));
        assert!(is_known_effect("Stack"));
        assert!(is_known_effect("Static"));
    }

    #[test]
    fn unknown_effects_rejected() {
        assert!(!is_known_effect("Foo"));
        assert!(!is_known_effect("io")); // case-sensitive
        assert!(!is_known_effect(""));
    }

    #[test]
    fn effect_variables_detected() {
        assert!(is_effect_variable("e"));
        assert!(is_effect_variable("eff"));
        assert!(is_effect_variable("e1"));
        assert!(is_effect_variable("myEffect"));
    }

    #[test]
    fn concrete_effects_not_variables() {
        assert!(!is_effect_variable("IO"));
        assert!(!is_effect_variable("Net"));
        assert!(!is_effect_variable("FS"));
        assert!(!is_effect_variable("Mut"));
        assert!(!is_effect_variable("Time"));
        assert!(!is_effect_variable("Stack"));
        assert!(!is_effect_variable("Static"));
        assert!(!is_effect_variable(""));
    }
}
