//! The session state reducer.
//!
//! [`SessionState`] consumes [`SessionEvent`]s and accumulates an immutable
//! view via [`SessionState::snapshot`]. The state itself is mutated only via
//! [`SessionState::apply`]; the snapshot is a separate, owned, frozen value.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use mscode_protocol::{
    Checkpoint, MessageContent, MessageId, Role, SessionEndReason, SessionEvent, SessionId,
    SessionMessage,
};
use mscode_rollout::StateReducer;

use crate::error::{CoreError, Result};

/// Lifecycle phase of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionStatus {
    /// No `SessionStarted` event seen yet.
    #[default]
    New,
    /// Active and accepting events.
    Active,
    /// Paused; awaiting `SessionResumed`.
    Paused,
    /// Terminated; no further events expected.
    Ended,
}

/// Frozen, immutable snapshot of [`SessionState`] at a point in time.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    /// Session identifier, if known.
    pub id: Option<SessionId>,
    /// Lifecycle status.
    pub status: SessionStatus,
    /// Working directory, latest known.
    pub cwd: Option<PathBuf>,
    /// Project root, if recorded.
    pub project_root: Option<PathBuf>,
    /// Messages in arrival order.
    pub messages: Vec<SessionMessage>,
    /// Last checkpoint, if any.
    pub last_checkpoint: Option<Checkpoint>,
    /// Reason the session ended, if applicable.
    pub end_reason: Option<SessionEndReason>,
    /// Total event count applied.
    pub event_count: usize,
    /// When the session started, if known.
    pub started_at: Option<DateTime<Utc>>,
    /// When the session ended, if applicable.
    pub ended_at: Option<DateTime<Utc>>,
}

/// Mutable reducer that folds events into session state.
#[derive(Debug, Default)]
pub struct SessionState {
    id: Option<SessionId>,
    status: SessionStatus,
    cwd: Option<PathBuf>,
    project_root: Option<PathBuf>,
    messages: Vec<SessionMessage>,
    last_checkpoint: Option<Checkpoint>,
    end_reason: Option<SessionEndReason>,
    event_count: usize,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
}

impl SessionState {
    /// Build an empty reducer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a single event to the state.
    ///
    /// Returns an error for invalid event sequences (e.g. attempting to start
    /// a session when one is already active).
    pub fn apply(&mut self, event: &SessionEvent) -> Result<()> {
        match event {
            SessionEvent::SessionStarted {
                id,
                cwd,
                project_root,
                timestamp,
            } => {
                if self.status == SessionStatus::Active {
                    return Err(CoreError::SessionAlreadyStarted(format!(
                        "session {id} already active"
                    )));
                }
                self.id = Some(*id);
                self.status = SessionStatus::Active;
                self.cwd = Some(cwd.clone());
                self.project_root = project_root.clone();
                self.started_at = Some(*timestamp);
            }
            SessionEvent::MessageAdded {
                message_id,
                role,
                content,
                timestamp,
            } => {
                let msg =
                    SessionMessage::new(*message_id, role.clone(), content.clone(), *timestamp);
                self.messages.push(msg);
            }
            SessionEvent::ToolCallRequested { .. } | SessionEvent::ToolCallCompleted { .. } => {
                // Tool calls are recorded as messages by the caller via
                // MessageAdded; the call-lifecycle events themselves do not
                // mutate the transcript here.
            }
            SessionEvent::CheckpointCreated {
                checkpoint_id,
                label,
                timestamp,
            } => {
                self.last_checkpoint = Some(Checkpoint {
                    id: *checkpoint_id,
                    label: label.clone(),
                    timestamp: *timestamp,
                });
            }
            SessionEvent::SessionPaused { timestamp: _ } => {
                self.status = SessionStatus::Paused;
            }
            SessionEvent::SessionResumed { cwd, timestamp: _ } => {
                self.status = SessionStatus::Active;
                self.cwd = Some(cwd.clone());
            }
            SessionEvent::SessionEnded { reason, timestamp } => {
                self.status = SessionStatus::Ended;
                self.end_reason = Some(reason.clone());
                self.ended_at = Some(*timestamp);
            }
            SessionEvent::Unknown { .. } => {
                // Forward-compat events are counted but do not mutate state.
            }
        }
        self.event_count += 1;
        Ok(())
    }

    /// Produce an immutable snapshot of the current state.
    #[must_use]
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            id: self.id,
            status: self.status,
            cwd: self.cwd.clone(),
            project_root: self.project_root.clone(),
            messages: self.messages.clone(),
            last_checkpoint: self.last_checkpoint.clone(),
            end_reason: self.end_reason.clone(),
            event_count: self.event_count,
            started_at: self.started_at,
            ended_at: self.ended_at,
        }
    }

    /// Borrow the current messages slice.
    #[must_use]
    pub fn current_messages(&self) -> &[SessionMessage] {
        &self.messages
    }

    /// Borrow the project root, if known.
    #[must_use]
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// Borrow the current working directory, if known.
    #[must_use]
    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Borrow the last checkpoint, if any.
    #[must_use]
    pub fn last_checkpoint(&self) -> Option<&Checkpoint> {
        self.last_checkpoint.as_ref()
    }

    /// Total number of events applied.
    #[must_use]
    pub fn event_count(&self) -> usize {
        self.event_count
    }

    /// Borrow the session id, if known.
    #[must_use]
    pub fn id(&self) -> Option<SessionId> {
        self.id
    }

    /// Current lifecycle status.
    #[must_use]
    pub fn status(&self) -> SessionStatus {
        self.status
    }
}

impl StateReducer for SessionState {
    fn apply_event(&mut self, event: &SessionEvent) -> mscode_rollout::Result<()> {
        self.apply(event)
            .map_err(|e| mscode_rollout::RolloutError::Reducer(e.to_string()))
    }
}

/// Convenience helper for tests and call sites: build a user message event.
#[must_use]
pub fn user_message(id: MessageId, text: impl Into<String>, at: DateTime<Utc>) -> SessionEvent {
    SessionEvent::MessageAdded {
        message_id: id,
        role: Role::User,
        content: MessageContent::text(text),
        timestamp: at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use mscode_protocol::{CheckpointId, MessageId, SessionId, ToolCallId};
    use std::path::PathBuf;

    fn started() -> SessionEvent {
        SessionEvent::SessionStarted {
            id: SessionId::new(),
            cwd: PathBuf::from("/tmp"),
            project_root: Some(PathBuf::from("/tmp")),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn apply_session_started_populates_fields() {
        let mut s = SessionState::new();
        s.apply(&started()).expect("apply");
        assert_eq!(s.status(), SessionStatus::Active);
        assert!(s.id().is_some());
        assert_eq!(s.cwd(), Some(Path::new("/tmp")));
        assert_eq!(s.project_root(), Some(Path::new("/tmp")));
        assert_eq!(s.event_count(), 1);
    }

    #[test]
    fn apply_message_added_appends_to_messages() {
        let mut s = SessionState::new();
        s.apply(&started()).expect("apply");
        let msg = user_message(MessageId::new(), "hello", Utc::now());
        s.apply(&msg).expect("apply msg");
        assert_eq!(s.current_messages().len(), 1);
        assert_eq!(
            s.current_messages()[0].content,
            MessageContent::text("hello")
        );
    }

    #[test]
    fn apply_checkpoint_updates_last_checkpoint() {
        let mut s = SessionState::new();
        s.apply(&started()).expect("apply");
        let cp = SessionEvent::CheckpointCreated {
            checkpoint_id: CheckpointId::new(),
            label: "first".into(),
            timestamp: Utc::now(),
        };
        s.apply(&cp).expect("apply cp");
        let last = s.last_checkpoint().expect("checkpoint present");
        assert_eq!(last.label, "first");
    }

    #[test]
    fn apply_paused_and_resumed_transitions_status() {
        let mut s = SessionState::new();
        s.apply(&started()).expect("apply");
        s.apply(&SessionEvent::SessionPaused {
            timestamp: Utc::now(),
        })
        .expect("apply paused");
        assert_eq!(s.status(), SessionStatus::Paused);
        s.apply(&SessionEvent::SessionResumed {
            cwd: PathBuf::from("/new"),
            timestamp: Utc::now(),
        })
        .expect("apply resumed");
        assert_eq!(s.status(), SessionStatus::Active);
        assert_eq!(s.cwd(), Some(Path::new("/new")));
    }

    #[test]
    fn apply_ended_records_reason() {
        let mut s = SessionState::new();
        s.apply(&started()).expect("apply");
        s.apply(&SessionEvent::SessionEnded {
            reason: SessionEndReason::Stopped,
            timestamp: Utc::now(),
        })
        .expect("apply ended");
        assert_eq!(s.status(), SessionStatus::Ended);
        assert_eq!(s.snapshot().end_reason, Some(SessionEndReason::Stopped));
    }

    #[test]
    fn apply_tool_call_events_are_counted() {
        let mut s = SessionState::new();
        s.apply(&started()).expect("apply");
        s.apply(&SessionEvent::ToolCallRequested {
            call_id: ToolCallId::new(),
            tool: "shell".into(),
            args: serde_json::json!({}),
            timestamp: Utc::now(),
        })
        .expect("apply request");
        s.apply(&SessionEvent::ToolCallCompleted {
            call_id: ToolCallId::new(),
            output: serde_json::json!({}),
            timestamp: Utc::now(),
        })
        .expect("apply completed");
        assert_eq!(s.event_count(), 3);
    }

    #[test]
    fn snapshot_is_independent_of_state() {
        let mut s = SessionState::new();
        s.apply(&started()).expect("apply");
        let snap = s.snapshot();
        let before = snap.messages.len();
        s.apply(&user_message(MessageId::new(), "more", Utc::now()))
            .expect("apply");
        assert_eq!(snap.messages.len(), before, "snapshot is frozen");
    }

    #[test]
    fn double_start_returns_error() {
        let mut s = SessionState::new();
        s.apply(&started()).expect("apply 1");
        let err = s.apply(&started()).unwrap_err();
        assert!(matches!(err, CoreError::SessionAlreadyStarted(_)));
    }

    #[test]
    fn unknown_event_is_counted_silently() {
        let mut s = SessionState::new();
        s.apply(&SessionEvent::Unknown {
            payload: serde_json::json!({"type": "future"}),
        })
        .expect("apply unknown");
        assert_eq!(s.event_count(), 1);
    }

    #[test]
    fn state_reducer_impl_delegates_to_apply() {
        let mut s = SessionState::new();
        let ev = started();
        s.apply_event(&ev).expect("reducer");
        assert_eq!(s.event_count(), 1);
    }
}
