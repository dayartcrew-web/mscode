//! JSON-RPC 2.0 envelope types used by the MCP wire format.
//!
//! Only the subset of JSON-RPC needed for MCP request/response (no batches,
//! no notifications on our client side) is modeled here. Notifications are
//! supported on the wire via [`JsonRpcNotification`] for completeness but the
//! client does not currently emit or surface them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 version tag.
pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC request envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Request id (must match the response).
    pub id: i64,
    /// Method name.
    pub method: String,
    /// Method parameters (positional or named).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Construct a new request with the given id, method, and optional params.
    pub fn new(id: i64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Numeric error code (negative for protocol-defined errors).
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC 2.0 response envelope. A response carries either a `result` or
/// an `error`, never both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Matches the request id.
    pub id: i64,
    /// Result payload (present iff `error` is absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error payload (present iff `result` is absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Returns `Ok(result)` if the response is successful, or `Err(error)`
    /// if the server returned an error.
    pub fn into_result(self) -> Result<Value, JsonRpcError> {
        if let Some(err) = self.error {
            return Err(err);
        }
        self.result.ok_or_else(|| JsonRpcError {
            code: -32603,
            message: "response had neither result nor error".into(),
            data: None,
        })
    }
}

/// JSON-RPC 2.0 notification envelope (no id, no response expected).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Method name.
    pub method: String,
    /// Notification parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Tool metadata returned by `tools/list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolInfo {
    /// Tool name as advertised by the server.
    pub name: String,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema describing the input shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_serializes_with_jsonrpc_tag() {
        let req = JsonRpcRequest::new(7, "tools/list", None);
        let v: Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "tools/list");
        // params should be omitted when None.
        assert!(v.get("params").is_none() || v["params"].is_null());
    }

    #[test]
    fn response_into_result_returns_payload_on_success() {
        let resp = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.into(),
            id: 1,
            result: Some(json!({"tools": []})),
            error: None,
        };
        let r = resp.into_result().unwrap();
        assert_eq!(r, json!({"tools": []}));
    }

    #[test]
    fn response_into_result_returns_error_on_failure() {
        let resp = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.into(),
            id: 1,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "method not found".into(),
                data: None,
            }),
        };
        let err = resp.into_result().unwrap_err();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "method not found");
    }

    #[test]
    fn tool_info_round_trips() {
        let info = McpToolInfo {
            name: "search".into(),
            description: Some("run a search".into()),
            input_schema: Some(json!({"type": "object"})),
        };
        let s = serde_json::to_string(&info).unwrap();
        let back: McpToolInfo = serde_json::from_str(&s).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn deserializes_real_world_response() {
        let raw = r#"{"jsonrpc":"2.0","id":42,"result":{"ok":true}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.id, 42);
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap()["ok"], true);
    }
}
