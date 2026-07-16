//! Slash-command parser.
//!
//! [`parse_slash_command`] accepts a single line of input. Only lines whose
//! first non-whitespace character is `/` are treated as command attempts; all
//! other lines return `None` (the caller treats them as free-text messages).
//!
//! # Supported commands (EXACTLY 13)
//!
//! | Command          | Variant                       |
//! | ---------------- | ----------------------------- |
//! | `/help`          | [`ParsedCommand::Help`]       |
//! | `/sessions`      | [`ParsedCommand::Sessions`]   |
//! | `/resume <id>`   | [`ParsedCommand::Resume`]     |
//! | `/new`           | [`ParsedCommand::New`]        |
//! | `/compact`       | [`ParsedCommand::Compact`]    |
//! | `/model <name>`  | [`ParsedCommand::Model`]      |
//! | `/models`        | [`ParsedCommand::Models`]     |
//! | `/config`        | [`ParsedCommand::Config`]     |
//! | `/plugin`        | [`ParsedCommand::Plugin`]     |
//! | `/memory`        | [`ParsedCommand::Memory`]     |
//! | `/export`        | [`ParsedCommand::Export`]     |
//! | `/clear`         | [`ParsedCommand::Clear`]      |
//! | `/quit`          | [`ParsedCommand::Quit`]       |
//!
//! Unknown `/foo` commands produce an [`Err(SlashCommandError)`] so the caller
//! can surface a helpful "did you mean?" message. The user is still free to
//! type arbitrary text into the prompt — only leading-`/` tokens are command
//! attempts.

/// All 13 supported slash commands, in the order they appear in `--help`.
pub const ALL_SLASH_COMMANDS: &[&str] = &[
    "help", "sessions", "resume", "new", "compact", "model", "models", "config", "plugin",
    "memory", "export", "clear", "quit",
];

/// Memory layer filter accepted by `/memory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryLayer {
    /// Per-session memory (scoped to the current thread).
    Session,
    /// Per-project memory (scoped to the current project root).
    Project,
    /// Per-user memory (cross-project, per-machine-user).
    User,
    /// Global memory (cross-user, cross-project).
    Global,
}

/// Successfully-parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    /// `/help` — list commands with descriptions.
    Help,
    /// `/sessions [--all]` — list sessions (cwd-soft-filtered by default).
    Sessions {
        /// Disable the cwd filter when `true`.
        all: bool,
    },
    /// `/resume <id>` — resume a session by ID or prefix.
    Resume {
        /// ID or prefix the user provided (trimmed, non-empty).
        id: String,
    },
    /// `/new` — start a new session.
    New,
    /// `/compact` — trigger context compaction.
    Compact,
    /// `/model <name>` — switch active model.
    Model {
        /// Model name (trimmed, non-empty).
        name: String,
    },
    /// `/models` — open fuzzy picker over logged-in providers' models. The
    /// resulting `Model { name: "{provider}/{model_id}" }` outcome is emitted
    /// by the app layer when the user picks a row.
    Models,
    /// `/config <key> [value]` — view (`value` is `None`) or set a config key.
    Config {
        /// Config key.
        key: String,
        /// Optional value.
        value: Option<String>,
    },
    /// `/plugin` — list/enable/disable plugins (no inline args at v1).
    Plugin,
    /// `/memory [layer]` — view/search memory.
    Memory {
        /// Layer filter (defaults to `Session`).
        layer: MemoryLayer,
    },
    /// `/export [path]` — export current session to markdown.
    Export {
        /// Optional target path; `None` means "default location".
        path: Option<String>,
    },
    /// `/clear` — clear screen / context.
    Clear,
    /// `/quit` — exit app (flushes state first).
    Quit,
}

/// Failure modes for the parser.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlashCommandError {
    /// The leading token was `/foo` but `foo` is not a known command.
    #[error("unknown command: /{0}")]
    UnknownCommand(String),

    /// A required argument was missing.
    #[error("missing argument: {0}")]
    MissingArgument(&'static str),

    /// An argument value was malformed.
    #[error("invalid argument {name}: {detail}")]
    InvalidArgument { name: &'static str, detail: String },
}

/// Parse a single line of input.
///
/// Returns:
/// - `Ok(Some(cmd))` — leading `/` token recognized, arguments parsed.
/// - `Ok(None)` — input is not a command attempt (treated as free text).
/// - `Err(_)` — leading `/` token was unrecognized OR a required argument is
///   missing/invalid. The caller should surface the error and let the user
///   edit the draft.
pub fn parse_slash_command(input: &str) -> Result<Option<ParsedCommand>, SlashCommandError> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') {
        return Ok(None);
    }
    let body = &trimmed[1..];
    let mut tokens = body.split_whitespace();
    let name = tokens.next().unwrap_or("");
    if name.is_empty() {
        // `/` alone is ambiguous; treat as unknown to nudge the user.
        return Err(SlashCommandError::UnknownCommand(String::new()));
    }

    let collected: Vec<String> = tokens.map(str::to_string).collect();

    match name {
        "help" => expect_no_args(&collected, "help").map(|_| Some(ParsedCommand::Help)),
        "sessions" => parse_sessions(&collected),
        "resume" => parse_resume(&collected),
        "new" => expect_no_args(&collected, "new").map(|_| Some(ParsedCommand::New)),
        "compact" => expect_no_args(&collected, "compact").map(|_| Some(ParsedCommand::Compact)),
        "model" => parse_model(&collected),
        "models" => expect_no_args(&collected, "models").map(|_| Some(ParsedCommand::Models)),
        "config" => parse_config(&collected),
        "plugin" => expect_no_args(&collected, "plugin").map(|_| Some(ParsedCommand::Plugin)),
        "memory" => parse_memory(&collected),
        "export" => parse_export(&collected),
        "clear" => expect_no_args(&collected, "clear").map(|_| Some(ParsedCommand::Clear)),
        "quit" => expect_no_args(&collected, "quit").map(|_| Some(ParsedCommand::Quit)),
        other => Err(SlashCommandError::UnknownCommand(other.to_string())),
    }
}

fn expect_no_args(args: &[String], cmd: &'static str) -> Result<(), SlashCommandError> {
    if !args.is_empty() {
        return Err(SlashCommandError::InvalidArgument {
            name: "extra_args",
            detail: format!("/{cmd} takes no arguments; got {}", args.len()),
        });
    }
    Ok(())
}

fn parse_sessions(args: &[String]) -> Result<Option<ParsedCommand>, SlashCommandError> {
    let mut all = false;
    for a in args {
        match a.as_str() {
            "--all" => all = true,
            other => {
                return Err(SlashCommandError::InvalidArgument {
                    name: "sessions",
                    detail: format!("unknown flag: {other}"),
                });
            }
        }
    }
    Ok(Some(ParsedCommand::Sessions { all }))
}

fn parse_resume(args: &[String]) -> Result<Option<ParsedCommand>, SlashCommandError> {
    let id = match args.first() {
        Some(s) if !s.is_empty() => s.clone(),
        _ => return Err(SlashCommandError::MissingArgument("id")),
    };
    if args.len() > 1 {
        return Err(SlashCommandError::InvalidArgument {
            name: "extra_args",
            detail: format!("/resume takes a single id; got {}", args.len()),
        });
    }
    Ok(Some(ParsedCommand::Resume { id }))
}

fn parse_model(args: &[String]) -> Result<Option<ParsedCommand>, SlashCommandError> {
    let name = match args.first() {
        Some(s) if !s.is_empty() => s.clone(),
        _ => return Err(SlashCommandError::MissingArgument("name")),
    };
    if args.len() > 1 {
        return Err(SlashCommandError::InvalidArgument {
            name: "extra_args",
            detail: format!("/model takes a single name; got {}", args.len()),
        });
    }
    Ok(Some(ParsedCommand::Model { name }))
}

fn parse_config(args: &[String]) -> Result<Option<ParsedCommand>, SlashCommandError> {
    let key = match args.first() {
        Some(s) if !s.is_empty() => s.clone(),
        _ => return Err(SlashCommandError::MissingArgument("key")),
    };
    let value = if args.len() >= 2 {
        Some(args[1..].join(" "))
    } else {
        None
    };
    Ok(Some(ParsedCommand::Config { key, value }))
}

fn parse_memory(args: &[String]) -> Result<Option<ParsedCommand>, SlashCommandError> {
    let layer = match args.first().map(String::as_str) {
        None | Some("") => MemoryLayer::Session,
        Some("session") => MemoryLayer::Session,
        Some("project") => MemoryLayer::Project,
        Some("user") => MemoryLayer::User,
        Some("global") => MemoryLayer::Global,
        Some(other) => {
            return Err(SlashCommandError::InvalidArgument {
                name: "layer",
                detail: format!("expected one of session|project|user|global; got `{other}`"),
            });
        }
    };
    if args.len() > 1 {
        return Err(SlashCommandError::InvalidArgument {
            name: "extra_args",
            detail: format!("/memory takes at most one layer; got {}", args.len()),
        });
    }
    Ok(Some(ParsedCommand::Memory { layer }))
}

fn parse_export(args: &[String]) -> Result<Option<ParsedCommand>, SlashCommandError> {
    if args.is_empty() {
        return Ok(Some(ParsedCommand::Export { path: None }));
    }
    if args.len() > 1 {
        return Err(SlashCommandError::InvalidArgument {
            name: "extra_args",
            detail: format!("/export takes at most one path; got {}", args.len()),
        });
    }
    Ok(Some(ParsedCommand::Export {
        path: Some(args[0].clone()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! ok_cmd {
        ($input:expr, $pat:pat $(if $guard:expr)? $(,)?) => {{
            let parsed = parse_slash_command($input).expect("parse should not error");
            match parsed {
                Some($pat) $(if $guard)? => {},
                other => panic!("expected {}, got {other:?}", stringify!($pat)),
            }
        }};
    }

    #[test]
    fn parse_slash_command_help() {
        ok_cmd!("/help", ParsedCommand::Help);
    }

    #[test]
    fn parse_slash_command_sessions() {
        ok_cmd!("/sessions", ParsedCommand::Sessions { all: false });
    }

    #[test]
    fn parse_slash_command_sessions_all_flag() {
        ok_cmd!("/sessions --all", ParsedCommand::Sessions { all: true });
    }

    #[test]
    fn parse_slash_command_resume_with_id() {
        ok_cmd!("/resume deadbeef", ParsedCommand::Resume { id } if id == "deadbeef");
    }

    #[test]
    fn parse_slash_command_new() {
        ok_cmd!("/new", ParsedCommand::New);
    }

    #[test]
    fn parse_slash_command_compact() {
        ok_cmd!("/compact", ParsedCommand::Compact);
    }

    #[test]
    fn parse_slash_command_model_with_name() {
        ok_cmd!("/model gpt-5", ParsedCommand::Model { name } if name == "gpt-5");
    }

    #[test]
    fn parse_slash_command_config_view() {
        ok_cmd!(
            "/config theme",
            ParsedCommand::Config { key, value } if key == "theme" && value.is_none()
        );
    }

    #[test]
    fn parse_slash_command_config_set() {
        ok_cmd!(
            "/config theme dark",
            ParsedCommand::Config { key, value } if key == "theme" && value.as_deref() == Some("dark")
        );
    }

    #[test]
    fn parse_slash_command_plugin() {
        ok_cmd!("/plugin", ParsedCommand::Plugin);
    }

    #[test]
    fn parse_slash_command_memory_default_layer() {
        ok_cmd!("/memory", ParsedCommand::Memory { layer } if layer == MemoryLayer::Session);
    }

    #[test]
    fn parse_slash_command_memory_with_layer() {
        ok_cmd!(
            "/memory project",
            ParsedCommand::Memory { layer } if layer == MemoryLayer::Project
        );
    }

    #[test]
    fn parse_slash_command_memory_rejects_bad_layer() {
        let err = parse_slash_command("/memory foo").unwrap_err();
        assert!(matches!(
            err,
            SlashCommandError::InvalidArgument { name: "layer", .. }
        ));
    }

    #[test]
    fn parse_slash_command_export_no_path() {
        ok_cmd!("/export", ParsedCommand::Export { path: None });
    }

    #[test]
    fn parse_slash_command_export_with_path() {
        ok_cmd!(
            "/export /tmp/out.md",
            ParsedCommand::Export { path } if path.as_deref() == Some("/tmp/out.md")
        );
    }

    #[test]
    fn parse_slash_command_clear() {
        ok_cmd!("/clear", ParsedCommand::Clear);
    }

    #[test]
    fn parse_slash_command_quit() {
        ok_cmd!("/quit", ParsedCommand::Quit);
    }

    #[test]
    fn parse_slash_command_rejects_unknown_command() {
        let err = parse_slash_command("/frobnicate").unwrap_err();
        match err {
            SlashCommandError::UnknownCommand(name) => assert_eq!(name, "frobnicate"),
            other => panic!("expected UnknownCommand, got {other:?}"),
        }
    }

    #[test]
    fn parse_slash_command_returns_none_for_plain_text() {
        assert!(parse_slash_command("hello world").unwrap().is_none());
        assert!(parse_slash_command("").unwrap().is_none());
    }

    #[test]
    fn parse_slash_command_handles_leading_whitespace() {
        ok_cmd!("   /help", ParsedCommand::Help);
        ok_cmd!("\t/help", ParsedCommand::Help);
    }

    #[test]
    fn parse_slash_command_resume_requires_id() {
        let err = parse_slash_command("/resume").unwrap_err();
        assert!(matches!(err, SlashCommandError::MissingArgument("id")));
    }

    #[test]
    fn parse_slash_command_model_requires_name() {
        let err = parse_slash_command("/model").unwrap_err();
        assert!(matches!(err, SlashCommandError::MissingArgument("name")));
    }

    #[test]
    fn parse_slash_command_config_requires_key() {
        let err = parse_slash_command("/config").unwrap_err();
        assert!(matches!(err, SlashCommandError::MissingArgument("key")));
    }

    #[test]
    fn parse_slash_command_help_rejects_extra_args() {
        let err = parse_slash_command("/help me").unwrap_err();
        assert!(matches!(err, SlashCommandError::InvalidArgument { .. }));
    }

    #[test]
    fn parse_slash_command_sessions_rejects_bad_flag() {
        let err = parse_slash_command("/sessions --bogus").unwrap_err();
        assert!(matches!(err, SlashCommandError::InvalidArgument { .. }));
    }

    #[test]
    fn parse_slash_command_models() {
        ok_cmd!("/models", ParsedCommand::Models);
    }

    #[test]
    fn parse_slash_command_models_rejects_args() {
        let err = parse_slash_command("/models openai").unwrap_err();
        assert!(matches!(err, SlashCommandError::InvalidArgument { .. }));
    }

    #[test]
    fn all_slash_commands_count_is_exactly_thirteen() {
        assert_eq!(ALL_SLASH_COMMANDS.len(), 13);
    }

    #[test]
    fn all_slash_commands_are_unique() {
        let mut sorted = ALL_SLASH_COMMANDS.to_vec();
        sorted.sort();
        let initial_len = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), initial_len, "duplicate slash command names");
    }

    #[test]
    fn bare_slash_is_unknown() {
        let err = parse_slash_command("/").unwrap_err();
        assert!(matches!(err, SlashCommandError::UnknownCommand(_)));
    }
}
