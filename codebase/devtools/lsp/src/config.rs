//! LSP workspace configuration (`.gradient/lsp.toml`).
//!
//! Closes adversarial finding F4 (input-surface companion to PR #508
//! `@untrusted` source mode). The LSP defaults to `@untrusted` mode for
//! every document so unsaved buffers — exactly the surface a hostile
//! agent uses to drive a developer's editor — get the same reduced
//! capability budget that the source-mode flag gives `gradient build`.
//!
//! Workspaces with `comptime` / FFI / inferred-effect builds opt back in
//! by writing:
//!
//! ```toml
//! # .gradient/lsp.toml
//! untrusted = false
//! ```
//!
//! See `docs/agent-integration.md` § "LSP trust mode" for the full
//! pattern and `docs/security/untrusted-source-mode.md` for the
//! restrictions enforced when the default is in effect.
//!
//! Tracking issue: #359. Companion source-mode PR: #508 (#360).

use std::path::{Path, PathBuf};

/// Workspace LSP configuration.
///
/// Loaded from `<root>/.gradient/lsp.toml`. Missing file = defaults.
/// Parse errors fall back to defaults (and the loader logs a warning;
/// we do not fail server startup on a malformed config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspConfig {
    /// True iff the LSP should treat unannotated documents as
    /// `@untrusted`. Defaults to `true` — see the module-level docs for
    /// rationale.
    pub untrusted: bool,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self { untrusted: true }
    }
}

impl LspConfig {
    /// Load `.gradient/lsp.toml` from the given workspace root.
    ///
    /// Returns the default config (`untrusted = true`) when the file is
    /// missing, unreadable, or malformed. The caller can inspect
    /// [`LoadOutcome`] to log a warning when a malformed file was
    /// silently ignored.
    pub fn load_from_workspace(root: &Path) -> (Self, LoadOutcome) {
        let path = root.join(".gradient").join("lsp.toml");
        Self::load_from_file(&path)
    }

    /// Load directly from a file path (used by tests and by
    /// [`load_from_workspace`]).
    pub fn load_from_file(path: &Path) -> (Self, LoadOutcome) {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), LoadOutcome::Missing);
            }
            Err(err) => {
                return (
                    Self::default(),
                    LoadOutcome::IoError {
                        path: path.to_path_buf(),
                        message: err.to_string(),
                    },
                );
            }
        };
        match parse_toml(&raw) {
            Ok(cfg) => (cfg, LoadOutcome::Loaded(path.to_path_buf())),
            Err(message) => (
                Self::default(),
                LoadOutcome::ParseError {
                    path: path.to_path_buf(),
                    message,
                },
            ),
        }
    }
}

/// Outcome of a config load attempt. Useful for the LSP backend to log
/// "loaded config from X" / "ignoring malformed config at X".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// File loaded successfully from this path.
    Loaded(PathBuf),
    /// File does not exist — defaults are in effect.
    Missing,
    /// File exists but I/O failed; defaults are in effect.
    IoError { path: PathBuf, message: String },
    /// File exists and was readable but did not parse; defaults are in
    /// effect.
    ParseError { path: PathBuf, message: String },
}

impl LoadOutcome {
    /// Human-readable summary for LSP `window/logMessage` / stderr.
    pub fn summary(&self) -> String {
        match self {
            LoadOutcome::Loaded(p) => format!("gradient-lsp: loaded config from {}", p.display()),
            LoadOutcome::Missing => {
                "gradient-lsp: no .gradient/lsp.toml — defaults in effect (untrusted = true)"
                    .to_string()
            }
            LoadOutcome::IoError { path, message } => format!(
                "gradient-lsp: could not read {} ({}); defaults in effect",
                path.display(),
                message
            ),
            LoadOutcome::ParseError { path, message } => format!(
                "gradient-lsp: malformed {} ({}); defaults in effect",
                path.display(),
                message
            ),
        }
    }
}

/// Minimal hand-rolled TOML parser tuned to this config's tiny schema.
///
/// We intentionally avoid pulling in the `toml` crate so the LSP binary
/// stays small and cold-builds quickly; the schema is one boolean with
/// optional comments/whitespace.
///
/// Accepted grammar (informal):
///
/// ```text
/// file        ::= line*
/// line        ::= comment | assignment | blank
/// comment     ::= '#' .* '\n'
/// blank       ::= whitespace* '\n'
/// assignment  ::= 'untrusted' '=' bool '\n'
/// bool        ::= 'true' | 'false'
/// ```
///
/// Anything else surfaces as a `ParseError` and the loader falls back
/// to defaults.
fn parse_toml(raw: &str) -> Result<LspConfig, String> {
    let mut cfg = LspConfig::default();
    let mut saw_untrusted = false;
    for (lineno, line) in raw.lines().enumerate() {
        let lineno = lineno + 1;
        // Strip inline comments and trim whitespace.
        let without_comment = match line.find('#') {
            Some(idx) => &line[..idx],
            None => line,
        };
        let trimmed = without_comment.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Look for `key = value`.
        let (key, value) = trimmed
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected `key = value`", lineno))?;
        let key = key.trim();
        let value = value.trim();
        match key {
            "untrusted" => {
                let parsed = match value {
                    "true" => true,
                    "false" => false,
                    other => {
                        return Err(format!(
                            "line {}: expected `true` or `false` for `untrusted`, got `{}`",
                            lineno, other
                        ));
                    }
                };
                if saw_untrusted {
                    return Err(format!("line {}: duplicate `untrusted` key", lineno));
                }
                cfg.untrusted = parsed;
                saw_untrusted = true;
            }
            other => {
                return Err(format!("line {}: unknown key `{}`", lineno, other));
            }
        }
    }
    Ok(cfg)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "gradient-lsp-config-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_config(root: &Path, body: &str) -> PathBuf {
        let cfg_dir = root.join(".gradient");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let path = cfg_dir.join("lsp.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn default_is_untrusted_true() {
        let cfg = LspConfig::default();
        assert!(cfg.untrusted, "default LSP trust mode must be untrusted");
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tmp_dir("missing");
        let (cfg, outcome) = LspConfig::load_from_workspace(&dir);
        assert_eq!(cfg, LspConfig::default());
        assert_eq!(outcome, LoadOutcome::Missing);
    }

    #[test]
    fn opt_in_to_trusted_mode() {
        let dir = tmp_dir("optin");
        write_config(&dir, "untrusted = false\n");
        let (cfg, outcome) = LspConfig::load_from_workspace(&dir);
        assert!(!cfg.untrusted, "explicit `untrusted = false` opts back in");
        assert!(matches!(outcome, LoadOutcome::Loaded(_)));
    }

    #[test]
    fn explicit_untrusted_true_matches_default() {
        let dir = tmp_dir("explicit-true");
        write_config(&dir, "untrusted = true\n");
        let (cfg, outcome) = LspConfig::load_from_workspace(&dir);
        assert!(cfg.untrusted);
        assert!(matches!(outcome, LoadOutcome::Loaded(_)));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let dir = tmp_dir("comments");
        write_config(
            &dir,
            "# top-level comment\n\nuntrusted = false  # inline comment\n\n",
        );
        let (cfg, _) = LspConfig::load_from_workspace(&dir);
        assert!(!cfg.untrusted);
    }

    #[test]
    fn malformed_falls_back_to_default_with_parse_error() {
        let dir = tmp_dir("malformed");
        write_config(&dir, "this is not valid toml\n");
        let (cfg, outcome) = LspConfig::load_from_workspace(&dir);
        assert_eq!(cfg, LspConfig::default());
        match outcome {
            LoadOutcome::ParseError { .. } => {}
            other => panic!("expected ParseError, got {:?}", other),
        }
    }

    #[test]
    fn unknown_key_falls_back_to_default() {
        let dir = tmp_dir("unknown-key");
        write_config(&dir, "frobnicate = 42\n");
        let (cfg, outcome) = LspConfig::load_from_workspace(&dir);
        assert_eq!(cfg, LspConfig::default());
        assert!(matches!(outcome, LoadOutcome::ParseError { .. }));
    }

    #[test]
    fn invalid_bool_value_falls_back_to_default() {
        let dir = tmp_dir("invalid-bool");
        write_config(&dir, "untrusted = sometimes\n");
        let (cfg, outcome) = LspConfig::load_from_workspace(&dir);
        assert_eq!(cfg, LspConfig::default());
        assert!(matches!(outcome, LoadOutcome::ParseError { .. }));
    }

    #[test]
    fn duplicate_key_is_an_error() {
        let dir = tmp_dir("duplicate-key");
        write_config(&dir, "untrusted = true\nuntrusted = false\n");
        let (cfg, outcome) = LspConfig::load_from_workspace(&dir);
        assert_eq!(cfg, LspConfig::default());
        assert!(matches!(outcome, LoadOutcome::ParseError { .. }));
    }

    #[test]
    fn summary_strings_are_informative() {
        assert!(LoadOutcome::Missing.summary().contains("defaults"));
        assert!(LoadOutcome::Loaded(PathBuf::from("/tmp/x"))
            .summary()
            .contains("/tmp/x"));
    }
}
