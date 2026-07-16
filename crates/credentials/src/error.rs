//! Error types for the credentials crate.

use thiserror::Error;

/// Result alias for all credential operations.
pub type Result<T> = std::result::Result<T, CredentialError>;

/// Errors emitted by the credential store.
///
/// Variants follow the security rules in `~/.claude/rules/rust/security.md`:
/// messages never leak secret material; IO failures name the operation but
/// not the underlying key value.
#[derive(Debug, Error)]
pub enum CredentialError {
    /// SQLite operation failed (metadata read/write).
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// `AppState` infrastructure failure (pool acquisition, migration).
    #[error("state error: {0}")]
    State(#[from] mscode_state::StateError),

    /// OS keyring backend rejected the operation. The string identifies the
    /// operation (`"store"`, `"load"`, `"delete"`) and the underlying cause
    /// without leaking the secret value.
    #[error("keyring {operation} failed: {source}")]
    Keyring {
        operation: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// OS keyring is entirely unavailable (e.g. headless Linux without
    /// Secret Service, or Windows DPAPI disabled). Surfaced distinctly so the
    /// CLI can print an actionable message rather than a generic IO error.
    #[error("OS keyring unavailable; install gnome-keyring or set MSCODE_CREDENTIALS_FILE")]
    KeyringUnavailable,

    /// Internal mutex poisoned by a panicking writer. Distinct from
    /// [`CredentialError::Keyring`] so callers don't confuse a concurrency bug
    /// with a storage failure.
    #[error("keyring mutex poisoned")]
    KeyringPoisoned,

    /// `(provider, label)` pair not found.
    #[error("no account found for provider `{provider}` label `{label}`")]
    NotFound { provider: String, label: String },

    /// `(provider, label)` already exists. Callers must `remove` first.
    #[error("account already exists for provider `{provider}` label `{label}`")]
    Duplicate { provider: String, label: String },

    /// Validation failure (bad provider name, malformed endpoint, empty key).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// No account is eligible for the provider (all in cooldown, or store
    /// is empty). Distinct from `NotFound` so callers can branch on
    /// "nothing configured" vs "everything cooldown'd".
    #[error("no eligible account for provider `{provider}`")]
    NoEligible { provider: String },

    /// Bug: a row in `provider_accounts` references a `key_id` that the
    /// keyring does not contain. Indicates partial write or external
    /// tampering.
    #[error("key with id `{key_id}` missing from keyring (row {provider}/{label})")]
    OrphanedKey {
        provider: String,
        label: String,
        key_id: String,
    },

    /// Serialization failure on the metadata JSON column.
    #[error("metadata serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Convenience converter so `?` works on validator functions returning
/// `Result<(), String>`.
impl From<String> for CredentialError {
    fn from(s: String) -> Self {
        CredentialError::InvalidInput(s)
    }
}
