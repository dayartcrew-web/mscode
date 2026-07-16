//! Error type for the sandbox crate.

use thiserror::Error;

/// Errors returned by [`crate::Sandbox`] policy checks.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SandboxError {
    /// Path contains a `..` segment, which is always treated as an escape attempt.
    #[error("path escapes workspace via `..` segment: {0}")]
    DotDotEscape(String),

    /// Path resolves outside the workspace and outside any allow-listed read root.
    #[error("path is outside the workspace: {0}")]
    OutsideWorkspace(String),

    /// Path is on the deny list (matched a configured deny glob).
    #[error("path is explicitly denied: {0}")]
    Denied(String),

    /// File extension is not on the allowed list.
    #[error("file extension not allowed: {0}")]
    ExtensionDenied(String),

    /// File exceeds the configured size cap.
    #[error("file exceeds max size ({limit} bytes): {actual} bytes")]
    FileTooLarge { limit: u64, actual: u64 },

    /// Command is not on the exec allowlist.
    #[error("command not allowed: {0}")]
    ExecDenied(String),

    /// Command could not be parsed into argv[0].
    #[error("could not extract command name from: {0}")]
    InvalidCommand(String),
}

/// Result alias for sandbox operations.
pub type SandboxResult<T> = std::result::Result<T, SandboxError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_descriptive() {
        assert_eq!(
            SandboxError::DotDotEscape("../foo".into()).to_string(),
            "path escapes workspace via `..` segment: ../foo"
        );
        assert_eq!(
            SandboxError::ExecDenied("rm".into()).to_string(),
            "command not allowed: rm"
        );
        assert_eq!(
            SandboxError::FileTooLarge {
                limit: 100,
                actual: 200
            }
            .to_string(),
            "file exceeds max size (100 bytes): 200 bytes"
        );
    }
}
