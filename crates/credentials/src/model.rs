//! Domain types for the credential store.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle status of a credential account.
///
/// Mirrors Hermes's three-bucket model: `active` (eligible), `cooldown`
/// (transiently ineligible, auto-recovers), `dead` (permanently ineligible
/// until explicit user action — used for revoked OAuth tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    /// Eligible for selection.
    Active,
    /// Temporarily ineligible until `cooldown_until`; auto-transitions to
    /// `Active` once `cooldown_until` passes.
    Cooldown,
    /// Permanently out of rotation. Set when the provider signals a terminal
    /// auth failure (`token_invalidated`, `token_revoked`, `invalid_grant`,
    /// `refresh_token_reused`). Clears only via explicit user action
    /// (`mscode login use` to re-validate, or `remove` + re-`add`).
    Dead,
}

impl AccountStatus {
    /// Returns `true` if the status permits selection at `now`, given an
    /// optional cooldown expiry timestamp.
    pub fn is_eligible_at(
        &self,
        now: DateTime<Utc>,
        cooldown_until: Option<DateTime<Utc>>,
    ) -> bool {
        match self {
            AccountStatus::Active => true,
            AccountStatus::Cooldown => match cooldown_until {
                Some(until) => now >= until,
                None => true,
            },
            AccountStatus::Dead => false,
        }
    }

    /// Stable string representation for SQLite column storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountStatus::Active => "active",
            AccountStatus::Cooldown => "cooldown",
            AccountStatus::Dead => "dead",
        }
    }

    /// Parse from the SQLite column representation.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "cooldown" => AccountStatus::Cooldown,
            "dead" => AccountStatus::Dead,
            _ => AccountStatus::Active,
        }
    }
}

/// Input for [`crate::store::CredentialStore::add`].
#[derive(Debug, Clone)]
pub struct NewAccount {
    /// Provider identifier (`openai`, `anthropic`, `ollama`, `openrouter`,
    /// or a custom namespaced id like `custom:together`).
    pub provider: String,
    /// User-chosen label (`work`, `personal`, etc.). Must be unique within
    /// the provider.
    pub label: String,
    /// Endpoint URL. Use the provider's default if `None`.
    pub endpoint: Option<String>,
    /// Plaintext API key. Will be stored in the OS keyring, never in SQLite.
    pub api_key: String,
    /// If `true`, mark this account as the default for its provider.
    pub set_default: bool,
}

impl NewAccount {
    /// Construct with required fields; endpoint and default flag optional.
    pub fn new(
        provider: impl Into<String>,
        label: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            label: label.into(),
            endpoint: None,
            api_key: api_key.into(),
            set_default: false,
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    pub fn with_default(mut self, default: bool) -> Self {
        self.set_default = default;
        self
    }
}

/// A persisted credential account. The secret bytes are intentionally absent;
/// load them via [`crate::store::CredentialStore::load_key`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAccount {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub endpoint: String,
    /// Stable UUID identifying the keyring entry. Renames (`label` change)
    /// do not invalidate the keyring lookup.
    pub key_id: String,
    pub is_default: bool,
    pub status: AccountStatus,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// Forward-compatible metadata bag (e.g. `{"source":"env","fingerprint":"sha256:..."}`
    /// for Hermes-style borrowed-secret tracking).
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl ProviderAccount {
    /// Returns `true` if this account is eligible for selection at `now`.
    pub fn is_eligible_at(&self, now: DateTime<Utc>) -> bool {
        self.status.is_eligible_at(now, self.cooldown_until)
    }
}

/// Validate a provider name. Allows lowercase ascii, digits, dashes, and the
/// `custom:` namespace prefix.
pub fn validate_provider(provider: &str) -> Result<(), String> {
    if provider.is_empty() {
        return Err("provider must not be empty".into());
    }
    if provider.len() > 64 {
        return Err("provider must be <= 64 chars".into());
    }
    let valid =
        |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == ':';
    if !provider.chars().all(valid) {
        return Err("provider must be lowercase ascii with optional '-' '_' ':'".into());
    }
    Ok(())
}

/// Validate a label. Allows ascii alphanumeric plus dash/underscore/dot.
pub fn validate_label(label: &str) -> Result<(), String> {
    if label.is_empty() {
        return Err("label must not be empty".into());
    }
    if label.len() > 64 {
        return Err("label must be <= 64 chars".into());
    }
    let valid = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.';
    if !label.chars().all(valid) {
        return Err("label must be ascii alphanumeric, '-', '_', or '.'".into());
    }
    Ok(())
}

/// Validate an endpoint URL. Must be `https://...` or `http://localhost...`.
pub fn validate_endpoint(endpoint: &str) -> Result<(), String> {
    if endpoint.is_empty() {
        return Err("endpoint must not be empty".into());
    }
    if endpoint.starts_with("https://") {
        return Ok(());
    }
    if endpoint.starts_with("http://localhost") || endpoint.starts_with("http://127.0.0.1") {
        return Ok(());
    }
    Err("endpoint must be https://, or http://localhost for local providers".into())
}

/// Default endpoint for a provider. Returns `None` for unknown providers —
/// callers must supply an explicit endpoint in that case.
///
/// Thin delegate to [`crate::catalog::default_endpoint`] so the model and
/// catalog modules agree on the well-known endpoints. The catalog covers the
/// full curated provider list; this function is kept for back-compat with
/// existing call sites in `store.rs` and `sqlite.rs`.
pub fn default_endpoint(provider: &str) -> Option<&'static str> {
    crate::catalog::default_endpoint(provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_active_always_eligible() {
        let now = Utc::now();
        assert!(AccountStatus::Active.is_eligible_at(now, None));
        assert!(AccountStatus::Active.is_eligible_at(now, Some(now + chrono::Duration::hours(1))));
    }

    #[test]
    fn status_cooldown_respects_until() {
        let now = Utc::now();
        let future = now + chrono::Duration::minutes(5);
        assert!(!AccountStatus::Cooldown.is_eligible_at(now, Some(future)));
        assert!(AccountStatus::Cooldown.is_eligible_at(future, Some(future)));
    }

    #[test]
    fn status_dead_never_eligible() {
        let now = Utc::now();
        assert!(!AccountStatus::Dead.is_eligible_at(now, None));
    }

    #[test]
    fn validate_provider_rejects_uppercase() {
        assert!(validate_provider("OpenAI").is_err());
        assert!(validate_provider("openai").is_ok());
        assert!(validate_provider("custom:together").is_ok());
        assert!(validate_provider("").is_err());
    }

    #[test]
    fn validate_label_allows_common_chars() {
        assert!(validate_label("work").is_ok());
        assert!(validate_label("personal-v2").is_ok());
        assert!(validate_label("ci.run").is_ok());
        assert!(validate_label("with spaces").is_err());
        assert!(validate_label("").is_err());
    }

    #[test]
    fn validate_endpoint_requires_https_or_localhost() {
        assert!(validate_endpoint("https://api.openai.com").is_ok());
        assert!(validate_endpoint("http://localhost:8080").is_ok());
        assert!(validate_endpoint("http://127.0.0.1:11434").is_ok());
        assert!(validate_endpoint("http://example.com").is_err());
        assert!(validate_endpoint("ftp://x").is_err());
    }

    #[test]
    fn default_endpoint_known_providers() {
        assert!(default_endpoint("openai").is_some());
        assert!(default_endpoint("anthropic").is_some());
        assert!(default_endpoint("custom-x").is_none());
    }
}
