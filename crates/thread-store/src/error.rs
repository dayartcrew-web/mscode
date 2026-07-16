//! Error type for the thread store crate.

use thiserror::Error;

/// Failures raised by [`crate::SessionStore`].
#[derive(Debug, Error)]
pub enum ThreadStoreError {
    /// Underlying SQLite failure.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Underlying state/pool failure.
    #[error("state error: {0}")]
    State(#[from] mscode_state::StateError),

    /// Multiple sessions matched a prefix — caller must provide more chars.
    #[error("ambiguous prefix: {0} sessions matched")]
    AmbiguousPrefix(usize),

    /// No session matched the provided ID or prefix.
    #[error("session not found: {0}")]
    NotFound(String),

    /// Attempted to operate on an empty / invalid identifier.
    #[error("invalid id: {0}")]
    InvalidId(String),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, ThreadStoreError>;
