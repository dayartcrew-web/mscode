//! MCP (Model Context Protocol) server library for mscode.
//!
//! This crate exposes mscode tool handlers to external MCP clients (Claude
//! Code, Cursor, rig-based consumers, etc.) via stdio JSON-RPC. It is a
//! **library**, not a binary — external clients link against it the same way
//! they link against `rmcp` / `rig`. There is no `mscode mcp` subcommand and
//! no sidecar process. The single-binary invariant of the CLI is preserved.
//!
//! # Wire format
//!
//! We use **newline-delimited JSON** (one JSON-RPC envelope per line) rather
//! than `Content-Length` header framing. Rationale:
//!
//! 1. Most MCP clients (and the codex-rs / rig reference impls) default to
//!    newline-delimited JSON over stdio because it is simpler to debug and
//!    avoids the partial-read pitfalls of header framing.
//! 2. It maps cleanly onto `tokio::io::AsyncBufReadExt::read_line` without a
//!    hand-rolled buffer + parser.
//! 3. Each line is a self-contained `JsonRpcRequest` or
//!    `JsonRpcNotification`; the server emits one `JsonRpcResponse` per line
//!    for any request that carries an `id`.
//!
//! # Protocol behavior
//!
//! | Method          | Behavior                                                        |
//! |-----------------|-----------------------------------------------------------------|
//! | `initialize`    | Respond with server capabilities (tools only, stdio).           |
//! | `tools/list`    | Return the specs of every registered tool.                       |
//! | `tools/call`    | Dispatch to the registry; return the result as a JSON-RPC result.|
//! | unknown method  | Return error code -32601 (Method not found).                    |
//! | malformed JSON  | Return error code -32700 (Parse error).                         |
//!
//! # Cold start
//!
//! [`McpServer::new`] registers built-in tools but does not start an async
//! runtime or open the SQLite pool. The library is safe to link from a binary
//! that does not invoke it; it contributes zero cost to the `mscode version`
//! fast path.

pub mod error;

pub use error::{McpServerError, McpServerResult};
pub use server::{McpServer, ServerCapabilities, ServerInfo};

mod server;

#[cfg(test)]
mod tests;
