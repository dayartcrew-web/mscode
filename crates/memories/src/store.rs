//! [`MemoryStore`] — CRUD + scoped queries for the `memories` table.

use std::sync::Arc;

use mscode_state::AppState;
use rusqlite::params;

use crate::error::{MemoryError, Result};
use crate::scope::Scope;
use crate::types::{Memory, NewMemory};

/// Filter for querying memories. All fields are optional AND-combined.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryQuery {
    /// Exact scope to match (e.g. `Scope::User`).
    pub scope: Option<Scope>,
    /// Exact key to match.
    pub key: Option<String>,
    /// Limit number of rows returned.
    pub limit: Option<u32>,
}

/// Storage facade for the `memories` table.
#[derive(Clone)]
pub struct MemoryStore {
    state: Arc<AppState>,
}

impl MemoryStore {
    /// Build a store backed by the given [`AppState`].
    pub fn new(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }

    /// Reference to the underlying [`AppState`].
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Persist a new memory. Returns the stored row.
    pub fn create(&self, input: NewMemory) -> Result<Memory> {
        if input.id.trim().is_empty() {
            return Err(MemoryError::InvalidInput("empty id".into()));
        }
        if input.key.trim().is_empty() {
            return Err(MemoryError::InvalidInput("empty key".into()));
        }
        let now = now_iso();
        let created_at = input.created_at.unwrap_or_else(|| now.clone());
        let scope_tag = input.scope.to_tag();
        let conn = self.state.conn()?;
        conn.execute(
            "INSERT INTO memories (id, scope, key, value, embedding, created_at, accessed_at, access_count) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
            params![
                input.id,
                scope_tag,
                input.key,
                input.value,
                input.embedding,
                created_at,
                created_at,
            ],
        )?;
        self.get_by_id(&input.id)?
            .ok_or_else(|| MemoryError::NotFound(input.id))
    }

    /// Fetch a memory by ID. Returns `None` when missing.
    pub fn get_by_id(&self, id: &str) -> Result<Option<Memory>> {
        if id.trim().is_empty() {
            return Err(MemoryError::InvalidInput("empty id".into()));
        }
        let conn = self.state.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, scope, key, value, embedding, created_at, accessed_at, access_count \
             FROM memories WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], map_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Touch a memory: bump `access_count` and refresh `accessed_at`.
    pub fn touch(&self, id: &str) -> Result<Memory> {
        if id.trim().is_empty() {
            return Err(MemoryError::InvalidInput("empty id".into()));
        }
        let now = now_iso();
        let conn = self.state.conn()?;
        let changed = conn.execute(
            "UPDATE memories SET access_count = access_count + 1, accessed_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        if changed == 0 {
            return Err(MemoryError::NotFound(id.into()));
        }
        self.get_by_id(id)?
            .ok_or_else(|| MemoryError::NotFound(id.into()))
    }

    /// Update the value (and optionally embedding) of a memory.
    pub fn update_value(&self, id: &str, value: &str, embedding: Option<&[u8]>) -> Result<Memory> {
        if id.trim().is_empty() {
            return Err(MemoryError::InvalidInput("empty id".into()));
        }
        let now = now_iso();
        let conn = self.state.conn()?;
        let changed = conn.execute(
            "UPDATE memories SET value = ?1, embedding = ?2, accessed_at = ?3 WHERE id = ?4",
            params![value, embedding, now, id],
        )?;
        if changed == 0 {
            return Err(MemoryError::NotFound(id.into()));
        }
        self.get_by_id(id)?
            .ok_or_else(|| MemoryError::NotFound(id.into()))
    }

    /// Query memories by scope and/or key.
    pub fn query(&self, q: &MemoryQuery) -> Result<Vec<Memory>> {
        let mut sql = String::from(
            "SELECT id, scope, key, value, embedding, created_at, accessed_at, access_count FROM memories",
        );
        let mut clauses: Vec<&'static str> = Vec::new();
        let mut bind_scope: Option<String> = None;
        let mut bind_key: Option<String> = None;
        if let Some(scope) = &q.scope {
            clauses.push("scope = ?");
            bind_scope = Some(scope.to_tag());
        }
        if let Some(key) = &q.key {
            clauses.push("key = ?");
            bind_key = Some(key.clone());
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        let limit = q.limit.unwrap_or(1_000).min(10_000);
        sql.push_str(" ORDER BY created_at ASC LIMIT ?");
        let conn = self.state.conn()?;
        let mut stmt = conn.prepare(&sql)?;
        let rows = match (bind_scope, bind_key) {
            (None, None) => stmt.query_map(params![limit], map_row)?,
            (Some(s), None) => stmt.query_map(params![s, limit], map_row)?,
            (None, Some(k)) => stmt.query_map(params![k, limit], map_row)?,
            (Some(s), Some(k)) => stmt.query_map(params![s, k, limit], map_row)?,
        }
        .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete a memory by ID. Returns `true` if a row was removed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        if id.trim().is_empty() {
            return Err(MemoryError::InvalidInput("empty id".into()));
        }
        let conn = self.state.conn()?;
        let changed = conn.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let id: String = row.get(0)?;
    let scope_tag: String = row.get(1)?;
    let scope = Scope::from_tag(&scope_tag)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    Ok(Memory {
        id,
        scope,
        key: row.get(2)?,
        value: row.get(3)?,
        embedding: row.get(4)?,
        created_at: row.get(5)?,
        accessed_at: row.get(6)?,
        access_count: row.get(7)?,
    })
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = epoch_to_ymd_hms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn epoch_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64 + 719_468;
    let time = (secs % 86_400) as u32;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (y + if m <= 2 { 1 } else { 0 }) as u32;
    (year, m, d, time / 3600, (time % 3600) / 60, time % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{Scope, project_root_hash};
    use crate::types::NewMemory;

    fn fixture() -> MemoryStore {
        let state = AppState::in_memory().unwrap();
        MemoryStore::new(state)
    }

    fn new_mem(id: &str, scope: Scope, key: &str, value: &str) -> NewMemory {
        NewMemory {
            id: id.into(),
            scope,
            key: key.into(),
            value: value.into(),
            embedding: None,
            created_at: None,
        }
    }

    #[test]
    fn create_and_get_by_id_roundtrip() {
        let store = fixture();
        let created = store
            .create(new_mem("m1", Scope::Global, "k", "v"))
            .unwrap();
        assert_eq!(created.id, "m1");
        assert_eq!(created.access_count, 0);
        let got = store.get_by_id("m1").unwrap().unwrap();
        assert_eq!(created, got);
    }

    #[test]
    fn create_rejects_empty_id() {
        let store = fixture();
        let err = store
            .create(new_mem("  ", Scope::Global, "k", "v"))
            .unwrap_err();
        assert!(matches!(err, MemoryError::InvalidInput(_)));
    }

    #[test]
    fn create_rejects_empty_key() {
        let store = fixture();
        let err = store
            .create(new_mem("id", Scope::Global, "  ", "v"))
            .unwrap_err();
        assert!(matches!(err, MemoryError::InvalidInput(_)));
    }

    #[test]
    fn touch_increments_access_count() {
        let store = fixture();
        store.create(new_mem("m", Scope::User, "k", "v")).unwrap();
        let touched = store.touch("m").unwrap();
        assert_eq!(touched.access_count, 1);
        let again = store.touch("m").unwrap();
        assert_eq!(again.access_count, 2);
    }

    #[test]
    fn touch_missing_errors() {
        let store = fixture();
        let err = store.touch("ghost").unwrap_err();
        assert!(matches!(err, MemoryError::NotFound(_)));
    }

    #[test]
    fn update_value_roundtrip() {
        let store = fixture();
        store
            .create(new_mem("m", Scope::Global, "k", "v1"))
            .unwrap();
        let updated = store.update_value("m", "v2", Some(&[1, 2, 3, 4])).unwrap();
        assert_eq!(updated.value, "v2");
        assert_eq!(updated.embedding.as_deref(), Some(&[1u8, 2, 3, 4][..]));
    }

    #[test]
    fn query_filters_by_scope() {
        let store = fixture();
        store
            .create(new_mem("g1", Scope::Global, "k", "v"))
            .unwrap();
        store.create(new_mem("u1", Scope::User, "k", "v")).unwrap();
        let q = MemoryQuery {
            scope: Some(Scope::User),
            key: None,
            limit: None,
        };
        let rows = store.query(&q).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "u1");
    }

    #[test]
    fn query_filters_by_key() {
        let store = fixture();
        store
            .create(new_mem("g1", Scope::Global, "k1", "v"))
            .unwrap();
        store
            .create(new_mem("g2", Scope::Global, "k2", "v"))
            .unwrap();
        let q = MemoryQuery {
            scope: None,
            key: Some("k2".into()),
            limit: None,
        };
        let rows = store.query(&q).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "g2");
    }

    #[test]
    fn query_combines_scope_and_key() {
        let store = fixture();
        let hash = project_root_hash("/work/proj");
        store
            .create(new_mem("p1", Scope::Project(hash.clone()), "k1", "v"))
            .unwrap();
        store
            .create(new_mem("p2", Scope::Project(hash.clone()), "k2", "v"))
            .unwrap();
        let q = MemoryQuery {
            scope: Some(Scope::Project(hash)),
            key: Some("k2".into()),
            limit: None,
        };
        let rows = store.query(&q).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "p2");
    }

    #[test]
    fn query_applies_limit() {
        let store = fixture();
        for i in 0..5 {
            store
                .create(new_mem(&format!("m{i}"), Scope::Global, "k", "v"))
                .unwrap();
        }
        let q = MemoryQuery {
            scope: None,
            key: None,
            limit: Some(2),
        };
        let rows = store.query(&q).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn delete_removes_memory() {
        let store = fixture();
        store.create(new_mem("m", Scope::Global, "k", "v")).unwrap();
        assert!(store.delete("m").unwrap());
        assert!(store.get_by_id("m").unwrap().is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = fixture();
        assert!(!store.delete("ghost").unwrap());
    }

    #[test]
    fn session_scope_round_trip() {
        let store = fixture();
        store
            .create(new_mem("s1", Scope::Session("sess-1".into()), "k", "v"))
            .unwrap();
        let q = MemoryQuery {
            scope: Some(Scope::Session("sess-1".into())),
            key: None,
            limit: None,
        };
        let rows = store.query(&q).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn get_by_id_missing_returns_none() {
        let store = fixture();
        assert!(store.get_by_id("ghost").unwrap().is_none());
    }

    #[test]
    fn embedding_persists_and_reads_back() {
        let store = fixture();
        let mut nm = new_mem("e1", Scope::Global, "k", "v");
        nm.embedding = Some(vec![0u8, 1, 2, 3, 4, 5, 6, 7]);
        store.create(nm).unwrap();
        let got = store.get_by_id("e1").unwrap().unwrap();
        assert_eq!(
            got.embedding.as_deref(),
            Some(&[0u8, 1, 2, 3, 4, 5, 6, 7][..])
        );
    }
}
