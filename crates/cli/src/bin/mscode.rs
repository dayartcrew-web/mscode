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

/// `mscode models [--all] [--provider <id>]` — list models as a text table.
///
/// Output is scriptable: header row followed by one row per model. Empty
/// states surface a hint instead of an empty table.
fn run_models(all: bool, provider: Option<&str>) -> ExitCode {
    let items = load_models_items(provider, all);
    if items.is_empty() {
        if all {
            match provider {
                Some(p) => println!("no models for provider `{p}`"),
                None => println!("no models in catalog"),
            }
        } else if let Some(p) = provider {
            println!(
                "no models for provider `{p}`; log in with `mscode login add --provider {p}` or use `--all`"
            );
        } else {
            println!("no providers configured; run `mscode login add` to add one");
        }
        return ExitCode::SUCCESS;
    }
    println!(
        "{:<14} {:<34} {:<10} {:<6} MODEL",
        "PROVIDER", "NAME", "CONTEXT", "TOOLS"
    );
    for item in items {
        let ctx = item
            .context_window
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".into());
        let tools = if item.supports_tools { "yes" } else { "" };
        println!(
            "{:<14} {:<34} {:<10} {:<6} {}",
            item.provider_id, item.display_label, ctx, tools, item.model_id
        );
    }
    ExitCode::SUCCESS
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

    // Inject the credential-gated catalog into the in-TUI `/models` picker.
    // Empty items is a valid state — the picker renders a "run `mscode login
    // add`" hint rather than a blank list. Errors are already surfaced via
    // stderr inside the helper; here we just propagate the (possibly empty)
    // vec.
    let models_items = load_models_items(None, false);
    let mut app = mscode_tui::App::new(TuiConfig::default()).with_models(models_items);
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
        Some(Commands::Models { all, provider }) => run_models(all, provider.as_deref()),
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
///
/// When `MSCODE_CREDENTIALS_FILE` is set, swap the OS keyring for a
/// plaintext-file backend. This is the explicit user opt-in for environments
/// where the OS keyring is unavailable (headless CI, locked-down sandboxes,
/// WSL without D-Bus secrets). The file is JSON `{key_id: secret}` and is
/// created with `0600` permissions on POSIX.
fn credential_store() -> Result<SqliteCredentialStore, String> {
    let state = open_state()?;
    if let Ok(file_path) = std::env::var("MSCODE_CREDENTIALS_FILE") {
        if !file_path.is_empty() {
            let backend = mscode_credentials::FileKeyringBackend::new(PathBuf::from(file_path))
                .map_err(|e| format!("failed to open MSCODE_CREDENTIALS_FILE: {e}"))?;
            return Ok(SqliteCredentialStore::with_backend(
                state,
                Box::new(backend),
            ));
        }
    }
    Ok(SqliteCredentialStore::new(state))
}

/// Build the list of models to surface in the CLI table and the in-TUI
/// `/models` picker.
///
/// The two surfaces share this helper so they always agree on what's visible.
///
/// # Filter semantics
///
/// - `all == true` → every provider in the embedded catalog, regardless of
///   credentials. Intended for `mscode models --all` (catalog browsing).
/// - `all == false` → only providers the user has at least one credential for.
///   Default for both `mscode models` (CLI) and `/models` (TUI).
/// - `provider_filter == Some(p)` → narrow further to a single provider id. The
///   filter is applied **after** the credential gate, so `--provider foo`
///   without `--all` and without a credential for `foo` returns empty.
///
/// # Error handling
///
/// Any keyring / SQLite failure is logged to stderr and treated as "no
/// credentials" — i.e. returns an empty vec. The caller's empty-state UX (a
/// "run `mscode login add`" hint) handles it. We never propagate the error
/// because both surfaces prefer degraded-but-functional output over a hard
/// failure on a non-essential feature.
fn load_models_items(provider_filter: Option<&str>, all: bool) -> Vec<mscode_tui::ModelItem> {
    use std::collections::HashSet;

    use mscode_provider::ModelsCatalog;

    let catalog = ModelsCatalog::get();

    // Resolve the set of provider ids we want to surface, then iterate the
    // catalog once and emit ModelRefs that borrow from the &'static catalog.
    // Owned `HashSet<String>` here so the set outlives any local String vecs
    // built while resolving credentials.
    let allow: HashSet<String> = if all {
        // Catalog-browsing mode: ignore credentials entirely.
        match provider_filter {
            Some(p) => std::iter::once(p.to_string()).collect(),
            None => catalog.providers().keys().cloned().collect(),
        }
    } else {
        // Credential-gated mode: resolve distinct provider ids from the store.
        let provider_ids: Vec<String> = match credential_store() {
            Ok(store) => match store.list() {
                Ok(accounts) => {
                    let mut ids: Vec<String> =
                        accounts.iter().map(|a| a.provider.clone()).collect();
                    ids.sort();
                    ids.dedup();
                    ids
                }
                Err(e) => {
                    eprintln!("mscode: failed to list credentials: {e}");
                    Vec::new()
                }
            },
            Err(e) => {
                eprintln!("mscode: {e}");
                Vec::new()
            }
        };
        match provider_filter {
            Some(p) if provider_ids.iter().any(|id| id == p) => {
                std::iter::once(p.to_string()).collect()
            }
            Some(_) => HashSet::new(),
            None => provider_ids.into_iter().collect(),
        }
    };

    catalog
        .all_models()
        .into_iter()
        .filter(|m| allow.contains(m.provider_id))
        .map(|r| mscode_tui::ModelItem {
            provider_id: r.provider_id.to_string(),
            model_id: r.model.id.clone(),
            display_label: format!("{} / {}", r.provider_name, r.model.name),
            context_window: r.model.limit.context,
            supports_tools: r.model.tool_call,
        })
        .collect()
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
/// When stdout is a TTY AND none of `--provider`, `--label`, `--api-key`,
/// `--api-key-stdin` are supplied, launches a fuzzy-search TUI wizard (the
/// "Full TUI flow" matching `opencode auth login`). Otherwise falls back to
/// the legacy text-prompt path (rpassword for the secret) so the command
/// remains scriptable and CI-friendly.
fn run_login_add(
    provider: Option<String>,
    label: Option<String>,
    endpoint: Option<String>,
    api_key: Option<String>,
    api_key_stdin: bool,
    set_default: bool,
) -> ExitCode {
    // Resolve provider / label / secret. The TUI wizard path can supply all
    // three at once; the text path resolves each independently.
    let (provider, label, secret) = if should_launch_wizard(
        is_stdout_tty(),
        provider.as_ref(),
        label.as_ref(),
        api_key.as_ref(),
        api_key_stdin,
    ) {
        match launch_login_wizard() {
            WizardOutcome::Finished(p, l, s) => (Some(p), Some(l), Some(s)),
            WizardOutcome::Cancelled => {
                // Silent exit — the user explicitly bailed out.
                return ExitCode::from(130);
            }
            WizardOutcome::Failed(e) => {
                eprintln!("mscode: login wizard error: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        // Pass through the flag values (any that were supplied); the text-prompt
        // path below fills in the gaps via rpassword / stdin.
        (provider, label, api_key)
    };

    let provider = match provider.or_else(prompt_provider) {
        Some(p) => p,
        None => return ExitCode::from(2),
    };
    let label = match label.or_else(prompt_label) {
        Some(l) => l,
        None => return ExitCode::from(2),
    };

    // Resolve the secret. Order of precedence: wizard > --api-key > --api-key-stdin > prompt.
    let secret = if let Some(k) = secret {
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

/// Decide whether the TUI wizard should run. We launch it only when stdout is
/// a real terminal AND the user did not supply any of the onboarding fields
/// via flags (otherwise the wizard would be redundant or confusing).
fn should_launch_wizard(
    is_tty: bool,
    provider: Option<&String>,
    label: Option<&String>,
    api_key: Option<&String>,
    api_key_stdin: bool,
) -> bool {
    is_tty && provider.is_none() && label.is_none() && api_key.is_none() && !api_key_stdin
}

/// Outcome of launching the login wizard.
enum WizardOutcome {
    /// User completed all steps.
    Finished(String, String, String),
    /// User cancelled (Esc on first step, or Ctrl-C anywhere).
    Cancelled,
    /// The wizard failed to start or render.
    Failed(String),
}

/// Build the picker catalog from `PROVIDER_CATALOG` and launch the wizard.
fn launch_login_wizard() -> WizardOutcome {
    use mscode_credentials::{AuthMethod, PROVIDER_CATALOG};
    use mscode_tui::PickerItem;

    let items: Vec<PickerItem> = PROVIDER_CATALOG
        .iter()
        .filter(|e| {
            matches!(
                e.auth,
                AuthMethod::ApiKey | AuthMethod::Both | AuthMethod::Local
            )
        })
        .map(|e| PickerItem::catalog(e.id, e.display_name, e.endpoint))
        .collect();

    match mscode_tui::run_login_wizard_on_stdout(items) {
        Ok(Some((p, l, s))) => WizardOutcome::Finished(p, l, s),
        Ok(None) => WizardOutcome::Cancelled,
        Err(e) => WizardOutcome::Failed(e.to_string()),
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
///
/// Shows a curated short list (recommended + popular) from the static catalog
/// and accepts any provider id from the full catalog or a `custom:<name>`
/// namespace. The full list lives in [`mscode_credentials::PROVIDER_CATALOG`].
fn prompt_provider() -> Option<String> {
    use mscode_credentials::{PROVIDER_CATALOG, is_recommended_provider};
    let popular: Vec<&str> = PROVIDER_CATALOG
        .iter()
        .map(|e| e.id)
        .filter(|id| {
            is_recommended_provider(id)
                || matches!(
                    *id,
                    "mistral" | "groq" | "deepseek" | "ollama" | "github-copilot"
                )
        })
        .collect();
    print!("provider [{}] (or `custom:<name>`): ", popular.join(", "));
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
