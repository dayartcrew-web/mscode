//! Multi-account provider credential storage for the `mscode` CLI.
//!
//! Builds on the sibling `multi-account-core-rs` crate's `ClaimsCoordinator`
//! for cross-process coordination, but stores account metadata in SQLite
//! (`provider_accounts` table in `state.db`) and secret bytes in the OS
//! keyring (Windows DPAPI / macOS Keychain / Linux Secret Service).
//!
//! # Design rationale
//!
//! Hermes (`NousResearch/hermes-agent`) punts on cross-process coordination —
//! its `threading.Lock` only protects in-process mutations. The Rust port's
//! `ClaimsCoordinator` solves this with PID-liveness zombie detection. We
//! therefore reuse that crate as a path dependency rather than reinventing it.
//!
//! # Storage split
//!
//! - **Metadata** (provider, label, endpoint, status, cooldown): SQLite.
//!   Queryable, transactional, WAL for concurrent readers.
//! - **Secret bytes**: OS keyring keyed by `key_id` (UUID). Atomic at the OS
//!   level, never touches disk in plaintext, survives filesystem races.
//!
//! # Status model
//!
//! Inspired by Hermes's `STATUS_DEAD` distinction: revoked OAuth tokens should
//! not re-enter rotation after a cooldown. The `status` column encodes
//! `active` / `cooldown` / `dead`:
//!
//! - `active` — eligible for selection.
//! - `cooldown` — temporarily ineligible until `cooldown_until`; used for
//!   rate-limit (429) and transient (5xx) failures.
//! - `dead` — permanently ineligible until manually cleared; used for
//!   terminal auth failures (revoked token, invalid_grant).

pub mod catalog;
pub mod error;
pub mod keyring_backend;
pub mod model;
pub mod sqlite;
pub mod store;

pub use catalog::{
    AuthMethod, PROVIDER_CATALOG, ProviderCatalogEntry,
    default_endpoint as catalog_default_endpoint, display_name as catalog_display_name, is_known,
    is_recommended_provider, lookup,
};
pub use error::{CredentialError, Result};
pub use keyring_backend::{KeyringBackend, OSKeyringBackend};
pub use model::{AccountStatus, NewAccount, ProviderAccount};
pub use sqlite::SqliteCredentialStore;
pub use store::{CredentialStore, InMemoryCredentialStore};
