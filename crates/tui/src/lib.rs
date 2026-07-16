//! mscode-tui — terminal UI dashboard for the mscode agentic CLI.
//!
//! This crate is **logic-first**: the rendering layer is a thin shell around a
//! pure-Rust state machine. All interesting behavior (slash-command parsing,
//! mode transitions, message buffering, session filtering) lives in plain
//! functions that can be unit-tested without a real TTY.
//!
//! Layout of this crate:
//!
//! - [`config`] — [`TuiConfig`] with sensible defaults.
//! - [`error`] — [`TuiError`] (thiserror-based).
//! - [`events`] — [`TuiEvent`] + [`ExternalEvent`].
//! - [`modes`] — [`InputMode`] + [`PlanMode`].
//! - [`slash`] — [`ParsedCommand`] + [`parse_slash_command`].
//! - [`message_buffer`] — [`MessageBuffer`] (input history + draft).
//! - [`session_list`] — [`SessionList`] (filterable session picker).
//! - [`app`] — [`App`] top-level state machine.
//! - [`render`] — ratatui rendering (lazy-loaded only when the TUI launches).
//!
//! # Local-first / cold-start invariants
//!
//! 1. `mscode version` does NOT pull in the TUI binary path — the CLI depends
//!    on this crate only at link time, and the CLI binary's `main` only
//!    constructs an [`App`] inside the `chat`/`resume` arms.
//! 2. The event loop is driven by [`tokio`] and uses
//!    [`tokio::task::spawn_blocking`] for persistence ops so disk I/O never
//!    blocks the input handler.

pub mod app;
pub mod config;
pub mod error;
pub mod events;
pub mod message_buffer;
pub mod modes;
pub mod render;
pub mod session_list;
pub mod slash;

pub use app::{App, AppExit};
pub use config::{TuiConfig, TuiTheme};
pub use error::TuiError;
pub use events::{ExternalEvent, TuiEvent};
pub use message_buffer::MessageBuffer;
pub use modes::{InputMode, PlanMode};
pub use session_list::{SessionEntry, SessionList, SessionLookup};
pub use slash::{ALL_SLASH_COMMANDS, ParsedCommand, SlashCommandError, parse_slash_command};

/// Result alias for the tui crate.
pub type Result<T> = std::result::Result<T, TuiError>;

/// Run the TUI against stdout. Handles crossterm raw-mode + alt-screen setup
/// and teardown. Returns the exit reason on clean shutdown.
///
/// This function is the single entry point that knows about the crossterm
/// backend; the binary never has to import `ratatui` / `crossterm` directly.
pub fn run_on_stdout(app: &mut App) -> Result<AppExit> {
    use ratatui_crossterm::CrosstermBackend;

    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)
        .map_err(|e| TuiError::TerminalInit(format!("failed to construct terminal: {e}")))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| TuiError::TerminalInit(format!("failed to start runtime: {e}")))?;

    let _guard = RawModeGuard::enable()?;
    let exit = runtime.block_on(app.run(&mut terminal));
    drop(_guard); // explicit teardown before runtime drops
    exit
}

/// RAII guard that enables raw mode + alt screen on construction and disables
/// both on drop. Public so tests can wrap their own backends.
pub struct RawModeGuard {
    already_enabled: bool,
}

impl RawModeGuard {
    /// Enable raw mode + enter the alternate screen. Returns a guard that
    /// restores the prior state on drop.
    pub fn enable() -> Result<Self> {
        use crossterm::execute;
        use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
        enable_raw_mode().map_err(TuiError::Io)?;
        execute!(std::io::stdout(), EnterAlternateScreen).map_err(TuiError::Io)?;
        Ok(Self {
            already_enabled: true,
        })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if !self.already_enabled {
            return;
        }
        use crossterm::execute;
        use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}
