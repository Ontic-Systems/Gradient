//! JSON-RPC 2.0 protocol types for the Gradient agent mode.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC 2.0 response.
///
/// Per JSON-RPC 2.0 spec, `id` is always present. For parse/invalid-request
/// errors where the id could not be determined, `id` is `Value::Null`.
#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

// Standard JSON-RPC error codes.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;

// Standard: server-side internal error.
pub const INTERNAL_ERROR: i32 = -32603;

// Custom error codes.
pub const NO_SESSION: i32 = -32001;
pub const FILE_NOT_FOUND: i32 = -32002;

impl Response {
    /// Build a success response. Pass `Value::Null` for id when unknown.
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response. Pass `Value::Null` for id when unknown.
    pub fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }

    pub fn notification(method: &str, params: Value) -> Result<String, serde_json::Error> {
        let obj = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        serde_json::to_string(&obj)
    }
}

/// Parse a JSON line into a Request.
pub fn parse_request(line: &str) -> Result<Request, Response> {
    let req: Request = serde_json::from_str(line)
        .map_err(|e| Response::error(Value::Null, PARSE_ERROR, format!("Invalid JSON: {}", e)))?;
    if req.jsonrpc != "2.0" {
        return Err(Response::error(
            req.id.unwrap_or(Value::Null),
            INVALID_REQUEST,
            "Expected jsonrpc version \"2.0\"",
        ));
    }
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_request() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"load","params":{"source":"fn main() -> !{IO} ():\n    print(\"hi\")"}}"#;
        let req = parse_request(line).unwrap();
        assert_eq!(req.method, "load");
        assert_eq!(req.id, Some(Value::Number(1.into())));
    }

    #[test]
    fn parse_request_no_params() {
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"symbols"}"#;
        let req = parse_request(line).unwrap();
        assert_eq!(req.method, "symbols");
        assert!(req.params.is_null());
    }

    #[test]
    fn parse_invalid_json() {
        let result = parse_request("{bad json}");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error.unwrap().code, PARSE_ERROR);
    }

    #[test]
    fn parse_wrong_version() {
        let line = r#"{"jsonrpc":"1.0","id":1,"method":"load"}"#;
        let result = parse_request(line);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error.unwrap().code, INVALID_REQUEST);
    }

    #[test]
    fn success_response_serialization() {
        let resp = Response::success(Value::Number(1.into()), serde_json::json!({"ok": true}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
        assert!(json.contains("\"id\":1"));
    }

    #[test]
    fn error_response_serialization() {
        let resp = Response::error(Value::Number(1.into()), NO_SESSION, "No active session");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"error\""));
        assert!(!json.contains("\"result\""));
        assert!(json.contains("-32001"));
    }

    #[test]
    fn error_response_null_id_for_parse_error() {
        // Per JSON-RPC 2.0: parse errors must include id as null, not omit it.
        let resp = Response::error(Value::Null, PARSE_ERROR, "Invalid JSON");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"id\":null"));
    }

    #[test]
    fn notification_format() {
        let notif =
            Response::notification("initialized", serde_json::json!({"version": "0.1.0"})).unwrap();
        let parsed: Value = serde_json::from_str(&notif).unwrap();
        assert_eq!(parsed["method"], "initialized");
        assert!(parsed.get("id").is_none());
    }
}
