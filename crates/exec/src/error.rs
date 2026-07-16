//! Error type for the executor crate.

use thiserror::Error;

/// Result alias used across the exec crate.
pub type ExecResult<T> = std::result::Result<T, ExecError>;

/// Failures raised by [`crate::Executor`] and [`crate::NodeHandler`]
/// implementations.
#[derive(Debug, Clone, Error)]
pub enum ExecError {
    /// The node's `label` did not match any registered handler.
    #[error("handler not found: {0}")]
    HandlerNotFound(String),

    /// The handler ran but returned an error.
    #[error("handler failed: {0}")]
    HandlerFailed(String),

    /// The handler rejected its input as malformed.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The [`crate::NodeContext`] was missing a required field (workspace
    /// path, identity, retry counter, etc.).
    #[error("invalid execution context: {0}")]
    Context(String),
}
