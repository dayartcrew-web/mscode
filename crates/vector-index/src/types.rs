//! Public types for the vector store.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One record in the vector index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorRecord {
    /// Stable unique identifier.
    pub id: String,
    /// Embedding vector (length must equal `VectorStore::embed_dim`).
    pub vector: Vec<f32>,
    /// Free-form metadata (must serialize to JSON).
    #[serde(default)]
    pub metadata: Value,
}

/// Query parameters for nearest-neighbor search.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorSearchQuery {
    /// Query vector.
    pub vector: Vec<f32>,
    /// Maximum number of hits to return.
    pub limit: usize,
}

impl VectorSearchQuery {
    /// Build a query with the given vector and limit.
    pub fn new(vector: Vec<f32>, limit: usize) -> Self {
        Self { vector, limit }
    }
}

/// One similarity search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorSearchHit {
    pub id: String,
    /// Cosine similarity in `[-1, 1]` (higher is closer).
    pub score: f32,
    pub metadata: Value,
}
