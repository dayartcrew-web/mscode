//! Glob-style path matcher used by the sandbox deny list.
//!
//! Wraps the [`glob::Pattern`] API behind a small newtype so callers don't
//! take a direct dependency on `glob`, and so the matcher can be extended
//! later (e.g. gitignore-style semantics) without changing call sites.

use std::str::FromStr;

use glob::MatchOptions;
use thiserror::Error;

/// Errors that can occur while constructing a [`PathMatcher`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MatcherError {
    /// The glob pattern failed to parse.
    #[error("invalid glob pattern `{pattern}`: {reason}")]
    InvalidPattern { pattern: String, reason: String },
}

/// A compiled glob pattern suitable for matching file paths against deny lists.
///
/// Patterns are matched against the *string form* of the path, using forward
/// slashes as separators regardless of platform. Callers should normalize
/// paths before matching.
#[derive(Debug, Clone)]
pub struct PathMatcher {
    pattern: glob::Pattern,
    raw: String,
}

impl PathMatcher {
    /// Construct a new matcher, compiling the glob eagerly.
    pub fn new(pattern: impl Into<String>) -> Result<Self, MatcherError> {
        let raw = pattern.into();
        let pattern = glob::Pattern::from_str(&raw).map_err(|e| MatcherError::InvalidPattern {
            pattern: raw.clone(),
            reason: e.to_string(),
        })?;
        Ok(Self { pattern, raw })
    }

    /// The original glob source.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Returns `true` if the candidate path matches this glob.
    ///
    /// Matching uses [`MatchOptions`] with `require_literal_separator = true`
    /// so `*` does **not** match `/`. Callers that need cross-segment globs
    /// should use `**` explicitly (e.g. `**/.env*`).
    pub fn matches(&self, candidate: &str) -> bool {
        self.pattern.matches_with(
            candidate,
            MatchOptions {
                case_sensitive: true,
                require_literal_separator: true,
                require_literal_leading_dot: false,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_valid_pattern() {
        let m = PathMatcher::new("**/.env*").unwrap();
        assert_eq!(m.as_str(), "**/.env*");
    }

    #[test]
    fn rejects_invalid_pattern() {
        // `glob` is permissive, but unclosed character classes are rejected.
        let err = PathMatcher::new("[unclosed").unwrap_err();
        assert!(matches!(err, MatcherError::InvalidPattern { .. }));
    }

    #[test]
    fn matches_recursive() {
        let m = PathMatcher::new("**/.env*").unwrap();
        assert!(m.matches("config/.env"));
        assert!(m.matches("config/subdir/.env.local"));
        assert!(!m.matches("config/README.md"));
    }

    #[test]
    fn matches_single_segment() {
        let m = PathMatcher::new("*.log").unwrap();
        assert!(m.matches("debug.log"));
        assert!(!m.matches("subdir/debug.log"));
    }
}
