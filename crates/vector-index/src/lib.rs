//! Vector store abstraction.
//!
//! The [`VectorStore`] trait defines the minimal API every backend must
//! provide: [`VectorStore::upsert`], [`VectorStore::query`], and
//! [`VectorStore::delete`]. A reference in-memory implementation
//! ([`InMemoryVectorStore`]) is shipped for tests and small datasets.
//!
//! LanceDB-backed implementation lives in `mscode-vector-index-lancedb`.

mod error;
mod memory;
mod types;

pub use error::VectorError;
pub use memory::InMemoryVectorStore;
pub use types::{VectorRecord, VectorSearchHit, VectorSearchQuery};

/// Result alias for the vector index crate.
pub type Result<T> = std::result::Result<T, VectorError>;

/// Async trait every vector backend implements.
///
/// Implementations must be `Send + Sync` so the trait object can be shared
/// across tasks. `embed_dim` lets callers validate vector dimensions before
/// issuing writes.
#[async_trait::async_trait]
pub trait VectorStore: Send + Sync {
    /// Insert or replace a record by `id`.
    async fn upsert(&self, record: VectorRecord) -> Result<()>;

    /// Search the index for the nearest neighbors of `query.vector`.
    /// Returns at most `query.limit` hits ordered by similarity descending.
    async fn query(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchHit>>;

    /// Delete a record by id. Returns `true` if a record was removed.
    async fn delete(&self, id: &str) -> Result<bool>;

    /// Number of dimensions expected by this store (fixed at construction).
    fn embed_dim(&self) -> usize;
}
