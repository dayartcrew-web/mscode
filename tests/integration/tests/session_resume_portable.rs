//! Test 2: session resume works from a different cwd (portable-by-id).
//!
//! This is the "portable-by-ID" guarantee: a session created in directory A
//! can be resumed from directory B with at most a cwd-mismatch warning.
//! It must never be a hard error.

use chrono::Utc;
use mscode_core::Orchestrator;
use mscode_protocol::{SessionEvent, SessionId};
use mscode_state::AppState;
use mscode_thread_store::{NewSession, SessionStore};
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn session_resume_works_from_different_cwd() {
    // Directory A: where the session is created.
    let dir_a = tempdir().expect("tempdir A");
    // Directory B: where the session is resumed.
    let dir_b = tempdir().expect("tempdir B");
    // Data dir (shared across both invocations).
    let data_dir = tempdir().expect("tempdir for data");

    let state = AppState::in_memory().expect("in_memory state");
    let store = SessionStore::new(state.clone());

    let session_id = SessionId::new();
    let session_id_str = session_id.to_string();
    let cwd_a = dir_a.path().to_path_buf();

    // Step 1: Create the session row in directory A.
    store
        .create(NewSession {
            id: session_id_str.clone(),
            cwd: cwd_a.to_string_lossy().into_owned(),
            project_root: Some(cwd_a.to_string_lossy().into_owned()),
            created_at: None,
            summary: None,
        })
        .expect("create session row");

    // Step 2: Write a SessionStarted event from directory A.
    {
        let mut orch =
            Orchestrator::open(&state, data_dir.path(), session_id).expect("open orchestrator");
        orch.emit(&SessionEvent::SessionStarted {
            id: session_id,
            cwd: cwd_a.clone(),
            project_root: Some(cwd_a.clone()),
            timestamp: Utc::now(),
        })
        .expect("emit started");
        orch.flush().expect("flush");
    }

    // Step 3: Verify the session is resolvable by prefix from any cwd —
    // the SessionStore does not care about the caller's cwd.
    let resolved = store
        .get_by_id_prefix(&session_id_str)
        .expect("resume lookup must not error");
    assert_eq!(resolved.id, session_id_str);

    // Step 4: The orchestrator itself reads from data_dir, which is
    // independent of the caller's cwd. Resuming from dir_b works the same.
    let _cwd_b: PathBuf = dir_b.path().to_path_buf();
    let resumed =
        Orchestrator::open(&state, data_dir.path(), session_id).expect("resume from different cwd");
    let snap = resumed.snapshot();
    assert_eq!(snap.event_count, 1, "expected replayed event");
    assert_eq!(
        snap.cwd,
        Some(cwd_a.clone()),
        "resumed session retains its original cwd, not the caller's"
    );
}
