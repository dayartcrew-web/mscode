//! [`SessionStore`] — CRUD operations for sessions.

use std::sync::Arc;

use mscode_state::AppState;
use rusqlite::params;

use crate::error::{Result, ThreadStoreError};
use crate::types::{NewSession, Session};

/// Soft filter applied by [`SessionStore::list`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListSessionsFilter {
    /// When set, only return sessions with this `cwd` (exact match).
    pub cwd: Option<String>,
    /// Limit the number of rows returned (most-recent first).
    pub limit: Option<u32>,
}

/// Storage facade for the `sessions` table.
///
/// Cloning is cheap — internally an [`Arc`] around the [`AppState`].
#[derive(Clone)]
pub struct SessionStore {
    state: Arc<AppState>,
}

impl SessionStore {
    /// Build a store backed by the given [`AppState`].
    pub fn new(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }

    /// Build a store from a shared [`AppState`] handle.
    pub fn with_shared(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Reference to the underlying [`AppState`].
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Create a new session row. Returns the persisted [`Session`].
    pub fn create(&self, new_session: NewSession) -> Result<Session> {
        if new_session.id.trim().is_empty() {
            return Err(ThreadStoreError::InvalidId("empty id".into()));
        }
        let now = now_iso();
        let created_at = new_session.created_at.unwrap_or_else(|| now.clone());
        let updated_at = created_at.clone();
        let conn = self.state.conn()?;
        conn.execute(
            "INSERT INTO sessions (id, cwd, project_root, created_at, updated_at, summary) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                new_session.id,
                new_session.cwd,
                new_session.project_root,
                created_at,
                updated_at,
                new_session.summary,
            ],
        )?;
        let id = new_session.id;
        self.get_by_id(&id)?
            .ok_or_else(|| ThreadStoreError::NotFound(id))
    }

    /// List sessions with an optional soft filter. Returns most-recent first.
    pub fn list(&self, filter: &ListSessionsFilter) -> Result<Vec<Session>> {
        let conn = self.state.conn()?;
        let limit = filter.limit.unwrap_or(1000).min(10_000);
        let mut sql = String::from(
            "SELECT id, cwd, project_root, created_at, updated_at, summary FROM sessions",
        );
        let mut params_vec: Vec<String> = Vec::new();
        if filter.cwd.is_some() {
            sql.push_str(" WHERE cwd = ?");
            params_vec.push(filter.cwd.clone().unwrap_or_default());
        }
        sql.push_str(" ORDER BY updated_at DESC LIMIT ?");
        let mut stmt = conn.prepare(&sql)?;
        let rows = match params_vec.len() {
            0 => stmt.query_map(params![limit], map_row)?,
            _ => stmt.query_map(params![params_vec[0], limit], map_row)?,
        }
        .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Fetch a session by full ID. Returns `None` if missing.
    pub fn get_by_id(&self, id: &str) -> Result<Option<Session>> {
        if id.trim().is_empty() {
            return Err(ThreadStoreError::InvalidId("empty id".into()));
        }
        let conn = self.state.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, cwd, project_root, created_at, updated_at, summary FROM sessions \
             WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], map_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Resolve a session by full ID or unambiguous prefix.
    ///
    /// Resolution rules:
    /// - 0 matches -> [`ThreadStoreError::NotFound`]
    /// - exactly 1 match -> Ok
    /// - 2+ matches -> [`ThreadStoreError::AmbiguousPrefix`]
    pub fn get_by_id_prefix(&self, id_or_prefix: &str) -> Result<Session> {
        if id_or_prefix.trim().is_empty() {
            return Err(ThreadStoreError::InvalidId("empty id".into()));
        }
        if let Some(exact) = self.get_by_id(id_or_prefix)? {
            return Ok(exact);
        }
        let conn = self.state.conn()?;
        let pattern = format!("{id_or_prefix}%");
        let mut stmt = conn.prepare(
            "SELECT id, cwd, project_root, created_at, updated_at, summary FROM sessions \
             WHERE id LIKE ?1 ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![pattern], map_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        match rows.len() {
            0 => Err(ThreadStoreError::NotFound(id_or_prefix.into())),
            1 => Ok(rows.into_iter().next().expect("length 1")),
            n => Err(ThreadStoreError::AmbiguousPrefix(n)),
        }
    }

    /// Update a session's `summary` field and bump `updated_at`.
    pub fn update_summary(&self, id: &str, summary: &str) -> Result<Session> {
        if id.trim().is_empty() {
            return Err(ThreadStoreError::InvalidId("empty id".into()));
        }
        let now = now_iso();
        let conn = self.state.conn()?;
        let changed = conn.execute(
            "UPDATE sessions SET summary = ?1, updated_at = ?2 WHERE id = ?3",
            params![summary, now, id],
        )?;
        if changed == 0 {
            return Err(ThreadStoreError::NotFound(id.into()));
        }
        self.get_by_id(id)?
            .ok_or_else(|| ThreadStoreError::NotFound(id.into()))
    }

    /// Delete a session by ID. Returns `true` if a row was removed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        if id.trim().is_empty() {
            return Err(ThreadStoreError::InvalidId("empty id".into()));
        }
        let conn = self.state.conn()?;
        let changed = conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get(0)?,
        cwd: row.get(1)?,
        project_root: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        summary: row.get(5)?,
    })
}

/// Produce an ISO-8601 UTC timestamp using only stdlib. Precision: seconds.
fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = epoch_to_ymd_hms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert UNIX seconds to (Y, M, D, h, m, s) in UTC using a civil calendar
/// algorithm (Howard Hinnant, http howardhinnant.github.io/date_algorithms.html).
fn epoch_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64 + 719_468; // days since 0000-03-01
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
    use crate::types::NewSession;

    fn fixture() -> SessionStore {
        let state = AppState::in_memory().expect("in_memory");
        SessionStore::new(state)
    }

    fn ns(id: &str) -> NewSession {
        NewSession {
            id: id.into(),
            cwd: "/work".into(),
            project_root: Some("/work".into()),
            created_at: None,
            summary: None,
        }
    }

    #[test]
    fn create_and_get_by_id_roundtrip() {
        let store = fixture();
        let created = store.create(ns("abc123")).unwrap();
        assert_eq!(created.id, "abc123");
        assert!(created.created_at.contains('T'));
        let got = store.get_by_id("abc123").unwrap().unwrap();
        assert_eq!(created, got);
    }

    #[test]
    fn create_rejects_empty_id() {
        let store = fixture();
        let err = store.create(ns("   ")).unwrap_err();
        assert!(matches!(err, ThreadStoreError::InvalidId(_)));
    }

    #[test]
    fn get_by_id_returns_none_for_missing() {
        let store = fixture();
        assert!(store.get_by_id("nope").unwrap().is_none());
    }

    #[test]
    fn list_returns_all_when_no_filter() {
        let store = fixture();
        store.create(ns("a")).unwrap();
        store.create(ns("b")).unwrap();
        let rows = store.list(&ListSessionsFilter::default()).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn list_applies_cwd_soft_filter() {
        let store = fixture();
        let mut s = ns("a");
        s.cwd = "/x".into();
        store.create(s).unwrap();
        store.create(ns("b")).unwrap(); // cwd /work
        let f = ListSessionsFilter {
            cwd: Some("/x".into()),
            limit: None,
        };
        let rows = store.list(&f).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a");
    }

    #[test]
    fn list_applies_limit() {
        let store = fixture();
        for i in 0..5 {
            store.create(ns(&format!("s{i}"))).unwrap();
        }
        let f = ListSessionsFilter {
            cwd: None,
            limit: Some(2),
        };
        let rows = store.list(&f).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn prefix_resolves_unique() {
        let store = fixture();
        store.create(ns("deadbeef")).unwrap();
        let resolved = store.get_by_id_prefix("dead").unwrap();
        assert_eq!(resolved.id, "deadbeef");
    }

    #[test]
    fn prefix_full_id_still_works() {
        let store = fixture();
        store.create(ns("cafebabe")).unwrap();
        let resolved = store.get_by_id_prefix("cafebabe").unwrap();
        assert_eq!(resolved.id, "cafebabe");
    }

    #[test]
    fn prefix_ambiguous_errors() {
        let store = fixture();
        store.create(ns("abc-one")).unwrap();
        store.create(ns("abc-two")).unwrap();
        let err = store.get_by_id_prefix("abc").unwrap_err();
        assert!(matches!(err, ThreadStoreError::AmbiguousPrefix(2)));
    }

    #[test]
    fn prefix_missing_errors_not_found() {
        let store = fixture();
        let err = store.get_by_id_prefix("zzz").unwrap_err();
        assert!(matches!(err, ThreadStoreError::NotFound(_)));
    }

    #[test]
    fn update_summary_roundtrip() {
        let store = fixture();
        store.create(ns("s1")).unwrap();
        let updated = store.update_summary("s1", "my summary").unwrap();
        assert_eq!(updated.summary.as_deref(), Some("my summary"));
    }

    #[test]
    fn update_summary_missing_session_errors() {
        let store = fixture();
        let err = store.update_summary("ghost", "x").unwrap_err();
        assert!(matches!(err, ThreadStoreError::NotFound(_)));
    }

    #[test]
    fn delete_removes_session() {
        let store = fixture();
        store.create(ns("x")).unwrap();
        assert!(store.delete("x").unwrap());
        assert!(store.get_by_id("x").unwrap().is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = fixture();
        assert!(!store.delete("ghost").unwrap());
    }

    #[test]
    fn now_iso_is_utc_format() {
        let s = now_iso();
        assert!(s.ends_with('Z'));
        assert_eq!(s.len(), 20);
    }

    #[test]
    fn epoch_to_ymd_hms_known_value() {
        // 1970-01-01 00:00:00 UTC
        let (y, m, d, h, mi, s) = epoch_to_ymd_hms(0);
        assert_eq!((y, m, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_to_ymd_hms_known_leap() {
        // 2024-02-29 14:10:45 UTC = 1709215845
        let (y, m, d, h, mi, s) = epoch_to_ymd_hms(1_709_215_845);
        assert_eq!((y, m, d, h, mi, s), (2024, 2, 29, 14, 10, 45));
    }
}
