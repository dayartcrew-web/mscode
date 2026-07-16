//! [`McpServer`] — JSON-RPC loop, capability advertisement, and tool dispatch.

use std::path::Path;
use std::sync::Arc;

use mscode_mcp_client::protocol::JSONRPC_VERSION;
use mscode_mcp_client::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use mscode_tools::{
    BashTool, GrepTool, ListDirTool, ReadFileTool, Tool, ToolRegistry, WriteFileTool,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

use crate::error::McpServerResult;

// ---------------------------------------------------------------------------
// JSON-RPC error codes (per spec — see <https://www.jsonrpc.org/specification>)
// ---------------------------------------------------------------------------

/// Parse error: invalid JSON was received.
pub const PARSE_ERROR: i64 = -32700;
/// Method not found: the requested JSON-RPC method does not exist.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Invalid params: the method exists but the params failed validation.
pub const INVALID_PARAMS: i64 = -32602;

// ---------------------------------------------------------------------------
// Capability advertisement
// ---------------------------------------------------------------------------

/// Tools-only capability set advertised to the client during `initialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Tools capability; advertised as `{ "listChanged": false }` because the
    /// built-in tool set is static within a single server lifetime.
    pub tools: Value,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            tools: json!({"listChanged": false}),
        }
    }
}

/// Server info returned from `initialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Canonical server name.
    pub name: String,
    /// Semver-style version string.
    pub version: String,
    /// Capabilities advertised to the client.
    pub capabilities: ServerCapabilities,
}

impl ServerInfo {
    fn mscode_default() -> Self {
        Self {
            name: "mscode".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: ServerCapabilities::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// MCP server: owns a [`ToolRegistry`] and routes JSON-RPC requests over an
/// async reader/writer pair.
///
/// External clients construct an `McpServer`, call [`McpServer::run_stdio`],
/// and pipe stdin/stdout (or a duplex) to it. The server reads
/// newline-delimited JSON, one request per line, and emits one response per
/// request.
pub struct McpServer {
    registry: ToolRegistry,
    server_info: ServerInfo,
}

impl McpServer {
    /// Construct a new server with the five built-in tools registered against
    /// a [`mscode_sandbox::Sandbox`] rooted at `workspace_root`:
    /// `read_file`, `write_file`, `list_dir`, `grep`, and `bash`.
    ///
    /// The `workspace_root` should be supplied by the embedding client; it
    /// determines the file-system scope every tool honors.
    pub fn new(workspace_root: &Path) -> Self {
        let sandbox = Arc::new(mscode_sandbox::Sandbox::new(workspace_root));
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ReadFileTool::new(sandbox.clone())));
        registry.register(Arc::new(WriteFileTool::new(sandbox.clone())));
        registry.register(Arc::new(ListDirTool::new(sandbox.clone())));
        registry.register(Arc::new(GrepTool::new(sandbox.clone())));
        registry.register(Arc::new(BashTool::new(sandbox)));
        Self {
            registry,
            server_info: ServerInfo::mscode_default(),
        }
    }

    /// Construct a server from an externally-assembled registry. Useful for
    /// tests and for clients that want to register additional tools before
    /// run.
    pub fn with_registry(registry: ToolRegistry) -> Self {
        Self {
            registry,
            server_info: ServerInfo::mscode_default(),
        }
    }

    /// Register an additional tool after construction. The new tool is added
    /// to the registry; existing tools with the same name are replaced.
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.registry.register(Arc::from(tool));
    }

    /// Borrow the underlying registry (for tests / inspection).
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Borrow the server info advertised to clients.
    pub fn server_info(&self) -> &ServerInfo {
        &self.server_info
    }

    /// Main JSON-RPC loop. Reads newline-delimited JSON from `reader`,
    /// dispatches each request, and writes the response (also
    /// newline-delimited) to `writer`.
    ///
    /// Returns when the reader hits EOF, or when an IO error occurs.
    pub async fn run_stdio<R, W>(&self, reader: R, mut writer: W) -> McpServerResult<()>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut buf = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            let n = buf.read_line(&mut line).await?;
            if n == 0 {
                // EOF — client closed the pipe.
                return Ok(());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let response = self.handle_line(trimmed).await;
            let serialized = match response {
                Some(value) => match serde_json::to_string(&value) {
                    Ok(s) => s,
                    Err(_) => {
                        // Serialization failed for a known request — this
                        // should not happen because we control the response
                        // shape. Fall back to a parse-error envelope.
                        serde_json::to_string(&parse_error_response())
                            .unwrap_or_else(|_| "{}".into())
                    }
                },
                None => continue, // notification — no response emitted.
            };
            writer.write_all(serialized.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }

    /// Handle a single line. Returns `None` for notifications (which do not
    /// get a response on the wire) and `Some(response)` for any request with
    /// an `id`.
    pub(crate) async fn handle_line(&self, line: &str) -> Option<JsonRpcResponse> {
        let parsed: Result<JsonRpcRequest, _> = serde_json::from_str(line);
        let request = match parsed {
            Ok(req) => req,
            Err(e) => {
                warn!(error = %e, "malformed JSON-RPC line");
                return Some(parse_error_response());
            }
        };
        debug!(method = %request.method, id = request.id, "dispatch");
        let result = self.dispatch(&request).await;
        Some(build_response(request.id, result))
    }

    /// Dispatch a parsed JSON-RPC request to the appropriate handler.
    pub(crate) async fn dispatch(&self, request: &JsonRpcRequest) -> Result<Value, JsonRpcError> {
        match request.method.as_str() {
            "initialize" => {
                Ok(serde_json::to_value(self.initialize_result()).unwrap_or(Value::Null))
            }
            "tools/list" => Ok(self.tools_list_result()),
            "tools/call" => self.tools_call(request).await,
            other => Err(JsonRpcError {
                code: METHOD_NOT_FOUND,
                message: format!("method not found: {other}"),
                data: None,
            }),
        }
    }

    fn initialize_result(&self) -> Value {
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": self.server_info.capabilities,
            "serverInfo": {
                "name": self.server_info.name,
                "version": self.server_info.version,
            },
        })
    }

    fn tools_list_result(&self) -> Value {
        let tools: Vec<Value> = self
            .registry
            .list()
            .into_iter()
            .map(|t| {
                json!({
                    "name": t.name(),
                    "description": t.description(),
                    "inputSchema": t.input_schema(),
                })
            })
            .collect();
        json!({"tools": tools})
    }

    async fn tools_call(&self, request: &JsonRpcRequest) -> Result<Value, JsonRpcError> {
        let params = request.params.clone().unwrap_or(Value::Null);
        let parsed: ToolsCallParams = serde_json::from_value(params).map_err(|e| JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("invalid tools/call params: {e}"),
            data: None,
        })?;
        let tool = self
            .registry
            .get(&parsed.name)
            .ok_or_else(|| JsonRpcError {
                code: INVALID_PARAMS,
                message: format!("unknown tool: {}", parsed.name),
                data: None,
            })?;
        match tool.invoke(parsed.arguments).await {
            Ok(output) => Ok(json!({
                "content": [
                    {"type": "text", "text": output.to_string()},
                ],
                "isError": false,
            })),
            Err(e) => {
                warn!(tool = %parsed.name, error = %e, "tool invocation failed");
                Ok(json!({
                    "content": [
                        {"type": "text", "text": e.to_string()},
                    ],
                    "isError": true,
                }))
            }
        }
    }
}

/// Parameters expected by `tools/call`.
#[derive(Debug, Deserialize)]
struct ToolsCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

/// Build a JSON-RPC response from a dispatch result. Errors are wrapped into
/// a proper `JsonRpcError` payload.
pub(crate) fn build_response(id: i64, result: Result<Value, JsonRpcError>) -> JsonRpcResponse {
    match result {
        Ok(value) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: Some(value),
            error: None,
        },
        Err(err) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: None,
            error: Some(err),
        },
    }
}

/// Construct a parse-error response. The id field is `null` per JSON-RPC spec
/// — when the request itself could not be parsed we cannot echo its id.
fn parse_error_response() -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.into(),
        id: 0,
        result: None,
        error: Some(JsonRpcError {
            code: PARSE_ERROR,
            message: "Parse error".into(),
            data: None,
        }),
    }
}
