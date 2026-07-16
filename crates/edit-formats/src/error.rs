//! Error type for the edit-formats crate.

use thiserror::Error;

/// Errors returned by edit-format parsers and the apply step.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditFormatError {
    /// The model output could not be parsed under the requested format.
    #[error("parse error ({parser}): {reason}")]
    Parse {
        parser: &'static str,
        reason: String,
    },

    /// The search block could not be located in the existing file, even with
    /// fuzzy matching.
    #[error("search block not found in content")]
    SearchNotFound,

    /// Multiple candidate locations matched the search block and the apply
    /// step refused to guess.
    #[error("search block matched multiple ({count}) locations")]
    AmbiguousMatch { count: usize },

    /// The supplied JSON tool-call payload was malformed.
    #[error("invalid tool-use payload: {0}")]
    InvalidToolUse(String),
}

/// Result alias for edit-format operations.
pub type EditFormatResult<T> = std::result::Result<T, EditFormatError>;
