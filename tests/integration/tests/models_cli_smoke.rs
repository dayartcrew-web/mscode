//! Integration smoke tests for the `mscode models` CLI subcommand.
//!
//! Exercises the four user-visible modes:
//!   - no credentials → empty-state hint
//!   - one credential → that provider's models listed
//!   - `--all` → entire catalog regardless of credentials
//!   - `--provider <id>` → narrow to a single provider
//!   - `--provider <unknown>` → empty + targeted message
//!
//! The harness mirrors [`login_flow_smoke.rs`]: hermetic MSCODE_HOME +
//! plaintext-file keyring so tests never touch the real OS keyring.

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

struct Harness {
    _data: tempfile::TempDir,
    _creds: tempfile::TempDir,
    home: PathBuf,
    creds_file: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let data = tempdir().expect("tempdir for MSCODE_HOME");
        let creds = tempdir().expect("tempdir for MSCODE_CREDENTIALS_FILE");
        let creds_file = creds.path().join("creds.json");
        Self {
            home: data.path().to_path_buf(),
            creds_file,
            _data: data,
            _creds: creds,
        }
    }

    fn mscode(&self) -> Command {
        let mut cmd = Command::new(bin());
        cmd.env("MSCODE_HOME", &self.home);
        cmd.env("MSCODE_CREDENTIALS_FILE", &self.creds_file);
        cmd
    }

    fn add(&self, provider: &str, label: &str, key: &str) {
        let out = self
            .mscode()
            .args([
                "login",
                "add",
                "--provider",
                provider,
                "--label",
                label,
                "--api-key",
                key,
            ])
            .output()
            .unwrap_or_else(|e| panic!("spawn login add {provider}/{label}: {e}"));
        assert!(
            out.status.success(),
            "login add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Run `mscode models [...]` and return (success?, stdout, stderr).
    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let mut cmd = self.mscode();
        cmd.args(args);
        let out = cmd
            .output()
            .unwrap_or_else(|e| panic!("spawn mscode {:?}: {e}", args));
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }
}

#[test]
fn models_no_credentials_shows_help_message() {
    let h = Harness::new();
    let (ok, stdout, _stderr) = h.run(&["models"]);
    assert!(ok, "models should exit 0 even with no credentials");
    assert!(
        stdout.contains("no providers configured"),
        "expected empty-state hint; got: {stdout}"
    );
    assert!(
        stdout.contains("mscode login add"),
        "hint should point at login add; got: {stdout}"
    );
}

#[test]
fn models_with_login_lists_provider_models() {
    let h = Harness::new();
    h.add("openai", "work", "sk-test-key-0123456789");

    let (ok, stdout, stderr) = h.run(&["models"]);
    assert!(ok, "stderr={stderr}");
    // Header is present.
    assert!(
        stdout.contains("PROVIDER") && stdout.contains("MODEL"),
        "expected table header; got: {stdout}"
    );
    // At least one openai row.
    assert!(
        stdout.lines().any(|l| l.contains("openai")),
        "expected an openai row; got: {stdout}"
    );
}

#[test]
fn models_all_flag_escapes_credentials_filter() {
    let h = Harness::new();
    // No credentials at all; --all should still list many providers.
    let (ok, stdout, stderr) = h.run(&["models", "--all"]);
    assert!(ok, "stderr={stderr}");
    let provider_rows = stdout
        .lines()
        .skip(1) // header
        .filter(|l| !l.trim().is_empty())
        .count();
    assert!(
        provider_rows > 50,
        "expected many provider rows with --all, got {provider_rows}; stdout head: {}",
        stdout.lines().take(3).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn models_provider_filter_isolates_rows() {
    let h = Harness::new();
    h.add("openai", "work", "sk-test-key-0123456789");
    h.add("anthropic", "work", "sk-ant-test-0123456789");

    let (ok, stdout, stderr) = h.run(&["models", "--provider", "anthropic"]);
    assert!(ok, "stderr={stderr}");
    // Every data row must be anthropic.
    for line in stdout.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let provider_field = line.split_whitespace().next().unwrap_or("");
        assert_eq!(
            provider_field, "anthropic",
            "expected only anthropic rows; got: {line}"
        );
    }
}

#[test]
fn models_unknown_provider_is_empty() {
    let h = Harness::new();
    // Logged in to openai, asking for ghost — should be empty with a targeted hint.
    h.add("openai", "work", "sk-test-key-0123456789");
    let (ok, stdout, _stderr) = h.run(&["models", "--provider", "ghost"]);
    assert!(ok);
    assert!(
        stdout.contains("no models for provider `ghost`"),
        "expected targeted empty message; got: {stdout}"
    );
}

#[test]
fn models_unknown_provider_with_all_still_empty_but_distinct() {
    let h = Harness::new();
    let (ok, stdout, _stderr) = h.run(&["models", "--all", "--provider", "ghost"]);
    assert!(ok);
    // The all-branch message is the narrower form.
    assert!(
        stdout.contains("no models for provider `ghost`"),
        "expected targeted empty message; got: {stdout}"
    );
}
