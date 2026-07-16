//! Error type for the MCP server crate.

use thiserror::Error;

/// Errors returned by [`crate::McpServer`].
#[derive(Debug, Error)]
pub enum McpServerError {
    /// Underlying IO failure on the stdio pipe.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization failure on the wire.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Tool invocation returned an error. Wrapped verbatim so callers can
    /// surface the underlying [`mscode_tools::ToolError`] message to the client.
    #[error("tool error: {0}")]
    Tool(#[from] mscode_tools::ToolError),

    /// MCP-client envelope / parse error. Surfaces issues with JSON-RPC
    /// framing that the client sent.
    #[error("mcp envelope error: {0}")]
    Mcp(#[from] mscode_mcp_client::McpError),

    /// The client sent an unrecognized JSON-RPC method.
    #[error("method not found: {0}")]
    MethodNotFound(String),

    /// The client sent a request that did not deserialize into the expected
    /// shape for the method (e.g. `tools/call` without a `name` field).
    #[error("invalid params: {0}")]
    InvalidParams(String),
}

/// Result alias for MCP server operations.
pub type McpServerResult<T> = std::result::Result<T, McpServerError>;
