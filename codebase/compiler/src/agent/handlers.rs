//! Method handlers for the Gradient agent mode.
//!
//! Each handler takes params and a session reference, and returns a JSON value.

use std::env;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::query::{self, Session};

use super::protocol::{self, Response};

/// The bundled response from `load` and `check`.
#[derive(Debug, Serialize)]
pub struct SessionReport {
    pub ok: bool,
    pub diagnostics: Vec<query::Diagnostic>,
    pub symbols: Vec<query::SymbolInfo>,
    pub holes: Vec<HoleInfo>,
    pub effects: Option<serde_json::Value>,
    pub summary: SessionSummary,
}

/// Compact summary counts.
#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub functions: usize,
    pub types: usize,
    pub errors: usize,
    pub warnings: usize,
    pub holes: usize,
}

/// Structured typed hole information.
#[derive(Debug, Clone, Serialize)]
pub struct HoleInfo {
    pub span: crate::ast::span::Span,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<String>,
    pub matching_bindings: Vec<HoleBinding>,
    pub matching_functions: Vec<HoleFunction>,
    pub matching_variants: Vec<String>,
}

/// A binding that matches a typed hole's expected type.
#[derive(Debug, Clone, Serialize)]
pub struct HoleBinding {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A function whose return type matches a typed hole's expected type.
#[derive(Debug, Clone, Serialize)]
pub struct HoleFunction {
    pub name: String,
    pub signature: String,
}

/// Extract structured hole info from diagnostics.
///
/// Reads the `hole` field on each diagnostic (populated by the typechecker
/// via `TypedHoleData`) and maps it into the agent's serialization shape.
/// No string parsing — consumers get fully structured data.
fn extract_holes(diagnostics: &[query::Diagnostic]) -> Vec<HoleInfo> {
    diagnostics
        .iter()
        .filter_map(|d| {
            let h = d.hole.as_ref()?;
            Some(HoleInfo {
                span: d.span,
                expected_type: h.expected_type.clone(),
                matching_bindings: h
                    .matching_bindings
                    .iter()
                    .map(|b| HoleBinding {
                        name: b.name.clone(),
                        ty: b.ty.clone(),
                    })
                    .collect(),
                matching_functions: h
                    .matching_functions
                    .iter()
                    .map(|f| HoleFunction {
                        name: f.name.clone(),
                        signature: f.signature.clone(),
                    })
                    .collect(),
                matching_variants: Vec::new(),
            })
        })
        .collect()
}

/// Build a full SessionReport from a Session.
fn build_report(session: &Session) -> SessionReport {
    let check = session.check();
    let symbols = session.symbols();
    let holes = extract_holes(&check.diagnostics);

    let effects = session
        .effect_summary()
        .and_then(|summary| serde_json::to_value(summary).ok());

    let function_count = symbols
        .iter()
        .filter(|s| matches!(s.kind, query::SymbolKind::Function))
        .count();
    let type_count = symbols
        .iter()
        .filter(|s| matches!(s.kind, query::SymbolKind::TypeAlias))
        .count();
    let error_count = check
        .diagnostics
        .iter()
        .filter(|d| matches!(d.severity, query::Severity::Error))
        .count();
    let warning_count = check
        .diagnostics
        .iter()
        .filter(|d| matches!(d.severity, query::Severity::Warning))
        .count();

    SessionReport {
        ok: check.ok,
        diagnostics: check.diagnostics,
        symbols,
        holes: holes.clone(),
        effects,
        summary: SessionSummary {
            functions: function_count,
            types: type_count,
            errors: error_count,
            warnings: warning_count,
            holes: holes.len(),
        },
    }
}

/// Build an internal-error response for serialization failures.
fn serialization_error(e: serde_json::Error) -> Response {
    Response::error(
        Value::Null,
        protocol::INTERNAL_ERROR,
        format!("Serialization error: {}", e),
    )
}

/// Validates that a file path is within the workspace root.
/// Prevents directory traversal attacks (e.g., `../../../etc/passwd`).
fn validate_workspace_path(path_str: &str) -> Result<PathBuf, Response> {
    // Get the workspace root (current working directory)
    let workspace_root = env::current_dir().map_err(|e| {
        Response::error(
            Value::Null,
            protocol::INTERNAL_ERROR,
            format!("Failed to determine workspace root: {}", e),
        )
    })?;

    // Parse the requested path
    let path = Path::new(path_str);

    // Reject absolute paths that point outside the workspace
    if path.is_absolute() {
        return Err(Response::error(
            Value::Null,
            protocol::INVALID_PARAMS,
            "Absolute paths are not allowed".to_string(),
        ));
    }

    // Construct the full path within the workspace
    let full_path = workspace_root.join(path);

    // Canonicalize to resolve any `..` or symlinks
    let canonical_path = full_path.canonicalize().map_err(|_| {
        Response::error(
            Value::Null,
            protocol::FILE_NOT_FOUND,
            format!("File not found or path traversal attempt: {}", path_str),
        )
    })?;

    // Canonicalize the workspace root for comparison
    let canonical_root = workspace_root.canonicalize().map_err(|e| {
        Response::error(
            Value::Null,
            protocol::INTERNAL_ERROR,
            format!("Failed to canonicalize workspace root: {}", e),
        )
    })?;

    // Ensure the canonical path starts with the canonical root
    if !canonical_path.starts_with(&canonical_root) {
        return Err(Response::error(
            Value::Null,
            protocol::INVALID_PARAMS,
            "Path escapes workspace root".to_string(),
        ));
    }

    Ok(canonical_path)
}

/// Handle the `load` or `check` method.
pub fn handle_load(params: &Value, session: &mut Option<Session>) -> Result<Value, Response> {
    let source = params.get("source").and_then(|v| v.as_str());
    let file = params.get("file").and_then(|v| v.as_str());

    let new_session = match (source, file) {
        (Some(src), _) => Session::from_source(src),
        (None, Some(path)) => {
            // SECURITY: Validate path is within workspace before any file operations
            let canonical_path = validate_workspace_path(path)?;

            // Pre-check path existence so file errors surface as FILE_NOT_FOUND
            // rather than being converted into diagnostics by Session::from_file.
            if !canonical_path.exists() {
                return Err(Response::error(
                    Value::Null,
                    protocol::FILE_NOT_FOUND,
                    format!("File not found: {}", canonical_path.display()),
                ));
            }
            if !canonical_path.is_file() {
                return Err(Response::error(
                    Value::Null,
                    protocol::FILE_NOT_FOUND,
                    format!("Not a file: {}", canonical_path.display()),
                ));
            }
            Session::from_file(&canonical_path)
                .map_err(|e| Response::error(Value::Null, protocol::FILE_NOT_FOUND, e))?
        }
        (None, None) => {
            return Err(Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "Either \"source\" or \"file\" parameter is required",
            ));
        }
    };

    let report = build_report(&new_session);
    *session = Some(new_session);

    serde_json::to_value(&report).map_err(serialization_error)
}

/// Handle the `symbols` method.
pub fn handle_symbols(session: &Session) -> Result<Value, Response> {
    serde_json::to_value(session.symbols()).map_err(serialization_error)
}

/// Handle the `holes` method.
pub fn handle_holes(session: &Session) -> Result<Value, Response> {
    let check = session.check();
    let holes = extract_holes(&check.diagnostics);
    serde_json::to_value(&holes).map_err(serialization_error)
}

/// Handle the `complete` method.
pub fn handle_complete(params: &Value, session: &Session) -> Result<Value, Response> {
    let line = params
        .get("line")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| {
            Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "\"line\" parameter required (u32)",
            )
        })?;
    let col = params
        .get("col")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| {
            Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "\"col\" parameter required (u32)",
            )
        })?;

    let ctx = session.completion_context(line, col);
    serde_json::to_value(&ctx).map_err(serialization_error)
}

/// Handle the `context_budget` method.
pub fn handle_context_budget(params: &Value, session: &Session) -> Result<Value, Response> {
    let function = params
        .get("function")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "\"function\" parameter required (string)",
            )
        })?;
    let budget = params
        .get("budget")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .ok_or_else(|| {
            Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "\"budget\" parameter required (u32)",
            )
        })?;

    let result = session.context_budget(function, budget);
    serde_json::to_value(&result).map_err(serialization_error)
}

/// Handle the `effects` method.
pub fn handle_effects(session: &Session) -> Result<Value, Response> {
    match session.effect_summary() {
        Some(summary) => serde_json::to_value(summary).map_err(serialization_error),
        None => Ok(serde_json::json!(null)),
    }
}

/// Handle the `inspect` method.
pub fn handle_inspect(session: &Session) -> Result<Value, Response> {
    serde_json::to_value(session.module_contract()).map_err(serialization_error)
}

/// Handle the `call_graph` method.
pub fn handle_call_graph(session: &Session) -> Result<Value, Response> {
    serde_json::to_value(session.call_graph()).map_err(serialization_error)
}

/// Handle the `doc` method — returns full module documentation.
pub fn handle_doc(session: &Session) -> Result<Value, Response> {
    serde_json::to_value(session.documentation()).map_err(serialization_error)
}

/// Handle the `type_at` method.
///
/// Params: `{ "line": u32, "col": u32 }`. Returns the type at that position,
/// or JSON `null` if no expression sits there.
pub fn handle_type_at(params: &Value, session: &Session) -> Result<Value, Response> {
    let line = params
        .get("line")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| {
            Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "\"line\" parameter required (u32)",
            )
        })?;
    let col = params
        .get("col")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| {
            Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "\"col\" parameter required (u32)",
            )
        })?;

    match session.type_at(line, col) {
        Some(r) => serde_json::to_value(r).map_err(serialization_error),
        None => Ok(serde_json::json!(null)),
    }
}

/// Handle the `rename` method.
///
/// Params: `{ "old_name": string, "new_name": string }`. Returns the rename
/// result (new source + locations + verification).
pub fn handle_rename(params: &Value, session: &Session) -> Result<Value, Response> {
    let old_name = params
        .get("old_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "\"old_name\" parameter required (string)",
            )
        })?;
    let new_name = params
        .get("new_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Response::error(
                Value::Null,
                protocol::INVALID_PARAMS,
                "\"new_name\" parameter required (string)",
            )
        })?;

    match session.rename(old_name, new_name) {
        Ok(r) => serde_json::to_value(r).map_err(serialization_error),
        Err(msg) => Err(Response::error(Value::Null, protocol::INVALID_PARAMS, msg)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_source() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        let result = handle_load(&params, &mut session).unwrap();
        assert!(result["ok"].as_bool().unwrap());
        assert!(session.is_some());
    }

    #[test]
    fn load_missing_params() {
        let params = serde_json::json!({});
        let mut session = None;
        let result = handle_load(&params, &mut session);
        assert!(result.is_err());
    }

    #[test]
    fn load_nonexistent_file() {
        // Pre-check should surface FILE_NOT_FOUND as a JSON-RPC error
        // rather than silently folding into diagnostics.
        let params = serde_json::json!({"file": "missing-file.gr"});
        let mut session = None;
        let result = handle_load(&params, &mut session);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error.unwrap().code, protocol::FILE_NOT_FOUND);
        assert!(session.is_none(), "failed load must not replace session");
    }

    #[test]
    fn load_directory_as_file() {
        let params = serde_json::json!({"file": "."});
        let mut session = None;
        let result = handle_load(&params, &mut session);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().error.unwrap().code,
            protocol::FILE_NOT_FOUND
        );
    }

    #[test]
    fn load_with_errors() {
        let params = serde_json::json!({"source": "fn bad() -> Int:\n    \"not an int\"\n"});
        let mut session = None;
        let result = handle_load(&params, &mut session).unwrap();
        assert!(!result["ok"].as_bool().unwrap());
        assert!(result["summary"]["errors"].as_u64().unwrap() > 0);
    }

    #[test]
    fn symbols_after_load() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let result = handle_symbols(session.as_ref().unwrap()).unwrap();
        assert!(result.as_array().unwrap().len() > 0);
    }

    #[test]
    fn holes_extraction() {
        let params = serde_json::json!({"source": "fn get_val(x: Int) -> Int:\n    ?\n"});
        let mut session = None;
        let result = handle_load(&params, &mut session).unwrap();
        let holes = result["holes"].as_array().unwrap();
        assert!(!holes.is_empty(), "should have at least one hole");
        // Structured data should be populated directly from the typechecker,
        // not parsed out of diagnostic notes.
        assert_eq!(holes[0]["expected_type"], "Int");
    }

    #[test]
    fn inspect_after_load() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let result = handle_inspect(session.as_ref().unwrap()).unwrap();
        assert!(result.get("symbols").is_some());
    }

    #[test]
    fn effects_after_load() {
        let params = serde_json::json!({"source": "fn greet() -> !{IO} ():\n    print(\"hi\")\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let result = handle_effects(session.as_ref().unwrap()).unwrap();
        // effects should be non-null for a valid program
        assert!(!result.is_null());
    }

    #[test]
    fn call_graph_after_load() {
        let params =
            serde_json::json!({"source": "fn a() -> Int:\n    b()\n\nfn b() -> Int:\n    42\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let result = handle_call_graph(session.as_ref().unwrap()).unwrap();
        let graph = result.as_array().unwrap();
        assert!(!graph.is_empty());
    }

    #[test]
    fn context_budget_after_load() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let budget_params = serde_json::json!({"function": "add", "budget": 1000});
        let result = handle_context_budget(&budget_params, session.as_ref().unwrap()).unwrap();
        assert_eq!(result["target_function"], "add");
    }

    #[test]
    fn doc_after_load() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let result = handle_doc(session.as_ref().unwrap()).unwrap();
        assert!(!result.is_null());
        assert!(result.get("module").is_some());
    }

    #[test]
    fn type_at_after_load() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        // Point at `a` in the body on line 2 (1-indexed), col 5 (after 4 spaces).
        let q = serde_json::json!({"line": 2, "col": 5});
        let result = handle_type_at(&q, session.as_ref().unwrap()).unwrap();
        // Either structured result with "type", or null if position misses.
        if !result.is_null() {
            assert!(result.get("type").is_some());
        }
    }

    #[test]
    fn type_at_missing_params() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let q = serde_json::json!({});
        let result = handle_type_at(&q, session.as_ref().unwrap());
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().error.unwrap().code,
            protocol::INVALID_PARAMS
        );
    }

    #[test]
    fn rename_after_load() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let q = serde_json::json!({"old_name": "add", "new_name": "sum"});
        let result = handle_rename(&q, session.as_ref().unwrap()).unwrap();
        assert!(!result.is_null());
    }

    #[test]
    fn rename_missing_params() {
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let q = serde_json::json!({"old_name": "add"});
        let result = handle_rename(&q, session.as_ref().unwrap());
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().error.unwrap().code,
            protocol::INVALID_PARAMS
        );
    }

    #[test]
    fn reload_replaces_session() {
        let params1 = serde_json::json!({"source": "fn a() -> Int:\n    1\n"});
        let params2 = serde_json::json!({"source": "fn b() -> Int:\n    2\n"});
        let mut session = None;
        handle_load(&params1, &mut session).unwrap();
        let r1 = handle_symbols(session.as_ref().unwrap()).unwrap();
        assert_eq!(r1.as_array().unwrap()[0]["name"], "a");

        handle_load(&params2, &mut session).unwrap();
        let r2 = handle_symbols(session.as_ref().unwrap()).unwrap();
        assert_eq!(r2.as_array().unwrap()[0]["name"], "b");
    }
}
