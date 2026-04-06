//! Method handlers for the Gradient agent mode.
//!
//! Each handler takes params and a session reference, and returns a JSON value.

use std::path::Path;

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
/// The typechecker emits hole diagnostics with notes like:
///   "expected type: Int"
///   "matching bindings in scope: `a` (Int), `b` (Int)"
///   "matching functions: `random_int(min: Int, max: Int)` -> Int, ..."
///
/// We parse these into structured data.
fn extract_holes(diagnostics: &[query::Diagnostic]) -> Vec<HoleInfo> {
    diagnostics
        .iter()
        .filter(|d| d.message.contains("typed hole"))
        .map(|d| {
            let expected_type = d.notes.iter().find_map(|n| {
                n.strip_prefix("expected type: ").map(|s| s.to_string())
            });

            let matching_bindings = d
                .notes
                .iter()
                .find(|n| n.contains("matching bindings"))
                .map(|n| {
                    // Format: "matching bindings in scope: `a` (Int), `b` (Int)"
                    // Split on the first colon to get the list part.
                    let list = n.splitn(2, ':').nth(1).unwrap_or("");
                    parse_binding_list(list)
                })
                .unwrap_or_default();

            let matching_functions = d
                .notes
                .iter()
                .find(|n| n.contains("matching functions"))
                .map(|n| {
                    // Format: "matching functions: `sig` -> Ret, `sig` -> Ret, ..."
                    let list = n.splitn(2, ':').nth(1).unwrap_or("");
                    parse_function_list(list)
                })
                .unwrap_or_default();

            HoleInfo {
                span: d.span,
                expected_type,
                matching_bindings,
                matching_functions,
                matching_variants: Vec::new(),
            }
        })
        .collect()
}

/// Parse binding list from note format: " `a` (Int), `b` (Int)"
fn parse_binding_list(s: &str) -> Vec<HoleBinding> {
    // Split on "), " to handle entries like "`a` (Int), `b` (Int)"
    let mut result = Vec::new();
    for entry in s.split("), ") {
        let entry = entry.trim().trim_end_matches(')');
        // Entry looks like: "`a` (Int" or "`name` (Type"
        if let Some(backtick_start) = entry.find('`') {
            if let Some(backtick_end) = entry[backtick_start + 1..].find('`') {
                let name = entry[backtick_start + 1..backtick_start + 1 + backtick_end].to_string();
                // Type is after the closing backtick, inside parens
                let rest = &entry[backtick_start + 1 + backtick_end + 1..];
                let ty = rest
                    .trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .trim()
                    .to_string();
                if !name.is_empty() {
                    result.push(HoleBinding {
                        name,
                        ty: if ty.is_empty() { "unknown".to_string() } else { ty },
                    });
                }
            }
        }
    }
    result
}

/// Parse function list from note format: " `sig(params)` -> Ret, ..."
fn parse_function_list(s: &str) -> Vec<HoleFunction> {
    let mut result = Vec::new();
    // Split on "`, " to separate entries, being careful with backtick boundaries.
    // Each entry looks like: "`random_int(min: Int, max: Int)` -> Int"
    // We can't naively split on ", " because param lists contain commas.
    // Instead, split on "` -> " boundaries which mark function entries.
    let mut remaining = s.trim();
    while !remaining.is_empty() {
        // Find the opening backtick
        let start = match remaining.find('`') {
            Some(i) => i,
            None => break,
        };
        remaining = &remaining[start + 1..];

        // Find the closing backtick — it's the one followed by " -> "
        // The signature may contain nested backticks in theory, but in practice
        // the format is `name(params)` -> RetType
        let end = match remaining.find("` -> ") {
            Some(i) => i,
            None => {
                // No return type arrow — might be end of string with just backtick
                if let Some(i) = remaining.find('`') {
                    let sig = remaining[..i].to_string();
                    let fn_name = sig.split('(').next().unwrap_or(&sig).to_string();
                    if !fn_name.is_empty() {
                        result.push(HoleFunction {
                            name: fn_name,
                            signature: format!("fn {}", sig),
                        });
                    }
                }
                break;
            }
        };

        let sig = remaining[..end].to_string();
        remaining = &remaining[end + 5..]; // skip "` -> "

        // The return type goes until the next ", `" or end of string
        let ret_end = remaining.find(", `").unwrap_or(remaining.len());
        let ret_type = remaining[..ret_end].trim().to_string();
        remaining = &remaining[ret_end..];
        if remaining.starts_with(", ") {
            remaining = &remaining[2..];
        }

        let fn_name = sig.split('(').next().unwrap_or(&sig).to_string();
        if !fn_name.is_empty() {
            result.push(HoleFunction {
                name: fn_name,
                signature: format!("fn {} -> {}", sig, ret_type),
            });
        }
    }
    result
}

/// Build a full SessionReport from a Session.
fn build_report(session: &Session) -> SessionReport {
    let check = session.check();
    let symbols = session.symbols();
    let holes = extract_holes(&check.diagnostics);

    let effects = session.effect_summary().map(|s| serde_json::to_value(s).unwrap());

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

/// Handle the `load` or `check` method.
pub fn handle_load(
    params: &Value,
    session: &mut Option<Session>,
) -> Result<Value, Response> {
    let source = params.get("source").and_then(|v| v.as_str());
    let file = params.get("file").and_then(|v| v.as_str());

    let new_session = match (source, file) {
        (Some(src), _) => Session::from_source(src),
        (None, Some(path)) => {
            Session::from_file(Path::new(path)).map_err(|e| {
                Response::error(None, protocol::FILE_NOT_FOUND, e)
            })?
        }
        (None, None) => {
            return Err(Response::error(
                None,
                protocol::INVALID_PARAMS,
                "Either \"source\" or \"file\" parameter is required",
            ));
        }
    };

    let report = build_report(&new_session);
    *session = Some(new_session);

    serde_json::to_value(&report).map_err(|e| {
        Response::error(None, protocol::PARSE_ERROR, format!("Serialization error: {}", e))
    })
}

/// Handle the `symbols` method.
pub fn handle_symbols(session: &Session) -> Result<Value, Response> {
    let symbols = session.symbols();
    serde_json::to_value(&symbols).map_err(|e| {
        Response::error(None, protocol::PARSE_ERROR, format!("Serialization error: {}", e))
    })
}

/// Handle the `holes` method.
pub fn handle_holes(session: &Session) -> Result<Value, Response> {
    let check = session.check();
    let holes = extract_holes(&check.diagnostics);
    serde_json::to_value(&holes).map_err(|e| {
        Response::error(None, protocol::PARSE_ERROR, format!("Serialization error: {}", e))
    })
}

/// Handle the `complete` method.
pub fn handle_complete(params: &Value, session: &Session) -> Result<Value, Response> {
    let line = params
        .get("line")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| {
            Response::error(None, protocol::INVALID_PARAMS, "\"line\" parameter required (u32)")
        })?;
    let col = params
        .get("col")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .ok_or_else(|| {
            Response::error(None, protocol::INVALID_PARAMS, "\"col\" parameter required (u32)")
        })?;

    let ctx = session.completion_context(line, col);
    serde_json::to_value(&ctx).map_err(|e| {
        Response::error(None, protocol::PARSE_ERROR, format!("Serialization error: {}", e))
    })
}

/// Handle the `context_budget` method.
pub fn handle_context_budget(params: &Value, session: &Session) -> Result<Value, Response> {
    let function = params
        .get("function")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Response::error(
                None,
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
                None,
                protocol::INVALID_PARAMS,
                "\"budget\" parameter required (u32)",
            )
        })?;

    let result = session.context_budget(function, budget);
    serde_json::to_value(&result).map_err(|e| {
        Response::error(None, protocol::PARSE_ERROR, format!("Serialization error: {}", e))
    })
}

/// Handle the `effects` method.
pub fn handle_effects(session: &Session) -> Result<Value, Response> {
    match session.effect_summary() {
        Some(summary) => serde_json::to_value(summary).map_err(|e| {
            Response::error(None, protocol::PARSE_ERROR, format!("Serialization error: {}", e))
        }),
        None => Ok(serde_json::json!(null)),
    }
}

/// Handle the `inspect` method.
pub fn handle_inspect(session: &Session) -> Result<Value, Response> {
    let contract = session.module_contract();
    serde_json::to_value(&contract).map_err(|e| {
        Response::error(None, protocol::PARSE_ERROR, format!("Serialization error: {}", e))
    })
}

/// Handle the `call_graph` method.
pub fn handle_call_graph(session: &Session) -> Result<Value, Response> {
    let graph = session.call_graph();
    serde_json::to_value(&graph).map_err(|e| {
        Response::error(None, protocol::PARSE_ERROR, format!("Serialization error: {}", e))
    })
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
        // Session::from_file converts file-not-found into a session with errors
        // rather than returning Err, so handle_load succeeds but reports errors.
        let params = serde_json::json!({"file": "/nonexistent/path.gr"});
        let mut session = None;
        let result = handle_load(&params, &mut session).unwrap();
        assert!(!result["ok"].as_bool().unwrap());
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
        let params =
            serde_json::json!({"source": "fn greet() -> !{IO} ():\n    print(\"hi\")\n"});
        let mut session = None;
        handle_load(&params, &mut session).unwrap();
        let result = handle_effects(session.as_ref().unwrap()).unwrap();
        // effects should be non-null for a valid program
        assert!(!result.is_null());
    }

    #[test]
    fn call_graph_after_load() {
        let params = serde_json::json!({"source": "fn a() -> Int:\n    b()\n\nfn b() -> Int:\n    42\n"});
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
