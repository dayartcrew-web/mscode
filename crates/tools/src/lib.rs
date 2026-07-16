//! Tool trait + registry and built-in filesystem/shell tools for mscode.
//!
//! The [`tool::Tool`] trait is the load-bearing abstraction that the rest of
//! the workspace (agents, MCP adapters, future WASM plugins) will talk to.
//! It is async-first and uses [`serde_json::Value`] for both input and output
//! so the same trait can serve:
//!
//!   * in-process Rust tools (this crate's built-ins),
//!   * MCP-server-backed tools (via the `mscode-mcp-client` adapter), and
//!   * future WASM plugins (the ABI seam will compile to the same shape).
//!
//! All built-in tools honour the [`mscode_sandbox::Sandbox`] policy for reads,
//! writes, and exec. Tools that bypass the sandbox cannot be registered via
//! [`tool::ToolRegistry::register_default_fs_tools`].

pub mod bash;
pub mod error;
pub mod fs_grep;
pub mod fs_list;
pub mod fs_read;
pub mod fs_write;
pub mod tool;

pub use bash::BashTool;
pub use error::{ToolError, ToolResult};
pub use fs_grep::GrepTool;
pub use fs_list::ListDirTool;
pub use fs_read::ReadFileTool;
pub use fs_write::WriteFileTool;
pub use tool::{Tool, ToolRegistry};
