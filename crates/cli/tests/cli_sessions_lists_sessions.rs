//! `mscode sessions` lists sessions with cwd-soft-filter; `--all` disables filter.

use std::process::Command;

use tempfile::tempdir;

fn mscode_bin() -> Command {
    let path = std::env::var("CARGO_BIN_EXE_mscode")
        .expect("CARGO_BIN_EXE_mscode must be set by the cargo test harness");
    Command::new(path)
}

#[test]
fn cli_sessions_exits_zero_with_empty_output_when_no_sessions() {
    let home = tempdir().expect("tempdir");
    let output = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("sessions")
        .output()
        .expect("spawn");

    assert!(
        output.status.success(),
        "mscode sessions should succeed on empty store, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "expected empty stdout, got: {stdout}"
    );
}

#[test]
fn cli_sessions_lists_created_session_after_new() {
    let home = tempdir().expect("tempdir");

    // Create a session first.
    let created = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("new")
        .output()
        .expect("spawn new");
    assert!(created.status.success());
    let session_id = String::from_utf8_lossy(&created.stdout).trim().to_string();
    assert!(!session_id.is_empty());

    // List — should contain the id we just created (cwd filter passes because
    // both invocations share the same CWD).
    let listed = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("sessions")
        .output()
        .expect("spawn sessions");
    assert!(listed.status.success());
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        stdout.contains(&session_id),
        "expected sessions list to include `{session_id}`, got: {stdout}"
    );
}

#[test]
fn cli_sessions_all_does_not_require_matching_cwd() {
    let home = tempdir().expect("tempdir");

    // Create one session.
    let created = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("new")
        .output()
        .expect("spawn new");
    assert!(created.status.success());

    // `sessions --all` should still list it.
    let listed = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("sessions")
        .arg("--all")
        .output()
        .expect("spawn sessions --all");
    assert!(listed.status.success());
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "--all should still surface created sessions"
    );
}
