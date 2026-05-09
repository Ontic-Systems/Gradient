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
//! | `Async` | Cross-task / await-point concurrency |
//! | `Send` | Cross-task/actor message transfer |
//! | `Atomic` | Atomic memory operations |
//! | `Volatile` | Volatile memory access (MMIO / signal-safe; no elision/reorder) |
//! | `Heap` | Heap allocation (lists, records, alloc-backed collections) |
//! | `Stack` | Stack-only storage / frame-local memory tier marker |
//! | `Static` | Static storage / data-section memory tier marker |
//!
//! # Parameterized effects
//!
//! Two effects carry an ABI/exception-type parameter:
//!
//! | Form | Meaning |
//! |------|---------|
//! | `Throws(E)` | Function may raise exception of type `E` (ADR 0001) |
//! | `FFI(C)` / `FFI(Wasm)` / `FFI(Sysv)` | Function crosses an FFI ABI boundary (ADR 0002 / `#322`) |
//!
//! `FFI(_)` is the audit-trail effect for `extern fn` declarations: every
//! user-written `extern fn` is auto-tagged with `FFI(C)` (or whichever ABI the
//! caller declares) so that the call graph surfaces every C boundary. The
//! `Unsafe` capability gate (`#321` / `#322`) consumes this effect at the call
//! site once the capability typestate engine lands; until then the auto-tag
//! is the launch-tier audit trail.
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
    "IO", "Net", "FS", "Mut", "Time", "Actor", "Async", "Send", "Atomic", "Volatile", "Heap",
    "Stack", "Static",
];
/// Conservative default for `@extern` declarations that omit explicit effects.
pub const EXTERN_DEFAULT_EFFECTS: &[&str] = &["IO", "Net", "FS", "Mut", "Time"];

/// Check whether an effect name is recognized.
pub fn is_known_effect(name: &str) -> bool {
    KNOWN_EFFECTS.contains(&name)
}

/// Check whether an effect is a parameterized throw effect, e.g. `Throws(ParseError)`.
pub fn is_throws_effect(name: &str) -> bool {
    let Some(inner) = name
        .strip_prefix("Throws(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };

    is_effect_type_name(inner)
}

/// The set of ABI tags accepted inside `FFI(_)`.
///
/// `FFI(C)` is the default for `extern fn` declarations (ADR 0002).
/// Additional ABIs are gated by additional capabilities (`UnsafeWasm`,
/// `UnsafeSysv`, etc.) — those are deferred to follow-on sub-issues.
pub const KNOWN_FFI_ABIS: &[&str] = &["C", "Wasm", "Sysv"];

/// The default ABI tag synthesized into `FFI(_)` when an `extern fn`
/// declaration omits an explicit `FFI(...)` effect (ADR 0002 / `#322`).
pub const DEFAULT_FFI_ABI: &str = "C";

/// The full default `FFI(_)` effect synthesized into `extern fn`
/// declarations when they don't explicitly carry one.
pub const DEFAULT_FFI_EFFECT: &str = "FFI(C)";

/// Check whether an effect is a parameterized FFI ABI effect, e.g. `FFI(C)`.
///
/// Recognized ABIs are the entries in [`KNOWN_FFI_ABIS`].
pub fn is_ffi_effect(name: &str) -> bool {
    let Some(inner) = name
        .strip_prefix("FFI(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };

    KNOWN_FFI_ABIS.contains(&inner)
}

/// Return the ABI tag from an `FFI(_)` effect, or `None` if not an FFI effect.
pub fn ffi_abi_tag(name: &str) -> Option<&str> {
    let inner = name
        .strip_prefix("FFI(")
        .and_then(|rest| rest.strip_suffix(')'))?;

    if KNOWN_FFI_ABIS.contains(&inner) {
        Some(inner)
    } else {
        None
    }
}

/// Check whether an effect is a parameterized arena-region effect, e.g.
/// `Arena(scratch)`.
///
/// The inner argument is a user-bound arena identifier — like `Throws(E)`
/// the argument space is open-ended (any syntactically-valid effect-type
/// name). The actual binding/lifetime/typestate enforcement on the named
/// arena is deferred to issue #321 (capability typestate engine); this
/// recognizer covers the first deliverable from issue #320 (language-side
/// parser + checker recognition of the `!{Arena(_)}` effect).
///
/// Examples accepted:
/// - `Arena(a)`
/// - `Arena(scratch)`
/// - `Arena(_workspace)`
///
/// Examples rejected:
/// - `Arena()` (empty)
/// - `Arena(123foo)` (must start with a letter or underscore)
/// - `Arena(a` (unbalanced)
pub fn is_arena_effect(name: &str) -> bool {
    let Some(inner) = name
        .strip_prefix("Arena(")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };

    is_effect_type_name(inner)
}

/// Return the arena identifier from an `Arena(_)` effect, or `None` if
/// `name` is not a well-formed arena effect.
pub fn arena_region_name(name: &str) -> Option<&str> {
    let inner = name
        .strip_prefix("Arena(")
        .and_then(|rest| rest.strip_suffix(')'))?;

    if is_effect_type_name(inner) {
        Some(inner)
    } else {
        None
    }
}

/// Check whether an effect annotation is valid in a declaration.
pub fn is_valid_effect_name(name: &str) -> bool {
    is_known_effect(name)
        || is_effect_variable(name)
        || is_throws_effect(name)
        || is_ffi_effect(name)
        || is_arena_effect(name)
}

fn is_effect_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
        assert!(is_known_effect("Async"));
        assert!(is_known_effect("Send"));
        assert!(is_known_effect("Atomic"));
        assert!(is_known_effect("Volatile"));
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
    fn throws_effects_detected() {
        assert!(is_throws_effect("Throws(ParseError)"));
        assert!(is_throws_effect("Throws(_InternalError)"));
        assert!(is_throws_effect("Throws(Error123)"));
        assert!(!is_throws_effect("Throws()"));
        assert!(!is_throws_effect("Throws(123Error)"));
        assert!(!is_throws_effect("Throws(ParseError"));
        assert!(!is_throws_effect("Throw(ParseError)"));
    }

    #[test]
    fn ffi_effects_detected() {
        assert!(is_ffi_effect("FFI(C)"));
        assert!(is_ffi_effect("FFI(Wasm)"));
        assert!(is_ffi_effect("FFI(Sysv)"));
        assert!(!is_ffi_effect("FFI()"));
        assert!(!is_ffi_effect("FFI(c)")); // case-sensitive
        assert!(!is_ffi_effect("FFI(Rust)")); // unknown ABI
        assert!(!is_ffi_effect("FFI(C")); // unbalanced
        assert!(!is_ffi_effect("FF(C)"));
    }

    #[test]
    fn arena_effects_detected() {
        // Open-ended argument space (like `Throws(E)`): any
        // syntactically-valid effect-type name is accepted.
        assert!(is_arena_effect("Arena(a)"));
        assert!(is_arena_effect("Arena(scratch)"));
        assert!(is_arena_effect("Arena(_workspace)"));
        assert!(is_arena_effect("Arena(arena123)"));
        // Malformed shapes are rejected.
        assert!(!is_arena_effect("Arena()"));
        assert!(!is_arena_effect("Arena(123foo)"));
        assert!(!is_arena_effect("Arena(a"));
        assert!(!is_arena_effect("Aren(a)"));
        assert!(!is_arena_effect("arena(a)")); // case-sensitive
        assert!(!is_arena_effect("Arena(a-b)")); // hyphens not allowed
    }

    #[test]
    fn arena_region_name_returns_inner() {
        assert_eq!(arena_region_name("Arena(a)"), Some("a"));
        assert_eq!(arena_region_name("Arena(scratch)"), Some("scratch"));
        assert_eq!(arena_region_name("Arena(_workspace)"), Some("_workspace"));
        assert_eq!(arena_region_name("Arena(123bad)"), None);
        assert_eq!(arena_region_name("Throws(E)"), None);
        assert_eq!(arena_region_name("FFI(C)"), None);
        assert_eq!(arena_region_name("Heap"), None);
    }

    #[test]
    fn ffi_abi_tag_returns_inner() {
        assert_eq!(ffi_abi_tag("FFI(C)"), Some("C"));
        assert_eq!(ffi_abi_tag("FFI(Wasm)"), Some("Wasm"));
        assert_eq!(ffi_abi_tag("FFI(Rust)"), None);
        assert_eq!(ffi_abi_tag("Throws(E)"), None);
        assert_eq!(ffi_abi_tag("IO"), None);
    }

    #[test]
    fn default_ffi_constants_are_consistent() {
        assert_eq!(DEFAULT_FFI_ABI, "C");
        assert_eq!(DEFAULT_FFI_EFFECT, "FFI(C)");
        assert!(is_ffi_effect(DEFAULT_FFI_EFFECT));
        assert!(KNOWN_FFI_ABIS.contains(&DEFAULT_FFI_ABI));
    }

    #[test]
    fn valid_effect_names_include_known_variables_and_throws() {
        assert!(is_valid_effect_name("IO"));
        assert!(is_valid_effect_name("eff"));
        assert!(is_valid_effect_name("Throws(ParseError)"));
        assert!(is_valid_effect_name("FFI(C)"));
        assert!(is_valid_effect_name("FFI(Wasm)"));
        assert!(is_valid_effect_name("Arena(a)"));
        assert!(is_valid_effect_name("Arena(scratch)"));
        assert!(!is_valid_effect_name("Foo"));
        assert!(!is_valid_effect_name("Throws()"));
        assert!(!is_valid_effect_name("FFI(Rust)"));
        assert!(!is_valid_effect_name("Arena()"));
        assert!(!is_valid_effect_name("Arena(123bad)"));
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
        assert!(!is_effect_variable("Volatile"));
        assert!(!is_effect_variable(""));
    }
}
