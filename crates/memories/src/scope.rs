//! Memory scope hierarchy.
//!
//! Scopes form a 4-layer hierarchy:
//! 1. [`Scope::Session`] — bound to a specific conversation thread.
//! 2. [`Scope::Project`] — bound to a project root directory (by hash).
//! 3. [`Scope::User`] — bound to the current user profile.
//! 4. [`Scope::Global`] — visible to every session and project.
//!
//! The `scope` column stores a tagged string of the form `session:<id>`,
//! `project:<hash>`, `user`, or `global`.

use serde::{Deserialize, Serialize};

use crate::error::{MemoryError, Result};

/// One layer of the memory hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    /// Visible only within a specific session/thread.
    Session(String),
    /// Visible within a specific project (identified by path hash).
    Project(String),
    /// Visible across all projects for the current user.
    User,
    /// Visible globally.
    Global,
}

impl Scope {
    /// Render the scope as a tagged string for the `scope` column.
    pub fn to_tag(&self) -> String {
        match self {
            Scope::Session(id) => format!("session:{id}"),
            Scope::Project(hash) => format!("project:{hash}"),
            Scope::User => "user".into(),
            Scope::Global => "global".into(),
        }
    }

    /// Parse a tagged scope string back into a [`Scope`].
    pub fn from_tag(tag: &str) -> Result<Self> {
        if let Some(id) = tag.strip_prefix("session:") {
            return Ok(Scope::Session(id.into()));
        }
        if let Some(hash) = tag.strip_prefix("project:") {
            return Ok(Scope::Project(hash.into()));
        }
        match tag {
            "user" => Ok(Scope::User),
            "global" => Ok(Scope::Global),
            other => Err(MemoryError::InvalidInput(format!(
                "unknown scope tag: {other}"
            ))),
        }
    }
}

/// Stable hash of a project root path so memories can be keyed by directory
/// without leaking the absolute path into the `scope` tag.
pub fn project_root_hash(project_root: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(project_root.as_bytes());
    hex_lower(&digest[..16])
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_tag_roundtrips() {
        for s in [
            Scope::Session("abc".into()),
            Scope::Project("deadbeef".into()),
            Scope::User,
            Scope::Global,
        ] {
            let tag = s.to_tag();
            let back = Scope::from_tag(&tag).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn from_tag_rejects_unknown() {
        let err = Scope::from_tag("wat:never").unwrap_err();
        assert!(matches!(err, MemoryError::InvalidInput(_)));
    }

    #[test]
    fn project_root_hash_is_stable_and_short() {
        let h = project_root_hash("/work/foo");
        let h2 = project_root_hash("/work/foo");
        assert_eq!(h, h2);
        assert_eq!(h.len(), 32); // 16 bytes hex
        assert_ne!(h, project_root_hash("/work/bar"));
    }
}
