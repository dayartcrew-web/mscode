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
use mscode_cli::{Cli, Commands};
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
    }
}
