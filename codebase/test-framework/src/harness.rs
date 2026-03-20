//! Core test harness types and execution logic for the Gradient test framework.
//!
//! Provides [`TestResult`], [`TestCase`], and functions to run individual test
//! cases or entire suites of tests discovered from a directory.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// Outcome of a single test execution.
#[derive(Debug, Clone)]
pub enum TestResult {
    /// The test passed: actual output matched expected output.
    Pass,

    /// The test failed: actual output diverged from expected output.
    Fail {
        expected: String,
        actual: String,
        diff: String,
    },

    /// The test could not be executed due to an infrastructure error.
    Error { message: String },
}

impl TestResult {
    /// Returns `true` if this result represents a passing test.
    pub fn is_pass(&self) -> bool {
        matches!(self, TestResult::Pass)
    }

    /// Returns `true` if this result represents a failing test.
    pub fn is_fail(&self) -> bool {
        matches!(self, TestResult::Fail { .. })
    }

    /// Returns `true` if this result represents an error.
    pub fn is_error(&self) -> bool {
        matches!(self, TestResult::Error { .. })
    }
}

impl fmt::Display for TestResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestResult::Pass => write!(f, "PASS"),
            TestResult::Fail { diff, .. } => write!(f, "FAIL\n{diff}"),
            TestResult::Error { message } => write!(f, "ERROR: {message}"),
        }
    }
}

/// A single test case with paths to its input and expected output files.
#[derive(Debug, Clone)]
pub struct TestCase {
    /// Human-readable name for the test (typically the stem of the `.gr` file).
    pub name: String,

    /// Path to the `.gr` input source file.
    pub input_path: PathBuf,

    /// Path to the expected stdout file.
    pub expected_stdout_path: PathBuf,

    /// Path to the expected stderr file.
    pub expected_stderr_path: PathBuf,
}

impl TestCase {
    /// Creates a new `TestCase`.
    pub fn new(
        name: impl Into<String>,
        input_path: impl Into<PathBuf>,
        expected_stdout_path: impl Into<PathBuf>,
        expected_stderr_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            input_path: input_path.into(),
            expected_stdout_path: expected_stdout_path.into(),
            expected_stderr_path: expected_stderr_path.into(),
        }
    }
}

/// Executes a single test case by invoking the compiler and comparing output.
///
/// # Arguments
///
/// * `test_case` - The test case to execute.
/// * `compiler_path` - Path to the Gradient compiler binary.
///
/// # Returns
///
/// A [`TestResult`] indicating pass, fail, or error.
pub fn run_test_case(test_case: &TestCase, compiler_path: &Path) -> TestResult {
    // Read expected output files.
    let expected_stdout = match fs::read_to_string(&test_case.expected_stdout_path) {
        Ok(content) => content,
        Err(e) => {
            return TestResult::Error {
                message: format!(
                    "failed to read expected stdout at {}: {e}",
                    test_case.expected_stdout_path.display()
                ),
            };
        }
    };

    let expected_stderr = match fs::read_to_string(&test_case.expected_stderr_path) {
        Ok(content) => content,
        Err(e) => {
            return TestResult::Error {
                message: format!(
                    "failed to read expected stderr at {}: {e}",
                    test_case.expected_stderr_path.display()
                ),
            };
        }
    };

    // Invoke the compiler on the input file.
    let output = match std::process::Command::new(compiler_path)
        .arg(&test_case.input_path)
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            return TestResult::Error {
                message: format!(
                    "failed to execute compiler at {}: {e}",
                    compiler_path.display()
                ),
            };
        }
    };

    let actual_stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let actual_stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // Compare stdout and stderr independently, collect diffs.
    let stdout_diff = compute_diff("stdout", &expected_stdout, &actual_stdout);
    let stderr_diff = compute_diff("stderr", &expected_stderr, &actual_stderr);

    match (stdout_diff, stderr_diff) {
        (None, None) => TestResult::Pass,
        (stdout_d, stderr_d) => {
            let mut combined_diff = String::new();
            if let Some(d) = stdout_d {
                combined_diff.push_str(&d);
            }
            if let Some(d) = stderr_d {
                if !combined_diff.is_empty() {
                    combined_diff.push('\n');
                }
                combined_diff.push_str(&d);
            }
            // For the Fail variant, report the combined expected/actual of stdout
            // (the primary output). The diff string contains both stdout and stderr diffs.
            TestResult::Fail {
                expected: expected_stdout,
                actual: actual_stdout,
                diff: combined_diff,
            }
        }
    }
}

/// Computes a unified diff between `expected` and `actual` for a given stream name.
///
/// Returns `None` if the strings are identical, or `Some(diff_string)` with a
/// human-readable unified diff.
fn compute_diff(stream_name: &str, expected: &str, actual: &str) -> Option<String> {
    if expected == actual {
        return None;
    }

    use similar::{ChangeTag, TextDiff};

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

    Some(result)
}

/// Result summary for an entire test suite.
#[derive(Debug, Clone)]
pub struct SuiteResult {
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub results: Vec<(TestCase, TestResult)>,
}

impl SuiteResult {
    /// Returns `true` if every test in the suite passed.
    pub fn all_passed(&self) -> bool {
        self.failed == 0 && self.errors == 0
    }
}

impl fmt::Display for SuiteResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Test Suite Results")?;
        writeln!(
            f,
            "  {} passed, {} failed, {} errors",
            self.passed, self.failed, self.errors
        )?;
        for (case, result) in &self.results {
            if !result.is_pass() {
                writeln!(f, "\n--- {} ---", case.name)?;
                writeln!(f, "{result}")?;
            }
        }
        Ok(())
    }
}

/// Discovers and runs all test cases in a directory.
///
/// Scans `cases_dir` for `.gr` files and looks up corresponding `.stdout` and
/// `.stderr` files in `expected_dir`. Runs each discovered test case against the
/// compiler at `compiler_path`.
///
/// # Arguments
///
/// * `cases_dir` - Directory containing `.gr` input files.
/// * `expected_dir` - Directory containing `.stdout` and `.stderr` expected output.
/// * `compiler_path` - Path to the Gradient compiler binary.
///
/// # Returns
///
/// A [`SuiteResult`] summarizing all test outcomes.
pub fn run_suite(
    cases_dir: &Path,
    expected_dir: &Path,
    compiler_path: &Path,
) -> SuiteResult {
    let mut results = Vec::new();
    let mut passed = 0;
    let mut failed = 0;
    let mut errors = 0;

    let mut cases: Vec<PathBuf> = WalkDir::new(cases_dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "gr")
        })
        .map(|entry| entry.into_path())
        .collect();

    // Sort for deterministic ordering.
    cases.sort();

    for input_path in cases {
        let stem = input_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        let expected_stdout_path = expected_dir.join(format!("{stem}.stdout"));
        let expected_stderr_path = expected_dir.join(format!("{stem}.stderr"));

        let test_case = TestCase::new(
            stem,
            &input_path,
            &expected_stdout_path,
            &expected_stderr_path,
        );

        let result = run_test_case(&test_case, compiler_path);

        match &result {
            TestResult::Pass => passed += 1,
            TestResult::Fail { .. } => failed += 1,
            TestResult::Error { .. } => errors += 1,
        }

        results.push((test_case, result));
    }

    SuiteResult {
        passed,
        failed,
        errors,
        results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_display_pass() {
        let result = TestResult::Pass;
        assert_eq!(format!("{result}"), "PASS");
    }

    #[test]
    fn test_result_display_error() {
        let result = TestResult::Error {
            message: "something broke".into(),
        };
        assert_eq!(format!("{result}"), "ERROR: something broke");
    }

    #[test]
    fn test_result_predicates() {
        assert!(TestResult::Pass.is_pass());
        assert!(!TestResult::Pass.is_fail());
        assert!(!TestResult::Pass.is_error());

        let fail = TestResult::Fail {
            expected: String::new(),
            actual: String::new(),
            diff: String::new(),
        };
        assert!(fail.is_fail());
        assert!(!fail.is_pass());

        let error = TestResult::Error {
            message: String::new(),
        };
        assert!(error.is_error());
        assert!(!error.is_pass());
    }

    #[test]
    fn test_compute_diff_identical() {
        assert!(compute_diff("stdout", "hello\n", "hello\n").is_none());
    }

    #[test]
    fn test_compute_diff_different() {
        let diff = compute_diff("stdout", "expected\n", "actual\n");
        assert!(diff.is_some());
        let diff_text = diff.unwrap();
        assert!(diff_text.contains("--- expected stdout"));
        assert!(diff_text.contains("+++ actual stdout"));
        assert!(diff_text.contains("-expected"));
        assert!(diff_text.contains("+actual"));
    }

    #[test]
    fn test_case_construction() {
        let tc = TestCase::new("hello", "/tmp/hello.gr", "/tmp/hello.stdout", "/tmp/hello.stderr");
        assert_eq!(tc.name, "hello");
        assert_eq!(tc.input_path, PathBuf::from("/tmp/hello.gr"));
    }

    #[test]
    fn test_suite_result_all_passed() {
        let suite = SuiteResult {
            passed: 3,
            failed: 0,
            errors: 0,
            results: vec![],
        };
        assert!(suite.all_passed());

        let suite_fail = SuiteResult {
            passed: 2,
            failed: 1,
            errors: 0,
            results: vec![],
        };
        assert!(!suite_fail.all_passed());
    }
}
