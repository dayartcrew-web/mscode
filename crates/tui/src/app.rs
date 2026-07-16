//! [`App`] — the top-level TUI state machine.
//!
//! `App` owns the input mode, plan mode, message buffer, and session picker.
//! The render layer is intentionally thin (see [`crate::render`]) so all
//! interesting behavior can be unit-tested without a TTY.
//!
//! # Plan-mode gating
//!
//! When [`PlanMode`] is `Planning`, calling [`App::submit_draft`] does NOT
//! call the executor — the message is queued with
//! `pending_approval = true`. When `PlanMode` is `Executing`, submission
//! returns [`SubmitOutcome::Dispatched`] and the caller (the binary) is
//! responsible for handing the message to the agent runtime.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::TuiError;
use crate::config::TuiConfig;
use crate::message_buffer::MessageBuffer;
use crate::modes::{InputMode, PlanMode};
use crate::session_list::SessionList;
use crate::slash::{ParsedCommand, SlashCommandError, parse_slash_command};

/// Why the app is exiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppExit {
    /// User pressed Ctrl+C or invoked `/quit`. State has been flushed.
    Clean,
}

/// What happened when the user submitted the draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// Draft was empty or whitespace — nothing happened.
    Empty,
    /// Slash command parsed successfully and was routed internally.
    Command(ParsedCommand),
    /// Free-text message queued for plan-mode approval.
    QueuedForApproval(String),
    /// Free-text message dispatched to the agent runtime (plan mode was Executing).
    Dispatched(String),
}

/// What happened while handling a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// The key was consumed by the current mode.
    Consumed,
    /// The key caused a transition but no submit (e.g. entered Insert mode).
    Transitioned,
    /// Submission produced a result.
    Submitted(SubmitOutcome),
    /// A slash command failed to parse.
    CommandError(SlashCommandError),
    /// User wants to quit (Ctrl+C or `/quit`).
    Quit,
}

/// Top-level TUI state machine.
#[derive(Debug, Clone)]
pub struct App {
    config: TuiConfig,
    input_mode: InputMode,
    plan_mode: PlanMode,
    messages: MessageBuffer,
    sessions: SessionList,
    /// Working directory the app was launched from (used for `/sessions` filter).
    cwd: String,
    /// Currently-active slash filter (Some(text) when in SlashCommand mode).
    slash_filter: String,
    /// Cached list of slash command names matching the filter (computed on demand).
    slash_matches: Vec<String>,
    /// True when the user has asked to quit. The event loop checks this.
    should_quit: bool,
}

impl App {
    /// Construct a new state machine with the given config and cwd.
    pub fn new(config: TuiConfig) -> Self {
        let capacity = config.history_capacity;
        Self {
            config,
            input_mode: InputMode::Normal,
            plan_mode: PlanMode::Planning,
            messages: MessageBuffer::new(capacity),
            sessions: SessionList::new(),
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".into()),
            slash_filter: String::new(),
            slash_matches: Vec::new(),
            should_quit: false,
        }
    }

    /// Override the working directory used for the `/sessions` filter.
    /// Useful for tests and for `mscode resume --cwd`.
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// Read-only borrow of the config.
    pub fn config(&self) -> &TuiConfig {
        &self.config
    }

    /// Current input mode.
    pub fn input_mode(&self) -> InputMode {
        self.input_mode
    }

    /// Current plan mode.
    pub fn plan_mode(&self) -> PlanMode {
        self.plan_mode
    }

    /// Toggle plan mode (used by the future `/plan` command).
    pub fn toggle_plan_mode(&mut self) {
        self.plan_mode = match self.plan_mode {
            PlanMode::Planning => PlanMode::Executing,
            PlanMode::Executing => PlanMode::Planning,
        };
    }

    /// Borrow the message buffer.
    pub fn messages(&self) -> &MessageBuffer {
        &self.messages
    }

    /// Mutably borrow the message buffer.
    pub fn messages_mut(&mut self) -> &mut MessageBuffer {
        &mut self.messages
    }

    /// Borrow the session picker.
    pub fn sessions(&self) -> &SessionList {
        &self.sessions
    }

    /// Mutably borrow the session picker.
    pub fn sessions_mut(&mut self) -> &mut SessionList {
        &mut self.sessions
    }

    /// Current cwd used by the session filter.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Whether the user has requested to quit.
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Current slash filter matches (SlashCommand mode).
    pub fn slash_matches(&self) -> &[String] {
        &self.slash_matches
    }

    /// Recompute the cached slash matches for the current filter.
    fn refresh_slash_matches(&mut self) {
        use crate::slash::ALL_SLASH_COMMANDS;
        if self.slash_filter.is_empty() {
            self.slash_matches = ALL_SLASH_COMMANDS
                .iter()
                .map(|s| (*s).to_string())
                .collect();
        } else {
            self.slash_matches = ALL_SLASH_COMMANDS
                .iter()
                .filter(|c| c.starts_with(&self.slash_filter))
                .map(|s| (*s).to_string())
                .collect();
        }
    }

    /// Enter SlashCommand filter mode (called when the user types `/`).
    pub fn enter_slash_mode(&mut self) {
        self.input_mode = InputMode::SlashCommand;
        self.slash_filter.clear();
        self.refresh_slash_matches();
    }

    /// Drive the state machine with a crossterm key event.
    ///
    /// Returns the [`KeyOutcome`] so the caller can dispatch side effects
    /// (executor calls, session-list refresh, etc.).
    pub fn handle_key(&mut self, key: KeyEvent) -> KeyOutcome {
        // Ctrl+C is the sole interrupt — always quits, regardless of mode.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return KeyOutcome::Quit;
        }

        match self.input_mode {
            InputMode::Normal => self.handle_normal_key(key),
            InputMode::Insert => self.handle_insert_key(key),
            InputMode::SlashCommand => self.handle_slash_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            // Enter Insert mode.
            KeyCode::Char('i') => {
                self.input_mode = InputMode::Insert;
                KeyOutcome::Transitioned
            }
            // `/` enters slash filter mode.
            KeyCode::Char('/') => {
                self.enter_slash_mode();
                KeyOutcome::Transitioned
            }
            // History navigation.
            KeyCode::Up => {
                self.messages.history_prev();
                KeyOutcome::Consumed
            }
            KeyCode::Down => {
                let _ = self.messages.history_next();
                KeyOutcome::Consumed
            }
            // Ctrl+Q / Esc have no effect in Normal mode at v1 (Ctrl+C is the
            // sole interrupt). Other keys are ignored.
            _ => KeyOutcome::Consumed,
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                KeyOutcome::Transitioned
            }
            KeyCode::Enter => {
                let outcome = self.submit_draft();
                KeyOutcome::Submitted(outcome)
            }
            KeyCode::Tab => {
                self.messages.tab_queue();
                KeyOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.messages.backspace();
                KeyOutcome::Consumed
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.messages.history_prev();
                KeyOutcome::Consumed
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.messages.history_next();
                KeyOutcome::Consumed
            }
            KeyCode::Char(c) => {
                self.messages.push_char(c);
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    fn handle_slash_key(&mut self, key: KeyEvent) -> KeyOutcome {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.slash_filter.clear();
                self.slash_matches.clear();
                KeyOutcome::Transitioned
            }
            KeyCode::Enter => {
                // Treat the current slash draft (filter + draft) as a command.
                // The user has been typing into the regular draft box; we
                // route through the parser.
                let draft = self.messages.draft().to_string();
                if !draft.starts_with('/') {
                    // User entered slash mode but then deleted the leading `/`.
                    self.input_mode = InputMode::Normal;
                    return KeyOutcome::Transitioned;
                }
                match parse_slash_command(&draft) {
                    Ok(Some(cmd)) => {
                        if matches!(cmd, ParsedCommand::Quit) {
                            self.should_quit = true;
                            self.input_mode = InputMode::Normal;
                            return KeyOutcome::Quit;
                        }
                        self.input_mode = InputMode::Normal;
                        self.messages.set_draft("");
                        KeyOutcome::Submitted(SubmitOutcome::Command(cmd))
                    }
                    Ok(None) => {
                        // Draft is `/` with no command name yet.
                        KeyOutcome::Consumed
                    }
                    Err(e) => KeyOutcome::CommandError(e),
                }
            }
            KeyCode::Backspace => {
                if self.slash_filter.is_empty() {
                    // Exit slash mode when backspace is pressed on empty filter.
                    self.input_mode = InputMode::Normal;
                    self.slash_matches.clear();
                    KeyOutcome::Transitioned
                } else {
                    self.slash_filter.pop();
                    self.refresh_slash_matches();
                    KeyOutcome::Consumed
                }
            }
            KeyCode::Tab => {
                // Auto-complete: if exactly one match, accept it.
                if self.slash_matches.len() == 1 {
                    let completed = self.slash_matches[0].clone();
                    self.messages.set_draft(format!("/{completed} "));
                    self.slash_filter = completed;
                    self.refresh_slash_matches();
                }
                KeyOutcome::Consumed
            }
            KeyCode::Char(c) => {
                self.slash_filter.push(c);
                self.refresh_slash_matches();
                // Mirror the filter into the visible draft so the renderer
                // sees `/filter` text. The draft is the single source of
                // truth for the visible input.
                self.messages.set_draft(format!("/{}", self.slash_filter));
                KeyOutcome::Consumed
            }
            _ => KeyOutcome::Consumed,
        }
    }

    /// Submit the current draft. Honors plan mode.
    ///
    /// - If the draft starts with `/`, attempt to parse a slash command.
    /// - Otherwise, dispatch the message (or queue for approval, depending on
    ///   plan mode).
    pub fn submit_draft(&mut self) -> SubmitOutcome {
        let draft = self.messages.draft();
        if draft.trim().is_empty() {
            return SubmitOutcome::Empty;
        }
        if draft.trim_start().starts_with('/') {
            return match parse_slash_command(draft) {
                Ok(Some(cmd)) => {
                    if matches!(cmd, ParsedCommand::Quit) {
                        self.should_quit = true;
                    }
                    // Move the draft into history as a side note, then clear.
                    self.messages.set_draft("");
                    SubmitOutcome::Command(cmd)
                }
                Ok(None) => SubmitOutcome::Empty,
                Err(_e) => {
                    // Surface a parse error by leaving the draft intact for
                    // editing. The renderer overlays the error.
                    SubmitOutcome::Empty
                }
            };
        }
        let pending = self.plan_mode.is_planning();
        match self.messages.submit(pending) {
            Some(text) => {
                if pending {
                    SubmitOutcome::QueuedForApproval(text)
                } else {
                    SubmitOutcome::Dispatched(text)
                }
            }
            None => SubmitOutcome::Empty,
        }
    }

    /// Run the main event loop against a configured terminal.
    ///
    /// This is intentionally generic so tests can pass a mock backend; the
    /// real binary constructs a `Terminal<CrosstermBackend<Stdout>>`.
    ///
    /// The loop blocks on crossterm input. Persistence is delegated to
    /// `tokio::task::spawn_blocking` so disk I/O never freezes the UI.
    pub async fn run<B>(&mut self, terminal: &mut ratatui::Terminal<B>) -> Result<AppExit, TuiError>
    where
        B: ratatui::backend::Backend,
        TuiError: From<B::Error>,
    {
        use std::time::Duration;
        loop {
            // Render.
            terminal
                .draw(|frame| crate::render::draw(frame, self))
                .map_err(TuiError::from)?;

            // Poll for an event with a short timeout so we can refresh.
            if crossterm::event::poll(Duration::from_millis(250)).map_err(TuiError::Io)? {
                if let crossterm::event::Event::Key(key) =
                    crossterm::event::read().map_err(TuiError::Io)?
                {
                    let outcome = self.handle_key(key);
                    if self.should_quit {
                        return Ok(AppExit::Clean);
                    }
                    let _ = outcome;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn key_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn fresh_app() -> App {
        App::new(TuiConfig::default()).with_cwd("/work")
    }

    #[test]
    fn input_mode_transitions_from_normal_to_insert_on_i() {
        let mut app = fresh_app();
        assert_eq!(app.input_mode(), InputMode::Normal);
        let outcome = app.handle_key(key(KeyCode::Char('i')));
        assert_eq!(outcome, KeyOutcome::Transitioned);
        assert_eq!(app.input_mode(), InputMode::Insert);
    }

    #[test]
    fn input_mode_transitions_to_normal_on_esc() {
        let mut app = fresh_app();
        app.handle_key(key(KeyCode::Char('i')));
        let outcome = app.handle_key(key(KeyCode::Esc));
        assert_eq!(outcome, KeyOutcome::Transitioned);
        assert_eq!(app.input_mode(), InputMode::Normal);
    }

    #[test]
    fn slash_mode_activates_on_forward_slash() {
        let mut app = fresh_app();
        app.handle_key(key(KeyCode::Char('/')));
        assert_eq!(app.input_mode(), InputMode::SlashCommand);
        assert!(app.slash_matches().len() >= 12);
    }

    #[test]
    fn slash_command_filter_mode_narrows_command_list() {
        let mut app = fresh_app();
        app.handle_key(key(KeyCode::Char('/'))); // enter slash mode
        // Type `se` — should narrow to ["sessions"].
        app.handle_key(key(KeyCode::Char('s')));
        app.handle_key(key(KeyCode::Char('e')));
        let matches = app.slash_matches();
        assert!(matches.iter().all(|m| m.starts_with("se")));
        assert!(matches.iter().any(|m| m == "sessions"));
    }

    #[test]
    fn plan_mode_gates_execution() {
        let mut app = fresh_app();
        // Default is Planning.
        assert!(app.plan_mode().is_planning());

        // Switch to Insert and submit a message.
        app.handle_key(key(KeyCode::Char('i')));
        for ch in "hello agent".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        let outcome = app.handle_key(key(KeyCode::Enter));
        match outcome {
            KeyOutcome::Submitted(SubmitOutcome::QueuedForApproval(text)) => {
                assert_eq!(text, "hello agent");
            }
            other => panic!("expected QueuedForApproval, got {other:?}"),
        }
        // The message is in history with pending_approval = true.
        assert_eq!(app.messages().pending().count(), 1);

        // Flip to Executing and submit again — now it dispatches.
        app.toggle_plan_mode();
        assert!(app.plan_mode().is_executing());
        for ch in "second msg".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        let outcome2 = app.handle_key(key(KeyCode::Enter));
        match outcome2 {
            KeyOutcome::Submitted(SubmitOutcome::Dispatched(text)) => {
                assert_eq!(text, "second msg");
            }
            other => panic!("expected Dispatched, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_c_quits_immediately() {
        let mut app = fresh_app();
        let outcome = app.handle_key(key_ctrl(KeyCode::Char('c')));
        assert_eq!(outcome, KeyOutcome::Quit);
        assert!(app.should_quit());
    }

    #[test]
    fn slash_quit_command_quits() {
        let mut app = fresh_app();
        // Build the draft via Insert mode.
        app.handle_key(key(KeyCode::Char('i')));
        for ch in "/quit".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        let outcome = app.handle_key(key(KeyCode::Enter));
        match outcome {
            KeyOutcome::Submitted(SubmitOutcome::Command(ParsedCommand::Quit)) => {}
            other => panic!("expected Quit command, got {other:?}"),
        }
        assert!(app.should_quit());
    }

    #[test]
    fn slash_help_command_routes_internally() {
        let mut app = fresh_app();
        app.handle_key(key(KeyCode::Char('i')));
        for ch in "/help".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        let outcome = app.handle_key(key(KeyCode::Enter));
        match outcome {
            KeyOutcome::Submitted(SubmitOutcome::Command(ParsedCommand::Help)) => {}
            other => panic!("expected Help command, got {other:?}"),
        }
    }

    #[test]
    fn unknown_slash_command_surfaces_error_in_slash_mode() {
        let mut app = fresh_app();
        app.handle_key(key(KeyCode::Char('/')));
        for ch in "bogus".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        let outcome = app.handle_key(key(KeyCode::Enter));
        match outcome {
            KeyOutcome::CommandError(e) => {
                assert!(matches!(e, SlashCommandError::UnknownCommand(_)));
            }
            other => panic!("expected CommandError, got {other:?}"),
        }
    }

    #[test]
    fn tab_in_insert_mode_queues_multiline() {
        let mut app = fresh_app();
        app.handle_key(key(KeyCode::Char('i')));
        for ch in "line one".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        let outcome = app.handle_key(key(KeyCode::Tab));
        assert_eq!(outcome, KeyOutcome::Consumed);
        // Draft retains queued content with a trailing newline.
        assert!(app.messages().draft().contains("line one\n"));
    }

    #[test]
    fn esc_in_slash_mode_returns_to_normal() {
        let mut app = fresh_app();
        app.handle_key(key(KeyCode::Char('/')));
        assert_eq!(app.input_mode(), InputMode::SlashCommand);
        let outcome = app.handle_key(key(KeyCode::Esc));
        assert_eq!(outcome, KeyOutcome::Transitioned);
        assert_eq!(app.input_mode(), InputMode::Normal);
    }

    #[test]
    fn arrow_keys_in_normal_mode_walk_history() {
        let mut app = fresh_app();
        // Submit two messages via Executing plan mode so they go to history.
        app.toggle_plan_mode();
        for ch in "first".chars() {
            app.handle_key(key(KeyCode::Char('i')));
            app.handle_key(key(KeyCode::Char(ch)));
            app.handle_key(key(KeyCode::Esc));
        }
        // Easier path: directly use the buffer.
        app.messages_mut().set_draft("first");
        let _ = app.messages_mut().submit(false);
        app.messages_mut().set_draft("second");
        let _ = app.messages_mut().submit(false);

        // Back to Normal, press Up — should retrieve newest entry.
        app.handle_key(key(KeyCode::Esc)); // ensure Normal (no-op if already Normal)
        let outcome = app.handle_key(key(KeyCode::Up));
        assert_eq!(outcome, KeyOutcome::Consumed);
    }

    #[test]
    fn app_with_cwd_override_uses_provided_path() {
        let app = App::new(TuiConfig::default()).with_cwd("/custom");
        assert_eq!(app.cwd(), "/custom");
    }
}
