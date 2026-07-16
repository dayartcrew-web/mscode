//! Text embeddings.
//!
//! [`Embedder`] is the small trait every embedding backend implements. The
//! crate ships two concrete impls:
//!
//! - [`MockEmbedder`] — deterministic vectors for tests, always available.
//! - [`FastEmbedder`] — local ONNX embeddings via `fastembed-rs`. This impl
//!   is gated behind the `fastembed` feature (enabled by default) because the
//!   underlying crate pulls in the ONNX runtime which is heavy. Slim builds
//!   that only need the trait can disable default features.
//!
//! # Cold-start note
//! The ONNX model is **not** loaded at construction time. [`FastEmbedder::new`]
//! is cheap; the model loads lazily on the first [`Embedder::embed`] /
//! [`Embedder::embed_batch`] call. This preserves the workspace's sub-200ms
//! cold-start budget for the CLI surface. The model is downloaded to the
//! local Hugging Face cache on first run (offline thereafter).

mod error;
#[cfg(feature = "fastembed")]
mod fast_embedder;
mod mock;

pub use error::EmbedError;
#[cfg(feature = "fastembed")]
pub use fast_embedder::{FastEmbedModel, FastEmbedder};
pub use mock::MockEmbedder;

/// Result alias.
pub type Result<T> = std::result::Result<T, EmbedError>;

/// Synchronous text-embedding API.
///
/// Implementations must be `Send + Sync` so they can be shared across tasks.
pub trait Embedder: Send + Sync {
    /// Embed a single text.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed a batch. Default impl loops [`Embedder::embed`]; backends that
    /// can batch natively should override.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(t)?);
        }
        Ok(out)
    }

    /// Dimensionality of the vectors produced by this embedder.
    fn dim(&self) -> usize;
}
