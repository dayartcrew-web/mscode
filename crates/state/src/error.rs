//! Error type for the state crate.

use thiserror::Error;

/// Failures raised while opening or interacting with the local SQLite store.
#[derive(Debug, Error)]
pub enum StateError {
    /// Wraps a rusqlite failure (e.g. malformed SQL, busy connection).
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Wraps an r2d2 pool acquisition failure.
    #[error("pool error: {0}")]
    Pool(String),

    /// Schema bootstrap / migration failure.
    #[error("migration error: {0}")]
    Migration(String),

    /// IO failure while creating the database file or its parent directory.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
