//! `mscode new` creates a session and prints a valid UUID to stdout.

use std::process::Command;

use tempfile::tempdir;

fn mscode_bin() -> Command {
    let path = std::env::var("CARGO_BIN_EXE_mscode")
        .expect("CARGO_BIN_EXE_mscode must be set by the cargo test harness");
    Command::new(path)
}

#[test]
fn cli_new_creates_session_and_prints_uuid() {
    let home = tempdir().expect("tempdir for MSCODE_HOME");
    let output = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("new")
        .output()
        .expect("spawn mscode new");

    assert!(
        output.status.success(),
        "mscode new did not exit cleanly: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    assert!(
        !trimmed.is_empty(),
        "expected a session id on stdout, got empty output"
    );

    // The id must round-trip through `uuid::Uuid::parse_str`. We don't import
    // uuid at the integration-test level; just sanity-check the shape.
    assert_eq!(
        trimmed.len(),
        36,
        "expected a 36-char UUID, got `{trimmed}`"
    );
    // UUID v4 format: 8-4-4-4-12 with hyphens.
    let groups: Vec<&str> = trimmed.split('-').collect();
    assert_eq!(groups.len(), 5, "expected 5 UUID groups, got {groups:?}");
    assert_eq!(groups[0].len(), 8);
    assert_eq!(groups[1].len(), 4);
    assert_eq!(groups[2].len(), 4);
    assert_eq!(groups[3].len(), 4);
    assert_eq!(groups[4].len(), 12);
}

#[test]
fn cli_new_creates_distinct_ids_across_invocations() {
    let home = tempdir().expect("tempdir");

    let first = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("new")
        .output()
        .expect("spawn");
    assert!(first.status.success());
    let first_id = String::from_utf8_lossy(&first.stdout).trim().to_string();

    let second = mscode_bin()
        .env("MSCODE_HOME", home.path())
        .arg("new")
        .output()
        .expect("spawn");
    assert!(second.status.success());
    let second_id = String::from_utf8_lossy(&second.stdout).trim().to_string();

    assert_ne!(first_id, second_id, "UUIDs must be unique");
}
