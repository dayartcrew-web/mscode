//! Error type for the memories crate.

use thiserror::Error;

/// Failures raised by [`crate::MemoryStore`].
#[derive(Debug, Error)]
pub enum MemoryError {
    /// Underlying SQLite failure.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Underlying state/pool failure.
    #[error("state error: {0}")]
    State(#[from] mscode_state::StateError),

    /// No memory matched the query.
    #[error("memory not found: {0}")]
    NotFound(String),

    /// Caller supplied an invalid input (empty key, empty id, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, MemoryError>;
