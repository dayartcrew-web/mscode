//! Schema bootstrap and migrations.
//!
//! Each entry in [`SCHEMA`] is a single SQL statement executed in order inside
//! a transaction. Statements must be idempotent (use `CREATE TABLE IF NOT
//! EXISTS`). When a statement needs to evolve, append a new entry rather than
//! editing an existing one — that preserves forward compatibility with
//! databases created by older binaries.

/// Ordered list of schema statements applied at every [`crate::AppState`]
/// construction.
pub const SCHEMA: &[&str] = &[
    // sessions — managed by mscode-thread-store, declared here so the bootstrap
    // is single-source-of-truth for every domain crate.
    "CREATE TABLE IF NOT EXISTS sessions (\
        id TEXT PRIMARY KEY,\
        cwd TEXT NOT NULL,\
        project_root TEXT,\
        created_at TEXT NOT NULL,\
        updated_at TEXT NOT NULL,\
        summary TEXT\
     )",
    // sessions index on cwd (soft filter for `list`).
    "CREATE INDEX IF NOT EXISTS idx_sessions_cwd ON sessions(cwd)",
    // memories — managed by mscode-memories.
    "CREATE TABLE IF NOT EXISTS memories (\
        id TEXT PRIMARY KEY,\
        scope TEXT NOT NULL,\
        key TEXT NOT NULL,\
        value TEXT NOT NULL,\
        embedding BLOB,\
        created_at TEXT NOT NULL,\
        accessed_at TEXT NOT NULL,\
        access_count INTEGER NOT NULL DEFAULT 0\
     )",
    // memories composite index on scope+key (lookup hot path).
    "CREATE INDEX IF NOT EXISTS idx_memories_scope_key ON memories(scope, key)",
    // schema version tracking (forward-compat for future ALTER migrations).
    "CREATE TABLE IF NOT EXISTS schema_version (\
        version INTEGER PRIMARY KEY,\
        applied_at TEXT NOT NULL\
     )",
    // provider_accounts — managed by mscode-credentials. Stores metadata only;
    // secret bytes live in the OS keyring keyed by `key_id`. `key_id` is
    // immutable per account so label renames don't require re-storing secrets.
    "CREATE TABLE IF NOT EXISTS provider_accounts (\
        id TEXT PRIMARY KEY,\
        provider TEXT NOT NULL,\
        label TEXT NOT NULL,\
        endpoint TEXT NOT NULL,\
        key_id TEXT NOT NULL,\
        is_default INTEGER NOT NULL DEFAULT 0 CHECK(is_default IN (0,1)),\
        status TEXT NOT NULL DEFAULT 'active',\
        cooldown_until TEXT,\
        last_used_at TEXT,\
        metadata TEXT NOT NULL DEFAULT '{}',\
        created_at TEXT NOT NULL,\
        UNIQUE(provider, label),\
        UNIQUE(key_id)\
     )",
    // one default per provider enforced by partial unique index.
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_provider_accounts_one_default \
        ON provider_accounts(provider) WHERE is_default = 1",
];

/// Apply every statement in [`SCHEMA`] inside a single transaction.
pub fn apply(conn: &rusqlite::Connection) -> Result<(), crate::StateError> {
    for stmt in SCHEMA {
        conn.execute_batch(stmt).map_err(|e| {
            crate::StateError::Migration(format!("failed applying statement: {e}; sql={stmt}"))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        rusqlite::Connection::open_in_memory().expect("open in-memory")
    }

    #[test]
    fn apply_creates_sessions_table() {
        let conn = mem_conn();
        apply(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn apply_creates_memories_table() {
        let conn = mem_conn();
        apply(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn apply_is_idempotent() {
        let conn = mem_conn();
        apply(&conn).expect("first apply");
        apply(&conn).expect("second apply must not error");
    }

    #[test]
    fn apply_records_schema_version_table() {
        let conn = mem_conn();
        apply(&conn).unwrap();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }
}
