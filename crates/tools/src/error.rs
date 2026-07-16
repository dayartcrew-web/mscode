//! Error type for the tools crate.

use thiserror::Error;

/// Errors returned by built-in tools and the [`crate::ToolRegistry`].
#[derive(Debug, Error)]
pub enum ToolError {
    /// Sandbox policy rejected the operation.
    #[error("sandbox rejected operation: {0}")]
    Sandbox(#[from] mscode_sandbox::SandboxError),

    /// Underlying IO failure (file not found, permission denied, etc.).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Caller-supplied input did not match the tool's schema.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Tool was invoked with an unknown name.
    #[error("tool not found: {0}")]
    NotFound(String),

    /// Regex compilation failed.
    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),

    /// A subprocess exited with a non-zero status or timed out.
    #[error("exec error: {0}")]
    Exec(String),

    /// JSON serialization/deserialization failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result alias for tool operations.
pub type ToolResult<T> = std::result::Result<T, ToolError>;
