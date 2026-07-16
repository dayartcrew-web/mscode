//! MCP (Model Context Protocol) client for mscode.
//!
//! Stdio transport only for v1.1. The transport trait ([`transport::McpTransport`])
//! is set up so future transports (HTTP+SSE, WebSocket) can plug in without
//! changing call sites. JSON-RPC 2.0 envelopes are owned by [`protocol`].
//!
//! MCP tools discovered via [`client::McpClient::list_tools`] are adapted to
//! the mscode [`Tool`][mscode_tools::Tool] trait via [`adapter::McpToolAdapter`],
//! so they can be registered in a [`ToolRegistry`][mscode_tools::ToolRegistry]
//! alongside the built-in filesystem tools.

pub mod adapter;
pub mod client;
pub mod error;
pub mod protocol;
pub mod transport;

pub use adapter::McpToolAdapter;
pub use client::McpClient;
pub use error::{McpError, McpResult};
pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpToolInfo};
pub use transport::{McpTransport, StdioTransport};
