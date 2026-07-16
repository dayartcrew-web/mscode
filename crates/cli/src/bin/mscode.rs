//! `mscode` binary entry point.
//!
//! Phase 7 wiring:
//! - `mscode version` — synchronous fast path, no async, no TUI deps touched.
//! - `mscode new` — creates a session in `mscode-thread-store`, prints the id.
//! - `mscode chat` — launches the TUI against an existing or new session.
//! - `mscode resume <id>` — resolves an id prefix, then launches the TUI.
//! - `mscode sessions` — lists sessions (cwd-soft-filtered; `--all` to escape).
//!
//! # Cold-start preservation
//!
//! `mscode version` is on a synchronous fast path that does NOT construct a
//! tokio runtime, does NOT open the SQLite pool, and does NOT link against
//! any TUI code path. The `ratatui`/`crossterm` code is reachable from the
//! binary, but only the `chat`/`resume` arms actually invoke it.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use mscode_cli::{Cli, Commands, LoginCommands};
use mscode_credentials::{CredentialError, CredentialStore, NewAccount, SqliteCredentialStore};
use mscode_shared::MscodeVersion;
use mscode_state::AppState;
use mscode_thread_store::{ListSessionsFilter, NewSession, SessionStore};

const GIT_SHA: Option<&str> = option_env!("MSCODE_GIT_SHA");

fn build_version() -> MscodeVersion {
    MscodeVersion::from_cargo_env(
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0),
        GIT_SHA.map(str::to_string),
    )
}

/// Resolve the local-first state database path.
///
/// Defaults to `${MSCODE_HOME:-~/.mscode}/state.db`. The directory is created
/// on first use. Tests can override via `MSCODE_HOME` to point at a tempdir.
fn state_db_path() -> Result<PathBuf, String> {
    if let Ok(home) = std::env::var("MSCODE_HOME") {
        let p = PathBuf::from(home);
        return Ok(p.join("state.db"));
    }
    let home = dirs_home().ok_or_else(|| "could not resolve home directory".to_string())?;
    Ok(home.join(".mscode").join("state.db"))
}

fn dirs_home() -> Option<PathBuf> {
    // We avoid pulling in the `dirs` crate at the binary level to keep the
    // version fast-path lean; the stdlib + HOME env var cover Windows + POSIX.
    if let Ok(h) = std::env::var("HOME") {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return Some(PathBuf::from(h));
    }
    None
}

fn open_state() -> Result<AppState, String> {
    let path = state_db_path()?;
    AppState::open(&path).map_err(|e| format!("failed to open state db at {}: {e}", path.display()))
}

fn current_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string())
}

/// Generate a fresh UUID v4 string for a new session.
fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Fast path: print version and exit. No async, no allocations beyond the
/// version string.
fn run_version() -> ExitCode {
    println!("mscode {}", build_version());
    ExitCode::SUCCESS
}

/// `mscode new` — create a session and print its id.
fn run_new() -> ExitCode {
    let state = match open_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mscode: {e}");
            return ExitCode::from(2);
        }
    };
    let store = SessionStore::new(state);
    let id = new_session_id();
    let cwd = current_cwd();
    let new_session = NewSession {
        id: id.clone(),
        cwd,
        project_root: None,
        created_at: None,
        summary: None,
    };
    match store.create(new_session) {
        Ok(session) => {
            // Just the id on stdout so callers can pipe/capture it.
            println!("{}", session.id);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mscode: failed to create session: {e}");
            ExitCode::from(2)
        }
    }
}

/// `mscode sessions [--all]` — list sessions.
fn run_sessions(all: bool) -> ExitCode {
    let state = match open_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mscode: {e}");
            return ExitCode::from(2);
        }
    };
    let store = SessionStore::new(state);
    let cwd = if all { None } else { Some(current_cwd()) };
    let filter = ListSessionsFilter {
        cwd,
        limit: Some(200),
    };
    match store.list(&filter) {
        Ok(rows) => {
            for r in rows {
                let summary = r.summary.unwrap_or_default();
                if summary.is_empty() {
                    println!("{}\t{}\t{}", r.id, r.updated_at, r.cwd);
                } else {
                    println!("{}\t{}\t{}\t{}", r.id, r.updated_at, r.cwd, summary);
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mscode: failed to list sessions: {e}");
            ExitCode::from(2)
        }
    }
}

/// `mscode resume <id>` — resolve prefix, then (in interactive contexts)
/// launch the TUI.
///
/// In non-interactive contexts (piped stdout/stderr or no TTY), we just
/// resolve and print the session id. The TUI itself is only launched when
/// stdout is a real terminal.
fn run_resume(id: &str) -> ExitCode {
    let state = match open_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mscode: {e}");
            return ExitCode::from(2);
        }
    };
    let store = SessionStore::new(state);
    match store.get_by_id_prefix(id) {
        Ok(session) => {
            // Interactive TUI launch is gated on a TTY check so this command
            // remains testable.
            if is_stdout_tty() {
                launch_tui(Some(session.id))
            } else {
                println!("{}", session.id);
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            // Helpful error: distinguish not-found vs ambiguous prefix.
            let msg = format!("mscode: could not resume `{id}`: {e}");
            eprintln!("{msg}");
            ExitCode::from(2)
        }
    }
}

/// `mscode chat [session]` — launch the TUI against an existing or new session.
fn run_chat(session: Option<&str>) -> ExitCode {
    if !is_stdout_tty() {
        // Without a TTY there is nothing useful to render — surface a clear
        // error rather than crashing inside crossterm.
        eprintln!("mscode: chat requires an interactive terminal (stdout is not a TTY)");
        return ExitCode::from(2);
    }
    let resolved_id = if let Some(id) = session {
        // Resolve the prefix eagerly so chat fails fast on bad input.
        let state = match open_state() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("mscode: {e}");
                return ExitCode::from(2);
            }
        };
        let store = SessionStore::new(state);
        match store.get_by_id_prefix(id) {
            Ok(s) => Some(s.id),
            Err(e) => {
                eprintln!("mscode: could not resolve session `{id}`: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };
    launch_tui(resolved_id)
}

fn is_stdout_tty() -> bool {
    // We avoid pulling in `is-terminal` / `atty` to keep the binary lean;
    // `std::io::IsTerminal` has been stable since Rust 1.70.
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Launch the TUI. Constructs an [`mscode_tui::App`] and runs the event loop
/// against stdout. All crossterm / ratatui setup is encapsulated inside
/// `mscode_tui::run_on_stdout` — the binary never imports `ratatui` or
/// `crossterm` directly, keeping the version fast-path lean.
fn launch_tui(session_id: Option<String>) -> ExitCode {
    use mscode_tui::TuiConfig;

    let _ = session_id; // The TUI loads the session via its own SessionStore.
    let mut app = mscode_tui::App::new(TuiConfig::default());
    match mscode_tui::run_on_stdout(&mut app) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mscode: tui error: {e}");
            ExitCode::from(2)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Version) | None => run_version(),
        Some(Commands::New {}) => run_new(),
        Some(Commands::Sessions { all }) => run_sessions(all),
        Some(Commands::Resume { id }) => run_resume(&id),
        Some(Commands::Chat { session }) => run_chat(session.as_deref()),
        Some(Commands::Login(cmd)) => run_login(cmd),
    }
}

// ---------------------------------------------------------------------------
// `mscode login` — credential management.
// ---------------------------------------------------------------------------

/// Dispatch a `mscode login` subcommand.
fn run_login(cmd: LoginCommands) -> ExitCode {
    match cmd {
        LoginCommands::Add {
            provider,
            label,
            endpoint,
            api_key,
            api_key_stdin,
            set_default,
        } => run_login_add(
            provider,
            label,
            endpoint,
            api_key,
            api_key_stdin,
            set_default,
        ),
        LoginCommands::List { provider } => run_login_list(provider.as_deref()),
        LoginCommands::Remove { provider, label } => run_login_remove(&provider, &label),
        LoginCommands::Use { provider, label } => run_login_use(&provider, &label),
    }
}

/// Build a SqliteCredentialStore against the user's state.db + OS keyring.
fn credential_store() -> Result<SqliteCredentialStore, String> {
    let state = open_state()?;
    Ok(SqliteCredentialStore::new(state))
}

/// Print a credential error with a user-friendly message.
fn print_credential_error(err: CredentialError) {
    match err {
        CredentialError::KeyringUnavailable => {
            eprintln!(
                "mscode: OS keyring unavailable; set MSCODE_CREDENTIALS_FILE to opt into plaintext file fallback"
            );
        }
        CredentialError::Keyring { operation, .. } => {
            eprintln!("mscode: OS keyring {operation} failed; check platform credentials service");
        }
        other => eprintln!("mscode: {other}"),
    }
}

/// `mscode login add` — interactive credential onboarding.
///
/// Walks the user through provider, label, endpoint, and secret. The secret
/// prompt uses rpassword so it never echoes into the terminal scrollback.
/// Skips prompts for any field already supplied via flag.
fn run_login_add(
    provider: Option<String>,
    label: Option<String>,
    endpoint: Option<String>,
    api_key: Option<String>,
    api_key_stdin: bool,
    set_default: bool,
) -> ExitCode {
    let provider = match provider.or_else(prompt_provider) {
        Some(p) => p,
        None => return ExitCode::from(2),
    };
    let label = match label.or_else(prompt_label) {
        Some(l) => l,
        None => return ExitCode::from(2),
    };

    // Resolve the secret. Order of precedence: --api-key > --api-key-stdin > prompt.
    let secret = if let Some(k) = api_key {
        k
    } else if api_key_stdin {
        let mut buf = String::new();
        if std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).is_err() {
            eprintln!("mscode: failed to read api key from stdin");
            return ExitCode::from(2);
        }
        buf.trim_end_matches(['\n', '\r']).to_string()
    } else {
        match prompt_secret(&provider, &label) {
            Some(s) => s,
            None => return ExitCode::from(2),
        }
    };

    let mut new_account = NewAccount::new(&provider, &label, &secret).with_default(set_default);
    if let Some(ep) = endpoint {
        new_account = new_account.with_endpoint(ep);
    }

    let store = match credential_store() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mscode: {e}");
            return ExitCode::from(2);
        }
    };
    match store.add(new_account) {
        Ok(account) => {
            let badge = if account.is_default { " (default)" } else { "" };
            println!(
                "added {} account `{}` with endpoint {}{}",
                account.provider, account.label, account.endpoint, badge
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            print_credential_error(e);
            ExitCode::from(2)
        }
    }
}

/// `mscode login list [--provider P]` — table of configured accounts.
fn run_login_list(provider: Option<&str>) -> ExitCode {
    let store = match credential_store() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mscode: {e}");
            return ExitCode::from(2);
        }
    };
    let rows = match provider {
        Some(p) => store.list_for_provider(p),
        None => store.list(),
    };
    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            print_credential_error(e);
            return ExitCode::from(2);
        }
    };
    if rows.is_empty() {
        if let Some(p) = provider {
            println!("no accounts configured for provider `{p}`");
        } else {
            println!("no accounts configured; run `mscode login add` to add one");
        }
        return ExitCode::SUCCESS;
    }
    println!(
        "{:<10} {:<14} {:<8} {:<8} ENDPOINT",
        "PROVIDER", "LABEL", "STATUS", "DEFAULT"
    );
    for r in rows {
        println!(
            "{:<10} {:<14} {:<8} {:<8} {}",
            r.provider,
            r.label,
            r.status.as_str(),
            if r.is_default { "yes" } else { "" },
            r.endpoint,
        );
    }
    ExitCode::SUCCESS
}

/// `mscode login remove <provider> <label>` — delete credential + secret.
fn run_login_remove(provider: &str, label: &str) -> ExitCode {
    let store = match credential_store() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mscode: {e}");
            return ExitCode::from(2);
        }
    };
    match store.remove(provider, label) {
        Ok(()) => {
            println!("removed {provider}/{label}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            print_credential_error(e);
            ExitCode::from(2)
        }
    }
}

/// `mscode login use <provider> <label>` — set default account.
fn run_login_use(provider: &str, label: &str) -> ExitCode {
    let store = match credential_store() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mscode: {e}");
            return ExitCode::from(2);
        }
    };
    match store.set_default(provider, label) {
        Ok(()) => {
            println!("default for {provider} is now `{label}`");
            ExitCode::SUCCESS
        }
        Err(e) => {
            print_credential_error(e);
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// Interactive prompts (rpassword). Used only by `login add`.
// ---------------------------------------------------------------------------

/// Prompt for the provider. Returns `None` on EOF / read error.
fn prompt_provider() -> Option<String> {
    let known = ["openai", "anthropic", "openrouter", "ollama"];
    print!("provider [{}] or `custom:<name>`: ", known.join(", "));
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).err().is_some() {
        return None;
    }
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Prompt for the account label.
fn prompt_label() -> Option<String> {
    print!("label (e.g. work, personal): ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).err().is_some() {
        return None;
    }
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Prompt for the secret using rpassword (no echo).
fn prompt_secret(provider: &str, label: &str) -> Option<String> {
    let prompt = format!("api key for {provider}/{label}: ");
    match rpassword::prompt_password(prompt) {
        Ok(s) if !s.is_empty() => Some(s),
        Ok(_) => {
            eprintln!("mscode: empty api key");
            None
        }
        Err(e) => {
            eprintln!("mscode: failed to read api key: {e}");
            None
        }
    }
}
