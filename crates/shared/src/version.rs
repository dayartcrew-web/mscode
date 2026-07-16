//! Version metadata shared across the mscode workspace.
//!
//! [`MscodeVersion`] is the canonical type rendered by the `mscode version`
//! command and embedded in tooling / log output. It carries semantic version
//! fields plus an optional git SHA so local builds can still identify
//! provenance.

use serde::{Deserialize, Serialize};

/// Semantic version + optional git provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MscodeVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
}

impl MscodeVersion {
    /// Build a version from the calling crate's `CARGO_PKG_VERSION_*` env vars.
    ///
    /// `git_sha` is optional — pass `Some(short_sha)` when the build script
    /// captures `VERGEN_GIT_SHA` (or equivalent), otherwise pass `None` for
    /// release builds.
    pub const fn from_cargo_env(
        major: u32,
        minor: u32,
        patch: u32,
        git_sha: Option<String>,
    ) -> Self {
        Self {
            major,
            minor,
            patch,
            git_sha,
        }
    }

    /// Render the version as `MAJOR.MINOR.PATCH` (no pre-release / git suffix).
    pub fn semver_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// Render the version with git SHA appended when present.
    ///
    /// Examples:
    /// - `0.1.0`
    /// - `0.1.0+abc1234`
    pub fn full_string(&self) -> String {
        match &self.git_sha {
            Some(sha) => format!("{}+{}", self.semver_string(), sha),
            None => self.semver_string(),
        }
    }
}

impl std::fmt::Display for MscodeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.full_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MscodeVersion {
        MscodeVersion {
            major: 0,
            minor: 1,
            patch: 0,
            git_sha: Some("abc1234".into()),
        }
    }

    #[test]
    fn semver_string_omits_git_sha() {
        assert_eq!(sample().semver_string(), "0.1.0");
    }

    #[test]
    fn full_string_appends_sha_when_present() {
        assert_eq!(sample().full_string(), "0.1.0+abc1234");
    }

    #[test]
    fn full_string_omits_suffix_when_sha_none() {
        let v = MscodeVersion {
            major: 1,
            minor: 2,
            patch: 3,
            git_sha: None,
        };
        assert_eq!(v.full_string(), "1.2.3");
    }

    #[test]
    fn display_matches_full_string() {
        let v = sample();
        assert_eq!(format!("{v}"), v.full_string());
    }

    #[test]
    fn serde_roundtrip_preserves_all_fields() {
        let original = sample();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: MscodeVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn serde_omits_git_sha_when_none() {
        let v = MscodeVersion {
            major: 1,
            minor: 0,
            patch: 0,
            git_sha: None,
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("git_sha"));
    }

    #[test]
    fn serde_accepts_missing_git_sha_on_deserialize() {
        let json = r#"{"major":1,"minor":0,"patch":0}"#;
        let parsed: MscodeVersion = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.git_sha, None);
        assert_eq!(parsed.major, 1);
    }

    #[test]
    fn from_cargo_env_const_constructor_builds_expected_struct() {
        let v = MscodeVersion::from_cargo_env(0, 2, 5, Some("deadbee".into()));
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 5);
        assert_eq!(v.git_sha.as_deref(), Some("deadbee"));
    }
}
