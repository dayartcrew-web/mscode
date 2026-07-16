//! Session event types — the canonical record format appended to a session log.
//!
//! Events are append-only and ordered chronologically per session. The
//! [`SessionEvent`] enum is serialized with an internal `type` tag so older
//! readers can skip variants they do not understand via
//! [`SessionEvent::Unknown`]. The unknown variant preserves the raw JSON so no
//! information is lost when replaying logs written by a newer producer.

use crate::ids::{CheckpointId, MessageId, SessionId, ToolCallId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::path::PathBuf;

/// Internal mirror of [`SessionEvent`] without the `Unknown` variant.
///
/// Used by the custom [`Deserialize`] impl on [`SessionEvent`] to parse known
/// variants via the standard derive, while unknown tags fall back to
/// [`SessionEvent::Unknown`] carrying the raw payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum KnownSessionEvent {
    SessionStarted {
        id: SessionId,
        cwd: PathBuf,
        project_root: Option<PathBuf>,
        timestamp: DateTime<Utc>,
    },
    MessageAdded {
        message_id: MessageId,
        role: crate::message::Role,
        content: crate::message::MessageContent,
        timestamp: DateTime<Utc>,
    },
    ToolCallRequested {
        call_id: ToolCallId,
        tool: String,
        args: Value,
        timestamp: DateTime<Utc>,
    },
    ToolCallCompleted {
        call_id: ToolCallId,
        output: Value,
        timestamp: DateTime<Utc>,
    },
    CheckpointCreated {
        checkpoint_id: CheckpointId,
        label: String,
        timestamp: DateTime<Utc>,
    },
    SessionPaused {
        timestamp: DateTime<Utc>,
    },
    SessionResumed {
        cwd: PathBuf,
        timestamp: DateTime<Utc>,
    },
    SessionEnded {
        reason: SessionEndReason,
        timestamp: DateTime<Utc>,
    },
}

impl From<KnownSessionEvent> for SessionEvent {
    fn from(value: KnownSessionEvent) -> Self {
        match value {
            KnownSessionEvent::SessionStarted {
                id,
                cwd,
                project_root,
                timestamp,
            } => Self::SessionStarted {
                id,
                cwd,
                project_root,
                timestamp,
            },
            KnownSessionEvent::MessageAdded {
                message_id,
                role,
                content,
                timestamp,
            } => Self::MessageAdded {
                message_id,
                role,
                content,
                timestamp,
            },
            KnownSessionEvent::ToolCallRequested {
                call_id,
                tool,
                args,
                timestamp,
            } => Self::ToolCallRequested {
                call_id,
                tool,
                args,
                timestamp,
            },
            KnownSessionEvent::ToolCallCompleted {
                call_id,
                output,
                timestamp,
            } => Self::ToolCallCompleted {
                call_id,
                output,
                timestamp,
            },
            KnownSessionEvent::CheckpointCreated {
                checkpoint_id,
                label,
                timestamp,
            } => Self::CheckpointCreated {
                checkpoint_id,
                label,
                timestamp,
            },
            KnownSessionEvent::SessionPaused { timestamp } => Self::SessionPaused { timestamp },
            KnownSessionEvent::SessionResumed { cwd, timestamp } => {
                Self::SessionResumed { cwd, timestamp }
            }
            KnownSessionEvent::SessionEnded { reason, timestamp } => {
                Self::SessionEnded { reason, timestamp }
            }
        }
    }
}

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    /// User-initiated graceful shutdown.
    Stopped,
    /// Hit an internal hard limit (e.g. token budget).
    Limit,
    /// Aborted due to an unrecoverable error.
    Failed,
    /// Cancelled externally.
    Cancelled,
}

/// A checkpoint marker — a named point-in-time state capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Stable identifier for the checkpoint.
    pub id: CheckpointId,
    /// Human-readable label.
    pub label: String,
    /// When the checkpoint was created.
    pub timestamp: DateTime<Utc>,
}

/// Canonical event types appended to a session rollout log.
///
/// Forward-compatibility: variants added in future versions are deserialized
/// as [`SessionEvent::Unknown`] carrying the raw JSON payload, so older
/// readers do not have to know about them. This is achieved via custom
/// [`Serialize`] and [`Deserialize`] implementations.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// Emitted when a session is first opened.
    SessionStarted {
        /// Identifier of the session this event belongs to.
        id: SessionId,
        /// Working directory at session start.
        cwd: PathBuf,
        /// Optional project root (e.g. git root) if known.
        project_root: Option<PathBuf>,
        /// Wall-clock time the session began.
        timestamp: DateTime<Utc>,
    },
    /// A new message was added to the session transcript.
    MessageAdded {
        /// Identifier of the message.
        message_id: MessageId,
        /// Authoring role.
        role: crate::message::Role,
        /// Body of the message.
        content: crate::message::MessageContent,
        /// When the message was emitted.
        timestamp: DateTime<Utc>,
    },
    /// A tool call has been requested (about to execute).
    ToolCallRequested {
        /// Identifier for this call.
        call_id: ToolCallId,
        /// Name of the tool being invoked.
        tool: String,
        /// Serialized tool arguments.
        args: Value,
        /// When the request was emitted.
        timestamp: DateTime<Utc>,
    },
    /// A tool call finished and produced output.
    ToolCallCompleted {
        /// Identifier linking back to the originating `ToolCallRequested`.
        call_id: ToolCallId,
        /// Serialized tool output.
        output: Value,
        /// When the result was emitted.
        timestamp: DateTime<Utc>,
    },
    /// A named checkpoint was recorded.
    CheckpointCreated {
        /// Identifier of the checkpoint.
        checkpoint_id: CheckpointId,
        /// Human-readable label.
        label: String,
        /// When the checkpoint was emitted.
        timestamp: DateTime<Utc>,
    },
    /// Session was paused (e.g. user stepped away).
    SessionPaused {
        /// When the pause was emitted.
        timestamp: DateTime<Utc>,
    },
    /// Session resumed after a pause.
    SessionResumed {
        /// Working directory at resume time (may differ from start).
        cwd: PathBuf,
        /// When the resume was emitted.
        timestamp: DateTime<Utc>,
    },
    /// Session ended.
    SessionEnded {
        /// Why the session ended.
        reason: SessionEndReason,
        /// When the end was emitted.
        timestamp: DateTime<Utc>,
    },
    /// Catch-all for event variants introduced by newer producers.
    ///
    /// Older readers preserve the raw payload so the log remains intact.
    Unknown {
        /// The unparsed JSON object (including its `type` tag).
        payload: Value,
    },
}

impl Serialize for SessionEvent {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // The Unknown variant serializes its raw payload verbatim so that
        // round-tripping preserves the original bytes.
        if let Self::Unknown { payload } = self {
            return payload.serialize(serializer);
        }
        // All other variants route through the derived serializer.
        let known: KnownSessionEvent = self.clone().into_known();
        known.serialize(serializer)
    }
}

impl SessionEvent {
    /// Convert a non-Unknown [`SessionEvent`] into the internal known-variant
    /// mirror used for serialization. Panics if called on `Unknown`.
    fn into_known(self) -> KnownSessionEvent {
        match self {
            Self::SessionStarted {
                id,
                cwd,
                project_root,
                timestamp,
            } => KnownSessionEvent::SessionStarted {
                id,
                cwd,
                project_root,
                timestamp,
            },
            Self::MessageAdded {
                message_id,
                role,
                content,
                timestamp,
            } => KnownSessionEvent::MessageAdded {
                message_id,
                role,
                content,
                timestamp,
            },
            Self::ToolCallRequested {
                call_id,
                tool,
                args,
                timestamp,
            } => KnownSessionEvent::ToolCallRequested {
                call_id,
                tool,
                args,
                timestamp,
            },
            Self::ToolCallCompleted {
                call_id,
                output,
                timestamp,
            } => KnownSessionEvent::ToolCallCompleted {
                call_id,
                output,
                timestamp,
            },
            Self::CheckpointCreated {
                checkpoint_id,
                label,
                timestamp,
            } => KnownSessionEvent::CheckpointCreated {
                checkpoint_id,
                label,
                timestamp,
            },
            Self::SessionPaused { timestamp } => KnownSessionEvent::SessionPaused { timestamp },
            Self::SessionResumed { cwd, timestamp } => {
                KnownSessionEvent::SessionResumed { cwd, timestamp }
            }
            Self::SessionEnded { reason, timestamp } => {
                KnownSessionEvent::SessionEnded { reason, timestamp }
            }
            Self::Unknown { .. } => {
                unreachable!("into_known must not be called on Unknown variant")
            }
        }
    }
}

impl<'de> Deserialize<'de> for SessionEvent {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Parse the value as a raw JSON value so we can inspect the tag and
        // decide whether to dispatch to a known variant or fall back to the
        // Unknown catch-all.
        let value = serde_json::Value::deserialize(deserializer)?;
        // First try to parse via the known enum. If the tag is unknown, the
        // derive returns an error and we capture the raw payload.
        match serde_json::from_value::<KnownSessionEvent>(value.clone()) {
            Ok(known) => Ok(known.into()),
            Err(_) => {
                let tag = value
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                // Known tags with malformed payloads should propagate the
                // parse error; truly unknown tags should fall back.
                let known_tags = [
                    "session_started",
                    "message_added",
                    "tool_call_requested",
                    "tool_call_completed",
                    "checkpoint_created",
                    "session_paused",
                    "session_resumed",
                    "session_ended",
                ];
                if known_tags.contains(&tag) {
                    Err(serde::de::Error::custom(format!(
                        "malformed `{tag}` event payload"
                    )))
                } else {
                    Ok(SessionEvent::Unknown { payload: value })
                }
            }
        }
    }
}

impl SessionEvent {
    /// Returns the wall-clock timestamp associated with the event, if any.
    ///
    /// `Unknown` events do not have a known timestamp field shape and return `None`.
    #[must_use]
    pub fn timestamp(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::SessionStarted { timestamp, .. }
            | Self::MessageAdded { timestamp, .. }
            | Self::ToolCallRequested { timestamp, .. }
            | Self::ToolCallCompleted { timestamp, .. }
            | Self::CheckpointCreated { timestamp, .. }
            | Self::SessionPaused { timestamp }
            | Self::SessionResumed { timestamp, .. }
            | Self::SessionEnded { timestamp, .. } => Some(*timestamp),
            Self::Unknown { .. } => None,
        }
    }

    /// Returns `true` if this event is the catch-all `Unknown` variant.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{MessageContent, Role};
    use std::path::Path;

    fn dummy_started() -> SessionEvent {
        SessionEvent::SessionStarted {
            id: SessionId::new(),
            cwd: PathBuf::from("/tmp"),
            project_root: Some(PathBuf::from("/tmp")),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn session_started_roundtrips() {
        let event = dummy_started();
        let json = serde_json::to_string(&event).expect("serialize");
        let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
        assert!(back.timestamp().is_some());
    }

    #[test]
    fn message_added_roundtrips() {
        let event = SessionEvent::MessageAdded {
            message_id: MessageId::new(),
            role: Role::User,
            content: MessageContent::text("hi"),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn tool_call_requested_roundtrips() {
        let event = SessionEvent::ToolCallRequested {
            call_id: ToolCallId::new(),
            tool: "shell".into(),
            args: serde_json::json!({"cmd": "ls"}),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn tool_call_completed_roundtrips() {
        let event = SessionEvent::ToolCallCompleted {
            call_id: ToolCallId::new(),
            output: serde_json::json!({"stdout": "ok"}),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn checkpoint_created_roundtrips() {
        let event = SessionEvent::CheckpointCreated {
            checkpoint_id: CheckpointId::new(),
            label: "after-init".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn session_paused_and_resumed_roundtrip() {
        let paused = SessionEvent::SessionPaused {
            timestamp: Utc::now(),
        };
        let resumed = SessionEvent::SessionResumed {
            cwd: PathBuf::from("/new"),
            timestamp: Utc::now(),
        };
        for ev in [paused, resumed] {
            let json = serde_json::to_string(&ev).expect("serialize");
            let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(ev, back);
        }
    }

    #[test]
    fn session_ended_roundtrips() {
        let event = SessionEvent::SessionEnded {
            reason: SessionEndReason::Stopped,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn unknown_variant_preserves_payload() {
        // A future producer might emit a variant we don't know about. We
        // should preserve it on round-trip.
        let raw = serde_json::json!({
            "type": "future_feature",
            "data": 42,
        });
        let parsed: SessionEvent =
            serde_json::from_value(raw.clone()).expect("deserialize unknown");
        assert!(parsed.is_unknown());
        assert!(parsed.timestamp().is_none());
        // And serializing it back should keep the payload.
        let back = serde_json::to_value(&parsed).expect("serialize");
        assert_eq!(back, raw);
    }

    #[test]
    fn cwd_is_pathbuf_for_started_event() {
        let event = SessionEvent::SessionStarted {
            id: SessionId::new(),
            cwd: PathBuf::from("/tmp"),
            project_root: None,
            timestamp: Utc::now(),
        };
        if let SessionEvent::SessionStarted { cwd, .. } = &event {
            assert_eq!(cwd, Path::new("/tmp"));
        } else {
            panic!("expected SessionStarted");
        }
    }
}
