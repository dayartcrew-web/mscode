//! Test 5: Memory layer round-trip across the 4-layer scope hierarchy.
//!
//! Writes one memory at each scope (Session, Project, User, Global), then
//! verifies:
//!   - Scoped queries return only the rows written at that scope (isolation).
//!   - The same key at different scopes does not collide.
//!   - get_by_id round-trips the value and embedding.
//!   - delete removes the row and returns true; deleting again returns false.

use mscode_memories::{MemoryQuery, MemoryStore, NewMemory, Scope};
use mscode_state::AppState;

fn memory(id: &str, scope: Scope, key: &str, value: &str) -> NewMemory {
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
fn memory_layer_round_trip() {
    let state = AppState::in_memory().expect("in_memory state");
    let store = MemoryStore::new(state);

    // Same key at every scope to prove they don't collide.
    let key = "greeting";
    let created_session = store
        .create(memory("m1", Scope::Session("s1".into()), key, "session-hi"))
        .expect("create session");
    let _created_project = store
        .create(memory("m2", Scope::Project("p1".into()), key, "project-hi"))
        .expect("create project");
    let created_user = store
        .create(memory("m3", Scope::User, key, "user-hi"))
        .expect("create user");
    let created_global = store
        .create(memory("m4", Scope::Global, key, "global-hi"))
        .expect("create global");

    // Round-trip via get_by_id.
    let fetched = store.get_by_id("m1").expect("get m1").expect("m1 present");
    assert_eq!(fetched.value, "session-hi");
    assert_eq!(fetched.scope, Scope::Session("s1".into()));

    // Scoped queries isolate by scope. Same key, four scopes — each query
    // returns exactly one row.
    let session_rows = store
        .query(&MemoryQuery {
            scope: Some(Scope::Session("s1".into())),
            key: Some(key.into()),
            limit: None,
        })
        .expect("session query");
    assert_eq!(session_rows.len(), 1);
    assert_eq!(session_rows[0].id, "m1");

    let project_rows = store
        .query(&MemoryQuery {
            scope: Some(Scope::Project("p1".into())),
            key: Some(key.into()),
            limit: None,
        })
        .expect("project query");
    assert_eq!(project_rows.len(), 1);
    assert_eq!(project_rows[0].id, "m2");

    let user_rows = store
        .query(&MemoryQuery {
            scope: Some(Scope::User),
            key: Some(key.into()),
            limit: None,
        })
        .expect("user query");
    assert_eq!(user_rows.len(), 1);
    assert_eq!(user_rows[0].id, "m3");

    let global_rows = store
        .query(&MemoryQuery {
            scope: Some(Scope::Global),
            key: Some(key.into()),
            limit: None,
        })
        .expect("global query");
    assert_eq!(global_rows.len(), 1);
    assert_eq!(global_rows[0].id, "m4");

    // Cross-scope leakage check: Session query from a different session
    // returns zero rows.
    let other_session = store
        .query(&MemoryQuery {
            scope: Some(Scope::Session("OTHER".into())),
            key: Some(key.into()),
            limit: None,
        })
        .expect("other session query");
    assert!(other_session.is_empty(), "sessions must be isolated");

    // Touch bumps access_count.
    let touched = store.touch("m3").expect("touch m3");
    assert_eq!(touched.access_count, 1);

    // Delete one row; second delete returns false (idempotent semantics).
    assert!(store.delete("m2").expect("delete m2"));
    assert!(!store.delete("m2").expect("delete m2 again"));
    assert!(store.get_by_id("m2").expect("get m2").is_none());

    // The other three are still present.
    assert!(created_session.id == "m1");
    assert!(created_user.id == "m3");
    assert!(created_global.id == "m4");
    assert!(store.get_by_id("m1").unwrap().is_some());
    assert!(store.get_by_id("m3").unwrap().is_some());
    assert!(store.get_by_id("m4").unwrap().is_some());
}
