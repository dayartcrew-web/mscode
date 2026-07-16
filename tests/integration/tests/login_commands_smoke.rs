//! Integration smoke test: `mscode login list` against a fresh data dir.
//!
//! Verifies the credential-management subcommand wiring:
//!   - `mscode login list` exits 0 against an empty store.
//!   - `mscode login list --provider openai` exits 0 with the "no accounts"
//!     message.
//!
//! We do NOT exercise `login add` here because it requires interactive input
//! (rpassword prompt). That path is covered by unit tests for the prompt
//! helpers and by the SqliteCredentialStore integration tests.

use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> PathBuf {
    if let Ok(p) = std::env::var("MSCODE_BIN_PATH") {
        return PathBuf::from(p);
    }
    let exe = if cfg!(windows) {
        "mscode.exe"
    } else {
        "mscode"
    };
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest
            .join("..")
            .join("..")
            .join("target")
            .join("debug")
            .join(exe),
        manifest.join("..").join("target").join("debug").join(exe),
        manifest
            .join("..")
            .join("..")
            .join("target")
            .join("release")
            .join(exe),
    ];
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }
    panic!("could not locate mscode binary; tried: {:?}", candidates);
}

#[test]
fn login_list_empty_store_exits_zero() {
    let data = tempdir().expect("tempdir");
    let out = Command::new(bin())
        .args(["login", "list"])
        .env("MSCODE_HOME", data.path())
        .output()
        .expect("spawn login list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "login list should exit 0 on empty store; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("no accounts configured"),
        "expected empty-store hint in stdout: {stdout}"
    );
}

#[test]
fn login_list_filtered_by_provider_exits_zero() {
    let data = tempdir().expect("tempdir");
    let out = Command::new(bin())
        .args(["login", "list", "--provider", "openai"])
        .env("MSCODE_HOME", data.path())
        .output()
        .expect("spawn login list --provider");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "login list --provider should exit 0 on empty store; stdout={stdout}"
    );
    assert!(
        stdout.contains("no accounts configured for provider `openai`"),
        "expected provider-filtered empty hint: {stdout}"
    );
}

#[test]
fn login_help_lists_subcommands() {
    let out = Command::new(bin())
        .args(["login", "--help"])
        .output()
        .expect("spawn login --help");
    assert!(out.status.success(), "login --help must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for sub in ["add", "list", "remove", "use"] {
        assert!(
            stdout.contains(sub),
            "expected `{sub}` in login --help output: {stdout}"
        );
    }
}
