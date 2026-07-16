//! Declare-only clap structures for the mscode CLI.
//!
//! This crate follows the uv pattern: it owns the clap definitions and nothing
//! else. No async runtime, no I/O, no resolver wiring. A later phase introduces
//! the `mscode-core` crate which performs actual command resolution and
//! dispatch; until then, [`Cli::parse`] is invoked by the thin binary at
//! `src/bin/mscode.rs` for the `version` and `--version` fast paths.
//!
//! ## Phase 7 subcommand shapes
//!
//! - [`Commands::New`] — no positional args at v1 (optional overrides will
//!   arrive in a later phase).
//! - [`Commands::Chat`] — optional session id / prefix; `None` means "use the
//!   current active session" (or open a new one).
//! - [`Commands::Resume`] — required session id or prefix (portable-by-ID).
//! - [`Commands::Sessions`] — `--all` flag disables the cwd soft filter.

use clap::{Parser, Subcommand};

/// Top-level CLI definition.
///
/// Derive-only — do not introduce side effects here. Resolution happens in a
/// downstream crate.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "mscode",
    about = "Local-first agentic CLI",
    version,
    long_about = None,
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Increase verbosity (-v info, -vv debug, -vvv trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Decrease verbosity (errors only; repeat for silent).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub quiet: u8,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Subcommand surface for v1.
///
/// Phase 7 wires all four subcommands to live implementations in the
/// `mscode-thread-store` and `mscode-tui` crates. Only [`Commands::Version`]
/// runs without an async runtime.
#[derive(Debug, Clone, Subcommand)]
pub enum Commands {
    /// Print build and version information.
    Version,

    /// Create a new session and print its id to stdout.
    New {
        // Reserved for future config overrides; intentionally empty at v1 so
        // the help output stays stable.
    },

    /// Launch the TUI against an existing or new session.
    Chat {
        /// Optional session id (full or prefix). When omitted, the chat
        /// command launches against the current active session, or creates a
        /// fresh one if none exists.
        session: Option<String>,
    },

    /// Resume a previous session by id (full UUID or unambiguous prefix).
    Resume {
        /// Session id or prefix. Resolution is portable — works from any cwd.
        id: String,
    },

    /// List previous sessions.
    ///
    /// Defaults to the current working directory; pass `--all` to disable the
    /// cwd filter (sessions are portable by id).
    Sessions {
        /// Disable the cwd filter.
        #[arg(long)]
        all: bool,
    },
}

impl Cli {
    /// Convenience: returns the parsed command if the user supplied one,
    /// otherwise `None`.
    pub fn command_or_default(&self) -> Option<&Commands> {
        self.command.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn parse_version_subcommand() {
        let cli = Cli::parse_from(["mscode", "version"]);
        assert!(matches!(cli.command, Some(Commands::Version)));
    }

    #[test]
    fn parse_no_subcommand_yields_none() {
        let cli = Cli::parse_from(["mscode"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parse_chat_with_session_arg() {
        let cli = Cli::parse_from(["mscode", "chat", "deadbeef"]);
        match cli.command {
            Some(Commands::Chat { session }) => {
                assert_eq!(session.as_deref(), Some("deadbeef"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_chat_without_session_arg() {
        let cli = Cli::parse_from(["mscode", "chat"]);
        match cli.command {
            Some(Commands::Chat { session }) => assert!(session.is_none()),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_sessions_all_flag() {
        let cli = Cli::parse_from(["mscode", "sessions", "--all"]);
        match cli.command {
            Some(Commands::Sessions { all }) => assert!(all),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_sessions_default_is_cwd_filtered() {
        let cli = Cli::parse_from(["mscode", "sessions"]);
        match cli.command {
            Some(Commands::Sessions { all }) => assert!(!all),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_resume_with_id() {
        let cli = Cli::parse_from(["mscode", "resume", "deadbeef"]);
        match cli.command {
            Some(Commands::Resume { id }) => assert_eq!(id, "deadbeef"),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parse_resume_requires_id() {
        // clap should reject `mscode resume` with no positional.
        let result = Cli::try_parse_from(["mscode", "resume"]);
        assert!(result.is_err(), "resume without id should fail to parse");
    }

    #[test]
    fn parse_new_takes_no_args() {
        let cli = Cli::parse_from(["mscode", "new"]);
        assert!(matches!(cli.command, Some(Commands::New {})));
    }

    #[test]
    fn parse_verbose_flags_are_global() {
        let cli = Cli::parse_from(["mscode", "-vv", "version"]);
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    fn command_or_default_returns_ref() {
        let cli = Cli::parse_from(["mscode", "version"]);
        assert!(matches!(cli.command_or_default(), Some(Commands::Version)));
    }

    #[test]
    fn help_subcommand_is_disabled_via_attribute() {
        // The `disable_help_subcommand = true` attribute is enforced at the
        // derive level; verify the struct exposes it via the command metadata.
        let cmd = Cli::command();
        let has_help_sub = cmd.get_subcommands().any(|s| s.get_name() == "help");
        assert!(
            !has_help_sub,
            "`help` should NOT be a registered subcommand"
        );
    }
}
