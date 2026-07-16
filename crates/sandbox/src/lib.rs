//! Sandboxing policy layer for the mscode CLI.
//!
//! The [`Sandbox`] is the boundary that decides whether a tool action — file
//! read, file write, or process exec — is allowed to proceed. It performs only
//! **policy validation**: there is no OS-level sandboxing here (no seccomp on
//! Linux, no Job Object restrictions on Windows, no chroot). The point is to
//! keep agents from accidentally clobbering files outside the workspace or
//! running shell commands that were never allow-listed, not to defend against
//! a malicious Rust process.
//!
//! ## Scope of enforcement
//!
//! | Action      | Allowed                                                      |
//! |-------------|--------------------------------------------------------------|
//! | Read        | Inside workspace_root, or inside the system temp directory   |
//! | Write       | Inside workspace_root only                                   |
//! | Exec        | Match against an allowlist (e.g. `git`, `cargo`, `python`)   |
//!
//! `..` traversal is always rejected, even when the canonicalized target would
//! land inside the workspace.
//!
//! ## Limitations
//!
//! The exec allowlist is a *name* match on the command's argv[0] stem — it is
//! not a cryptographic verifier. Operators that need stronger guarantees must
//! layer OS sandboxing on top.

pub mod error;
pub mod matcher;
pub mod policy;

pub use error::{SandboxError, SandboxResult};
pub use matcher::PathMatcher;
pub use policy::{ExecAllowlist, Sandbox, SandboxConfig};

/// Re-export of the canonical workspace error type so callers can convert
/// sandbox results into the workspace-wide [`mscode_shared::MscodeError`].
pub use mscode_shared::MscodeError;
