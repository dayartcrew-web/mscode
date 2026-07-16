//! Application state: SQLite connection pool + schema bootstrap.
//!
//! [`AppState`] is the central handle every domain store crate receives. It
//! owns an [`r2d2::Pool`] of [`rusqlite::Connection`]s backed by the
//! `bundled` SQLite build (no system SQLite dep). All operations are blocking
//! — there is no daemon, no async runtime required here.
//!
//! Two constructors cover both worlds:
//! - [`AppState::open`] opens (or creates) a file-backed database.
//! - [`AppState::in_memory`] creates an ephemeral database for tests.

mod error;
mod migrations;
mod pool;

pub use error::StateError;
pub use pool::AppState;

/// Result alias for the state crate.
pub type Result<T> = std::result::Result<T, StateError>;
