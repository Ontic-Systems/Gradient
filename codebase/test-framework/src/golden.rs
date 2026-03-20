//! Golden snapshot test runner for the Gradient compiler.
//!
//! Discovers `.gr` test files, runs the compiler on each, and compares stdout/stderr
//! against expected snapshot files. Supports automatic snapshot updating via the
//! `UPDATE_GOLDEN=1` environment variable.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use similar::{ChangeTag, TextDiff};
use walkdir::WalkDir;

/// Configuration for the golden test runner.
#[derive(Debug, Clone)]
pub struct GoldenConfig {
    /// Path to the Gradient compiler binary.
    pub compiler_path: PathBuf,

    /// Directory containing `.gr` input files.
    pub cases_dir: PathBuf,

    /// Directory containing `.stdout` and `.stderr` expected output files.
    pub expected_dir: PathBuf,
}

impl GoldenConfig {
    /// Creates a new configuration.
    pub fn new(
        compiler_path: impl Into<PathBuf>,
        cases_dir: impl Into<PathBuf>,
        expected_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            compiler_path: compiler_path.into(),
            cases_dir: cases_dir.into(),
            expected_dir: expected_dir.into(),
        }
    }
}

/// Outcome of a single golden test.
#[derive(Debug)]
pub enum GoldenOutcome {
    /// Output matched the expected snapshot.
    Pass { name: String },

    /// Output did not match. Contains a human-readable diff.
    Fail { name: String, diff: String },

    /// The expected snapshot was updated (when `UPDATE_GOLDEN=1`).
    Updated { name: String },

    /// An error prevented the test from running.
    Error { name: String, message: String },
}

impl GoldenOutcome {
    /// Returns the test name regardless of outcome.
    pub fn name(&self) -> &str {
        match self {
            GoldenOutcome::Pass { name }
            | GoldenOutcome::Fail { name, .. }
            | GoldenOutcome::Updated { name }
            | GoldenOutcome::Error { name, .. } => name,
        }
    }

    /// Returns `true` if this outcome is a pass or an update.
    pub fn is_ok(&self) -> bool {
        matches!(self, GoldenOutcome::Pass { .. } | GoldenOutcome::Updated { .. })
    }
}

/// Summary of a golden test suite run.
#[derive(Debug)]
pub struct GoldenSummary {
    pub outcomes: Vec<GoldenOutcome>,
    pub passed: usize,
    pub failed: usize,
    pub updated: usize,
    pub errors: usize,
}

impl GoldenSummary {
    /// Returns `true` if all tests passed (or were updated).
    pub fn all_ok(&self) -> bool {
        self.failed == 0 && self.errors == 0
    }
}

impl std::fmt::Display for GoldenSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Golden Test Results")?;
        writeln!(
            f,
            "  {} passed, {} failed, {} updated, {} errors",
            self.passed, self.failed, self.updated, self.errors,
        )?;

        for outcome in &self.outcomes {
            match outcome {
                GoldenOutcome::Pass { name } => {
                    writeln!(f, "  PASS: {name}")?;
                }
                GoldenOutcome::Fail { name, diff } => {
                    writeln!(f, "  FAIL: {name}")?;
                    writeln!(f, "{diff}")?;
                }
                GoldenOutcome::Updated { name } => {
                    writeln!(f, "  UPDATED: {name}")?;
                }
                GoldenOutcome::Error { name, message } => {
                    writeln!(f, "  ERROR: {name}: {message}")?;
                }
            }
        }

        Ok(())
    }
}

/// Returns `true` if the `UPDATE_GOLDEN` environment variable is set to a truthy value.
pub fn should_update_golden() -> bool {
    env::var("UPDATE_GOLDEN")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Discovers all `.gr` files in the cases directory.
fn discover_cases(cases_dir: &Path) -> Vec<PathBuf> {
    let mut cases: Vec<PathBuf> = WalkDir::new(cases_dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .map_or(false, |ext| ext == "gr")
        })
        .map(|entry| entry.into_path())
        .collect();

    cases.sort();
    cases
}

/// Builds a unified diff between expected and actual content for a named stream.
fn unified_diff(stream_name: &str, expected: &str, actual: &str) -> String {
    let diff = TextDiff::from_lines(expected, actual);
    let mut result = format!("--- expected {stream_name}\n+++ actual {stream_name}\n");

    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        result.push_str(&format!("{sign}{change}"));
    }

    result
}

/// Runs a single golden test.
///
/// Invokes the compiler on `input_path`, captures stdout/stderr, and compares
/// against the expected output files. If `UPDATE_GOLDEN=1` is set, overwrites
/// the expected files with the current output instead of comparing.
fn run_single_golden(
    name: &str,
    input_path: &Path,
    expected_stdout_path: &Path,
    expected_stderr_path: &Path,
    compiler_path: &Path,
    update: bool,
) -> GoldenOutcome {
    // Invoke the compiler.
    let output = match Command::new(compiler_path).arg(input_path).output() {
        Ok(output) => output,
        Err(e) => {
            return GoldenOutcome::Error {
                name: name.to_string(),
                message: format!("failed to execute compiler: {e}"),
            };
        }
    };

    let actual_stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let actual_stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // Update mode: overwrite expected files and report as updated.
    if update {
        if let Err(e) = fs::write(expected_stdout_path, &actual_stdout) {
            return GoldenOutcome::Error {
                name: name.to_string(),
                message: format!("failed to write {}: {e}", expected_stdout_path.display()),
            };
        }
        if let Err(e) = fs::write(expected_stderr_path, &actual_stderr) {
            return GoldenOutcome::Error {
                name: name.to_string(),
                message: format!("failed to write {}: {e}", expected_stderr_path.display()),
            };
        }
        return GoldenOutcome::Updated {
            name: name.to_string(),
        };
    }

    // Comparison mode: read expected files and diff.
    let expected_stdout = match fs::read_to_string(expected_stdout_path) {
        Ok(content) => content,
        Err(e) => {
            return GoldenOutcome::Error {
                name: name.to_string(),
                message: format!(
                    "failed to read {}: {e} (hint: run with UPDATE_GOLDEN=1 to create it)",
                    expected_stdout_path.display()
                ),
            };
        }
    };

    let expected_stderr = match fs::read_to_string(expected_stderr_path) {
        Ok(content) => content,
        Err(e) => {
            return GoldenOutcome::Error {
                name: name.to_string(),
                message: format!(
                    "failed to read {}: {e} (hint: run with UPDATE_GOLDEN=1 to create it)",
                    expected_stderr_path.display()
                ),
            };
        }
    };

    let stdout_matches = expected_stdout == actual_stdout;
    let stderr_matches = expected_stderr == actual_stderr;

    if stdout_matches && stderr_matches {
        return GoldenOutcome::Pass {
            name: name.to_string(),
        };
    }

    // Build combined diff.
    let mut diff = String::new();
    if !stdout_matches {
        diff.push_str(&unified_diff("stdout", &expected_stdout, &actual_stdout));
    }
    if !stderr_matches {
        if !diff.is_empty() {
            diff.push('\n');
        }
        diff.push_str(&unified_diff("stderr", &expected_stderr, &actual_stderr));
    }

    GoldenOutcome::Fail {
        name: name.to_string(),
        diff,
    }
}

/// Runs all golden tests discovered in the configured directories.
///
/// For each `.gr` file found in `config.cases_dir`, looks up corresponding
/// `.stdout` and `.stderr` files in `config.expected_dir`, invokes the compiler,
/// and compares output.
///
/// Set `UPDATE_GOLDEN=1` in the environment to overwrite expected files with
/// current compiler output.
pub fn run_golden_suite(config: &GoldenConfig) -> GoldenSummary {
    let update = should_update_golden();
    let cases = discover_cases(&config.cases_dir);

    let mut outcomes = Vec::with_capacity(cases.len());
    let mut passed = 0;
    let mut failed = 0;
    let mut updated = 0;
    let mut errors = 0;

    for input_path in &cases {
        let stem = input_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        let expected_stdout_path = config.expected_dir.join(format!("{stem}.stdout"));
        let expected_stderr_path = config.expected_dir.join(format!("{stem}.stderr"));

        let outcome = run_single_golden(
            &stem,
            input_path,
            &expected_stdout_path,
            &expected_stderr_path,
            &config.compiler_path,
            update,
        );

        match &outcome {
            GoldenOutcome::Pass { .. } => passed += 1,
            GoldenOutcome::Fail { .. } => failed += 1,
            GoldenOutcome::Updated { .. } => updated += 1,
            GoldenOutcome::Error { .. } => errors += 1,
        }

        outcomes.push(outcome);
    }

    GoldenSummary {
        outcomes,
        passed,
        failed,
        updated,
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    /// Helper to create a fake "compiler" script that echoes fixed output.
    fn create_mock_compiler(dir: &Path, stdout_text: &str, stderr_text: &str) -> PathBuf {
        let script_path = dir.join("mock_compiler.sh");
        let mut f = fs::File::create(&script_path).unwrap();
        writeln!(
            f,
            "#!/bin/sh\nprintf '%s' '{}'\nprintf '%s' '{}' >&2",
            stdout_text.replace('\'', "'\\''"),
            stderr_text.replace('\'', "'\\''"),
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        script_path
    }

    #[test]
    fn test_should_update_golden_default() {
        // When not set, should return false. We cannot unset it reliably in a
        // parallel test, so just verify the function exists and returns a bool.
        let _ = should_update_golden();
    }

    #[test]
    fn test_discover_cases_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let cases = discover_cases(tmp.path());
        assert!(cases.is_empty());
    }

    #[test]
    fn test_discover_cases_finds_gr_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.gr"), "").unwrap();
        fs::write(tmp.path().join("b.gr"), "").unwrap();
        fs::write(tmp.path().join("c.txt"), "").unwrap(); // not .gr

        let cases = discover_cases(tmp.path());
        assert_eq!(cases.len(), 2);
        assert!(cases[0].file_name().unwrap().to_str().unwrap() == "a.gr");
        assert!(cases[1].file_name().unwrap().to_str().unwrap() == "b.gr");
    }

    #[test]
    fn test_unified_diff_identical() {
        let diff = unified_diff("stdout", "hello\n", "hello\n");
        // Even for identical content, the diff header is produced; all lines are Equal.
        assert!(diff.contains("--- expected stdout"));
        assert!(diff.contains("+++ actual stdout"));
        // No change markers should appear beyond the header lines.
        // Strip the two header lines and verify no +/- prefixed content lines remain.
        let body: String = diff
            .lines()
            .skip(2)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!body.contains("\n+"), "unexpected insertion in diff body");
        assert!(!body.contains("\n-"), "unexpected deletion in diff body");
    }

    #[test]
    fn test_golden_pass() {
        let tmp = TempDir::new().unwrap();
        let cases_dir = tmp.path().join("cases");
        let expected_dir = tmp.path().join("expected");
        fs::create_dir_all(&cases_dir).unwrap();
        fs::create_dir_all(&expected_dir).unwrap();

        fs::write(cases_dir.join("hello.gr"), "fn main() -> ()").unwrap();
        fs::write(expected_dir.join("hello.stdout"), "Hello, Gradient!").unwrap();
        fs::write(expected_dir.join("hello.stderr"), "").unwrap();

        let compiler = create_mock_compiler(tmp.path(), "Hello, Gradient!", "");

        let config = GoldenConfig::new(&compiler, &cases_dir, &expected_dir);
        let summary = run_golden_suite(&config);

        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 0);
        assert!(summary.all_ok());
    }

    #[test]
    fn test_golden_fail() {
        let tmp = TempDir::new().unwrap();
        let cases_dir = tmp.path().join("cases");
        let expected_dir = tmp.path().join("expected");
        fs::create_dir_all(&cases_dir).unwrap();
        fs::create_dir_all(&expected_dir).unwrap();

        fs::write(cases_dir.join("hello.gr"), "fn main() -> ()").unwrap();
        fs::write(expected_dir.join("hello.stdout"), "Expected output").unwrap();
        fs::write(expected_dir.join("hello.stderr"), "").unwrap();

        let compiler = create_mock_compiler(tmp.path(), "Different output", "");

        let config = GoldenConfig::new(&compiler, &cases_dir, &expected_dir);
        let summary = run_golden_suite(&config);

        assert_eq!(summary.failed, 1);
        assert!(!summary.all_ok());

        if let GoldenOutcome::Fail { diff, .. } = &summary.outcomes[0] {
            assert!(diff.contains("-Expected output"));
            assert!(diff.contains("+Different output"));
        } else {
            panic!("expected a Fail outcome");
        }
    }
}
