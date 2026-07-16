//! Smoke test: `mscode version` and `mscode --version` exit 0 and print the
//! version string built from CARGO_PKG_VERSION_*.

use std::process::Command;

fn mscode_bin() -> Command {
    // `CARGO_BIN_EXE_mscode` is set by cargo for integration tests and points
    // at the compiled binary.
    let path = std::env::var("CARGO_BIN_EXE_mscode")
        .expect("CARGO_BIN_EXE_mscode must be set by the cargo test harness");
    Command::new(path)
}

#[test]
fn version_subcommand_exits_zero_and_prints_version() {
    let output = mscode_bin().arg("version").output().expect("spawn mscode");
    assert!(
        output.status.success(),
        "mscode version did not exit cleanly"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("mscode"),
        "expected 'mscode' in stdout, got: {stdout}"
    );
    // Should contain at least one dot (major.minor.patch)
    assert!(
        stdout.contains('.'),
        "expected version string, got: {stdout}"
    );
}

#[test]
fn version_flag_exits_zero_and_prints_version() {
    let output = mscode_bin()
        .arg("--version")
        .output()
        .expect("spawn mscode");
    assert!(
        output.status.success(),
        "mscode --version did not exit cleanly"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("mscode"),
        "expected 'mscode' in stdout, got: {stdout}"
    );
}

#[test]
fn no_subcommand_defaults_to_version_like_output() {
    let output = mscode_bin().output().expect("spawn mscode");
    assert!(output.status.success(), "bare mscode did not exit cleanly");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mscode"));
}
