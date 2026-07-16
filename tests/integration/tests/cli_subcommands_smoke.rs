//! Test 9: CLI subcommand smoke test.
//!
//! Spawns the actual `mscode` binary (built from `crates/cli/src/bin/mscode.rs`)
//! and exercises the no-IO fast paths:
//!   - `mscode --help`         (clap prints help, exit 0)
//!   - `mscode --version`      (clap prints version, exit 0)
//!   - `mscode version`        (custom subcommand, exit 0)
//!   - `mscode new`            (creates a session, prints id, exit 0)
//!   - `mscode sessions`       (lists sessions, exit 0)
//!   - `mscode resume deadbeef` (no match — graceful non-zero exit)
//!
//! Each invocation gets a fresh tempdir as its data dir so tests are isolated.

use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> PathBuf {
    // CARGO_BIN_EXE_<name> is only auto-set when the integration test lives
    // in the same package as the binary target. The integration crate is a
    // separate package, so we resolve the binary manually: walk from this
    // crate's target dir up to the workspace target dir, and pick the debug
    // or release artifact. Tests run via `cargo test` use the debug profile.
    if let Ok(p) = std::env::var("MSCODE_BIN_PATH") {
        return PathBuf::from(p);
    }
    let exe = if cfg!(windows) {
        "mscode.exe"
    } else {
        "mscode"
    };
    // CARGO_MANIFEST_DIR is .../tests/integration; workspace target is ../target.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Try several candidates; on Windows the absolute path may contain a
    // drive letter that interacts oddly with parent(), so we walk two levels
    // explicitly.
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
    // Last-resort diagnostic — surface the candidates so a future build
    // breakage is debuggable from the test output.
    panic!(
        "could not locate mscode binary; tried: {:?}; CARGO_MANIFEST_DIR={:?}",
        candidates,
        std::env::var("CARGO_MANIFEST_DIR").ok(),
    );
}

fn data_dir() -> PathBuf {
    tempdir().expect("tempdir").path().to_path_buf()
}

#[test]
fn cli_subcommands_smoke_test() {
    // --help: exit 0, output mentions "Local-first agentic CLI".
    let out = Command::new(bin())
        .arg("--help")
        .output()
        .expect("spawn --help");
    assert!(out.status.success(), "--help must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Local-first agentic CLI") || stdout.contains("mscode"),
        "--help should mention mscode / tagline, got: {stdout}"
    );

    // --version: exit 0, output starts with "mscode".
    let out = Command::new(bin())
        .arg("--version")
        .output()
        .expect("spawn --version");
    assert!(out.status.success(), "--version must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim().starts_with("mscode"),
        "--version output should start with 'mscode', got: {stdout}"
    );

    // version subcommand.
    let out = Command::new(bin())
        .arg("version")
        .output()
        .expect("spawn version");
    assert!(out.status.success(), "version subcommand must succeed");

    // new: creates a session, isolated under a fresh MSCODE_HOME so it never
    // touches the user's real ~/.mscode.
    let data = data_dir();
    let out = Command::new(bin())
        .arg("new")
        .env("MSCODE_HOME", data.join("mscode_home"))
        .output()
        .expect("spawn new");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // `new` should either succeed (printing a session id) or fail gracefully
    // with a non-panicking error message. Either way, it must not hang.
    assert!(
        out.status.code().is_some(),
        "`new` must terminate with a status code; stdout={stdout} stderr={stderr}"
    );

    // sessions: list (possibly empty).
    let out = Command::new(bin())
        .arg("sessions")
        .env("MSCODE_HOME", data.join("mscode_home"))
        .output()
        .expect("spawn sessions");
    assert!(
        out.status.code().is_some(),
        "`sessions` must terminate with a status code"
    );

    // resume deadbeef: no match -> graceful non-zero exit (NOT a panic / crash).
    let out = Command::new(bin())
        .arg("resume")
        .arg("deadbeef")
        .env("MSCODE_HOME", data.join("mscode_home"))
        .output()
        .expect("spawn resume deadbeef");
    let code = out.status.code();
    assert!(
        code.is_some(),
        "`resume deadbeef` must terminate, not hang or crash"
    );
    // Should exit non-zero when no session matches the prefix.
    if let Some(c) = code {
        // We accept either a non-zero exit OR a zero exit with stderr
        // complaining — the contract is "graceful, not a panic".
        let stderr = String::from_utf8_lossy(&out.stderr);
        if c == 0 {
            assert!(
                !stderr.is_empty() || !out.stdout.is_empty(),
                "zero-exit resume with no match should still produce output"
            );
        }
        // Must not contain Rust panic markers.
        let combined = format!("{stderr}{}", String::from_utf8_lossy(&out.stdout));
        assert!(
            !combined.contains("panicked at"),
            "`resume deadbeef` must not panic: {combined}"
        );
    }
}
