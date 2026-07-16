//! Adapter that exposes an MCP-discovered tool as a `mscode_tools::Tool`.
//!
//! The adapter holds an `Arc<tokio::sync::Mutex<McpClient>>` so that multiple
//! tools discovered from the same MCP server can share a single transport.
//! (Each call to `invoke` locks the client for the duration of the round-trip.)

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use mscode_tools::ToolError;
use mscode_tools::tool::{Tool, ToolRegistry};

use crate::client::McpClient;
use crate::error::McpError;
use crate::protocol::McpToolInfo;

/// Adapter that turns a single [`McpToolInfo`] into a `mscode_tools::Tool`.
///
/// Construct one per discovered tool, then `register` them all into a
/// [`ToolRegistry`].
pub struct McpToolAdapter {
    info: McpToolInfo,
    client: Arc<Mutex<McpClient>>,
}

impl McpToolAdapter {
    /// Construct a new adapter for `info` that dispatches calls via `client`.
    pub fn new(info: McpToolInfo, client: Arc<Mutex<McpClient>>) -> Self {
        Self { info, client }
    }

    /// Borrow the underlying tool info.
    pub fn info(&self) -> &McpToolInfo {
        &self.info
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.info.name
    }

    fn description(&self) -> &str {
        self.info
            .description
            .as_deref()
            .unwrap_or("MCP-backed tool (no description)")
    }

    fn input_schema(&self) -> Value {
        self.info
            .input_schema
            .clone()
            .unwrap_or_else(|| json!({"type": "object"}))
    }

    async fn invoke(&self, input: Value) -> Result<Value, ToolError> {
        let mut client = self.client.lock().await;
        client
            .call_tool(&self.info.name, input)
            .await
            .map_err(map_mcp_err)
    }
}

fn map_mcp_err(err: McpError) -> ToolError {
    match err {
        McpError::UnknownTool(n) => {
            ToolError::InvalidInput(format!("mcp server reports unknown tool: {n}"))
        }
        other => ToolError::Exec(format!("mcp error: {other}")),
    }
}

/// Convenience helper: discover tools from an already-connected client and
/// register every one of them into the supplied registry.
///
/// Returns the names of the tools that were registered.
pub async fn register_all(
    registry: &mut ToolRegistry,
    client: McpClient,
) -> Result<Vec<String>, McpError> {
    let shared = Arc::new(Mutex::new(client));
    // We need to call list_tools through the shared mutex, but the borrow
    // needs to be brief so we can drop the guard before iterating.
    let tools = {
        let mut guard = shared.lock().await;
        guard.list_tools().await?
    };
    let mut names = Vec::with_capacity(tools.len());
    for info in tools {
        let name = info.name.clone();
        let adapter = McpToolAdapter::new(info, Arc::clone(&shared));
        registry.register(Arc::new(adapter));
        names.push(name);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{JsonRpcResponse, McpToolInfo};
    use crate::transport::MockTransport;
    use serde_json::json;

    fn make_client(responses: Vec<JsonRpcResponse>) -> McpClient {
        McpClient::with_transport(Box::new(MockTransport::with_responses(responses)))
    }

    #[tokio::test]
    async fn adapter_forwards_invoke_to_client() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: Some(json!({"content": [{"text": "hi"}]})),
            error: None,
        };
        let client = make_client(vec![resp]);
        let shared = Arc::new(Mutex::new(client));
        let info = McpToolInfo {
            name: "search".into(),
            description: Some("run a search".into()),
            input_schema: Some(json!({"type": "object"})),
        };
        let adapter = McpToolAdapter::new(info, shared);
        assert_eq!(adapter.name(), "search");
        assert_eq!(adapter.description(), "run a search");
        let out = adapter.invoke(json!({"q": "rust"})).await.unwrap();
        assert_eq!(out["content"][0]["text"], "hi");
    }

    #[tokio::test]
    async fn adapter_uses_default_description_and_schema() {
        let client = make_client(vec![]);
        let shared = Arc::new(Mutex::new(client));
        let info = McpToolInfo {
            name: "noop".into(),
            description: None,
            input_schema: None,
        };
        let adapter = McpToolAdapter::new(info, shared);
        assert!(adapter.description().contains("MCP-backed"));
        assert_eq!(adapter.input_schema(), json!({"type": "object"}));
    }

    #[tokio::test]
    async fn adapter_translates_errors() {
        let err_resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: None,
            error: Some(crate::protocol::JsonRpcError {
                code: -32603,
                message: "boom".into(),
                data: None,
            }),
        };
        let client = make_client(vec![err_resp]);
        let shared = Arc::new(Mutex::new(client));
        let info = McpToolInfo {
            name: "x".into(),
            description: None,
            input_schema: None,
        };
        let adapter = McpToolAdapter::new(info, shared);
        let err = adapter.invoke(json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::Exec(_)));
    }

    #[tokio::test]
    async fn register_all_registers_every_tool() {
        // Push list response and one call response per tool.
        let list_resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: Some(json!({
                "tools": [
                    {"name": "alpha"},
                    {"name": "beta"},
                ]
            })),
            error: None,
        };
        let client = make_client(vec![list_resp]);
        let mut registry = ToolRegistry::new();
        let names = register_all(&mut registry, client).await.unwrap();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert_eq!(registry.len(), 2);
    }
}
