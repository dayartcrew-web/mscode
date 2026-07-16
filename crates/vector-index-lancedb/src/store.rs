//! LanceDB-backed vector store stub. See crate docs for status and TODO.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use mscode_vector_index::{
    VectorError, VectorRecord, VectorSearchHit, VectorSearchQuery, VectorStore,
};

/// LanceDB-backed vector store. **Stubbed**: all operations return an error
/// indicating the integration is not yet wired up.
///
/// Construction is cheap and does not touch the filesystem; the struct simply
/// records the target path + dimension for use once the real integration
/// arrives.
pub struct LanceDbVectorStore {
    path: PathBuf,
    embed_dim: usize,
}

impl LanceDbVectorStore {
    /// Build a store targeting a LanceDB directory at `path`. Dimension
    /// is the fixed length of every vector this store will accept.
    pub fn new(path: impl AsRef<Path>, embed_dim: usize) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            embed_dim,
        }
    }

    /// Configured LanceDB target path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for LanceDbVectorStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanceDbVectorStore")
            .field("path", &self.path)
            .field("embed_dim", &self.embed_dim)
            .finish()
    }
}

#[async_trait]
impl VectorStore for LanceDbVectorStore {
    async fn upsert(&self, _record: VectorRecord) -> Result<(), VectorError> {
        Err(VectorError::Backend(
            "LanceDbVectorStore::upsert not implemented (see crate docs)".into(),
        ))
    }

    async fn query(&self, _query: VectorSearchQuery) -> Result<Vec<VectorSearchHit>, VectorError> {
        Err(VectorError::Backend(
            "LanceDbVectorStore::query not implemented (see crate docs)".into(),
        ))
    }

    async fn delete(&self, _id: &str) -> Result<bool, VectorError> {
        Err(VectorError::Backend(
            "LanceDbVectorStore::delete not implemented (see crate docs)".into(),
        ))
    }

    fn embed_dim(&self) -> usize {
        self.embed_dim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_records_path_and_dim() {
        let s = LanceDbVectorStore::new("/tmp/lance.db", 384);
        assert_eq!(s.path(), Path::new("/tmp/lance.db"));
        assert_eq!(s.embed_dim(), 384);
    }

    #[test]
    fn debug_format_does_not_panic() {
        let s = LanceDbVectorStore::new("/tmp/x", 8);
        let out = format!("{s:?}");
        assert!(out.contains("LanceDbVectorStore"));
        assert!(out.contains("8"));
    }

    #[tokio::test]
    async fn upsert_returns_backend_error() {
        let s = LanceDbVectorStore::new("/tmp/x", 4);
        let err = s
            .upsert(VectorRecord {
                id: "a".into(),
                vector: vec![0.0; 4],
                metadata: serde_json::Value::Null,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, VectorError::Backend(_)));
    }

    #[tokio::test]
    async fn query_returns_backend_error() {
        let s = LanceDbVectorStore::new("/tmp/x", 4);
        let err = s
            .query(VectorSearchQuery::new(vec![0.0; 4], 1))
            .await
            .unwrap_err();
        assert!(matches!(err, VectorError::Backend(_)));
    }

    #[tokio::test]
    async fn delete_returns_backend_error() {
        let s = LanceDbVectorStore::new("/tmp/x", 4);
        let err = s.delete("a").await.unwrap_err();
        assert!(matches!(err, VectorError::Backend(_)));
    }
}
