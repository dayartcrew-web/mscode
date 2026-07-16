//! [`InMemoryVectorStore`] — reference implementation for tests / small data.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::VectorStore;
use crate::error::{Result, VectorError};
use crate::types::{VectorRecord, VectorSearchHit, VectorSearchQuery};

/// Thread-safe in-memory vector store. Records are keyed by `id`.
pub struct InMemoryVectorStore {
    embed_dim: usize,
    inner: RwLock<HashMap<String, (Vec<f32>, Value)>>,
}

impl InMemoryVectorStore {
    /// Build a new store expecting `embed_dim`-dimensional vectors.
    pub fn new(embed_dim: usize) -> Self {
        Self {
            embed_dim,
            inner: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl VectorStore for InMemoryVectorStore {
    async fn upsert(&self, record: VectorRecord) -> Result<()> {
        if record.id.trim().is_empty() {
            return Err(VectorError::InvalidId("empty id".into()));
        }
        if record.vector.len() != self.embed_dim {
            return Err(VectorError::DimensionMismatch {
                expected: self.embed_dim,
                actual: record.vector.len(),
            });
        }
        let mut guard = self.inner.write().await;
        guard.insert(record.id, (record.vector, record.metadata));
        Ok(())
    }

    async fn query(&self, query: VectorSearchQuery) -> Result<Vec<VectorSearchHit>> {
        if query.vector.len() != self.embed_dim {
            return Err(VectorError::DimensionMismatch {
                expected: self.embed_dim,
                actual: query.vector.len(),
            });
        }
        let guard = self.inner.read().await;
        let mut hits: Vec<VectorSearchHit> = guard
            .iter()
            .map(|(id, (vec, meta))| VectorSearchHit {
                id: id.clone(),
                score: cosine(&query.vector, vec),
                metadata: meta.clone(),
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(query.limit.max(1));
        Ok(hits)
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        if id.trim().is_empty() {
            return Err(VectorError::InvalidId("empty id".into()));
        }
        let mut guard = self.inner.write().await;
        Ok(guard.remove(id).is_some())
    }

    fn embed_dim(&self) -> usize {
        self.embed_dim
    }
}

/// Cosine similarity. Returns 0.0 when either vector is empty.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(id: &str, vector: Vec<f32>) -> VectorRecord {
        VectorRecord {
            id: id.into(),
            vector,
            metadata: json!({"tag": id}),
        }
    }

    #[tokio::test]
    async fn upsert_then_query_returns_record() {
        let store = InMemoryVectorStore::new(3);
        store.upsert(rec("a", vec![1.0, 0.0, 0.0])).await.unwrap();
        let hits = store
            .query(VectorSearchQuery::new(vec![1.0, 0.0, 0.0], 10))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
        assert!((hits[0].score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn upsert_rejects_dimension_mismatch() {
        let store = InMemoryVectorStore::new(3);
        let err = store.upsert(rec("a", vec![1.0])).await.unwrap_err();
        assert!(matches!(err, VectorError::DimensionMismatch { .. }));
    }

    #[tokio::test]
    async fn upsert_rejects_empty_id() {
        let store = InMemoryVectorStore::new(2);
        let err = store.upsert(rec("  ", vec![0.0, 0.0])).await.unwrap_err();
        assert!(matches!(err, VectorError::InvalidId(_)));
    }

    #[tokio::test]
    async fn query_rejects_dimension_mismatch() {
        let store = InMemoryVectorStore::new(3);
        let err = store
            .query(VectorSearchQuery::new(vec![1.0], 5))
            .await
            .unwrap_err();
        assert!(matches!(err, VectorError::DimensionMismatch { .. }));
    }

    #[tokio::test]
    async fn query_orders_by_similarity_descending() {
        let store = InMemoryVectorStore::new(2);
        store.upsert(rec("a", vec![1.0, 0.0])).await.unwrap();
        store.upsert(rec("b", vec![0.0, 1.0])).await.unwrap();
        let hits = store
            .query(VectorSearchQuery::new(vec![0.9, 0.1], 10))
            .await
            .unwrap();
        assert_eq!(hits[0].id, "a");
        assert_eq!(hits[1].id, "b");
    }

    #[tokio::test]
    async fn query_respects_limit() {
        let store = InMemoryVectorStore::new(2);
        store.upsert(rec("a", vec![1.0, 0.0])).await.unwrap();
        store.upsert(rec("b", vec![0.0, 1.0])).await.unwrap();
        let hits = store
            .query(VectorSearchQuery::new(vec![1.0, 0.0], 1))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn delete_removes_record() {
        let store = InMemoryVectorStore::new(2);
        store.upsert(rec("a", vec![1.0, 0.0])).await.unwrap();
        assert!(store.delete("a").await.unwrap());
        let hits = store
            .query(VectorSearchQuery::new(vec![1.0, 0.0], 5))
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn delete_missing_returns_false() {
        let store = InMemoryVectorStore::new(2);
        assert!(!store.delete("ghost").await.unwrap());
    }

    #[tokio::test]
    async fn delete_rejects_empty_id() {
        let store = InMemoryVectorStore::new(2);
        let err = store.delete("").await.unwrap_err();
        assert!(matches!(err, VectorError::InvalidId(_)));
    }

    #[tokio::test]
    async fn upsert_overwrites_existing_id() {
        let store = InMemoryVectorStore::new(2);
        store.upsert(rec("a", vec![1.0, 0.0])).await.unwrap();
        store.upsert(rec("a", vec![0.0, 1.0])).await.unwrap();
        let hits = store
            .query(VectorSearchQuery::new(vec![0.0, 1.0], 5))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
    }

    #[test]
    fn cosine_identical_vectors_is_one() {
        let s = cosine(&[1.0, 0.0, 0.0], &[1.0, 0.0, 0.0]);
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_vectors_is_zero() {
        let s = cosine(&[1.0, 0.0], &[0.0, 1.0]);
        assert!(s.abs() < 1e-6);
    }

    #[test]
    fn cosine_handles_zero_vector_safely() {
        let s = cosine(&[0.0, 0.0], &[1.0, 0.0]);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn cosine_different_lengths_returns_zero() {
        let s = cosine(&[1.0], &[1.0, 0.0]);
        assert_eq!(s, 0.0);
    }

    #[tokio::test]
    async fn embed_dim_reports_configured_value() {
        let store = InMemoryVectorStore::new(384);
        assert_eq!(store.embed_dim(), 384);
    }
}
