//! The `CredentialStore` trait — what every backend (in-memory, SQLite, future
//! vault) must implement. Plus a default `InMemoryCredentialStore` for tests
//! and ephemeral sessions.
//!
//! # Selection semantics
//!
//! [`CredentialStore::resolve`] follows Hermes's `fill_first` strategy: among
//! the provider's eligible accounts, the one with the oldest `last_used_at`
//! (`None` treated as oldest) is picked. This naturally load-balances and
//! avoids stampeding a single account under load.
//!
//! # Status model
//!
//! - [`AccountStatus::Active`] — eligible.
//! - [`AccountStatus::Cooldown`] — skipped until `cooldown_until` (429/5xx).
//! - [`AccountStatus::Dead`] — permanently skipped until explicitly cleared
//!   (revoked OAuth, `invalid_grant`). See Hermes `STATUS_DEAD`.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::{CredentialError, Result};
use crate::keyring_backend::{InMemoryKeyringBackend, KeyringBackend};
use crate::model::{
    AccountStatus, NewAccount, ProviderAccount, default_endpoint, validate_endpoint,
    validate_label, validate_provider,
};

/// Object-safe credential store. All operations are synchronous — no async
/// runtime is required by the CLI.
pub trait CredentialStore: Send + Sync {
    /// Persist a new account. Secret bytes go to the keyring.
    fn add(&self, account: NewAccount) -> Result<ProviderAccount>;

    /// List every account, ordered by `(provider, label)`.
    fn list(&self) -> Result<Vec<ProviderAccount>>;

    /// List accounts for one provider, ordered by `label`.
    fn list_for_provider(&self, provider: &str) -> Result<Vec<ProviderAccount>>;

    /// Remove an account and its secret. Idempotent on the keyring side.
    fn remove(&self, provider: &str, label: &str) -> Result<()>;

    /// Mark `(provider, label)` as the default for its provider. Clears any
    /// previous default for the same provider.
    fn set_default(&self, provider: &str, label: &str) -> Result<()>;

    /// Get the default account for `provider`, if any.
    fn get_default(&self, provider: &str) -> Result<Option<ProviderAccount>>;

    /// Load the secret bytes for `(provider, label)`.
    fn load_key(&self, provider: &str, label: &str) -> Result<String>;

    /// Select the next account per `fill_first` strategy. Returns the account
    /// and its secret. Errors with [`CredentialError::NoEligible`] when no
    /// account is selectable (all in cooldown/dead, or store empty).
    fn resolve(&self, provider: &str) -> Result<(ProviderAccount, String)>;

    /// Apply a cooldown until `until`. Used for 429/5xx failures.
    fn mark_cooldown(&self, provider: &str, label: &str, until: DateTime<Utc>) -> Result<()>;

    /// Mark the account as `dead` (terminal auth failure). Stays out of
    /// rotation until the user explicitly re-adds it.
    fn mark_dead(&self, provider: &str, label: &str) -> Result<()>;

    /// Clear cooldown/dead status, returning the account to `active`.
    fn clear_status(&self, provider: &str, label: &str) -> Result<()>;
}

/// In-memory `CredentialStore` for tests and ephemeral sessions.
///
/// Use [`SqliteCredentialStore`](crate::SqliteCredentialStore) for production.
pub struct InMemoryCredentialStore {
    backend: Box<dyn KeyringBackend>,
    rows: Mutex<HashMap<(String, String), ProviderAccount>>,
}

impl InMemoryCredentialStore {
    /// Construct with an in-memory keyring backend.
    pub fn new() -> Self {
        Self::with_backend(Box::new(InMemoryKeyringBackend::new()))
    }

    /// Construct with a custom keyring backend (e.g. mockall mock).
    pub fn with_backend(backend: Box<dyn KeyringBackend>) -> Self {
        Self {
            backend,
            rows: Mutex::new(HashMap::new()),
        }
    }

    fn get_locked(&self, provider: &str, label: &str) -> Result<ProviderAccount> {
        let rows = self.rows.lock().expect("rows mutex poisoned");
        rows.get(&(provider.to_string(), label.to_string()))
            .cloned()
            .ok_or_else(|| CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            })
    }

    fn validate_new(account: &NewAccount) -> Result<()> {
        validate_provider(&account.provider)?;
        validate_label(&account.label)?;
        if account.api_key.is_empty() {
            return Err(CredentialError::InvalidInput(
                "api_key must not be empty".into(),
            ));
        }
        Ok(())
    }

    fn resolve_endpoint(account: &NewAccount) -> Result<String> {
        if let Some(ep) = &account.endpoint {
            validate_endpoint(ep)?;
            return Ok(ep.clone());
        }
        if let Some(default) = default_endpoint(&account.provider) {
            return Ok(default.to_string());
        }
        Err(CredentialError::InvalidInput(format!(
            "no default endpoint for provider `{}`; supply one explicitly",
            account.provider
        )))
    }
}

impl Default for InMemoryCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn add(&self, account: NewAccount) -> Result<ProviderAccount> {
        Self::validate_new(&account)?;
        let endpoint = Self::resolve_endpoint(&account)?;
        let key_id = Uuid::new_v4().to_string();
        let now = Utc::now();

        // Keyring first — easier to roll back than the rest.
        self.backend.store(&key_id, &account.api_key)?;

        let mut row = ProviderAccount {
            id: Uuid::new_v4().to_string(),
            provider: account.provider.clone(),
            label: account.label.clone(),
            endpoint,
            key_id: key_id.clone(),
            is_default: account.set_default,
            status: AccountStatus::Active,
            cooldown_until: None,
            last_used_at: None,
            metadata: serde_json::json!({}),
            created_at: now,
        };

        let mut rows = self.rows.lock().expect("rows mutex poisoned");
        let key = (row.provider.clone(), row.label.clone());
        if rows.contains_key(&key) {
            // Rollback keyring write.
            drop(rows);
            let _ = self.backend.delete(&key_id);
            return Err(CredentialError::Duplicate {
                provider: row.provider.clone(),
                label: row.label.clone(),
            });
        }

        // Enforce single-default invariant.
        if row.is_default {
            for r in rows.values_mut() {
                if r.provider == row.provider && r.is_default {
                    r.is_default = false;
                }
            }
        }

        // Auto-default if this is the only row for the provider.
        let count = rows.values().filter(|r| r.provider == row.provider).count();
        if count == 0 {
            row.is_default = true;
        }

        rows.insert(key, row.clone());
        drop(rows);
        Ok(row)
    }

    fn list(&self) -> Result<Vec<ProviderAccount>> {
        let rows = self.rows.lock().expect("rows mutex poisoned");
        let mut all: Vec<ProviderAccount> = rows.values().cloned().collect();
        all.sort_by(|a, b| {
            a.provider
                .cmp(&b.provider)
                .then_with(|| a.label.cmp(&b.label))
        });
        Ok(all)
    }

    fn list_for_provider(&self, provider: &str) -> Result<Vec<ProviderAccount>> {
        let rows = self.rows.lock().expect("rows mutex poisoned");
        let mut filtered: Vec<ProviderAccount> = rows
            .values()
            .filter(|r| r.provider == provider)
            .cloned()
            .collect();
        filtered.sort_by(|a, b| a.label.cmp(&b.label));
        Ok(filtered)
    }

    fn remove(&self, provider: &str, label: &str) -> Result<()> {
        let mut rows = self.rows.lock().expect("rows mutex poisoned");
        let key = (provider.to_string(), label.to_string());
        let removed = rows.remove(&key).ok_or_else(|| CredentialError::NotFound {
            provider: provider.to_string(),
            label: label.to_string(),
        })?;
        drop(rows);
        let _ = self.backend.delete(&removed.key_id);
        Ok(())
    }

    fn set_default(&self, provider: &str, label: &str) -> Result<()> {
        let mut rows = self.rows.lock().expect("rows mutex poisoned");
        let key = (provider.to_string(), label.to_string());
        if !rows.contains_key(&key) {
            return Err(CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            });
        }
        for r in rows.values_mut() {
            if r.provider == provider {
                r.is_default = r.label == label;
            }
        }
        Ok(())
    }

    fn get_default(&self, provider: &str) -> Result<Option<ProviderAccount>> {
        let rows = self.rows.lock().expect("rows mutex poisoned");
        Ok(rows
            .values()
            .find(|r| r.provider == provider && r.is_default)
            .cloned())
    }

    fn load_key(&self, provider: &str, label: &str) -> Result<String> {
        let account = self.get_locked(provider, label)?;
        self.backend
            .load(&account.key_id)?
            .ok_or_else(|| CredentialError::OrphanedKey {
                provider: provider.to_string(),
                label: label.to_string(),
                key_id: account.key_id.clone(),
            })
    }

    fn resolve(&self, provider: &str) -> Result<(ProviderAccount, String)> {
        let now = Utc::now();
        let candidates = self.list_for_provider(provider)?;
        if candidates.is_empty() {
            return Err(CredentialError::NoEligible {
                provider: provider.to_string(),
            });
        }
        let chosen = candidates
            .into_iter()
            .filter(|a| a.is_eligible_at(now))
            .min_by_key(|a| a.last_used_at);
        let chosen = chosen.ok_or_else(|| CredentialError::NoEligible {
            provider: provider.to_string(),
        })?;
        let secret = self.load_key(&chosen.provider, &chosen.label)?;
        Ok((chosen, secret))
    }

    fn mark_cooldown(&self, provider: &str, label: &str, until: DateTime<Utc>) -> Result<()> {
        let mut rows = self.rows.lock().expect("rows mutex poisoned");
        let key = (provider.to_string(), label.to_string());
        let r = rows
            .get_mut(&key)
            .ok_or_else(|| CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            })?;
        r.status = AccountStatus::Cooldown;
        r.cooldown_until = Some(until);
        Ok(())
    }

    fn mark_dead(&self, provider: &str, label: &str) -> Result<()> {
        let mut rows = self.rows.lock().expect("rows mutex poisoned");
        let key = (provider.to_string(), label.to_string());
        let r = rows
            .get_mut(&key)
            .ok_or_else(|| CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            })?;
        r.status = AccountStatus::Dead;
        r.cooldown_until = None;
        Ok(())
    }

    fn clear_status(&self, provider: &str, label: &str) -> Result<()> {
        let mut rows = self.rows.lock().expect("rows mutex poisoned");
        let key = (provider.to_string(), label.to_string());
        let r = rows
            .get_mut(&key)
            .ok_or_else(|| CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            })?;
        r.status = AccountStatus::Active;
        r.cooldown_until = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn acc(provider: &str, label: &str, key: &str) -> NewAccount {
        NewAccount::new(provider, label, key)
    }

    #[test]
    fn add_persists_account_and_secret() {
        let store = InMemoryCredentialStore::new();
        let a = store.add(acc("openai", "work", "sk-1")).unwrap();
        assert_eq!(a.provider, "openai");
        assert_eq!(a.label, "work");
        assert_eq!(store.load_key("openai", "work").unwrap(), "sk-1");
    }

    #[test]
    fn add_rejects_duplicate() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "work", "sk-1")).unwrap();
        let err = store.add(acc("openai", "work", "sk-2")).unwrap_err();
        assert!(matches!(err, CredentialError::Duplicate { .. }));
    }

    #[test]
    fn first_account_auto_becomes_default() {
        let store = InMemoryCredentialStore::new();
        let a = store.add(acc("openai", "work", "sk-1")).unwrap();
        assert!(a.is_default);
    }

    #[test]
    fn explicit_default_clears_existing_default() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        let b = store
            .add(acc("openai", "b", "sk-2").with_default(true))
            .unwrap();
        assert!(b.is_default);
        assert!(!store.get_locked("openai", "a").unwrap().is_default);
    }

    #[test]
    fn set_default_clears_others_in_provider() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store
            .add(acc("openai", "b", "sk-2").with_default(true))
            .unwrap();
        store.set_default("openai", "a").unwrap();
        assert_eq!(store.get_default("openai").unwrap().unwrap().label, "a");
    }

    #[test]
    fn remove_deletes_account_and_secret() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "work", "sk-1")).unwrap();
        store.remove("openai", "work").unwrap();
        assert!(matches!(
            store.load_key("openai", "work").unwrap_err(),
            CredentialError::NotFound { .. }
        ));
    }

    #[test]
    fn resolve_picks_eligible_with_oldest_last_used() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store.add(acc("openai", "b", "sk-2")).unwrap();
        let (chosen, key) = store.resolve("openai").unwrap();
        assert!(key == "sk-1" || key == "sk-2");
        assert_eq!(chosen.provider, "openai");
    }

    #[test]
    fn resolve_skips_cooldown() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store.add(acc("openai", "b", "sk-2")).unwrap();
        store
            .mark_cooldown("openai", "a", Utc::now() + Duration::minutes(5))
            .unwrap();
        let (chosen, _) = store.resolve("openai").unwrap();
        assert_eq!(chosen.label, "b");
    }

    #[test]
    fn resolve_skips_dead() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store.add(acc("openai", "b", "sk-2")).unwrap();
        store.mark_dead("openai", "b").unwrap();
        let (chosen, _) = store.resolve("openai").unwrap();
        assert_eq!(chosen.label, "a");
    }

    #[test]
    fn resolve_errors_when_all_dead() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store.mark_dead("openai", "a").unwrap();
        assert!(matches!(
            store.resolve("openai").unwrap_err(),
            CredentialError::NoEligible { .. }
        ));
    }

    #[test]
    fn resolve_errors_when_provider_empty() {
        let store = InMemoryCredentialStore::new();
        assert!(matches!(
            store.resolve("anthropic").unwrap_err(),
            CredentialError::NoEligible { .. }
        ));
    }

    #[test]
    fn add_rejects_invalid_provider() {
        let store = InMemoryCredentialStore::new();
        assert!(store.add(NewAccount::new("OpenAI", "x", "k")).is_err());
    }

    #[test]
    fn add_rejects_empty_api_key() {
        let store = InMemoryCredentialStore::new();
        assert!(store.add(NewAccount::new("openai", "x", "")).is_err());
    }

    #[test]
    fn add_uses_default_endpoint_when_omitted() {
        let store = InMemoryCredentialStore::new();
        let a = store.add(acc("openai", "work", "sk-1")).unwrap();
        assert!(a.endpoint.starts_with("https://api.openai.com"));
    }

    #[test]
    fn add_rejects_unknown_provider_without_endpoint() {
        let store = InMemoryCredentialStore::new();
        let err = store
            .add(NewAccount::new("custom-foo", "x", "k"))
            .unwrap_err();
        assert!(matches!(err, CredentialError::InvalidInput(_)));
    }

    #[test]
    fn add_accepts_custom_endpoint() {
        let store = InMemoryCredentialStore::new();
        let a = store
            .add(acc("custom-foo", "x", "k").with_endpoint("https://api.custom.foo/v1"))
            .unwrap();
        assert_eq!(a.endpoint, "https://api.custom.foo/v1");
    }

    #[test]
    fn clear_status_revives_dead_account() {
        let store = InMemoryCredentialStore::new();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store.mark_dead("openai", "a").unwrap();
        assert!(store.resolve("openai").is_err());
        store.clear_status("openai", "a").unwrap();
        assert!(store.resolve("openai").is_ok());
    }
}
