//! Session orchestrator: ties rollout + state together.
//!
//! [`Orchestrator::open`] resolves the rollout file for a session, replays
//! any existing events into a fresh [`SessionState`], and returns a ready
//! handle. Subsequent [`Orchestrator::emit`] calls append-and-apply in a
//! single step so the on-disk log and the in-memory state never drift apart.

use std::path::{Path, PathBuf};

use mscode_protocol::{SessionEvent, SessionId};
use mscode_state::AppState;

use crate::error::Result;
use crate::state::{SessionSnapshot, SessionState};

/// Resolves the rollout file path for a given session id under a data dir.
///
/// Pure helper exposed for tests and external callers that want to inspect
/// the path without opening an orchestrator.
#[must_use]
pub fn session_log_path(data_dir: &Path, session_id: SessionId) -> PathBuf {
    data_dir
        .join("sessions")
        .join(format!("{session_id}.jsonl"))
}

/// Ties together the rollout writer and the in-memory state reducer for one
/// session.
pub struct Orchestrator {
    state: SessionState,
    writer: mscode_rollout::RolloutWriter,
}

impl Orchestrator {
    /// Open or resume a session.
    ///
    /// If the session log already exists, events are replayed into a fresh
    /// [`SessionState`] before the handle is returned. If the log does not
    /// exist, the orchestrator is created in `New` state — callers are
    /// expected to emit a [`SessionEvent::SessionStarted`] next.
    ///
    /// The `AppState` is accepted for symmetry with other domain stores but
    /// is not used directly by the orchestrator in this phase — it is
    /// reserved for future integration (e.g. indexing session events into
    /// the memory store).
    pub fn open(_state: &AppState, data_dir: &Path, session_id: SessionId) -> Result<Self> {
        let path = session_log_path(data_dir, session_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Replay any existing events before opening the writer for append.
        let mut state = SessionState::new();
        if path.exists() {
            let reader = mscode_rollout::RolloutReader::open(&path)?;
            reader.replay(&mut state)?;
        }
        let writer = mscode_rollout::RolloutWriter::create(&path)?;
        Ok(Self { state, writer })
    }

    /// Append an event to the rollout and immediately apply it to the
    /// in-memory state. If either step fails, the other is left untouched —
    /// the writer flushes eagerly so durable order matches apply order.
    pub fn emit(&mut self, event: &SessionEvent) -> Result<()> {
        self.writer.append(event)?;
        // Apply after the durable append succeeds. If apply fails the on-disk
        // log will contain the event but the in-memory state will not; the
        // next process restart will replay the event (and presumably fail
        // the same way), so divergence cannot accumulate silently.
        self.state.apply(event)?;
        Ok(())
    }

    /// Build an immutable snapshot of the current state.
    #[must_use]
    pub fn snapshot(&self) -> SessionSnapshot {
        self.state.snapshot()
    }

    /// Borrow the underlying state reducer.
    #[must_use]
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// Borrow the underlying writer (e.g. to force a flush).
    pub fn writer(&mut self) -> &mut mscode_rollout::RolloutWriter {
        &mut self.writer
    }

    /// Force a flush + fsync on the underlying rollout file.
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Returns the path of the rollout file backing this orchestrator.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.writer.path()
    }
}

impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("state", &self.state)
            .field("writer", &self.writer)
            .finish()
    }
}

// Note: we intentionally do NOT auto-emit SessionStarted here. Callers decide
// whether they are creating a new session or resuming an existing one; the
// orchestrator merely enforces event ordering via SessionState::apply.

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use mscode_protocol::{MessageContent, MessageId, Role, SessionEvent};
    use mscode_state::AppState;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn app_state() -> AppState {
        AppState::in_memory().expect("in-memory state")
    }

    fn started(id: SessionId) -> SessionEvent {
        SessionEvent::SessionStarted {
            id,
            cwd: PathBuf::from("/tmp"),
            project_root: Some(PathBuf::from("/tmp")),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn open_new_session_starts_in_new_status() {
        let dir = tempdir().expect("tempdir");
        let state = app_state();
        let orch = Orchestrator::open(&state, dir.path(), SessionId::new()).expect("open");
        assert_eq!(orch.snapshot().status, crate::state::SessionStatus::New);
        assert_eq!(orch.snapshot().event_count, 0);
    }

    #[test]
    fn emit_records_event_to_disk_and_state() {
        let dir = tempdir().expect("tempdir");
        let state = app_state();
        let id = SessionId::new();
        let mut orch = Orchestrator::open(&state, dir.path(), id).expect("open");
        orch.emit(&started(id)).expect("emit start");
        let snap = orch.snapshot();
        assert_eq!(snap.status, crate::state::SessionStatus::Active);
        assert_eq!(snap.event_count, 1);
        assert!(orch.path().exists());
    }

    #[test]
    fn open_resumes_existing_session_via_replay() {
        let dir = tempdir().expect("tempdir");
        let state = app_state();
        let id = SessionId::new();
        {
            let mut orch = Orchestrator::open(&state, dir.path(), id).expect("open");
            orch.emit(&started(id)).expect("start");
            orch.emit(&SessionEvent::MessageAdded {
                message_id: MessageId::new(),
                role: Role::User,
                content: MessageContent::text("hi"),
                timestamp: Utc::now(),
            })
            .expect("emit msg");
        }
        // Reopen — replay should restore both events.
        let orch = Orchestrator::open(&state, dir.path(), id).expect("reopen");
        assert_eq!(orch.snapshot().event_count, 2);
        assert_eq!(orch.snapshot().messages.len(), 1);
        assert_eq!(orch.snapshot().status, crate::state::SessionStatus::Active);
    }

    #[test]
    fn emit_preserves_order_across_restarts() {
        let dir = tempdir().expect("tempdir");
        let state = app_state();
        let id = SessionId::new();
        let ids = (MessageId::new(), MessageId::new(), MessageId::new());
        {
            let mut orch = Orchestrator::open(&state, dir.path(), id).expect("open");
            orch.emit(&started(id)).expect("start");
            orch.emit(&SessionEvent::MessageAdded {
                message_id: ids.0,
                role: Role::User,
                content: MessageContent::text("first"),
                timestamp: Utc::now(),
            })
            .expect("first");
        }
        {
            let mut orch = Orchestrator::open(&state, dir.path(), id).expect("reopen 1");
            orch.emit(&SessionEvent::MessageAdded {
                message_id: ids.1,
                role: Role::Assistant,
                content: MessageContent::text("second"),
                timestamp: Utc::now(),
            })
            .expect("second");
        }
        let orch = Orchestrator::open(&state, dir.path(), id).expect("reopen 2");
        let snap = orch.snapshot();
        assert_eq!(snap.messages.len(), 2);
        assert_eq!(snap.messages[0].id, ids.0);
        assert_eq!(snap.messages[1].id, ids.1);
    }

    #[test]
    fn flush_does_not_error_after_emit() {
        let dir = tempdir().expect("tempdir");
        let state = app_state();
        let id = SessionId::new();
        let mut orch = Orchestrator::open(&state, dir.path(), id).expect("open");
        orch.emit(&started(id)).expect("start");
        orch.flush().expect("flush");
    }

    #[test]
    fn session_log_path_is_under_sessions_subdir() {
        let id = SessionId::new();
        let p = session_log_path(Path::new("/data"), id);
        assert!(p.starts_with("/data/sessions"));
        assert!(p.to_string_lossy().ends_with(".jsonl"));
        assert!(p.to_string_lossy().contains(&id.as_uuid().to_string()));
    }

    #[test]
    fn debug_repr_does_not_panic() {
        let dir = tempdir().expect("tempdir");
        let state = app_state();
        let orch = Orchestrator::open(&state, dir.path(), SessionId::new()).expect("open");
        let _ = format!("{orch:?}");
    }

    #[test]
    fn emit_records_unknown_event_silently() {
        let dir = tempdir().expect("tempdir");
        let state = app_state();
        let id = SessionId::new();
        let mut orch = Orchestrator::open(&state, dir.path(), id).expect("open");
        orch.emit(&started(id)).expect("start");
        orch.emit(&SessionEvent::Unknown {
            payload: serde_json::json!({"type": "future"}),
        })
        .expect("emit unknown");
        assert_eq!(orch.snapshot().event_count, 2);
    }

    #[test]
    fn orchestrator_recovers_from_truncated_tail() {
        let dir = tempdir().expect("tempdir");
        let state = app_state();
        let id = SessionId::new();
        {
            let mut orch = Orchestrator::open(&state, dir.path(), id).expect("open");
            orch.emit(&started(id)).expect("start");
        }
        // Corrupt the file by appending junk without a trailing newline.
        let path = session_log_path(dir.path(), id);
        let mut existing = std::fs::read_to_string(&path).expect("read");
        existing.push_str("{\"type\":\"bogus\"");
        std::fs::write(&path, existing).expect("write junk");
        // Reopen should not panic; replay yields just the well-formed event.
        let orch = Orchestrator::open(&state, dir.path(), id).expect("open");
        assert_eq!(orch.snapshot().event_count, 1, "truncated tail recovered");
    }
}
