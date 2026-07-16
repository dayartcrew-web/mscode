//! `mscode resume <unknown-id>` exits non-zero with a helpful error.

use std::process::Command;

use tempfile::tempdir;

fn mscode_bin() -> Command {
    let path = std::env::var("CARGO_BIN_EXE_mscode")
        .expect("CARGO_BIN_EXE_mscode must be set by the cargo test harness");
    Command::new(path)
}

#[test]
fn cli_resume_unknown_id_exits_nonzero_with_helpful_error() {
    let home = tempdir().expect("tempdir");
    let output = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("resume")
        .arg("deadbeef")
        .output()
        .expect("spawn resume");

    let code = output.status.code().unwrap_or(-1);
    assert_ne!(code, 0, "expected non-zero exit for unknown id, got {code}");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("deadbeef"),
        "expected error to echo the unknown id; stderr={stderr}"
    );
    assert!(
        stderr.to_lowercase().contains("not found") || stderr.to_lowercase().contains("could not"),
        "expected helpful error message; stderr={stderr}"
    );
}

#[test]
fn cli_resume_full_id_round_trips_after_new() {
    let home = tempdir().expect("tempdir");

    // Create a session, then resume its full id. We don't have a TTY in tests,
    // so the binary should resolve the id and exit cleanly without launching
    // the TUI.
    let created = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("new")
        .output()
        .expect("spawn new");
    assert!(created.status.success());
    let id = String::from_utf8_lossy(&created.stdout).trim().to_string();

    let resumed = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("resume")
        .arg(&id)
        .output()
        .expect("spawn resume");
    assert!(
        resumed.status.success(),
        "resume with a valid id should succeed (non-TTY fast-path); stderr={}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let stdout = String::from_utf8_lossy(&resumed.stdout);
    assert_eq!(stdout.trim(), id);
}

#[test]
fn cli_resume_short_prefix_round_trips_after_new() {
    let home = tempdir().expect("tempdir");

    let created = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("new")
        .output()
        .expect("spawn new");
    assert!(created.status.success());
    let id = String::from_utf8_lossy(&created.stdout).trim().to_string();
    let prefix = &id[..8]; // first 8 chars is unambiguous

    let resumed = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("resume")
        .arg(prefix)
        .output()
        .expect("spawn resume prefix");
    assert!(
        resumed.status.success(),
        "resume with valid prefix should succeed; stderr={}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let stdout = String::from_utf8_lossy(&resumed.stdout);
    assert_eq!(stdout.trim(), id);
}
