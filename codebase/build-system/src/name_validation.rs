//! Strict validators for package names, version strings, and cache paths.
//!
//! Issue #177: package names and version strings flow from manifests
//! (potentially attacker-controlled TOML) directly into filesystem joins
//! such as `~/.gradient/cache/{name}/{version}`. Without validation, a
//! crafted name like `../../etc` or one containing NUL/control chars could
//! escape the cache root or interact unsafely with the OS. This module
//! provides:
//!
//! * [`validate_package_name`] — strict allowlist for package names.
//! * [`validate_version`] — strict allowlist for version strings.
//! * [`safe_cache_path`] — validates name+version, joins them onto a
//!   provided cache root, and verifies the resulting path is contained
//!   within that root (defence-in-depth against canonicalization tricks).
//!
//! All checks are performed *before* any filesystem operation.
//!
//! Allowed package names match `^[a-z0-9][a-z0-9_-]*$` with a length cap.
//! Allowed versions match a conservative subset of SemVer:
//! `^[0-9A-Za-z.+_-]+$` (no path separators, no NUL, no control chars,
//! no `..` segments) with a length cap.

use std::path::{Component, Path, PathBuf};

/// Maximum permitted length for a package name (bytes).
pub const MAX_NAME_LEN: usize = 128;

/// Maximum permitted length for a version string (bytes).
pub const MAX_VERSION_LEN: usize = 64;

/// Errors produced by name/version/path validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// Input was empty.
    Empty,
    /// Input exceeded the maximum permitted length.
    TooLong { len: usize, max: usize },
    /// Input contained a NUL or ASCII control character.
    ControlChar,
    /// Input contained a path separator (`/` or `\`).
    PathSeparator,
    /// Input contained a `..` segment (potential path traversal).
    DotDot,
    /// Input started with `.` (hidden file / dot segment).
    LeadingDot,
    /// Input started with `-` (could be confused with CLI flags).
    LeadingHyphen,
    /// Input contained a character outside the allowlist.
    DisallowedChar(char),
    /// The constructed path escapes the provided cache root.
    EscapesRoot,
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameError::Empty => write!(f, "value must not be empty"),
            NameError::TooLong { len, max } => {
                write!(f, "value is {} bytes, exceeds maximum of {}", len, max)
            }
            NameError::ControlChar => {
                write!(f, "value contains a NUL or ASCII control character")
            }
            NameError::PathSeparator => write!(f, "value contains a path separator"),
            NameError::DotDot => write!(f, "value contains a '..' segment"),
            NameError::LeadingDot => write!(f, "value starts with '.'"),
            NameError::LeadingHyphen => write!(f, "value starts with '-'"),
            NameError::DisallowedChar(c) => {
                write!(f, "value contains disallowed character {:?}", c)
            }
            NameError::EscapesRoot => {
                write!(f, "constructed path escapes the cache root")
            }
        }
    }
}

impl std::error::Error for NameError {}

/// Common rejections that apply to *every* path component candidate.
fn check_common(s: &str) -> Result<(), NameError> {
    if s.is_empty() {
        return Err(NameError::Empty);
    }

    // Reject NUL and any ASCII control character (0x00..=0x1F, 0x7F).
    if s.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return Err(NameError::ControlChar);
    }

    // Reject path separators on any platform.
    if s.contains('/') || s.contains('\\') {
        return Err(NameError::PathSeparator);
    }

    // Reject `..` (anywhere) and bare `.` to defeat traversal segments.
    if s == "." || s == ".." || s.contains("..") {
        return Err(NameError::DotDot);
    }

    Ok(())
}

/// Validate a package name against a strict allowlist.
///
/// Rules:
///   - non-empty, length <= [`MAX_NAME_LEN`]
///   - no NUL or ASCII control chars
///   - no `/` or `\`
///   - no `..` segment
///   - must not start with `.` or `-`
///   - allowed characters: lowercase ASCII letters, digits, `_`, `-`
///   - first char must be lowercase letter or digit
pub fn validate_package_name(s: &str) -> Result<&str, NameError> {
    check_common(s)?;

    if s.len() > MAX_NAME_LEN {
        return Err(NameError::TooLong {
            len: s.len(),
            max: MAX_NAME_LEN,
        });
    }

    let mut chars = s.chars();
    let first = chars.next().expect("non-empty checked above");
    if first == '.' {
        return Err(NameError::LeadingDot);
    }
    if first == '-' {
        return Err(NameError::LeadingHyphen);
    }
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(NameError::DisallowedChar(first));
    }

    for c in s.chars() {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_';
        if !ok {
            return Err(NameError::DisallowedChar(c));
        }
    }

    Ok(s)
}

/// Validate a version string for safe use as a path component.
///
/// Rules:
///   - non-empty, length <= [`MAX_VERSION_LEN`]
///   - no NUL or ASCII control chars
///   - no `/` or `\`
///   - no `..` segment
///   - must not start with `.` or `-`
///   - allowed characters: ASCII alphanumerics plus `.` `+` `-` `_`
///
/// This is a conservative subset of SemVer — the actual semver parser is
/// applied separately. This validator only enforces filesystem safety.
pub fn validate_version(s: &str) -> Result<&str, NameError> {
    check_common(s)?;

    if s.len() > MAX_VERSION_LEN {
        return Err(NameError::TooLong {
            len: s.len(),
            max: MAX_VERSION_LEN,
        });
    }

    let first = s.chars().next().expect("non-empty checked above");
    if first == '.' {
        return Err(NameError::LeadingDot);
    }
    if first == '-' {
        return Err(NameError::LeadingHyphen);
    }

    for c in s.chars() {
        let ok = c.is_ascii_alphanumeric() || c == '.' || c == '+' || c == '-' || c == '_';
        if !ok {
            return Err(NameError::DisallowedChar(c));
        }
    }

    Ok(s)
}

/// Build a cache subpath of the form `{root}/{name}/{version}`, validating
/// `name` and `version` and verifying that the resulting path is contained
/// within `root`.
///
/// `root` is treated as already trusted (it comes from
/// `~/.gradient/cache`, which is constructed from the process environment,
/// not from manifests). The function does not require `root` to exist on
/// disk — `Path::components` plus a logical containment check is used so
/// the function works before the cache directory is created.
pub fn safe_cache_path(root: &Path, name: &str, version: &str) -> Result<PathBuf, NameError> {
    let name = validate_package_name(name)?;
    let version = validate_version(version)?;

    let candidate = root.join(name).join(version);

    // Defence-in-depth: walk components and ensure no `..` or absolute
    // root snuck in via a future code path. After validation above this
    // shouldn't be reachable for adversarial inputs, but cheap to enforce.
    let root_components: Vec<Component<'_>> = root.components().collect();
    let cand_components: Vec<Component<'_>> = candidate.components().collect();

    if cand_components.len() < root_components.len() + 2 {
        return Err(NameError::EscapesRoot);
    }
    for (i, rc) in root_components.iter().enumerate() {
        if cand_components[i] != *rc {
            return Err(NameError::EscapesRoot);
        }
    }
    for c in &cand_components[root_components.len()..] {
        match c {
            Component::Normal(_) => {}
            _ => return Err(NameError::EscapesRoot),
        }
    }

    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---------- validate_package_name ----------

    #[test]
    fn name_accepts_simple_lowercase() {
        assert!(validate_package_name("math").is_ok());
        assert!(validate_package_name("my-package").is_ok());
        assert!(validate_package_name("my_package").is_ok());
        assert!(validate_package_name("pkg123").is_ok());
        assert!(validate_package_name("0pkg").is_ok()); // leading digit allowed
        assert!(validate_package_name("a").is_ok()); // single char
    }

    #[test]
    fn name_rejects_empty() {
        assert_eq!(validate_package_name(""), Err(NameError::Empty));
    }

    #[test]
    fn name_rejects_path_traversal() {
        assert_eq!(validate_package_name(".."), Err(NameError::DotDot));
        assert_eq!(
            validate_package_name("../etc"),
            Err(NameError::PathSeparator)
        );
        assert_eq!(
            validate_package_name("foo/../bar"),
            Err(NameError::PathSeparator)
        );
        // Pure dot segments without slashes still rejected
        assert_eq!(validate_package_name("foo..bar"), Err(NameError::DotDot));
    }

    #[test]
    fn name_rejects_path_separators() {
        assert_eq!(
            validate_package_name("/etc/passwd"),
            Err(NameError::PathSeparator)
        );
        assert_eq!(
            validate_package_name("foo/bar"),
            Err(NameError::PathSeparator)
        );
        assert_eq!(
            validate_package_name("foo\\bar"),
            Err(NameError::PathSeparator)
        );
    }

    #[test]
    fn name_rejects_nul_and_control_chars() {
        assert_eq!(
            validate_package_name("foo\0bar"),
            Err(NameError::ControlChar)
        );
        assert_eq!(
            validate_package_name("foo\x01bar"),
            Err(NameError::ControlChar)
        );
        assert_eq!(
            validate_package_name("foo\nbar"),
            Err(NameError::ControlChar)
        );
        assert_eq!(
            validate_package_name("foo\tbar"),
            Err(NameError::ControlChar)
        );
        assert_eq!(
            validate_package_name("foo\x7Fbar"),
            Err(NameError::ControlChar)
        );
    }

    #[test]
    fn name_rejects_leading_dot() {
        assert_eq!(validate_package_name("."), Err(NameError::DotDot));
        assert_eq!(validate_package_name(".hidden"), Err(NameError::LeadingDot));
    }

    #[test]
    fn name_rejects_leading_hyphen() {
        assert_eq!(
            validate_package_name("-rf"),
            Err(NameError::LeadingHyphen)
        );
    }

    #[test]
    fn name_rejects_uppercase() {
        // Mixed-case-only allowlist test: uppercase is disallowed.
        assert_eq!(
            validate_package_name("Foo"),
            Err(NameError::DisallowedChar('F'))
        );
        assert_eq!(
            validate_package_name("fooBar"),
            Err(NameError::DisallowedChar('B'))
        );
    }

    #[test]
    fn name_rejects_unicode_lookalikes() {
        // Cyrillic 'а' (U+0430) is not ASCII lowercase.
        assert!(matches!(
            validate_package_name("\u{0430}pkg"),
            Err(NameError::DisallowedChar(_))
        ));
    }

    #[test]
    fn name_rejects_special_chars() {
        for bad in ["foo!", "foo bar", "foo@bar", "foo:bar", "foo$bar"] {
            assert!(
                matches!(validate_package_name(bad), Err(NameError::DisallowedChar(_))),
                "expected DisallowedChar for {:?}",
                bad
            );
        }
    }

    #[test]
    fn name_rejects_too_long() {
        let long = "a".repeat(MAX_NAME_LEN + 1);
        assert!(matches!(
            validate_package_name(&long),
            Err(NameError::TooLong { .. })
        ));
        // exactly at the limit is fine
        let at_limit = "a".repeat(MAX_NAME_LEN);
        assert!(validate_package_name(&at_limit).is_ok());
    }

    // ---------- validate_version ----------

    #[test]
    fn version_accepts_typical_semver() {
        assert!(validate_version("1.0.0").is_ok());
        assert!(validate_version("0.1.0").is_ok());
        assert!(validate_version("1.2.3-alpha.1").is_ok());
        assert!(validate_version("1.0.0+build.42").is_ok());
        assert!(validate_version("1.0.0-rc.1+sha.abc").is_ok());
    }

    #[test]
    fn version_rejects_path_traversal_and_separators() {
        assert_eq!(
            validate_version("../1.0.0"),
            Err(NameError::PathSeparator)
        );
        assert_eq!(
            validate_version("1.0.0/extra"),
            Err(NameError::PathSeparator)
        );
        assert_eq!(
            validate_version("1.0.0\\extra"),
            Err(NameError::PathSeparator)
        );
        assert_eq!(validate_version(".."), Err(NameError::DotDot));
        // Embedded dot-dot
        assert_eq!(validate_version("1..0"), Err(NameError::DotDot));
    }

    #[test]
    fn version_rejects_nul_and_controls() {
        assert_eq!(validate_version("1.0\0"), Err(NameError::ControlChar));
        assert_eq!(validate_version("1.0\x01"), Err(NameError::ControlChar));
    }

    #[test]
    fn version_rejects_leading_dot_or_hyphen() {
        assert_eq!(validate_version(".1.0"), Err(NameError::LeadingDot));
        assert_eq!(validate_version("-1.0"), Err(NameError::LeadingHyphen));
    }

    #[test]
    fn version_rejects_disallowed_chars() {
        for bad in ["1.0 0", "1.0;rm", "1.0$x", "1.0,x"] {
            assert!(
                matches!(validate_version(bad), Err(NameError::DisallowedChar(_))),
                "expected DisallowedChar for {:?}",
                bad
            );
        }
    }

    #[test]
    fn version_rejects_too_long() {
        let long = "1".repeat(MAX_VERSION_LEN + 1);
        assert!(matches!(
            validate_version(&long),
            Err(NameError::TooLong { .. })
        ));
    }

    // ---------- safe_cache_path ----------

    #[test]
    fn safe_cache_path_happy() {
        let root = PathBuf::from("/home/u/.gradient/cache");
        let p = safe_cache_path(&root, "math", "1.0.0").unwrap();
        assert_eq!(p, root.join("math").join("1.0.0"));
    }

    #[test]
    fn safe_cache_path_rejects_traversal_in_name() {
        let root = PathBuf::from("/home/u/.gradient/cache");
        assert!(safe_cache_path(&root, "../etc", "1.0.0").is_err());
        assert!(safe_cache_path(&root, "..", "1.0.0").is_err());
        assert!(safe_cache_path(&root, "/etc/passwd", "1.0.0").is_err());
        assert!(safe_cache_path(&root, "foo\\bar", "1.0.0").is_err());
    }

    #[test]
    fn safe_cache_path_rejects_traversal_in_version() {
        let root = PathBuf::from("/home/u/.gradient/cache");
        assert!(safe_cache_path(&root, "math", "../1.0.0").is_err());
        assert!(safe_cache_path(&root, "math", "1.0.0/extra").is_err());
        assert!(safe_cache_path(&root, "math", "1.0\0").is_err());
    }

    #[test]
    fn safe_cache_path_rejects_nul() {
        let root = PathBuf::from("/home/u/.gradient/cache");
        assert!(safe_cache_path(&root, "ma\0th", "1.0.0").is_err());
    }

    #[test]
    fn safe_cache_path_relative_root() {
        // Works with a relative root too; result remains under it.
        let root = PathBuf::from("cache");
        let p = safe_cache_path(&root, "math", "1.0.0").unwrap();
        assert!(p.starts_with(&root));
        assert_eq!(p, PathBuf::from("cache").join("math").join("1.0.0"));
    }
}
