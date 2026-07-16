//! [`MockEmbedder`] — deterministic embeddings for tests.

use crate::error::EmbedError;
use crate::{Embedder, Result};

/// Deterministic embedder: maps each text to a stable vector of `dim` floats.
///
/// The vector is derived from a simple hash of the text content — it has no
/// semantic meaning but is stable across runs, which is all tests need.
pub struct MockEmbedder {
    dim: usize,
}

impl MockEmbedder {
    /// Build a mock embedder producing vectors of length `dim`.
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(1) }
    }
}

impl Default for MockEmbedder {
    fn default() -> Self {
        Self::new(384)
    }
}

impl Embedder for MockEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            return Err(EmbedError::EmptyInput);
        }
        let hash = fnv1a(text);
        Ok((0..self.dim)
            .map(|i| {
                // Mix the per-index counter into the hash for variety across
                // dimensions, then map the low 32 bits into [-1, 1].
                let mixed = hash
                    .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                    .wrapping_add((i as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f));
                let low = (mixed & 0xFFFF_FFFF) as u32;
                let normalized = (low as f64) / (u32::MAX as f64); // [0, 1]
                (normalized * 2.0 - 1.0) as f32 // [-1, 1]
            })
            .collect())
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(t)?);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

/// FNV-1a 64-bit hash (deterministic, no extra deps).
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_is_deterministic() {
        let m = MockEmbedder::new(16);
        let a = m.embed("hello").unwrap();
        let b = m.embed("hello").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn embed_different_texts_differ() {
        let m = MockEmbedder::new(16);
        let a = m.embed("hello").unwrap();
        let b = m.embed("world").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn embed_dim_matches_constructor() {
        let m = MockEmbedder::new(64);
        assert_eq!(m.dim(), 64);
        assert_eq!(m.embed("x").unwrap().len(), 64);
    }

    #[test]
    fn embed_rejects_empty() {
        let m = MockEmbedder::new(8);
        assert!(matches!(m.embed("").unwrap_err(), EmbedError::EmptyInput));
    }

    #[test]
    fn embed_batch_preserves_order_and_count() {
        let m = MockEmbedder::new(4);
        let out = m.embed_batch(&["a", "b", "c"]).unwrap();
        assert_eq!(out.len(), 3);
        for v in &out {
            assert_eq!(v.len(), 4);
        }
    }

    #[test]
    fn embed_batch_rejects_any_empty() {
        let m = MockEmbedder::new(4);
        let err = m.embed_batch(&["ok", ""]).unwrap_err();
        assert!(matches!(err, EmbedError::EmptyInput));
    }

    #[test]
    fn default_dim_is_384() {
        let m = MockEmbedder::default();
        assert_eq!(m.dim(), 384);
    }

    #[test]
    fn embed_values_in_minus_one_to_one() {
        let m = MockEmbedder::new(32);
        for v in m.embed("some text").unwrap() {
            assert!((-1.0..=1.0).contains(&v), "value out of range: {v}");
        }
    }
}
