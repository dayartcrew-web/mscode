//! Canonical error type for the mscode workspace.
//!
//! Every domain crate converts its internal errors into [`MscodeError`] at the
//! boundary so callers get a single, exhaustive match target. The
//! [`Other`](MscodeError::Other) variant wraps [`anyhow::Error`] for ad-hoc
//! plumbing where a dedicated variant would be premature.

use thiserror::Error;

/// The one error type used across the mscode workspace.
///
/// Domain-specific failures map onto a single variant; the wrapped payload is
/// responsible for the rich detail. Convert with `?` using the `From`
/// impls provided below.
#[derive(Debug, Error)]
pub enum MscodeError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Database(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("dag error: {0}")]
    Dag(String),

    #[error("memory error: {0}")]
    Memory(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Workspace-standard `Result` alias.
pub type Result<T> = std::result::Result<T, MscodeError>;

impl PartialEq for MscodeError {
    // Two errors are equal iff they share a variant and the rendered message
    // matches. This keeps equality test-friendly without requiring inner
    // types (e.g. `std::io::Error`) to be `PartialEq`.
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string() && fmt_variant_name(self) == fmt_variant_name(other)
    }
}

impl Eq for MscodeError {}

fn fmt_variant_name(err: &MscodeError) -> &'static str {
    match err {
        MscodeError::Config(_) => "Config",
        MscodeError::Io(_) => "Io",
        MscodeError::Database(_) => "Database",
        MscodeError::Provider(_) => "Provider",
        MscodeError::Session(_) => "Session",
        MscodeError::Tool(_) => "Tool",
        MscodeError::Plugin(_) => "Plugin",
        MscodeError::Dag(_) => "Dag",
        MscodeError::Memory(_) => "Memory",
        MscodeError::Other(_) => "Other",
    }
}

/// Helper for ergonomic ad-hoc errors at call sites that lack a domain variant.
///
/// Equivalent to `MscodeError::Other(anyhow::anyhow!(...))` but shorter.
#[macro_export]
macro_rules! mscode_other {
    ($($args:tt)*) => {
        $crate::error::MscodeError::Other(anyhow::anyhow!($($args)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_roundtrip_config_variant() {
        let err = MscodeError::Config("missing field `provider`".into());
        assert_eq!(err.to_string(), "config error: missing field `provider`");
    }

    #[test]
    fn display_roundtrip_io_variant() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let err = MscodeError::Io(io);
        assert!(err.to_string().contains("io error"));
        assert!(err.to_string().contains("no such file"));
    }

    #[test]
    fn display_roundtrip_other_variant() {
        let err = MscodeError::Other(anyhow::anyhow!("boom: {x}", x = 7));
        assert_eq!(err.to_string(), "boom: 7");
    }

    #[test]
    fn debug_format_includes_variant_name() {
        let err = MscodeError::Provider("rate limited".into());
        let s = format!("{err:?}");
        assert!(s.contains("Provider"));
        assert!(s.contains("rate limited"));
    }

    #[test]
    fn from_io_error_via_question_operator() {
        fn fallible() -> Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "nope",
            ))?;
            Ok(())
        }
        let err = fallible().unwrap_err();
        assert!(matches!(err, MscodeError::Io(_)));
    }

    #[test]
    fn from_anyhow_error_via_question_operator() {
        fn fallible() -> Result<()> {
            Err(anyhow::anyhow!("custom failure"))?;
            Ok(())
        }
        let err = fallible().unwrap_err();
        assert!(matches!(err, MscodeError::Other(_)));
        assert_eq!(err.to_string(), "custom failure");
    }

    #[test]
    fn partial_eq_compares_message_and_variant() {
        let a = MscodeError::Session("not found".into());
        let b = MscodeError::Session("not found".into());
        let c = MscodeError::Tool("not found".into());
        let d = MscodeError::Session("different".into());
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn macro_constructs_other_variant() {
        let err: MscodeError = mscode_other!("computed {v}", v = 42);
        assert_eq!(err.to_string(), "computed 42");
        assert!(matches!(err, MscodeError::Other(_)));
    }

    #[test]
    fn fmt_variant_name_covers_every_variant() {
        for variant in [
            MscodeError::Config("x".into()),
            MscodeError::Io(std::io::Error::other("x")),
            MscodeError::Database("x".into()),
            MscodeError::Provider("x".into()),
            MscodeError::Session("x".into()),
            MscodeError::Tool("x".into()),
            MscodeError::Plugin("x".into()),
            MscodeError::Dag("x".into()),
            MscodeError::Memory("x".into()),
            MscodeError::Other(anyhow::anyhow!("x")),
        ] {
            assert!(!fmt_variant_name(&variant).is_empty());
        }
    }

    // Quiet clippy about `fmt` not being used directly in production code.
    #[test]
    fn debug_format_for_every_variant_does_not_panic() {
        // Sanity check that `Debug` is implemented on every variant by
        // rendering each one through `format!("{:?}", ...)` and ensuring the
        // rendered form is non-empty.
        let variants: [MscodeError; 10] = [
            MscodeError::Config("x".into()),
            MscodeError::Io(std::io::Error::other("x")),
            MscodeError::Database("x".into()),
            MscodeError::Provider("x".into()),
            MscodeError::Session("x".into()),
            MscodeError::Tool("x".into()),
            MscodeError::Plugin("x".into()),
            MscodeError::Dag("x".into()),
            MscodeError::Memory("x".into()),
            MscodeError::Other(anyhow::anyhow!("x")),
        ];
        for v in variants {
            assert!(!format!("{v:?}").is_empty());
        }
    }
}
