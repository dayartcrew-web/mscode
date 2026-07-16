//! [`FastEmbedder`] — local ONNX embeddings via `fastembed-rs`.
//!
//! The model is **not** loaded at [`FastEmbedder::new`] time. It loads lazily
//! on the first [`crate::Embedder::embed`] / [`crate::Embedder::embed_batch`]
//! call, preserving the sub-200ms cold-start budget for the CLI. On the first
//! run, `fastembed` downloads the model into the local Hugging Face cache;
//! subsequent runs work offline.
//!
//! `fastembed`'s inference API is **synchronous** (`&mut self` blocking call).
//! We wrap the model in a [`std::sync::Mutex`] so the [`Embedder`](crate::Embedder)
//! trait can stay sync — no tokio runtime is required.

use std::sync::{Mutex, OnceLock};

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::error::EmbedError;
use crate::{Embedder, Result};

/// Selectable fastembed model.
///
/// Defaults to BAAI/bge-small-en-v1.5 (384 dims) which works offline and is
/// fast on CPU.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FastEmbedModel {
    /// BAAI/bge-small-en-v1.5 (384 dims).
    #[default]
    BgeSmallEn,
}

impl FastEmbedModel {
    /// Underlying fastembed model.
    fn to_fastembed(self) -> EmbeddingModel {
        match self {
            FastEmbedModel::BgeSmallEn => EmbeddingModel::BGESmallENV15,
        }
    }

    /// Vector dimensionality for this model.
    pub fn dim(self) -> usize {
        match self {
            FastEmbedModel::BgeSmallEn => 384,
        }
    }
}

/// Local ONNX embedder. Lazily initializes the model on first embed call.
pub struct FastEmbedder {
    model: FastEmbedModel,
    inner: OnceLock<Mutex<TextEmbedding>>,
}

impl FastEmbedder {
    /// Build a new embedder targeting the default model
    /// ([`FastEmbedModel::BgeSmallEn`]).
    pub fn new() -> Self {
        Self::with_model(FastEmbedModel::default())
    }

    /// Build a new embedder targeting a specific model.
    pub fn with_model(model: FastEmbedModel) -> Self {
        Self {
            model,
            inner: OnceLock::new(),
        }
    }

    /// Configured model.
    pub fn model(&self) -> FastEmbedModel {
        self.model
    }

    /// Returns `true` once the underlying ONNX session has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.inner.get().is_some()
    }

    fn with_embedding<R>(&self, f: impl FnOnce(&mut TextEmbedding) -> Result<R>) -> Result<R> {
        // Fast path: already initialized.
        if let Some(m) = self.inner.get() {
            let mut guard = m
                .lock()
                .map_err(|e| EmbedError::backend(format!("model lock poisoned: {e}")))?;
            return f(&mut guard);
        }
        // Slow path: lazy init.
        let options =
            InitOptions::new(self.model.to_fastembed()).with_show_download_progress(false);
        let embedding = TextEmbedding::try_new(options)
            .map_err(|e| EmbedError::backend(format!("init fastembed: {e}")))?;
        let mutex = Mutex::new(embedding);
        // Race-safe: race loser's `set` returns Err with the winning mutex.
        let stored: &Mutex<TextEmbedding> = match self.inner.set(mutex) {
            Ok(()) => self.inner.get().expect("just-set must be Some"),
            Err(rejected) => {
                // Drop the loser; use the canonical reference returned by get.
                drop(rejected);
                self.inner.get().expect("set err implies Some is present")
            }
        };
        let mut guard = stored
            .lock()
            .map_err(|e| EmbedError::backend(format!("model lock poisoned: {e}")))?;
        f(&mut guard)
    }
}

impl Default for FastEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FastEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastEmbedder")
            .field("model", &self.model)
            .field("initialized", &self.is_initialized())
            .finish()
    }
}

impl Embedder for FastEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            return Err(EmbedError::EmptyInput);
        }
        self.with_embedding(|model| {
            let docs = vec![text.to_string()];
            let mut vectors = model
                .embed(docs, None)
                .map_err(|e| EmbedError::backend(format!("inference: {e}")))?;
            vectors
                .pop()
                .ok_or_else(|| EmbedError::backend("fastembed returned no vectors"))
        })
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if texts.iter().any(|t| t.is_empty()) {
            return Err(EmbedError::EmptyInput);
        }
        self.with_embedding(|model| {
            let docs: Vec<String> = texts.iter().map(|s| (*s).to_string()).collect();
            model
                .embed(docs, None)
                .map_err(|e| EmbedError::backend(format!("inference: {e}")))
        })
    }

    fn dim(&self) -> usize {
        self.model.dim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_dim_reports_384_for_bge_small() {
        assert_eq!(FastEmbedModel::BgeSmallEn.dim(), 384);
    }

    #[test]
    fn default_is_bge_small() {
        assert_eq!(FastEmbedModel::default(), FastEmbedModel::BgeSmallEn);
    }

    #[test]
    fn fastembedder_carries_model_metadata() {
        let e = FastEmbedder::new();
        assert_eq!(e.model(), FastEmbedModel::BgeSmallEn);
        assert_eq!(e.dim(), 384);
    }

    #[test]
    fn debug_format_does_not_panic() {
        let e = FastEmbedder::new();
        let _ = format!("{e:?}");
    }

    #[test]
    fn is_initialized_false_before_first_call() {
        let e = FastEmbedder::new();
        assert!(!e.is_initialized());
    }
}
