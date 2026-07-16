//! LanceDB-backed [`VectorStore`] implementation.
//!
//! ## Status: stub
//!
//! This crate compiles cleanly and exposes the [`LanceDbVectorStore`] type
//! implementing [`mscode_vector_index::VectorStore`], but every operation
//! currently returns [`VectorError::Backend`] with a "not implemented" message.
//!
//! ## Why stubbed
//!
//! The `vectordb` Rust SDK pulls in the Arrow / DataFusion / Lance columnar
//! stack. In CI on Windows MSVC, native compilation of that stack is
//! currently flaky and adds significant binary size, which would push the
//! CLI above the sub-200ms cold-start budget. To stay within the workspace
//! hard constraints, the LanceDB integration is feature-gated for now.
//!
//! ## TODO before activating
//!
//! 1. Validate `vectordb = "0.4"` compiles on Windows MSVC in CI.
//! 2. Wire real schema (`id: string`, `vector: fixed-size list<f32>`,
//!    `metadata: utf8` JSON) and table creation against a `&Path` target.
//! 3. Implement [`VectorStore::upsert`], [`VectorStore::query`],
//!    [`VectorStore::delete`] against the LanceDB table API.
//! 4. Add integration tests under `tests/` using a tempdir target.
//!
//! The trait abstraction in `mscode-vector-index` is the load-bearing
//! deliverable for Phase 2; this crate can be activated later behind a
//! feature flag without breaking any caller.

mod store;

pub use store::LanceDbVectorStore;

// Re-export the error for callers' convenience.
pub use mscode_vector_index::VectorError;
