//! Error type for the core crate.

use thiserror::Error;

/// Errors emitted by the core crate.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Wraps an upstream rollout error.
    #[error("rollout error: {0}")]
    Rollout(#[from] mscode_rollout::RolloutError),

    /// Attempted to start a session that was already started.
    #[error("session already started: {0}")]
    SessionAlreadyStarted(String),

    /// Attempted to operate on a session that has not been started.
    #[error("session not started")]
    SessionNotStarted,

    /// Wraps a low-level filesystem failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias used across the core crate.
pub type Result<T> = std::result::Result<T, CoreError>;
