//! Error type for the vector index crate.

use thiserror::Error;

/// Failures raised by [`crate::VectorStore`] implementations.
#[derive(Debug, Error)]
pub enum VectorError {
    /// Caller supplied a vector whose dimensionality does not match the store.
    #[error("dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    /// Caller supplied an invalid id (empty, etc.).
    #[error("invalid id: {0}")]
    InvalidId(String),

    /// Generic backend failure (LanceDB IO, etc.).
    #[error("backend error: {0}")]
    Backend(String),

    /// Serialization failure of the `metadata` JSON blob.
    #[error("metadata serde error: {0}")]
    MetadataSerde(#[from] serde_json::Error),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, VectorError>;
