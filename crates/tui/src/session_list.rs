//! Session picker model for the `/sessions` slash command.
//!
//! [`SessionList`] is a **frontend** view-model over session rows. It is
//! intentionally storage-agnostic: callers populate it from a
//! [`SessionLookup`] implementation (which the real `SessionStore` satisfies,
//! and which tests can mock).
//!
//! The list is filtered by `cwd` by default — the hard constraint that
//! `/sessions works from any cwd` means sessions are portable by ID (so the
//! user can always `/resume <id>` from anywhere), but the `/sessions` listing
//! defaults to "sessions you started in this directory" to stay useful.

use mscode_thread_store::{ListSessionsFilter, Session, SessionStore};

/// One row in the session picker (a lightweight projection of [`Session`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: String,
    pub cwd: String,
    pub summary: Option<String>,
    pub updated_at: String,
}

impl From<Session> for SessionEntry {
    fn from(s: Session) -> Self {
        Self {
            id: s.id,
            cwd: s.cwd,
            summary: s.summary,
            updated_at: s.updated_at,
        }
    }
}

/// Storage-side interface the picker depends on.
///
/// The real `SessionStore` satisfies this; tests can substitute their own.
pub trait SessionLookup {
    /// List sessions according to `filter`. Most-recent first.
    fn list(&self, filter: &ListSessionsFilter) -> Result<Vec<Session>, String>;
}

/// The real `SessionStore` is the canonical implementation.
impl SessionLookup for SessionStore {
    fn list(&self, filter: &ListSessionsFilter) -> Result<Vec<Session>, String> {
        SessionStore::list(self, filter).map_err(|e| e.to_string())
    }
}

/// Filterable, navigable picker over sessions.
#[derive(Debug, Clone)]
pub struct SessionList {
    entries: Vec<SessionEntry>,
    /// Active highlight (0-based index into `entries`). `None` = nothing
    /// highlighted.
    cursor: Option<usize>,
    /// Whether the cwd filter is currently applied.
    cwd_filter_active: bool,
}

impl Default for SessionList {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            cursor: None,
            cwd_filter_active: true,
        }
    }
}

impl SessionList {
    /// Build a new empty picker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the picker's contents.
    pub fn set_entries(&mut self, entries: Vec<SessionEntry>) {
        self.entries = entries;
        // Reset cursor to the first row if any, else clear it.
        self.cursor = if self.entries.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    /// Read-only view of the current entries.
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// Active cursor index, if any.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// `true` if the cwd filter is currently applied.
    pub fn cwd_filter_active(&self) -> bool {
        self.cwd_filter_active
    }

    /// Toggle the cwd filter on/off in-place.
    pub fn toggle_cwd_filter(&mut self) {
        self.cwd_filter_active = !self.cwd_filter_active;
    }

    /// Load sessions from `store`, applying the current filter state.
    ///
    /// `current_cwd` is the directory used by the soft filter when
    /// `cwd_filter_active == true`.
    pub fn load(
        &mut self,
        store: &dyn SessionLookup,
        current_cwd: &str,
        limit: Option<u32>,
    ) -> Result<(), String> {
        let filter = ListSessionsFilter {
            cwd: self.cwd_filter_active.then(|| current_cwd.to_string()),
            limit,
        };
        let rows = store.list(&filter)?;
        let entries = rows.into_iter().map(SessionEntry::from).collect();
        self.set_entries(entries);
        Ok(())
    }

    /// Move cursor up (toward index 0). No-op at the top.
    pub fn move_up(&mut self) {
        if let Some(i) = self.cursor {
            if i > 0 {
                self.cursor = Some(i - 1);
            }
        }
    }

    /// Move cursor down (toward the end). No-op at the bottom.
    pub fn move_down(&mut self) {
        if let Some(i) = self.cursor {
            if i + 1 < self.entries.len() {
                self.cursor = Some(i + 1);
            }
        }
    }

    /// Return the highlighted entry, if any.
    pub fn selected(&self) -> Option<&SessionEntry> {
        self.cursor.and_then(|i| self.entries.get(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mscode_state::AppState;
    use mscode_thread_store::{NewSession, SessionStore};

    fn fixture_store() -> SessionStore {
        let state = AppState::in_memory().expect("in_memory");
        SessionStore::new(state)
    }

    fn new_session(id: &str, cwd: &str) -> NewSession {
        NewSession {
            id: id.into(),
            cwd: cwd.into(),
            project_root: None,
            created_at: None,
            summary: None,
        }
    }

    #[test]
    fn session_list_filters_by_cwd_by_default() {
        let store = fixture_store();
        store.create(new_session("a", "/work")).unwrap();
        store.create(new_session("b", "/other")).unwrap();

        let mut list = SessionList::new();
        list.load(&store, "/work", None).unwrap();
        let ids: Vec<&str> = list.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["a"]);
    }

    #[test]
    fn session_list_includes_all_with_flag() {
        let store = fixture_store();
        store.create(new_session("a", "/work")).unwrap();
        store.create(new_session("b", "/other")).unwrap();

        let mut list = SessionList::new();
        // Toggle filter off — equivalent to `--all`.
        list.toggle_cwd_filter();
        assert!(!list.cwd_filter_active());
        list.load(&store, "/work", None).unwrap();
        let ids: Vec<&str> = list.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn cursor_defaults_to_first_row_when_loaded() {
        let store = fixture_store();
        store.create(new_session("a", "/work")).unwrap();
        store.create(new_session("b", "/work")).unwrap();

        let mut list = SessionList::new();
        list.load(&store, "/work", None).unwrap();
        assert_eq!(list.cursor(), Some(0));
        assert_eq!(list.selected().map(|e| e.id.as_str()), Some("a"));
    }

    #[test]
    fn move_down_advances_cursor() {
        let store = fixture_store();
        store.create(new_session("a", "/work")).unwrap();
        store.create(new_session("b", "/work")).unwrap();
        store.create(new_session("c", "/work")).unwrap();

        let mut list = SessionList::new();
        list.load(&store, "/work", None).unwrap();
        list.move_down();
        assert_eq!(list.cursor(), Some(1));
        list.move_down();
        assert_eq!(list.cursor(), Some(2));
        // At bottom: stays put.
        list.move_down();
        assert_eq!(list.cursor(), Some(2));
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let store = fixture_store();
        store.create(new_session("a", "/work")).unwrap();
        store.create(new_session("b", "/work")).unwrap();

        let mut list = SessionList::new();
        list.load(&store, "/work", None).unwrap();
        list.move_down();
        list.move_up();
        assert_eq!(list.cursor(), Some(0));
        // At top: stays put.
        list.move_up();
        assert_eq!(list.cursor(), Some(0));
    }

    #[test]
    fn load_with_empty_store_clears_cursor() {
        let store = fixture_store();
        let mut list = SessionList::new();
        list.load(&store, "/work", None).unwrap();
        assert!(list.entries().is_empty());
        assert!(list.cursor().is_none());
        assert!(list.selected().is_none());
    }

    #[test]
    fn load_surfaces_store_errors() {
        struct AlwaysFails;
        impl SessionLookup for AlwaysFails {
            fn list(&self, _f: &ListSessionsFilter) -> Result<Vec<Session>, String> {
                Err("store down".into())
            }
        }
        let mut list = SessionList::new();
        let err = list.load(&AlwaysFails, "/work", None).unwrap_err();
        assert!(err.contains("store down"));
    }

    #[test]
    fn session_entry_from_session_projects_fields() {
        let s = Session {
            id: "abc".into(),
            cwd: "/x".into(),
            project_root: None,
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-02T00:00:00Z".into(),
            summary: Some("hi".into()),
        };
        let e = SessionEntry::from(s);
        assert_eq!(e.id, "abc");
        assert_eq!(e.cwd, "/x");
        assert_eq!(e.summary.as_deref(), Some("hi"));
        assert_eq!(e.updated_at, "2024-01-02T00:00:00Z");
    }
}
