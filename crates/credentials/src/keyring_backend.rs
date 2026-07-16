//! Abstraction over the OS keyring (Windows DPAPI, macOS Keychain, Linux
//! Secret Service).
//!
//! Secret bytes never touch the SQLite database. Each account gets a stable
//! `key_id` (UUID); the keyring entry is `(service="mscode", username=key_id)`.
//! This indirection means renaming a label does NOT require re-storing the
//! secret — only the SQLite row's `label` column changes.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{CredentialError, Result};

/// Backend for storing secret bytes outside the SQLite metadata DB.
///
/// All methods are synchronous because (a) the `keyring` crate is sync, and
/// (b) the CLI is a short-lived foreground process — no async runtime needed.
pub trait KeyringBackend: Send + Sync {
    /// Store `secret` under `key_id`. Overwrites if already present.
    fn store(&self, key_id: &str, secret: &str) -> Result<()>;

    /// Load the secret for `key_id`. Returns `Ok(None)` if absent.
    fn load(&self, key_id: &str) -> Result<Option<String>>;

    /// Delete the entry for `key_id`. Idempotent — missing is OK.
    fn delete(&self, key_id: &str) -> Result<()>;

    /// Returns `true` when the backend is the production OS keyring. Callers
    /// use this to decide between failing loud and warning.
    fn is_real(&self) -> bool;
}

/// Production backend wrapping the `keyring` crate.
///
/// Service name is fixed to `"mscode"` so all entries cluster together in
/// platform keyring UIs (Windows Credential Manager, macOS Keychain Access).
pub struct OSKeyringBackend {
    service: &'static str,
}

impl OSKeyringBackend {
    const DEFAULT_SERVICE: &'static str = "mscode";

    /// Construct with the default service name (`"mscode"`).
    pub fn new() -> Self {
        Self {
            service: Self::DEFAULT_SERVICE,
        }
    }
}

impl Default for OSKeyringBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyringBackend for OSKeyringBackend {
    fn store(&self, key_id: &str, secret: &str) -> Result<()> {
        let entry =
            keyring::Entry::new(self.service, key_id).map_err(|e| CredentialError::Keyring {
                operation: "store",
                source: Box::new(e),
            })?;
        entry
            .set_password(secret)
            .map_err(|e| CredentialError::Keyring {
                operation: "store",
                source: Box::new(e),
            })
    }

    fn load(&self, key_id: &str) -> Result<Option<String>> {
        let entry =
            keyring::Entry::new(self.service, key_id).map_err(|e| CredentialError::Keyring {
                operation: "load",
                source: Box::new(e),
            })?;
        match entry.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CredentialError::Keyring {
                operation: "load",
                source: Box::new(e),
            }),
        }
    }

    fn delete(&self, key_id: &str) -> Result<()> {
        let entry =
            keyring::Entry::new(self.service, key_id).map_err(|e| CredentialError::Keyring {
                operation: "delete",
                source: Box::new(e),
            })?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CredentialError::Keyring {
                operation: "delete",
                source: Box::new(e),
            }),
        }
    }

    fn is_real(&self) -> bool {
        true
    }
}

/// In-memory backend for tests. Thread-safe via `Mutex<HashMap>`.
#[derive(Default)]
pub struct InMemoryKeyringBackend {
    inner: Mutex<HashMap<String, String>>,
}

impl InMemoryKeyringBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyringBackend for InMemoryKeyringBackend {
    fn store(&self, key_id: &str, secret: &str) -> Result<()> {
        self.inner
            .lock()
            .expect("InMemoryKeyringBackend mutex poisoned")
            .insert(key_id.to_string(), secret.to_string());
        Ok(())
    }

    fn load(&self, key_id: &str) -> Result<Option<String>> {
        Ok(self
            .inner
            .lock()
            .expect("InMemoryKeyringBackend mutex poisoned")
            .get(key_id)
            .cloned())
    }

    fn delete(&self, key_id: &str) -> Result<()> {
        self.inner
            .lock()
            .expect("InMemoryKeyringBackend mutex poisoned")
            .remove(key_id);
        Ok(())
    }

    fn is_real(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_store_load_delete_roundtrip() {
        let kr = InMemoryKeyringBackend::new();
        kr.store("k1", "secret1").unwrap();
        assert_eq!(kr.load("k1").unwrap().as_deref(), Some("secret1"));
        kr.delete("k1").unwrap();
        assert_eq!(kr.load("k1").unwrap(), None);
    }

    #[test]
    fn in_memory_delete_missing_is_idempotent() {
        let kr = InMemoryKeyringBackend::new();
        kr.delete("never-stored").unwrap();
    }

    #[test]
    fn in_memory_store_overwrites() {
        let kr = InMemoryKeyringBackend::new();
        kr.store("k1", "old").unwrap();
        kr.store("k1", "new").unwrap();
        assert_eq!(kr.load("k1").unwrap().as_deref(), Some("new"));
    }

    #[test]
    fn in_memory_backend_reports_not_real() {
        let kr = InMemoryKeyringBackend::new();
        assert!(!kr.is_real());
    }
}
