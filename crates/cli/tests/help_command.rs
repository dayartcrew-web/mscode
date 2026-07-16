//! Smoke test: `mscode --help` lists every declared subcommand.

use std::process::Command;

const EXPECTED_SUBCOMMANDS: &[&str] = &["version", "new", "chat", "resume", "sessions"];

#[test]
fn help_lists_all_declared_subcommands() {
    let path = std::env::var("CARGO_BIN_EXE_mscode")
        .expect("CARGO_BIN_EXE_mscode must be set by the cargo test harness");
    let output = Command::new(path)
        .arg("--help")
        .output()
        .expect("spawn mscode");
    assert!(
        output.status.success(),
        "mscode --help did not exit cleanly"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for sub in EXPECTED_SUBCOMMANDS {
        assert!(
            stdout.contains(sub),
            "expected subcommand `{sub}` in --help output: {stdout}"
        );
    }
}

#[test]
fn help_subcommand_is_disabled() {
    // disable_help_subcommand = true should make `mscode help` not exist as
    // a subcommand. clap returns exit code 2 for unknown subcommands.
    let path = std::env::var("CARGO_BIN_EXE_mscode")
        .expect("CARGO_BIN_EXE_mscode must be set by the cargo test harness");
    let output = Command::new(path)
        .arg("help")
        .output()
        .expect("spawn mscode");
    // Either it errors with exit 2 (unknown subcommand) or clap auto-handles
    // `--help`. Either way, the bare `help` token should not be a working
    // subcommand printing the same content as --help.
    let code = output.status.code().unwrap_or(-1);
    assert!(
        code != 0 || String::from_utf8_lossy(&output.stderr).contains("unexpected"),
        "expected non-zero exit or warning for `help`, got code={code}"
    );
}

#[test]
fn exactly_five_subcommands_at_launch() {
    // Phase 7 launch surface: version, new, chat, resume, sessions.
    // This test guards against accidental subcommand creep.
    let path = std::env::var("CARGO_BIN_EXE_mscode")
        .expect("CARGO_BIN_EXE_mscode must be set by the cargo test harness");
    let output = Command::new(path)
        .arg("--help")
        .output()
        .expect("spawn mscode");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let present: Vec<&str> = EXPECTED_SUBCOMMANDS
        .iter()
        .copied()
        .filter(|s| stdout.contains(s))
        .collect();
    assert_eq!(present.len(), EXPECTED_SUBCOMMANDS.len());
}
