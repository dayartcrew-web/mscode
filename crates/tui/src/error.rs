//! Error type for the TUI crate.
//!
//! Domain-specific failures are wrapped with `#[from]` so `?` works at every
//! call site. Internal error details are never serialized into a response —
//! this enum is for the binary and tests only.

use thiserror::Error;

/// All failures raised by the TUI layer.
#[derive(Debug, Error)]
pub enum TuiError {
    /// Terminal setup failed (raw-mode enable, alt-screen enter, etc.).
    #[error("terminal init failed: {0}")]
    TerminalInit(String),

    /// Underlying stdlib I/O failure (read event, write frame, etc.).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Ratatui rendering failed.
    #[error("render error: {0}")]
    Render(String),

    /// Configuration was malformed (bad keybinding, invalid theme, etc.).
    #[error("config error: {0}")]
    Config(String),

    /// Session lookup failed (not found, ambiguous prefix, store error).
    #[error("session lookup failed: {0}")]
    SessionLookup(String),
}

/// Result alias.
pub type Result<T> = std::result::Result<T, TuiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_error_from_io_preserves_message() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let err: TuiError = io_err.into();
        assert!(matches!(err, TuiError::Io(_)));
        assert!(err.to_string().contains("io error"));
        assert!(err.to_string().contains("missing file"));
    }

    #[test]
    fn terminal_init_variant_formats() {
        let err = TuiError::TerminalInit("no tty".into());
        assert!(err.to_string().contains("terminal init failed"));
        assert!(err.to_string().contains("no tty"));
    }

    #[test]
    fn render_variant_formats() {
        let err = TuiError::Render("frame overflow".into());
        assert!(err.to_string().contains("render error"));
    }

    #[test]
    fn config_variant_formats() {
        let err = TuiError::Config("bad keybinding".into());
        assert!(err.to_string().contains("config error"));
    }

    #[test]
    fn session_lookup_variant_formats() {
        let err = TuiError::SessionLookup("not found: deadbeef".into());
        assert!(err.to_string().contains("session lookup failed"));
        assert!(err.to_string().contains("deadbeef"));
    }
}
