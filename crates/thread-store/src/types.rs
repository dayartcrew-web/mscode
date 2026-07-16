//! Public data types for sessions.

use serde::{Deserialize, Serialize};

/// One conversation thread / session row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Stable unique identifier (typically a UUID v4 string).
    pub id: String,
    /// Working directory the session was started in.
    pub cwd: String,
    /// Optional project root (heuristic; may equal `cwd`).
    pub project_root: Option<String>,
    /// ISO-8601 timestamp the session was created.
    pub created_at: String,
    /// ISO-8601 timestamp of the last activity.
    pub updated_at: String,
    /// Optional human-readable summary (set on close or via `update_summary`).
    pub summary: Option<String>,
}

/// Input for creating a new session. Timestamps default to now (UTC) when
/// omitted; the store fills them in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSession {
    pub id: String,
    pub cwd: String,
    pub project_root: Option<String>,
    pub created_at: Option<String>,
    pub summary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_serde_roundtrip() {
        let s = Session {
            id: "abc".into(),
            cwd: "/tmp".into(),
            project_root: Some("/tmp".into()),
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-02T00:00:00Z".into(),
            summary: Some("hello".into()),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn new_session_optional_fields_round_trip_with_nulls() {
        let n = NewSession {
            id: "x".into(),
            cwd: "/x".into(),
            project_root: None,
            created_at: None,
            summary: None,
        };
        let json = serde_json::to_string(&n).unwrap();
        assert!(json.contains("null"));
    }
}
