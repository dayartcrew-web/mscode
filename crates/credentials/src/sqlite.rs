//! SQLite-backed `CredentialStore` — the production path.
//!
//! - Metadata rows live in the `provider_accounts` table (created by
//!   [`mscode_state::migrations`]).
//! - Secret bytes live in the OS keyring (or whichever [`KeyringBackend`] is
//!   injected).
//!
//! # Atomicity
//!
//! `add` stores the keyring entry first, then begins a `BEGIN IMMEDIATE`
//! SQLite transaction. If the SQL insert fails (duplicate, etc.) we roll back
//! the keyring write. `remove` deletes the SQLite row first and best-effort
//! deletes the keyring entry — orphaned keyring entries are harmless and
//! self-healing on the next `add` (which generates a fresh `key_id`).
//!
//! # Concurrency
//!
//! Every write acquires a connection from the [`AppState`] pool and uses an
//! immediate transaction. WAL mode is enabled at pool construction. Multiple
//! CLI invocations may safely write concurrently; SQLite will serialize via
//! `SQLITE_BUSY` retries (handled by the `r2d2` pool's wait policy).

use chrono::{DateTime, Utc};
use mscode_state::AppState;
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::error::{CredentialError, Result};
use crate::keyring_backend::{KeyringBackend, OSKeyringBackend};
use crate::model::{
    AccountStatus, NewAccount, ProviderAccount, default_endpoint, validate_endpoint,
    validate_label, validate_provider,
};
use crate::store::CredentialStore;

/// SQLite + keyring backed credential store.
pub struct SqliteCredentialStore {
    state: AppState,
    keyring: Box<dyn KeyringBackend>,
}

impl SqliteCredentialStore {
    /// Construct against an existing [`AppState`] using the production OS
    /// keyring backend.
    pub fn new(state: AppState) -> Self {
        Self::with_backend(state, Box::new(OSKeyringBackend::new()))
    }

    /// Construct with an explicit keyring backend (used by tests and the
    /// plaintext-file opt-in fallback).
    pub fn with_backend(state: AppState, keyring: Box<dyn KeyringBackend>) -> Self {
        Self { state, keyring }
    }
}

const SELECT_COLS: &str = "id, provider, label, endpoint, key_id, is_default, \
     status, cooldown_until, last_used_at, metadata, created_at";

fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderAccount> {
    let id: String = row.get(0)?;
    let provider: String = row.get(1)?;
    let label: String = row.get(2)?;
    let endpoint: String = row.get(3)?;
    let key_id: String = row.get(4)?;
    let is_default_int: i64 = row.get(5)?;
    let status_str: String = row.get(6)?;
    let cooldown_str: Option<String> = row.get(7)?;
    let last_used_str: Option<String> = row.get(8)?;
    let metadata_str: String = row.get(9).unwrap_or_else(|_| "{}".into());
    let created_str: String = row.get(10)?;

    let cooldown_until = cooldown_str
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));
    let last_used_at = last_used_str
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));
    let created_at = DateTime::parse_from_rfc3339(&created_str)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata_str).unwrap_or(serde_json::json!({}));

    Ok(ProviderAccount {
        id,
        provider,
        label,
        endpoint,
        key_id,
        is_default: is_default_int != 0,
        status: AccountStatus::from_str_lossy(&status_str),
        cooldown_until,
        last_used_at,
        metadata,
        created_at,
    })
}

fn to_rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

impl CredentialStore for SqliteCredentialStore {
    fn add(&self, account: NewAccount) -> Result<ProviderAccount> {
        validate_provider(&account.provider)?;
        validate_label(&account.label)?;
        if account.api_key.is_empty() {
            return Err(CredentialError::InvalidInput(
                "api_key must not be empty".into(),
            ));
        }
        let endpoint = if let Some(ep) = &account.endpoint {
            validate_endpoint(ep)?;
            ep.clone()
        } else if let Some(default) = default_endpoint(&account.provider) {
            default.to_string()
        } else {
            return Err(CredentialError::InvalidInput(format!(
                "no default endpoint for provider `{}`; supply one explicitly",
                account.provider
            )));
        };

        let key_id = Uuid::new_v4().to_string();
        let id = Uuid::new_v4().to_string();
        let now_str = to_rfc3339(Utc::now());

        // Keyring first — easier to roll back than SQLite.
        self.keyring.store(&key_id, &account.api_key)?;

        let mut conn = self.state.conn()?;
        let tx = conn.transaction()?;
        // Enforce single-default invariant before insert.
        if account.set_default {
            tx.execute(
                "UPDATE provider_accounts SET is_default = 0 WHERE provider = ?1",
                params![account.provider],
            )?;
        }
        let insert_result = tx.execute(
            "INSERT INTO provider_accounts \
                (id, provider, label, endpoint, key_id, is_default, status, cooldown_until, \
                 last_used_at, metadata, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', NULL, NULL, ?7, ?8)",
            params![
                id,
                account.provider,
                account.label,
                endpoint,
                key_id,
                if account.set_default { 1 } else { 0 },
                "{}",
                now_str,
            ],
        );
        match insert_result {
            Ok(_) => {
                // Auto-default if this is the only row for the provider.
                let count: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM provider_accounts WHERE provider = ?1",
                    params![account.provider],
                    |r| r.get(0),
                )?;
                if count == 1 {
                    tx.execute(
                        "UPDATE provider_accounts SET is_default = 1 WHERE id = ?1",
                        params![id],
                    )?;
                }
                tx.commit()?;
            }
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                let _ = self.keyring.delete(&key_id);
                return Err(CredentialError::Duplicate {
                    provider: account.provider.clone(),
                    label: account.label.clone(),
                });
            }
            Err(e) => {
                let _ = self.keyring.delete(&key_id);
                return Err(CredentialError::Sqlite(e));
            }
        }

        // Re-fetch the row to return canonical state.
        let conn = self.state.conn()?;
        let account_out = conn
            .query_row(
                &format!(
                    "SELECT {SELECT_COLS} FROM provider_accounts WHERE provider = ?1 AND label = ?2"
                ),
                params![account.provider, account.label],
                row_to_account,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CredentialError::NotFound {
                    provider: account.provider.clone(),
                    label: account.label.clone(),
                },
                other => CredentialError::Sqlite(other),
            })?;
        Ok(account_out)
    }

    fn list(&self) -> Result<Vec<ProviderAccount>> {
        let conn = self.state.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLS} FROM provider_accounts ORDER BY provider ASC, label ASC"
        ))?;
        let rows = stmt.query_map([], row_to_account)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn list_for_provider(&self, provider: &str) -> Result<Vec<ProviderAccount>> {
        let conn = self.state.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {SELECT_COLS} FROM provider_accounts WHERE provider = ?1 ORDER BY label ASC"
        ))?;
        let rows = stmt.query_map(params![provider], row_to_account)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn remove(&self, provider: &str, label: &str) -> Result<()> {
        let mut conn = self.state.conn()?;
        let tx = conn.transaction()?;
        let key_id: Option<String> = tx
            .query_row(
                "SELECT key_id FROM provider_accounts WHERE provider = ?1 AND label = ?2",
                params![provider, label],
                |r| r.get(0),
            )
            .optional()?;
        let key_id = match key_id {
            Some(k) => k,
            None => {
                return Err(CredentialError::NotFound {
                    provider: provider.to_string(),
                    label: label.to_string(),
                });
            }
        };
        let affected = tx.execute(
            "DELETE FROM provider_accounts WHERE provider = ?1 AND label = ?2",
            params![provider, label],
        )?;
        tx.commit()?;
        if affected == 0 {
            return Err(CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            });
        }
        let _ = self.keyring.delete(&key_id);
        Ok(())
    }

    fn set_default(&self, provider: &str, label: &str) -> Result<()> {
        let mut conn = self.state.conn()?;
        let tx = conn.transaction()?;
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM provider_accounts WHERE provider = ?1 AND label = ?2",
            params![provider, label],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Err(CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            });
        }
        tx.execute(
            "UPDATE provider_accounts SET is_default = 0 WHERE provider = ?1",
            params![provider],
        )?;
        tx.execute(
            "UPDATE provider_accounts SET is_default = 1 WHERE provider = ?1 AND label = ?2",
            params![provider, label],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn get_default(&self, provider: &str) -> Result<Option<ProviderAccount>> {
        let conn = self.state.conn()?;
        let account = conn
            .query_row(
                &format!(
                    "SELECT {SELECT_COLS} FROM provider_accounts \
                     WHERE provider = ?1 AND is_default = 1 LIMIT 1"
                ),
                params![provider],
                row_to_account,
            )
            .optional()?;
        Ok(account)
    }

    fn load_key(&self, provider: &str, label: &str) -> Result<String> {
        let conn = self.state.conn()?;
        let account = conn
            .query_row(
                &format!(
                    "SELECT {SELECT_COLS} FROM provider_accounts WHERE provider = ?1 AND label = ?2"
                ),
                params![provider, label],
                row_to_account,
            )
            .optional()?
            .ok_or_else(|| CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            })?;
        self.keyring
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
            .min_by_key(|a| a.last_used_at)
            .ok_or_else(|| CredentialError::NoEligible {
                provider: provider.to_string(),
            })?;
        let secret =
            self.keyring
                .load(&chosen.key_id)?
                .ok_or_else(|| CredentialError::OrphanedKey {
                    provider: chosen.provider.clone(),
                    label: chosen.label.clone(),
                    key_id: chosen.key_id.clone(),
                })?;
        Ok((chosen, secret))
    }

    fn mark_cooldown(&self, provider: &str, label: &str, until: DateTime<Utc>) -> Result<()> {
        let conn = self.state.conn()?;
        let until_str = to_rfc3339(until);
        let affected = conn.execute(
            "UPDATE provider_accounts SET status = 'cooldown', cooldown_until = ?3 \
             WHERE provider = ?1 AND label = ?2",
            params![provider, label, until_str],
        )?;
        if affected == 0 {
            return Err(CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            });
        }
        Ok(())
    }

    fn mark_dead(&self, provider: &str, label: &str) -> Result<()> {
        let conn = self.state.conn()?;
        let affected = conn.execute(
            "UPDATE provider_accounts SET status = 'dead', cooldown_until = NULL \
             WHERE provider = ?1 AND label = ?2",
            params![provider, label],
        )?;
        if affected == 0 {
            return Err(CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            });
        }
        Ok(())
    }

    fn clear_status(&self, provider: &str, label: &str) -> Result<()> {
        let conn = self.state.conn()?;
        let affected = conn.execute(
            "UPDATE provider_accounts SET status = 'active', cooldown_until = NULL \
             WHERE provider = ?1 AND label = ?2",
            params![provider, label],
        )?;
        if affected == 0 {
            return Err(CredentialError::NotFound {
                provider: provider.to_string(),
                label: label.to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyring_backend::InMemoryKeyringBackend;
    use chrono::Duration;

    fn setup() -> SqliteCredentialStore {
        let state = AppState::in_memory().expect("in_memory");
        SqliteCredentialStore::with_backend(state, Box::new(InMemoryKeyringBackend::new()))
    }

    fn acc(provider: &str, label: &str, key: &str) -> NewAccount {
        NewAccount::new(provider, label, key)
    }

    #[test]
    fn add_persists_metadata_and_secret() {
        let store = setup();
        let a = store.add(acc("openai", "work", "sk-1")).unwrap();
        assert_eq!(a.provider, "openai");
        assert_eq!(store.load_key("openai", "work").unwrap(), "sk-1");
    }

    #[test]
    fn first_account_auto_becomes_default() {
        let store = setup();
        let a = store.add(acc("openai", "work", "sk-1")).unwrap();
        assert!(a.is_default);
    }

    #[test]
    fn duplicate_provider_label_pair_is_rejected_and_keyring_rolls_back() {
        let store = setup();
        store.add(acc("openai", "work", "sk-1")).unwrap();
        let err = store.add(acc("openai", "work", "sk-2")).unwrap_err();
        assert!(matches!(err, CredentialError::Duplicate { .. }));
        // The original entry must still load with the original secret.
        assert_eq!(store.load_key("openai", "work").unwrap(), "sk-1");
    }

    #[test]
    fn set_default_clears_others() {
        let store = setup();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store
            .add(acc("openai", "b", "sk-2").with_default(true))
            .unwrap();
        assert_eq!(store.get_default("openai").unwrap().unwrap().label, "b");
        store.set_default("openai", "a").unwrap();
        assert_eq!(store.get_default("openai").unwrap().unwrap().label, "a");
    }

    #[test]
    fn remove_deletes_account_and_orphans_keyring_entry() {
        let store = setup();
        store.add(acc("openai", "work", "sk-1")).unwrap();
        store.remove("openai", "work").unwrap();
        let err = store.load_key("openai", "work").unwrap_err();
        assert!(matches!(err, CredentialError::NotFound { .. }));
    }

    #[test]
    fn resolve_skips_cooldown_and_dead() {
        let store = setup();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store.add(acc("openai", "b", "sk-2")).unwrap();
        store
            .mark_cooldown("openai", "a", Utc::now() + Duration::minutes(5))
            .unwrap();
        let (chosen, _) = store.resolve("openai").unwrap();
        assert_eq!(chosen.label, "b");
        // Clear cooldown on "a" so we can verify the dead-b path independently.
        store.clear_status("openai", "a").unwrap();
        store.mark_dead("openai", "b").unwrap();
        let (chosen, _) = store.resolve("openai").unwrap();
        assert_eq!(chosen.label, "a");
    }

    #[test]
    fn resolve_errors_when_all_dead() {
        let store = setup();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store.mark_dead("openai", "a").unwrap();
        assert!(matches!(
            store.resolve("openai").unwrap_err(),
            CredentialError::NoEligible { .. }
        ));
    }

    #[test]
    fn list_orders_by_provider_then_label() {
        let store = setup();
        store.add(acc("openai", "b", "sk-2")).unwrap();
        store.add(acc("anthropic", "x", "sk-3")).unwrap();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        let all = store.list().unwrap();
        let labels: Vec<_> = all
            .iter()
            .map(|a| (a.provider.as_str(), a.label.as_str()))
            .collect();
        assert_eq!(
            labels,
            vec![("anthropic", "x"), ("openai", "a"), ("openai", "b")]
        );
    }

    #[test]
    fn clear_status_revives_dead_account() {
        let store = setup();
        store.add(acc("openai", "a", "sk-1")).unwrap();
        store.mark_dead("openai", "a").unwrap();
        assert!(store.resolve("openai").is_err());
        store.clear_status("openai", "a").unwrap();
        assert!(store.resolve("openai").is_ok());
    }

    #[test]
    fn add_rejects_unknown_provider_without_endpoint() {
        let store = setup();
        let err = store
            .add(NewAccount::new("custom-foo", "x", "k"))
            .unwrap_err();
        assert!(matches!(err, CredentialError::InvalidInput(_)));
    }

    #[test]
    fn load_key_errors_orphaned_when_keyring_loses_entry() {
        // Construct with an empty keyring; manually insert a SQLite row that
        // references a key_id the keyring doesn't have.
        let state = AppState::in_memory().unwrap();
        {
            let conn = state.conn().unwrap();
            conn.execute(
                "INSERT INTO provider_accounts (id, provider, label, endpoint, key_id, \
                 is_default, status, metadata, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, 'active', '{}', ?6)",
                params![
                    "row-1",
                    "openai",
                    "ghost",
                    "https://api.openai.com/v1/chat/completions",
                    "missing-key-id",
                    to_rfc3339(Utc::now()),
                ],
            )
            .unwrap();
        }
        let store =
            SqliteCredentialStore::with_backend(state, Box::new(InMemoryKeyringBackend::new()));
        let err = store.load_key("openai", "ghost").unwrap_err();
        assert!(matches!(err, CredentialError::OrphanedKey { .. }));
    }
}
