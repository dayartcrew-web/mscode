//! Public data types for memories.

use serde::{Deserialize, Serialize};

use crate::scope::Scope;

/// One memory row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub scope: Scope,
    pub key: String,
    pub value: String,
    /// Optional raw embedding bytes (big-endian f32 sequence).
    pub embedding: Option<Vec<u8>>,
    pub created_at: String,
    pub accessed_at: String,
    pub access_count: i64,
}

/// Input for creating a memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewMemory {
    pub id: String,
    pub scope: Scope,
    pub key: String,
    pub value: String,
    pub embedding: Option<Vec<u8>>,
    pub created_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_serde_roundtrip() {
        let m = Memory {
            id: "1".into(),
            scope: Scope::Global,
            key: "k".into(),
            value: "v".into(),
            embedding: Some(vec![0u8; 4]),
            created_at: "2024-01-01T00:00:00Z".into(),
            accessed_at: "2024-01-02T00:00:00Z".into(),
            access_count: 3,
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Memory = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
