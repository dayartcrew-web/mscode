//! Strongly-typed identifiers used by session protocol events.
//!
//! Each ID is a wrapper around a [`uuid::Uuid`] so that callers cannot mix up a
//! session ID with a message ID at the type level. IDs are generated via
//! [`Id::new`] (which delegates to `Uuid::new_v4`) or parsed from strings.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Macro that generates a newtype wrapper around [`Uuid`] for a specific
/// domain identifier. Each generated type is `Copy`, `Eq`, `Hash`, and
/// serde-transparent so callers can treat it as a UUID externally.
macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Generate a fresh random identifier using `Uuid::new_v4`.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Read the underlying [`Uuid`].
            #[must_use]
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }
    };
}

id_type! {
    /// Unique identifier for a session log.
    SessionId
}

id_type! {
    /// Unique identifier for a single message within a session.
    MessageId
}

id_type! {
    /// Unique identifier for a tool call request.
    ToolCallId
}

id_type! {
    /// Unique identifier for a session checkpoint.
    CheckpointId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_generates_unique_values() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn session_id_roundtrips_through_string() {
        let id = SessionId::new();
        let s = id.to_string();
        let parsed: SessionId = s.parse().expect("valid uuid");
        assert_eq!(id, parsed);
    }

    #[test]
    fn ids_serialize_as_plain_uuid_strings() {
        let id = MessageId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        // Serialized form is the bare UUID string, no wrapper object.
        assert_eq!(json, format!("\"{}\"", id.as_uuid()));
        let back: MessageId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn invalid_uuid_string_is_rejected() {
        let result: std::result::Result<SessionId, _> = "not-a-uuid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn default_generates_a_fresh_id() {
        let id = ToolCallId::default();
        let other = ToolCallId::default();
        assert_ne!(id, other);
    }
}
