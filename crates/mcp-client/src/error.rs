//! Error type for the MCP client crate.

use thiserror::Error;

/// Errors returned by [`crate::McpClient`] and transports.
#[derive(Debug, Error)]
pub enum McpError {
    /// Underlying IO failure talking to the server process.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Server returned a JSON-RPC error response.
    #[error("json-rpc error ({code}): {message}")]
    Rpc {
        /// JSON-RPC error code.
        code: i64,
        /// Server-supplied message.
        message: String,
    },

    /// Could not parse a JSON-RPC message from the server.
    #[error("malformed json-rpc message: {0}")]
    Malformed(String),

    /// The transport was closed before a response arrived.
    #[error("transport closed before response arrived")]
    TransportClosed,

    /// JSON-RPC id mismatch (response did not match request).
    #[error("json-rpc id mismatch: expected {expected}, got {got:?}")]
    IdMismatch { expected: i64, got: Option<i64> },

    /// Spawn failed.
    #[error("spawn failed: {0}")]
    Spawn(String),

    /// The tool name was not advertised by the server.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}

/// Result alias for MCP client operations.
pub type McpResult<T> = std::result::Result<T, McpError>;
