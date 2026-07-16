//! Abstraction over the OS keyring (Windows DPAPI, macOS Keychain, Linux
//! Secret Service).
//!
//! Secret bytes never touch the SQLite database. Each account gets a stable
//! `key_id` (UUID); the keyring entry is `(service="mscode", username=key_id)`.
//! This indirection means renaming a label does NOT require re-storing the
//! secret — only the SQLite row's `label` column changes.

use std::collections::HashMap;
use std::path::PathBuf;
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

/// Plaintext-file backend — explicit user opt-in for environments where the
/// OS keyring is unavailable (headless CI, locked-down sandboxes, WSL without
/// D-Bus secrets, etc.).
///
/// Triggered by the `MSCODE_CREDENTIALS_FILE` env var. The file is a JSON map
/// of `{key_id: secret}`. Writes are atomic (tmp file + rename) and the file
/// is created with restrictive permissions on POSIX (`0600`). On Windows the
/// file inherits the user's default ACL — Windows already encrypts at rest via
/// DPAPI when BitLocker or equivalent is enabled.
///
/// # Security posture
///
/// This is **less secure than the OS keyring**. We only use it when the user
/// has explicitly pointed at a file path, because the alternative — failing
/// every CLI invocation on a headless host — is worse for usability and
/// pushes users toward storing secrets in plain env vars or shell history.
pub struct FileKeyringBackend {
    path: PathBuf,
    inner: Mutex<HashMap<String, String>>,
}

impl FileKeyringBackend {
    /// Construct against `path`. The file is loaded eagerly; missing file is
    /// treated as an empty store (the file will be created on the first
    /// `store` call).
    pub fn new(path: PathBuf) -> Result<Self> {
        let inner = if path.exists() {
            Self::load_from(&path)?
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Returns the configured file path (used by tests and diagnostics).
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn load_from(path: &PathBuf) -> Result<HashMap<String, String>> {
        let bytes = std::fs::read(path).map_err(|e| CredentialError::Keyring {
            operation: "file-read",
            source: Box::new(e),
        })?;
        if bytes.is_empty() {
            return Ok(HashMap::new());
        }
        serde_json::from_slice(&bytes).map_err(|e| CredentialError::Keyring {
            operation: "file-parse",
            source: Box::new(e),
        })
    }

    /// Persist the map atomically: write to `<path>.tmp` then rename. On POSIX,
    /// restrict to `0600` so only the owning user can read it.
    fn flush(&self, map: &HashMap<String, String>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CredentialError::Keyring {
                operation: "file-mkdir",
                source: Box::new(e),
            })?;
        }
        let tmp = self.path.with_extension("tmp");
        let bytes = serde_json::to_vec(map).map_err(|e| CredentialError::Keyring {
            operation: "file-serialize",
            source: Box::new(e),
        })?;
        std::fs::write(&tmp, &bytes).map_err(|e| CredentialError::Keyring {
            operation: "file-write",
            source: Box::new(e),
        })?;
        Self::restrict_perms(&tmp)?;
        std::fs::rename(&tmp, &self.path).map_err(|e| CredentialError::Keyring {
            operation: "file-rename",
            source: Box::new(e),
        })?;
        Ok(())
    }

    #[cfg(unix)]
    fn restrict_perms(path: &PathBuf) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms).map_err(|e| CredentialError::Keyring {
            operation: "file-chmod",
            source: Box::new(e),
        })?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn restrict_perms(_path: &PathBuf) -> Result<()> {
        // Windows: the file inherits the user's default ACL from the parent
        // directory. We rely on user-level isolation rather than per-file ACL.
        Ok(())
    }
}

impl KeyringBackend for FileKeyringBackend {
    fn store(&self, key_id: &str, secret: &str) -> Result<()> {
        let mut map = self
            .inner
            .lock()
            .map_err(|_| CredentialError::KeyringPoisoned)?;
        map.insert(key_id.to_string(), secret.to_string());
        self.flush(&map)
    }

    fn load(&self, key_id: &str) -> Result<Option<String>> {
        let map = self
            .inner
            .lock()
            .map_err(|_| CredentialError::KeyringPoisoned)?;
        Ok(map.get(key_id).cloned())
    }

    fn delete(&self, key_id: &str) -> Result<()> {
        let mut map = self
            .inner
            .lock()
            .map_err(|_| CredentialError::KeyringPoisoned)?;
        map.remove(key_id);
        self.flush(&map)
    }

    fn is_real(&self) -> bool {
        // FileKeyringBackend is not the OS keyring — callers use this signal
        // to decide between warning and failing loud. The CLI's
        // print_credential_error mentions the env var only when the OS keyring
        // is unavailable, so this returning `false` is correct.
        false
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

    // ----- FileKeyringBackend -----

    fn file_setup() -> FileKeyringBackend {
        let dir = std::env::temp_dir().join(format!(
            "mscode-keyring-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir temp");
        let path = dir.join("creds.json");
        FileKeyringBackend::new(path).expect("file backend init")
    }

    #[test]
    fn file_store_load_delete_roundtrip() {
        let kr = file_setup();
        kr.store("k1", "secret1").unwrap();
        assert_eq!(kr.load("k1").unwrap().as_deref(), Some("secret1"));
        kr.delete("k1").unwrap();
        assert_eq!(kr.load("k1").unwrap(), None);
    }

    #[test]
    fn file_persists_across_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "mscode-keyring-test-persist-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir temp");
        let path = dir.join("creds.json");

        {
            let kr = FileKeyringBackend::new(path.clone()).unwrap();
            kr.store("k1", "secret1").unwrap();
        }
        // Re-open the same file — the entry must survive.
        let kr = FileKeyringBackend::new(path.clone()).unwrap();
        assert_eq!(kr.load("k1").unwrap().as_deref(), Some("secret1"));
    }

    #[test]
    fn file_store_overwrites() {
        let kr = file_setup();
        kr.store("k1", "old").unwrap();
        kr.store("k1", "new").unwrap();
        assert_eq!(kr.load("k1").unwrap().as_deref(), Some("new"));
    }

    #[test]
    fn file_delete_missing_is_idempotent() {
        let kr = file_setup();
        kr.delete("never-stored").unwrap();
    }

    #[test]
    fn file_backend_reports_not_real() {
        let kr = file_setup();
        assert!(!kr.is_real());
    }

    #[test]
    fn file_loads_existing_empty_file() {
        let dir = std::env::temp_dir().join(format!(
            "mscode-keyring-test-empty-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir temp");
        let path = dir.join("empty.json");
        std::fs::write(&path, b"").unwrap();
        let kr = FileKeyringBackend::new(path).unwrap();
        assert_eq!(kr.load("anything").unwrap(), None);
    }

    #[test]
    fn file_creates_parent_directory_if_missing() {
        let dir = std::env::temp_dir().join(format!(
            "mscode-keyring-test-nested-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        // Note: do not create `dir` — the backend should make it.
        let nested = dir.join("subdir").join("creds.json");
        let kr = FileKeyringBackend::new(nested.clone()).unwrap();
        kr.store("k1", "v1").unwrap();
        assert!(nested.exists(), "expected creds file to be created");
    }
}
