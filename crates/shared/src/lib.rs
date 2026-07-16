//! Shared types for the mscode CLI workspace.
//!
//! This crate is the foundation of the workspace: every other member depends on
//! it for the canonical error type and the small shared serde models that cross
//! crate boundaries (CLI ↔ config ↔ future core/runtime). Keep this crate
//! dependency-light and synchronous — no I/O, no async runtime.

pub mod error;
pub mod version;

pub use error::{MscodeError, Result};
pub use version::MscodeVersion;
