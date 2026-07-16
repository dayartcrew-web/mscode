//! Integration smoke tests: full `mscode login` flow against real provider ids.
//!
//! These tests exercise the end-to-end credential lifecycle using a curated
//! subset of the static `PROVIDER_CATALOG`:
//!   - `login add --api-key` (non-interactive secret onboarding)
//!   - `login list` and `login list --provider <p>`
//!   - `login use <p> <label>` (switch default)
//!   - `login remove <p> <label>`
//!
//! # Hermetic isolation
//!
//! Each test runs against a fresh `tempfile::TempDir` for both:
//!   - `MSCODE_HOME` — points the SQLite `state.db` at a throwaway path.
//!   - `MSCODE_CREDENTIALS_FILE` — points the `FileKeyringBackend` at a
//!     throwaway JSON file. This is the explicit user opt-in for environments
//!     without an OS keyring; we use it here precisely so tests do NOT touch
//!     the real Windows Credential Manager / macOS Keychain / Linux Secret
//!     Service. Touching the real keyring from tests would (a) prompt for
//!     passwords on some systems, (b) leak test secrets into the user's
//!     keychain, and (c) fail in CI.
//!
//! # Provider coverage
//!
//! Providers are picked to cover the catalog breadth: Tier 1 majors (openai,
//! anthropic), Tier 2 popular OpenAI-compatible (deepinfra, mistral, groq),
//! Tier 4 Chinese (zai, deepseek), Tier 5 local (ollama). Endpoints are
//! resolved from the catalog; no `--endpoint` flag is needed for these.

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

/// Fixture: hermetic data dir + keyring file. Drops clean up automatically.
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

    /// Spawn `mscode` with hermetic env vars set.
    fn mscode(&self) -> Command {
        let mut cmd = Command::new(bin());
        cmd.env("MSCODE_HOME", &self.home);
        cmd.env("MSCODE_CREDENTIALS_FILE", &self.creds_file);
        cmd
    }

    /// Run `mscode login add --provider p --label l --api-key k` and assert
    /// success. Returns captured stdout for assertions.
    fn add(&self, provider: &str, label: &str, key: &str) -> String {
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
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success(),
            "login add {provider}/{label} should exit 0; stdout={stdout} stderr={stderr}"
        );
        stdout
    }
}

/// After the first account is added for a provider, it auto-becomes default
/// and the success line tags it `(default)`.
#[test]
fn login_add_first_account_marks_default() {
    let h = Harness::new();
    let stdout = h.add("openai", "work", "sk-test-openai-work");
    assert!(
        stdout.contains("added openai account `work`"),
        "expected add confirmation: {stdout}"
    );
    assert!(
        stdout.contains("(default)"),
        "first account should be tagged default: {stdout}"
    );
}

/// Catalog-listed providers resolve their endpoint without `--endpoint`.
#[test]
fn login_add_uses_catalog_default_endpoint() {
    let h = Harness::new();
    let stdout = h.add("anthropic", "personal", "sk-ant-test");
    // Anthropic's catalog endpoint — asserted verbatim so we catch regressions
    // if the catalog entry is ever accidentally removed or renamed.
    assert!(
        stdout.contains("https://api.anthropic.com"),
        "expected anthropic default endpoint in stdout: {stdout}"
    );
}

/// Multiple providers can coexist; `login list` shows all of them.
#[test]
fn login_list_shows_multiple_providers() {
    let h = Harness::new();
    h.add("openai", "work", "sk-openai");
    h.add("anthropic", "personal", "sk-ant");
    h.add("mistral", "ci", "tk-mistral");

    let out = h
        .mscode()
        .args(["login", "list"])
        .output()
        .expect("spawn login list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "login list should exit 0; stdout={stdout} stderr={stderr}"
    );
    for (provider, label) in [
        ("openai", "work"),
        ("anthropic", "personal"),
        ("mistral", "ci"),
    ] {
        assert!(
            stdout.contains(provider) && stdout.contains(label),
            "expected `{provider}` / `{label}` in list output: {stdout}"
        );
    }
}

/// `login list --provider P` only shows P's accounts.
#[test]
fn login_list_provider_filter_isolates_rows() {
    let h = Harness::new();
    h.add("openai", "work", "sk-openai");
    h.add("anthropic", "personal", "sk-ant");

    let out = h
        .mscode()
        .args(["login", "list", "--provider", "openai"])
        .output()
        .expect("spawn login list --provider");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("openai") && stdout.contains("work"),
        "filter should include the openai row: {stdout}"
    );
    assert!(
        !stdout.contains("anthropic"),
        "filter should exclude other providers: {stdout}"
    );
}

/// Switching the default updates both rows correctly.
#[test]
fn login_use_switches_default() {
    let h = Harness::new();
    h.add("openai", "first", "sk-1");
    // Second account with explicit default — should clear the first.
    h.mscode()
        .args([
            "login",
            "add",
            "--provider",
            "openai",
            "--label",
            "second",
            "--api-key",
            "sk-2",
            "--set-default",
        ])
        .output()
        .expect("spawn login add --set-default");

    // Verify default is on `second`.
    let out = h.mscode().args(["login", "list"]).output().expect("list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first_idx = stdout.find("first").unwrap();
    let second_idx = stdout.find("second").unwrap();
    // Find the "yes" default marker on each line by extracting the lines.
    let first_line = stdout[first_idx..].lines().next().unwrap();
    let second_line = stdout[second_idx..].lines().next().unwrap();
    assert!(
        !first_line.contains(" yes "),
        "first should not be default after --set-default on second: {first_line}"
    );
    assert!(
        second_line.contains(" yes "),
        "second should be default: {second_line}"
    );

    // Now flip back to `first` via `login use`.
    let out = h
        .mscode()
        .args(["login", "use", "openai", "first"])
        .output()
        .expect("spawn login use");
    assert!(out.status.success(), "login use should exit 0");
    let use_stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        use_stdout.contains("default for openai is now `first`"),
        "expected default-flip confirmation: {use_stdout}"
    );

    // Verify the default actually flipped.
    let out = h.mscode().args(["login", "list"]).output().expect("list 2");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first_idx = stdout.find("first").unwrap();
    let second_idx = stdout.find("second").unwrap();
    let first_line = stdout[first_idx..].lines().next().unwrap();
    let second_line = stdout[second_idx..].lines().next().unwrap();
    assert!(
        first_line.contains(" yes "),
        "first should be default: {first_line}"
    );
    assert!(
        !second_line.contains(" yes "),
        "second should no longer be default: {second_line}"
    );
}

/// Removing an account drops it from the list AND from the keyring file.
#[test]
fn login_remove_drops_account_and_secret() {
    let h = Harness::new();
    h.add("groq", "ci", "gsk-groq-test");

    // Confirm presence.
    let out = h.mscode().args(["login", "list"]).output().expect("list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("groq"),
        "groq should be listed before remove: {stdout}"
    );

    // Remove.
    let out = h
        .mscode()
        .args(["login", "remove", "groq", "ci"])
        .output()
        .expect("spawn login remove");
    assert!(out.status.success(), "login remove should exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("removed groq/ci"),
        "expected remove confirmation: {stdout}"
    );

    // Confirm absence.
    let out = h.mscode().args(["login", "list"]).output().expect("list 2");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("groq"),
        "groq should not appear after remove: {stdout}"
    );

    // The creds file should now hold an empty map.
    let bytes = std::fs::read(&h.creds_file).expect("read creds file");
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("creds file is JSON");
    assert!(
        parsed.as_object().map(|o| o.is_empty()).unwrap_or(true),
        "creds file should be empty after remove; got: {parsed}"
    );
}

/// Removing a non-existent account fails loudly.
#[test]
fn login_remove_missing_fails_loud() {
    let h = Harness::new();
    let out = h
        .mscode()
        .args(["login", "remove", "openai", "ghost"])
        .output()
        .expect("spawn login remove ghost");
    assert!(
        !out.status.success(),
        "login remove of missing account should NOT exit 0"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no account") || stderr.contains("not found") || stderr.contains("mscode:"),
        "expected error mentioning missing account: {stderr}"
    );
}

/// `login use` on a missing account fails loudly.
#[test]
fn login_use_missing_fails_loud() {
    let h = Harness::new();
    let out = h
        .mscode()
        .args(["login", "use", "openai", "ghost"])
        .output()
        .expect("spawn login use ghost");
    assert!(
        !out.status.success(),
        "login use on missing account should NOT exit 0"
    );
}

/// Duplicate `(provider, label)` add fails — the keyring write rolls back.
#[test]
fn login_add_duplicate_pair_is_rejected() {
    let h = Harness::new();
    h.add("deepseek", "work", "first-key");
    let out = h
        .mscode()
        .args([
            "login",
            "add",
            "--provider",
            "deepseek",
            "--label",
            "work",
            "--api-key",
            "second-key",
        ])
        .output()
        .expect("spawn duplicate login add");
    assert!(!out.status.success(), "duplicate add should NOT exit 0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists"),
        "expected `already exists` error: {stderr}"
    );
}

/// Chinese provider (zai) flows through the catalog.
#[test]
fn login_add_chinese_provider_zai() {
    let h = Harness::new();
    let stdout = h.add("zai", "work", "zai-test-key");
    assert!(
        stdout.contains("added zai account `work`"),
        "expected zai add confirmation: {stdout}"
    );
    assert!(
        stdout.contains("https://api.z.ai"),
        "expected z.ai default endpoint: {stdout}"
    );
}

/// Popular OpenAI-compatible inference provider (deepinfra).
#[test]
fn login_add_deepinfra_provider() {
    let h = Harness::new();
    let stdout = h.add("deepinfra", "work", "di-test-key");
    assert!(
        stdout.contains("added deepinfra account `work`"),
        "expected deepinfra add confirmation: {stdout}"
    );
}

/// Local provider (ollama) uses http://localhost endpoint.
#[test]
fn login_add_local_provider_ollama() {
    let h = Harness::new();
    let stdout = h.add("ollama", "local", "ollama-nokey");
    assert!(
        stdout.contains("added ollama account `local`"),
        "expected ollama add confirmation: {stdout}"
    );
    assert!(
        stdout.contains("http://localhost:11434"),
        "expected ollama localhost endpoint: {stdout}"
    );
}

/// Reading the creds file from disk shows the secret was persisted (not just
/// kept in process memory) — proves the FileKeyringBackend is actually being
/// used by the binary.
#[test]
fn file_backend_persists_secret_to_disk() {
    let h = Harness::new();
    h.add("openai", "work", "sk-persisted-marker");

    let bytes = std::fs::read(&h.creds_file).expect("creds file");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("sk-persisted-marker"),
        "expected secret to appear in plaintext creds file (file backend opt-in): {text}"
    );
}

/// Two independent harnesses (different MSCODE_HOME + creds files) do not
/// bleed state — proves hermetic isolation.
#[test]
fn two_harnesses_are_isolated() {
    let h1 = Harness::new();
    let h2 = Harness::new();

    h1.add("openai", "first", "sk-first");

    // h2's list should be empty.
    let out = h2
        .mscode()
        .args(["login", "list"])
        .output()
        .expect("list h2");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no accounts configured"),
        "second harness should not see first harness's accounts: {stdout}"
    );
}

/// When stdout is NOT a TTY (as is the case for all spawned subprocesses in
/// tests), `mscode login add` with no flags must fall through to the legacy
/// text-prompt path rather than trying to launch the TUI wizard. Reaching the
/// wizard would crash on crossterm raw-mode setup; reaching the text path
/// hits EOF on `prompt_provider` and returns exit 2.
///
/// This test is the regression gate for the wizard's TTY check: if anyone
/// removes the `is_stdout_tty()` guard, this test will start failing with a
/// panic from inside the TUI rather than the expected exit-2.
#[test]
fn login_add_no_flags_non_tty_uses_text_fallback() {
    let h = Harness::new();
    // Spawn with stdin piped-empty so `prompt_provider` sees EOF immediately.
    let out = h
        .mscode()
        .args(["login", "add"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn login add")
        .wait_with_output()
        .expect("wait_for_output");

    assert!(
        !out.status.success(),
        "non-TTY login add with no flags should NOT exit 0; stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // Exit code should be 2 (the CLI's generic error code). The wizard path
    // would have panicked or returned a different failure mode.
    let code = out.status.code().expect("exit code");
    assert_eq!(
        code, 2,
        "expected exit 2 from text-fallback EOF; got {code}"
    );
}
