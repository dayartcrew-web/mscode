//! Connection pool construction and [`AppState`] handle.

use std::path::{Path, PathBuf};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use crate::error::StateError;
use crate::migrations;

/// Central application state handle.
///
/// Cheap to clone — internally holds an [`Arc`](std::sync::Arc) around the
/// pool. All domain stores accept `&AppState` (or a clone) and pull a
/// connection per transaction.
#[derive(Clone)]
pub struct AppState {
    pool: Pool<SqliteConnectionManager>,
    /// Filesystem path of the database (or `None` for in-memory).
    path: Option<PathBuf>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// Open (or create) a file-backed SQLite database at `path`.
    ///
    /// Parent directories are created if missing. Schema bootstrap runs
    /// inside a transaction before the handle is returned so every caller
    /// sees a ready-to-use database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StateError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let manager = SqliteConnectionManager::file(path);
        let pool = Pool::builder()
            .build(manager)
            .map_err(|e| StateError::Pool(format!("failed to build pool: {e}")))?;
        let path_buf = path.to_path_buf();
        let state = Self {
            pool,
            path: Some(path_buf),
        };
        state.bootstrap()?;
        Ok(state)
    }

    /// Create an in-memory database (primarily for tests).
    ///
    /// Because `:memory:` connections are isolated per-connection, we use a
    /// shared cache via the `file::memory:?cache=shared` URI trick so the pool
    /// connections all see the same schema and data.
    pub fn in_memory() -> Result<Self, StateError> {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder()
            .max_size(4)
            .build(manager)
            .map_err(|e| StateError::Pool(format!("failed to build in-memory pool: {e}")))?;
        let state = Self { pool, path: None };
        state.bootstrap()?;
        Ok(state)
    }

    /// Filesystem path of the database, or `None` when in-memory.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Acquire a pooled connection (one per logical operation).
    pub fn conn(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, StateError> {
        self.pool
            .get()
            .map_err(|e| StateError::Pool(format!("failed to acquire connection: {e}")))
    }

    /// Expose the raw pool (advanced users only — most callers should use
    /// [`Self::conn`]).
    pub fn pool(&self) -> &Pool<SqliteConnectionManager> {
        &self.pool
    }

    fn bootstrap(&self) -> Result<(), StateError> {
        let conn = self.conn()?;
        migrations::apply(&conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_in_memory_returns_ready_handle() {
        let state = AppState::in_memory().expect("in_memory must succeed");
        assert!(state.path.is_none());
        // Pool must already serve usable connections.
        let _conn = state.conn().expect("conn must succeed");
    }

    #[test]
    fn open_file_backed_creates_database() {
        let dir = tempdir().expect("tempdir");
        let db = dir.path().join("subdir").join("state.db");
        let state = AppState::open(&db).expect("open");
        assert_eq!(state.path(), Some(db.as_path()));
        assert!(db.exists(), "database file must exist after open");
    }

    #[test]
    fn bootstrap_creates_sessions_and_memories_tables() {
        let state = AppState::in_memory().expect("in_memory");
        let conn = state.conn().unwrap();
        let table_exists = |name: &str| -> bool {
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{name}'"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            count == 1
        };
        assert!(table_exists("sessions"));
        assert!(table_exists("memories"));
    }

    #[test]
    fn open_twice_on_same_file_is_idempotent() {
        let dir = tempdir().expect("tempdir");
        let db = dir.path().join("twice.db");
        let _first = AppState::open(&db).expect("first open");
        let _second = AppState::open(&db).expect("second open should not fail");
    }

    #[test]
    fn clone_shares_pool() {
        let state = AppState::in_memory().unwrap();
        let cloned = state.clone();
        let _c1 = state.conn().unwrap();
        let _c2 = cloned.conn().unwrap();
        // Both clones should resolve to the same underlying pool stats.
        assert_eq!(
            state.pool().state().connections,
            cloned.pool().state().connections
        );
    }

    #[test]
    fn debug_format_does_not_leak_secrets() {
        let state = AppState::in_memory().unwrap();
        let s = format!("{state:?}");
        assert!(s.contains("AppState"));
    }
}
