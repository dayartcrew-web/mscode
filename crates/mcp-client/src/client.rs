//! High-level MCP client that owns a transport and exposes typed helpers.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::{Value, json};

use crate::error::{McpError, McpResult};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolInfo};
use crate::transport::McpTransport;

/// High-level MCP client.
///
/// Owns a transport, generates monotonically-increasing JSON-RPC ids, and
/// surfaces typed helpers for the two MCP calls mscode cares about:
/// `tools/list` and `tools/call`.
pub struct McpClient {
    transport: Box<dyn McpTransport>,
    next_id: Arc<AtomicI64>,
}

impl McpClient {
    /// Construct a new client on top of an existing transport.
    pub fn with_transport(transport: Box<dyn McpTransport>) -> Self {
        Self {
            transport,
            next_id: Arc::new(AtomicI64::new(1)),
        }
    }

    /// Spawn a stdio server and construct a client on top of it.
    pub async fn connect_stdio(command: &str, args: &[&str]) -> McpResult<Self> {
        let transport = crate::transport::StdioTransport::spawn(command, args).await?;
        Ok(Self::with_transport(Box::new(transport)))
    }

    /// Issue a request and wait for the matching response.
    pub async fn request(&mut self, method: &str, params: Option<Value>) -> McpResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest::new(id, method, params);
        self.transport.send(&req).await?;
        let resp: JsonRpcResponse = self.transport.recv().await?;
        if resp.id != id {
            return Err(McpError::IdMismatch {
                expected: id,
                got: Some(resp.id),
            });
        }
        let value = resp.into_result().map_err(|e| McpError::Rpc {
            code: e.code,
            message: e.message,
        })?;
        Ok(value)
    }

    /// Call `tools/list` and parse the response into [`McpToolInfo`] entries.
    pub async fn list_tools(&mut self) -> McpResult<Vec<McpToolInfo>> {
        let value = self.request("tools/list", None).await?;
        let tools_value = value
            .get("tools")
            .cloned()
            .ok_or_else(|| McpError::Malformed("missing `tools` field".into()))?;
        let tools: Vec<McpToolInfo> = serde_json::from_value(tools_value)?;
        Ok(tools)
    }

    /// Call `tools/call` with the given tool name and arguments.
    pub async fn call_tool(&mut self, name: &str, args: Value) -> McpResult<Value> {
        let params = json!({"name": name, "arguments": args});
        self.request("tools/call", Some(params)).await
    }

    /// Close the underlying transport, releasing any spawned child process.
    pub async fn close(mut self) -> McpResult<()> {
        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcResponse;
    use crate::transport::MockTransport;
    use serde_json::json;

    fn resp(id: i64, result: Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    #[tokio::test]
    async fn list_tools_returns_parsed_entries() {
        // The mock pops responses from the back of the queue; we push in the
        // reverse order we expect them to be returned.
        let result = json!({
            "tools": [
                {"name": "search", "description": "search docs"},
                {"name": "write", "description": "write file"}
            ]
        });
        let mock = MockTransport::with_responses(vec![resp(1, result)]);
        let mut client = McpClient::with_transport(Box::new(mock));
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "search");
        assert_eq!(tools[1].name, "write");
    }

    #[tokio::test]
    async fn call_tool_returns_payload() {
        let result = json!({"content": [{"type": "text", "text": "ok"}]});
        let mock = MockTransport::with_responses(vec![resp(1, result)]);
        let mut client = McpClient::with_transport(Box::new(mock));
        let out = client
            .call_tool("search", json!({"q": "rust"}))
            .await
            .unwrap();
        assert_eq!(out["content"][0]["text"], "ok");
    }

    #[tokio::test]
    async fn request_propagates_rpc_errors() {
        let err_resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: None,
            error: Some(crate::protocol::JsonRpcError {
                code: -32601,
                message: "method not found".into(),
                data: None,
            }),
        };
        let mock = MockTransport::with_responses(vec![err_resp]);
        let mut client = McpClient::with_transport(Box::new(mock));
        let err = client.request("nope", None).await.unwrap_err();
        match err {
            McpError::Rpc { code, message } => {
                assert_eq!(code, -32601);
                assert_eq!(message, "method not found");
            }
            _ => panic!("expected Rpc error"),
        }
    }

    #[tokio::test]
    async fn request_detects_id_mismatch() {
        let mock = MockTransport::with_responses(vec![resp(99, json!({}))]);
        let mut client = McpClient::with_transport(Box::new(mock));
        let err = client.request("ping", None).await.unwrap_err();
        assert!(matches!(err, McpError::IdMismatch { .. }));
    }

    #[tokio::test]
    async fn close_invokes_transport_close() {
        let mock = MockTransport::with_responses(vec![]);
        let closed_transport = Box::new(mock);
        let client = McpClient::with_transport(closed_transport);
        client.close().await.unwrap();
    }
}
