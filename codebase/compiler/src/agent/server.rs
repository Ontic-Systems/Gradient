//! Agent mode server: stdin/stdout JSON-RPC loop.

use std::io::{self, BufRead, Write};

use serde_json::Value;

use crate::query::Session;

use super::handlers;
use super::protocol::{self, Response};

const VERSION: &str = "0.1.0";

const CAPABILITIES: &[&str] = &[
    "load",
    "check",
    "symbols",
    "holes",
    "complete",
    "context_budget",
    "effects",
    "inspect",
    "call_graph",
    "shutdown",
];

/// Run the agent server loop.
///
/// Reads JSON-RPC requests from stdin (one per line), dispatches to handlers,
/// writes JSON-RPC responses to stdout (one per line). Holds a `Session` in
/// memory across requests.
pub fn run(pretty: bool) {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    // Send initialized notification.
    let caps: Vec<&str> = CAPABILITIES.to_vec();
    let init = Response::notification(
        "initialized",
        serde_json::json!({
            "version": VERSION,
            "capabilities": caps,
        }),
    );
    if writeln!(out, "{}", init).is_err() || out.flush().is_err() {
        return;
    }

    let mut session: Option<Session> = None;

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break, // stdin closed
        };

        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let req = match protocol::parse_request(&line) {
            Ok(r) => r,
            Err(err_resp) => {
                // Parse errors always get a response (with id = null per spec).
                if !write_response(&mut out, &err_resp, pretty) {
                    return;
                }
                continue;
            }
        };

        // Per JSON-RPC 2.0: a request without `id` is a notification and must
        // not receive a response. We still dispatch so the method executes.
        let is_notification = req.id.is_none();
        let id = req.id.clone().unwrap_or(serde_json::Value::Null);
        let method = req.method.clone();
        let response = dispatch(&req.method, &req.params, &mut session, id);

        if !is_notification && !write_response(&mut out, &response, pretty) {
            return;
        }

        // Check for shutdown after dispatching.
        if method == "shutdown" {
            break;
        }
    }
}

/// Dispatch a method call to the appropriate handler.
fn dispatch(
    method: &str,
    params: &Value,
    session: &mut Option<Session>,
    id: Value,
) -> Response {
    match method {
        "load" | "check" => match handlers::handle_load(params, session) {
            Ok(result) => Response::success(id, result),
            Err(mut e) => {
                e.id = id;
                e
            }
        },

        "symbols" => with_session(session, id, handlers::handle_symbols),

        "holes" => with_session(session, id, handlers::handle_holes),

        "complete" => with_session(session, id, |s| handlers::handle_complete(params, s)),

        "context_budget" => {
            with_session(session, id, |s| handlers::handle_context_budget(params, s))
        }

        "effects" => with_session(session, id, handlers::handle_effects),

        "inspect" => with_session(session, id, handlers::handle_inspect),

        "call_graph" => with_session(session, id, handlers::handle_call_graph),

        "shutdown" => Response::success(id, serde_json::json!({"ok": true})),

        _ => Response::error(id, protocol::METHOD_NOT_FOUND, format!("Unknown method: {}", method)),
    }
}

/// Helper: require an active session, then call the handler.
fn with_session<F>(session: &Option<Session>, id: Value, f: F) -> Response
where
    F: FnOnce(&Session) -> Result<Value, Response>,
{
    match session {
        Some(s) => match f(s) {
            Ok(result) => Response::success(id, result),
            Err(mut e) => {
                e.id = id;
                e
            }
        },
        None => Response::error(
            id,
            protocol::NO_SESSION,
            "No active session. Call \"load\" first.",
        ),
    }
}

/// Write a response to the output stream.
///
/// Returns `false` if the write or flush fails (broken pipe, closed stdout).
/// The server loop should terminate when this happens rather than spin
/// silently dropping responses.
fn write_response(out: &mut impl Write, response: &Response, pretty: bool) -> bool {
    let json = if pretty {
        serde_json::to_string_pretty(response)
    } else {
        serde_json::to_string(response)
    };
    match json {
        Ok(json) => writeln!(out, "{}", json).is_ok() && out.flush().is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: i64) -> Value {
        Value::Number(n.into())
    }

    #[test]
    fn dispatch_unknown_method() {
        let mut session = None;
        let resp = dispatch("nonexistent", &Value::Null, &mut session, id(1));
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, protocol::METHOD_NOT_FOUND);
    }

    #[test]
    fn dispatch_symbols_without_session() {
        let mut session = None;
        let resp = dispatch("symbols", &Value::Null, &mut session, id(1));
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, protocol::NO_SESSION);
    }

    #[test]
    fn dispatch_load_then_symbols() {
        let mut session = None;
        let params = serde_json::json!({"source": "fn add(a: Int, b: Int) -> Int:\n    a + b\n"});
        let resp = dispatch("load", &params, &mut session, id(1));
        assert!(resp.result.is_some());

        let resp2 = dispatch("symbols", &Value::Null, &mut session, id(2));
        assert!(resp2.result.is_some());
        let symbols = resp2.result.unwrap();
        assert!(symbols.as_array().unwrap().len() > 0);
    }

    #[test]
    fn dispatch_shutdown() {
        let mut session = None;
        let resp = dispatch("shutdown", &Value::Null, &mut session, id(1));
        assert!(resp.result.is_some());
        assert_eq!(resp.result.unwrap()["ok"], true);
    }

    #[test]
    fn dispatch_check_aliases_load() {
        let mut session = None;
        let params = serde_json::json!({"source": "fn id(x: Int) -> Int:\n    x\n"});
        let resp = dispatch("check", &params, &mut session, id(1));
        assert!(resp.result.is_some());
        assert!(session.is_some());
    }

    #[test]
    fn response_always_includes_id_field() {
        // JSON-RPC 2.0: responses MUST include id. Even on errors.
        let mut session = None;
        let resp = dispatch("nonexistent", &Value::Null, &mut session, id(1));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"id\":1"), "response missing id: {}", json);

        // And null-id error responses (parse errors) should serialize id as null.
        let err = Response::error(Value::Null, protocol::PARSE_ERROR, "bad");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"id\":null"), "null id not serialized: {}", json);
    }
}
