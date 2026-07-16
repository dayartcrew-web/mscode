//! 4-layer memory store: session → project → user → global.
//!
//! Memories are key/value rows tagged with a [`Scope`]. The store is
//! portable: memories can be queried by any combination of scope and key.
//!
//! Storage backend: `mscode-state` (rusqlite bundled).

mod error;
mod scope;
mod store;
mod types;

pub use error::MemoryError;
pub use scope::{Scope, project_root_hash};
pub use store::{MemoryQuery, MemoryStore};
pub use types::{Memory, NewMemory};

/// Result alias for the memories crate.
pub type Result<T> = std::result::Result<T, MemoryError>;
