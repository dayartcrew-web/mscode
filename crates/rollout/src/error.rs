//! Error type for the rollout crate.

use thiserror::Error;

/// All errors emitted by the rollout crate.
#[derive(Debug, Error)]
pub enum RolloutError {
    /// Wraps a low-level filesystem I/O failure.
    #[error("rollout io error: {0}")]
    Io(#[from] std::io::Error),

    /// A line in the log could not be parsed as JSON, or an event could not
    /// be serialized prior to appending.
    #[error("rollout json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A line in the log could not be parsed as JSON.
    #[error("rollout parse error at line {line}: {source}")]
    Parse {
        /// 1-based line number in the file.
        line: usize,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },

    /// The reducer returned an error while applying an event.
    #[error("rollout reducer error: {0}")]
    Reducer(String),
}

/// Result alias used across the rollout crate.
pub type Result<T> = std::result::Result<T, RolloutError>;
