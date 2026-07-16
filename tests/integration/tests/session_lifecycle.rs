//! Test 1: session lifecycle — create, turn, persist, resume.
//!
//! Exercises the full lifecycle:
//! 1. Create a new session via `mscode-thread-store`.
//! 2. Open an `Orchestrator` against that session.
//! 3. Emit 3 events (UserMessage, AssistantMessage, ToolCall) via
//!    `orchestrator.emit(...)`.
//! 4. Flush the orchestrator.
//! 5. Drop the orchestrator.
//! 6. Re-open the orchestrator by session id (resume).
//! 7. Verify all 3 events are present in the replayed state.
//! 8. Verify `SessionStatus` is correct.

use chrono::Utc;
use mscode_core::{Orchestrator, SessionStatus};
use mscode_protocol::{MessageContent, MessageId, Role, SessionEvent, SessionId, ToolCallId};
use mscode_state::AppState;
use mscode_thread_store::{NewSession, SessionStore};
use tempfile::tempdir;

#[test]
fn session_lifecycle_create_turn_persist_resume() {
    let data_dir = tempdir().expect("tempdir for data_dir");
    let state = AppState::in_memory().expect("in_memory state");
    let store = SessionStore::new(state.clone());

    let session_id = SessionId::new();
    let session_id_str = session_id.to_string();
    let cwd = data_dir.path().to_path_buf();

    // Step 1: Create the session in thread-store.
    store
        .create(NewSession {
            id: session_id_str.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            project_root: Some(cwd.to_string_lossy().into_owned()),
            created_at: None,
            summary: Some("integration-test".into()),
        })
        .expect("create session row");

    // Step 2 + 3: Open the orchestrator and emit 3 events.
    {
        let mut orch =
            Orchestrator::open(&state, data_dir.path(), session_id).expect("open orchestrator");
        let now = Utc::now();
        orch.emit(&SessionEvent::SessionStarted {
            id: session_id,
            cwd: cwd.clone(),
            project_root: Some(cwd.clone()),
            timestamp: now,
        })
        .expect("emit started");

        // UserMessage
        orch.emit(&SessionEvent::MessageAdded {
            message_id: MessageId::new(),
            role: Role::User,
            content: MessageContent::text("hello, can you help?"),
            timestamp: now,
        })
        .expect("emit user msg");

        // AssistantMessage
        orch.emit(&SessionEvent::MessageAdded {
            message_id: MessageId::new(),
            role: Role::Assistant,
            content: MessageContent::text("of course!"),
            timestamp: now,
        })
        .expect("emit assistant msg");

        // ToolCall (Requested only — sufficient to verify lifecycle replay)
        orch.emit(&SessionEvent::ToolCallRequested {
            call_id: ToolCallId::new(),
            tool: "read_file".into(),
            args: serde_json::json!({"path": "/tmp/probe"}),
            timestamp: now,
        })
        .expect("emit tool call");

        // Step 4: Flush
        orch.flush().expect("flush");
        // Step 5: Drop — explicit for readability.
        drop(orch);
    }

    // Step 6: Re-open by session id (resume).
    let resumed = Orchestrator::open(&state, data_dir.path(), session_id).expect("resume");

    // Step 7 + 8: Verify replayed state.
    let snap = resumed.snapshot();
    assert_eq!(
        snap.status,
        SessionStatus::Active,
        "session should be Active after SessionStarted + replay"
    );
    assert_eq!(
        snap.event_count, 4,
        "expected 4 events (started + 2 messages + 1 tool call), got {}",
        snap.event_count
    );
    assert_eq!(
        snap.messages.len(),
        2,
        "expected 2 messages in transcript, got {}",
        snap.messages.len()
    );
    assert_eq!(snap.messages[0].role, Role::User);
    assert_eq!(snap.messages[1].role, Role::Assistant);

    // cwd is portable-by-id: the resumed session keeps the original cwd.
    assert_eq!(snap.cwd, Some(cwd.clone()));
    // project_root travels with the session, not the cwd.
    assert_eq!(snap.project_root, Some(cwd));
}
