//! Session / thread storage on top of SQLite.
//!
//! [`Session`] represents one conversation thread. Sessions are
//! **portable-by-ID**: `cwd` is provided as a soft list filter, never as a
//! resume gate. A user can resume any session from any directory by full ID
//! or by an unambiguous ID prefix.
//!
//! Storage backend: `mscode-state` (rusqlite bundled).

mod error;
mod store;
mod types;

pub use error::ThreadStoreError;
pub use store::{ListSessionsFilter, SessionStore};
pub use types::{NewSession, Session};

/// Result alias for the thread store crate.
pub type Result<T> = std::result::Result<T, ThreadStoreError>;
