//! Error type for the embeddings crate.

use thiserror::Error;

/// Failures raised by [`crate::Embedder`] implementations.
#[derive(Debug, Error)]
pub enum EmbedError {
    /// Underlying `fastembed` failure (model load, inference).
    #[error("embedding backend error: {0}")]
    Backend(String),

    /// Caller supplied empty input.
    #[error("empty input")]
    EmptyInput,
}

impl EmbedError {
    /// Convenience constructor for the `Backend` variant.
    pub fn backend(msg: impl Into<String>) -> Self {
        Self::Backend(msg.into())
    }
}
