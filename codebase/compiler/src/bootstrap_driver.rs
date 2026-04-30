//! Issue #231: usable bootstrap compiler driver kernel.
//!
//! `compiler/main.gr` historically returned `default_config()` from
//! `parse_args`, stubbed file writes, and printed nothing. This module
//! replaces the driver innards with a single Rust kernel entry point
//! that:
//!
//! 1. Reads source from disk (or accepts an in-memory string for tests).
//! 2. Drives the full pipeline via `bootstrap_pipeline_*` (#230).
//! 3. Writes the emitted textual IR to `output_path` (or holds it in
//!    the session for tests / stdout consumers).
//! 4. Surfaces a structured exit code that maps directly to the integer
//!    `main` returns.
//! 5. Captures human-readable diagnostics keyed by the same session id
//!    so test-side or future .gr-side callers can render them.
//!
//! Exit codes (stable wire format — main.gr / external scripts depend
//! on these):
//!
//!   0 — success
//!   1 — input file not found / unreadable
//!   2 — parse errors
//!   3 — type-check errors
//!   4 — lowering produced no IR (no functions in the bootstrap subset)
//!   5 — output write failed
//!   6 — internal error (unknown session, etc.)
//!
//! Boundary contract: the kernel never emits its own diagnostics — error
//! counts and messages come from the existing lexer / parser / checker.
//! Reading and writing files is the kernel's only OS-level effect, kept
//! narrow on purpose so the .gr-side driver stays free of FS plumbing.
//!
//! Companion gates: self_hosted_pipeline (#230), self_hosted_codegen_text
//! (#229), ir_differential_tests (#228).

use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::bootstrap_pipeline::{
    bootstrap_pipeline_check, bootstrap_pipeline_emit, bootstrap_pipeline_lex,
    bootstrap_pipeline_lower, bootstrap_pipeline_parse, bootstrap_pipeline_parse_error_count,
};
use crate::lexer::Lexer;
use crate::parser as ast_parser;
use crate::typechecker;

// ── Exit codes ───────────────────────────────────────────────────────────

pub const DRIVER_OK: i64 = 0;
pub const DRIVER_READ_ERROR: i64 = 1;
pub const DRIVER_PARSE_ERROR: i64 = 2;
pub const DRIVER_TYPE_ERROR: i64 = 3;
pub const DRIVER_LOWER_ERROR: i64 = 4;
pub const DRIVER_WRITE_ERROR: i64 = 5;
pub const DRIVER_INTERNAL_ERROR: i64 = 6;

// ── Driver session table (separate from pipeline table) ──────────────────
//
// We keep the driver's own per-run state — diagnostics, captured output —
// keyed by an integer id so the .gr-side `main` driver can read them
// back via simple `Int -> String` calls without ever passing complex
// types across the FFI.

#[derive(Default, Debug)]
struct DriverRun {
    exit_code: i64,
    diagnostics: Vec<String>,
    /// Captured emitted text (only set when output_path is empty).
    captured_output: String,
    /// File the driver wrote to, if any.
    written_path: String,
    /// Module name derived from input path or "main" for in-memory runs.
    module_name: String,
}

#[derive(Default, Debug)]
struct DriverStore {
    runs: Vec<DriverRun>,
}

impl DriverStore {
    fn alloc(&mut self) -> i64 {
        let id = (self.runs.len() as i64) + 1;
        self.runs.push(DriverRun::default());
        id
    }

    fn get(&self, id: i64) -> Option<&DriverRun> {
        if id <= 0 {
            return None;
        }
        self.runs.get((id as usize) - 1)
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut DriverRun> {
        if id <= 0 {
            return None;
        }
        self.runs.get_mut((id as usize) - 1)
    }
}

fn driver_store() -> &'static Mutex<DriverStore> {
    static STORE: OnceLock<Mutex<DriverStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(DriverStore::default()))
}

fn with_driver<R>(f: impl FnOnce(&mut DriverStore) -> R) -> R {
    let mut s = driver_store().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut s)
}

/// Reset driver run table. Test-only.
pub fn reset_driver_store() {
    with_driver(|s| s.runs.clear());
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn extract_module_name(path: &str) -> String {
    if path.is_empty() || path == "<memory>" {
        return "main".to_string();
    }
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main")
        .to_string()
}

// ── Public driver entry points ──────────────────────────────────────────

/// Run the full driver against an in-memory source string. Returns a
/// run-id whose details (exit code, diagnostics, captured output) can
/// be inspected via the `bootstrap_driver_get_*` accessors. Useful for
/// tests and any caller that already has the source in memory and
/// wants the textual IR back as a string rather than a file.
///
/// `output_path` may be empty, in which case the emitted text is
/// captured in the run record (see `bootstrap_driver_get_captured_output`).
pub fn bootstrap_driver_run_source(source: &str, output_path: &str) -> i64 {
    let run_id = with_driver(|s| s.alloc());

    if source.is_empty() {
        with_driver(|s| {
            if let Some(r) = s.get_mut(run_id) {
                r.exit_code = DRIVER_READ_ERROR;
                r.diagnostics
                    .push("error: empty source provided to driver".to_string());
            }
        });
        return run_id;
    }

    drive_pipeline(run_id, source, "<memory>", output_path);
    run_id
}

/// Run the full driver against a file on disk. Reads `input_path`,
/// reports a `DRIVER_READ_ERROR` if the file is missing or unreadable,
/// otherwise drives the pipeline through the same path as
/// `bootstrap_driver_run_source`. Writes emitted text to `output_path`
/// when non-empty.
pub fn bootstrap_driver_run_file(input_path: &str, output_path: &str) -> i64 {
    let run_id = with_driver(|s| s.alloc());

    let source = match fs::read_to_string(input_path) {
        Ok(s) => s,
        Err(e) => {
            with_driver(|s| {
                if let Some(r) = s.get_mut(run_id) {
                    r.exit_code = DRIVER_READ_ERROR;
                    r.diagnostics
                        .push(format!("error: cannot read `{}`: {}", input_path, e));
                }
            });
            return run_id;
        }
    };

    drive_pipeline(run_id, &source, input_path, output_path);
    run_id
}

/// Inspect the exit code from a driver run. The `.gr` `main` function
/// returns this value directly.
pub fn bootstrap_driver_get_exit_code(run_id: i64) -> i64 {
    with_driver(|s| {
        s.get(run_id)
            .map(|r| r.exit_code)
            .unwrap_or(DRIVER_INTERNAL_ERROR)
    })
}

/// Number of diagnostics captured by the driver run.
pub fn bootstrap_driver_get_diagnostic_count(run_id: i64) -> i64 {
    with_driver(|s| {
        s.get(run_id)
            .map(|r| r.diagnostics.len() as i64)
            .unwrap_or(0)
    })
}

/// Get the diagnostic at `index` (0-based). Returns "" for out-of-bounds.
pub fn bootstrap_driver_get_diagnostic_at(run_id: i64, index: i64) -> String {
    with_driver(|s| {
        s.get(run_id)
            .and_then(|r| r.diagnostics.get(index as usize).cloned())
            .unwrap_or_default()
    })
}

/// Captured output text (only populated when `output_path` was empty
/// and the run succeeded).
pub fn bootstrap_driver_get_captured_output(run_id: i64) -> String {
    with_driver(|s| {
        s.get(run_id)
            .map(|r| r.captured_output.clone())
            .unwrap_or_default()
    })
}

/// Path the driver wrote to (empty if no file was written).
pub fn bootstrap_driver_get_written_path(run_id: i64) -> String {
    with_driver(|s| {
        s.get(run_id)
            .map(|r| r.written_path.clone())
            .unwrap_or_default()
    })
}

/// Module name extracted from the input path, or "main" for in-memory
/// runs.
pub fn bootstrap_driver_get_module_name(run_id: i64) -> String {
    with_driver(|s| {
        s.get(run_id)
            .map(|r| r.module_name.clone())
            .unwrap_or_default()
    })
}

// ── Internal driver core ─────────────────────────────────────────────────

fn record_diagnostic(run_id: i64, msg: String) {
    with_driver(|s| {
        if let Some(r) = s.get_mut(run_id) {
            r.diagnostics.push(msg);
        }
    });
}

fn set_exit_code(run_id: i64, code: i64) {
    with_driver(|s| {
        if let Some(r) = s.get_mut(run_id) {
            r.exit_code = code;
        }
    });
}

fn set_module_name(run_id: i64, name: String) {
    with_driver(|s| {
        if let Some(r) = s.get_mut(run_id) {
            r.module_name = name;
        }
    });
}

fn drive_pipeline(run_id: i64, source: &str, source_label: &str, output_path: &str) {
    let module_name = extract_module_name(source_label);
    set_module_name(run_id, module_name.clone());

    // Phase 1: lex via the pipeline kernel — gets us a session id we can
    // reuse for parse / check / lower / emit.
    let session = bootstrap_pipeline_lex(source, 0);
    if session == 0 {
        set_exit_code(run_id, DRIVER_READ_ERROR);
        record_diagnostic(
            run_id,
            "error: lex phase produced no tokens (empty source?)".to_string(),
        );
        return;
    }

    // Phase 2: parse. Capture parse error messages by re-running the parser
    // on the same source — the pipeline kernel doesn't expose individual
    // error texts yet, only counts. This is intentionally a kernel-level
    // duplication: the diagnostic surface is opt-in and only paid for
    // when the driver actually needs to render messages.
    let items = bootstrap_pipeline_parse(session);
    let parse_errs = bootstrap_pipeline_parse_error_count(session);
    if parse_errs > 0 || items == 0 {
        // Re-run parser for diagnostic text.
        let mut lex = Lexer::new(source, 0);
        let toks = lex.tokenize();
        let (_module, errs) = ast_parser::parse(toks, 0);
        for e in errs {
            record_diagnostic(run_id, format!("parse error: {:?}", e));
        }
        if parse_errs > 0 {
            set_exit_code(run_id, DRIVER_PARSE_ERROR);
            return;
        }
        // No parse errors but no items — empty / non-bootstrap module.
        set_exit_code(run_id, DRIVER_LOWER_ERROR);
        record_diagnostic(
            run_id,
            "error: module contains no bootstrap-subset functions".to_string(),
        );
        return;
    }

    // Phase 3: check. Re-run for diagnostic text (same trade-off as parse).
    let check_errs = bootstrap_pipeline_check(session);
    if check_errs > 0 {
        let mut lex = Lexer::new(source, 0);
        let toks = lex.tokenize();
        let (m, _) = ast_parser::parse(toks, 0);
        let type_errors = typechecker::check_module(&m, 0);
        for e in type_errors.iter().filter(|e| !e.is_warning) {
            record_diagnostic(run_id, format!("type error: {}", e.message));
        }
        set_exit_code(run_id, DRIVER_TYPE_ERROR);
        return;
    }

    // Phase 4: lower.
    let ir_id = bootstrap_pipeline_lower(session, &module_name);
    if ir_id == 0 {
        set_exit_code(run_id, DRIVER_LOWER_ERROR);
        record_diagnostic(
            run_id,
            "error: lowering produced no IR module — bootstrap subset may not cover this input"
                .to_string(),
        );
        return;
    }

    // Phase 5: emit.
    let text = bootstrap_pipeline_emit(ir_id);
    if text.is_empty() {
        set_exit_code(run_id, DRIVER_LOWER_ERROR);
        record_diagnostic(
            run_id,
            "error: emit produced empty text from a non-zero IR id (regression)".to_string(),
        );
        return;
    }

    // Phase 6: write or capture.
    if output_path.is_empty() {
        with_driver(|s| {
            if let Some(r) = s.get_mut(run_id) {
                r.captured_output = text;
                r.exit_code = DRIVER_OK;
            }
        });
    } else {
        match fs::write(output_path, &text) {
            Ok(_) => {
                with_driver(|s| {
                    if let Some(r) = s.get_mut(run_id) {
                        r.written_path = output_path.to_string();
                        r.exit_code = DRIVER_OK;
                    }
                });
            }
            Err(e) => {
                set_exit_code(run_id, DRIVER_WRITE_ERROR);
                record_diagnostic(
                    run_id,
                    format!("error: cannot write `{}`: {}", output_path, e),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_ast_bridge::reset_ast_store;
    use crate::bootstrap_ir_bridge::{reset_ir_store, shared_test_lock};
    use crate::bootstrap_pipeline::reset_pipeline_store;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        shared_test_lock()
    }

    fn reset_all() {
        reset_driver_store();
        reset_pipeline_store();
        reset_ast_store();
        reset_ir_store();
    }

    #[test]
    fn happy_path_in_memory_run_returns_ok() {
        let _g = lock();
        reset_all();
        let src = "fn add(x: Int, y: Int) -> Int:\n    ret x + y\n";
        let run = bootstrap_driver_run_source(src, "");
        assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_OK);
        let captured = bootstrap_driver_get_captured_output(run);
        assert!(captured.contains("fn add"));
        assert!(captured.contains("ret"));
        assert_eq!(bootstrap_driver_get_written_path(run), "");
        assert_eq!(bootstrap_driver_get_module_name(run), "main");
    }

    #[test]
    fn parse_error_returns_parse_exit_code() {
        let _g = lock();
        reset_all();
        let bad = "fn broken(x: Int) -> Int:\n    ret x +\n";
        let run = bootstrap_driver_run_source(bad, "");
        assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_PARSE_ERROR);
        assert!(bootstrap_driver_get_diagnostic_count(run) > 0);
    }

    #[test]
    fn type_error_returns_type_exit_code() {
        let _g = lock();
        reset_all();
        let bad = "fn f(x: Int) -> Int:\n    ret bogus\n";
        let run = bootstrap_driver_run_source(bad, "");
        assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_TYPE_ERROR);
        assert!(bootstrap_driver_get_diagnostic_count(run) > 0);
        let first = bootstrap_driver_get_diagnostic_at(run, 0);
        assert!(first.contains("bogus") || first.contains("undefined"));
    }

    #[test]
    fn empty_source_returns_read_error() {
        let _g = lock();
        reset_all();
        let run = bootstrap_driver_run_source("", "");
        assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_READ_ERROR);
    }

    #[test]
    fn missing_input_file_returns_read_error() {
        let _g = lock();
        reset_all();
        let run = bootstrap_driver_run_file("/definitely/not/a/real/path.gr", "");
        assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_READ_ERROR);
        assert!(bootstrap_driver_get_diagnostic_count(run) > 0);
    }

    #[test]
    fn module_name_extraction_from_path() {
        let _g = lock();
        reset_all();
        // Use a tempfile because file extraction needs a real path.
        let tmp = std::env::temp_dir().join("bootstrap_driver_test_demo.gr");
        std::fs::write(&tmp, "fn answer() -> Int:\n    ret 42\n").unwrap();
        let run = bootstrap_driver_run_file(tmp.to_str().unwrap(), "");
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_OK);
        assert_eq!(
            bootstrap_driver_get_module_name(run),
            "bootstrap_driver_test_demo"
        );
    }

    #[test]
    fn writes_output_file_when_path_provided() {
        let _g = lock();
        reset_all();
        let src = "fn answer() -> Int:\n    ret 42\n";
        let tmp = std::env::temp_dir().join("bootstrap_driver_test_out.txt");
        let _ = std::fs::remove_file(&tmp);
        let run = bootstrap_driver_run_source(src, tmp.to_str().unwrap());
        assert_eq!(bootstrap_driver_get_exit_code(run), DRIVER_OK);
        let written = bootstrap_driver_get_written_path(run);
        assert_eq!(written, tmp.to_str().unwrap());
        let on_disk = std::fs::read_to_string(&tmp).expect("driver wrote file");
        assert!(on_disk.contains("fn answer"));
        let _ = std::fs::remove_file(&tmp);
    }
}
