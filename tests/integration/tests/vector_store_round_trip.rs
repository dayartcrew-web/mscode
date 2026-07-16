//! Test 6: `InMemoryVectorStore` round-trip via the [`VectorStore`] trait.
//!
//! Verifies the trait object can be used polymorphically:
//!   1. Upsert 5 records.
//!   2. Query top-3 nearest neighbors.
//!   3. Hits are ordered by cosine similarity descending.
//!   4. Delete one record; subsequent queries no longer return it.
//!   5. Dimension mismatch is rejected as an error (never a panic).

use mscode_vector_index::{InMemoryVectorStore, VectorRecord, VectorSearchQuery, VectorStore};
use serde_json::json;

fn rec(id: &str, vector: Vec<f32>) -> VectorRecord {
    VectorRecord {
        id: id.into(),
        vector,
        metadata: json!({"tag": id}),
    }
}

#[tokio::test]
async fn vector_store_trait_in_memory_round_trip() {
    // Use the trait object explicitly so the test exercises dynamic dispatch.
    let store: Box<dyn VectorStore> = Box::new(InMemoryVectorStore::new(3));

    // 1. Upsert 5 records. The "closest" one is identical to the query.
    store.upsert(rec("a", vec![1.0, 0.0, 0.0])).await.unwrap();
    store.upsert(rec("b", vec![0.9, 0.1, 0.0])).await.unwrap();
    store.upsert(rec("c", vec![0.0, 1.0, 0.0])).await.unwrap();
    store.upsert(rec("d", vec![0.0, 0.0, 1.0])).await.unwrap();
    store.upsert(rec("e", vec![-1.0, 0.0, 0.0])).await.unwrap();

    // 2. Top-3 query.
    let hits = store
        .query(VectorSearchQuery::new(vec![1.0, 0.0, 0.0], 3))
        .await
        .expect("query");
    assert_eq!(hits.len(), 3, "expected 3 hits");

    // 3. Ordered by similarity descending.
    assert_eq!(hits[0].id, "a");
    assert!(
        (hits[0].score - 1.0).abs() < 1e-6,
        "exact match is score 1.0"
    );
    assert_eq!(hits[1].id, "b", "next closest is b");
    // Score is monotonically non-increasing.
    assert!(hits[0].score >= hits[1].score);
    assert!(hits[1].score >= hits[2].score);

    // 4. Delete one record.
    let deleted = store.delete("a").await.expect("delete");
    assert!(deleted, "delete of existing record should be true");

    // Re-query; 'a' must not appear.
    let hits2 = store
        .query(VectorSearchQuery::new(vec![1.0, 0.0, 0.0], 10))
        .await
        .expect("query after delete");
    assert!(
        !hits2.iter().any(|h| h.id == "a"),
        "deleted record must not appear in subsequent queries"
    );

    // Deleting a missing record returns false (not an error).
    let again = store.delete("a").await.expect("delete again");
    assert!(!again, "delete of missing record should be false");

    // 5. Dimension mismatch is a typed error, never a panic.
    let dim_err = store.upsert(rec("bad", vec![1.0])).await;
    assert!(dim_err.is_err(), "dimension mismatch must be rejected");

    // embed_dim is reported correctly through the trait object.
    assert_eq!(store.embed_dim(), 3);
}
