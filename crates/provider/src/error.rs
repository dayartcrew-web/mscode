//! Error type for the provider crate.
//!
//! Mirrors the four-bucket taxonomy used by `multi-account-core-rs`'s
//! `ProviderAdapter::classify_error` so callers can drive their own
//! retry/rotation policy from a single classification. Each HTTP-level
//! failure is stored alongside its bucket so the original context is never
//! lost.

use thiserror::Error;

/// Failures raised by [`crate::LlmProvider`] implementations.
#[derive(Debug, Clone, Error)]
pub enum ProviderError {
    /// Authentication failed (HTTP 401, 403). The caller should rotate to a
    /// different account if rotation is available.
    #[error("auth error ({status}): {detail}")]
    Auth {
        /// Raw HTTP status code.
        status: u16,
        /// Provider-supplied error detail.
        detail: String,
    },

    /// Provider rate-limited the request (HTTP 429). The caller should cool
    /// the account down before retrying or rotating.
    #[error("rate limited ({status}): {detail}")]
    RateLimit {
        /// Raw HTTP status code.
        status: u16,
        /// Provider-supplied error detail.
        detail: String,
    },

    /// Transient server or network failure (HTTP 5xx, connection reset, etc.).
    /// The caller may retry the same account with backoff.
    #[error("transient error ({status}): {detail}")]
    Transient {
        /// Raw HTTP status code, or `0` when no response was received.
        status: u16,
        /// Provider-supplied or transport-level error detail.
        detail: String,
    },

    /// Permanent client-side failure (HTTP 4xx other than 401/403/429). The
    /// caller should surface this error and not retry.
    #[error("fatal error ({status}): {detail}")]
    Fatal {
        /// Raw HTTP status code.
        status: u16,
        /// Provider-supplied error detail.
        detail: String,
    },

    /// Response body could not be parsed into the expected shape.
    #[error("decode error: {0}")]
    Decode(String),

    /// SSE stream contained malformed framing or an unknown event type.
    #[error("stream parse error: {0}")]
    StreamParse(String),

    /// The caller asked for tool-use but the provider does not support it.
    #[error("tool use requested but provider {provider} does not support tools")]
    ToolUnsupported {
        /// Human-readable provider name (matches [`crate::LlmProvider::name`]).
        provider: String,
    },

    /// The configured endpoint URL was malformed.
    #[error("invalid endpoint URL: {0}")]
    InvalidEndpoint(String),

    /// Configuration is missing a required value (e.g. API key).
    #[error("missing configuration: {0}")]
    MissingConfig(String),
}

impl ProviderError {
    /// Returns the [`ErrorKind`] bucket for this error.
    ///
    /// Useful for routing decisions in callers that already speak the
    /// four-bucket taxonomy (rotate / cooldown / retry / surface).
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Auth { .. } => ErrorKind::Auth,
            Self::RateLimit { .. } => ErrorKind::RateLimit,
            Self::Transient { .. } => ErrorKind::Transient,
            Self::Fatal { .. }
            | Self::Decode(_)
            | Self::StreamParse(_)
            | Self::ToolUnsupported { .. }
            | Self::InvalidEndpoint(_)
            | Self::MissingConfig(_) => ErrorKind::Fatal,
        }
    }

    /// Construct an error from a raw HTTP status and body slice using the
    /// same mapping the provider adapters apply.
    pub fn from_http_status(status: u16, body: &[u8]) -> Self {
        let detail = std::str::from_utf8(body)
            .unwrap_or("<non-utf8 body>")
            .trim()
            .to_owned();
        let detail = if detail.is_empty() {
            format!("HTTP {status}")
        } else {
            detail
        };
        match status {
            401 | 403 => Self::Auth { status, detail },
            429 => Self::RateLimit { status, detail },
            500 | 502 | 503 | 504 | 529 => Self::Transient { status, detail },
            _ => Self::Fatal { status, detail },
        }
    }
}

/// Coarse classification of provider HTTP failures.
///
/// Mirrors `multi_account_core::provider::ErrorKind` so any caller that
/// previously consumed that enum can reuse the same routing logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Authentication failed (HTTP 401, 403).
    Auth,
    /// Provider rate-limited the request (HTTP 429).
    RateLimit,
    /// Transient server or network failure (HTTP 5xx).
    Transient,
    /// Permanent client-side failure (HTTP 4xx other than 401/403/429).
    Fatal,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_http_status_classifies_auth() {
        assert_eq!(
            ProviderError::from_http_status(401, b"bad token").kind(),
            ErrorKind::Auth
        );
        assert_eq!(
            ProviderError::from_http_status(403, b"forbidden").kind(),
            ErrorKind::Auth
        );
    }

    #[test]
    fn from_http_status_classifies_rate_limit() {
        assert_eq!(
            ProviderError::from_http_status(429, b"slow down").kind(),
            ErrorKind::RateLimit
        );
    }

    #[test]
    fn from_http_status_classifies_transient() {
        for code in [500u16, 502, 503, 504, 529] {
            assert_eq!(
                ProviderError::from_http_status(code, b"oops").kind(),
                ErrorKind::Transient,
                "code {code} should map to Transient"
            );
        }
    }

    #[test]
    fn from_http_status_classifies_fatal() {
        for code in [400u16, 404, 422] {
            assert_eq!(
                ProviderError::from_http_status(code, b"bad request").kind(),
                ErrorKind::Fatal,
                "code {code} should map to Fatal"
            );
        }
    }

    #[test]
    fn from_http_status_handles_empty_and_non_utf8_body() {
        let e = ProviderError::from_http_status(400, b"");
        assert!(matches!(e, ProviderError::Fatal { status: 400, .. }));

        let e = ProviderError::from_http_status(400, &[0xff, 0xfe]);
        assert!(matches!(e, ProviderError::Fatal { .. }));
    }

    #[test]
    fn kind_mapping_covers_all_variants() {
        assert_eq!(
            ProviderError::Auth {
                status: 401,
                detail: "x".into()
            }
            .kind(),
            ErrorKind::Auth
        );
        assert_eq!(
            ProviderError::RateLimit {
                status: 429,
                detail: "x".into()
            }
            .kind(),
            ErrorKind::RateLimit
        );
        assert_eq!(
            ProviderError::Transient {
                status: 503,
                detail: "x".into()
            }
            .kind(),
            ErrorKind::Transient
        );
        assert_eq!(
            ProviderError::Fatal {
                status: 400,
                detail: "x".into()
            }
            .kind(),
            ErrorKind::Fatal
        );
        assert_eq!(ProviderError::Decode("x".into()).kind(), ErrorKind::Fatal);
        assert_eq!(
            ProviderError::StreamParse("x".into()).kind(),
            ErrorKind::Fatal
        );
        assert_eq!(
            ProviderError::ToolUnsupported {
                provider: "x".into()
            }
            .kind(),
            ErrorKind::Fatal
        );
        assert_eq!(
            ProviderError::InvalidEndpoint("x".into()).kind(),
            ErrorKind::Fatal
        );
        assert_eq!(
            ProviderError::MissingConfig("x".into()).kind(),
            ErrorKind::Fatal
        );
    }

    #[test]
    fn error_kind_variants_are_distinct() {
        let variants = [
            ErrorKind::Auth,
            ErrorKind::RateLimit,
            ErrorKind::Transient,
            ErrorKind::Fatal,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn display_messages_are_non_empty() {
        let cases = [
            ProviderError::Auth {
                status: 401,
                detail: "d".into(),
            },
            ProviderError::RateLimit {
                status: 429,
                detail: "d".into(),
            },
            ProviderError::Transient {
                status: 0,
                detail: "d".into(),
            },
            ProviderError::Fatal {
                status: 400,
                detail: "d".into(),
            },
            ProviderError::Decode("d".into()),
            ProviderError::StreamParse("d".into()),
            ProviderError::ToolUnsupported {
                provider: "p".into(),
            },
            ProviderError::InvalidEndpoint("d".into()),
            ProviderError::MissingConfig("d".into()),
        ];
        for e in &cases {
            assert!(!e.to_string().is_empty());
        }
    }
}
