//! Message types that flow through a session log.

use crate::ids::MessageId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Who authored a given message in the session transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// End-user input.
    User,
    /// Assistant (model) response.
    Assistant,
    /// Out-of-band system instruction.
    System,
    /// Output returned from a tool invocation.
    Tool,
}

/// Structured variant of [`MessageContent`] used when a message carries
/// typed blocks (tool calls, results, multi-part text) rather than free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text segment.
    Text {
        /// The literal text payload.
        text: String,
    },
    /// Reference to a tool call that the assistant requested.
    ToolUse {
        /// Identifier linking back to the originating `ToolCallRequested` event.
        call_id: String,
        /// Name of the tool being invoked.
        name: String,
        /// Serialized tool arguments (caller-defined shape).
        arguments: serde_json::Value,
    },
    /// Result of a tool call returned to the model.
    ToolResult {
        /// Identifier linking back to the originating `ToolCallRequested` event.
        call_id: String,
        /// Serialized tool output (caller-defined shape).
        output: serde_json::Value,
    },
}

/// Either plain-text content or a vector of structured blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Free-form text — common case for user/assistant turns.
    Text(String),
    /// Structured blocks (tool uses, multi-part output, etc.).
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Build a `MessageContent::Text` from any string-like input.
    #[must_use]
    pub fn text<T: Into<String>>(value: T) -> Self {
        Self::Text(value.into())
    }

    /// Build a `MessageContent::Blocks` from a vector of blocks.
    #[must_use]
    pub fn blocks(items: Vec<ContentBlock>) -> Self {
        Self::Blocks(items)
    }
}

/// A single message recorded against a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessage {
    /// Stable identifier for this message.
    pub id: MessageId,
    /// Who sent the message.
    pub role: Role,
    /// The message body (plain text or structured blocks).
    pub content: MessageContent,
    /// When the message was emitted.
    pub timestamp: DateTime<Utc>,
}

impl SessionMessage {
    /// Build a new message with the given fields.
    #[must_use]
    pub fn new(
        id: MessageId,
        role: Role,
        content: MessageContent,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            role,
            content,
            timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_content_roundtrips() {
        let msg = SessionMessage::new(
            MessageId::new(),
            Role::User,
            MessageContent::text("hello"),
            Utc::now(),
        );
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: SessionMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg, back);
    }

    #[test]
    fn blocks_content_roundtrips() {
        let blocks = vec![ContentBlock::Text {
            text: "multi".into(),
        }];
        let msg = SessionMessage::new(
            MessageId::new(),
            Role::Assistant,
            MessageContent::blocks(blocks),
            Utc::now(),
        );
        let json = serde_json::to_string(&msg).expect("serialize");
        let back: SessionMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg, back);
    }

    #[test]
    fn role_serializes_as_lowercase() {
        let json = serde_json::to_string(&Role::Assistant).expect("serialize");
        assert_eq!(json, "\"assistant\"");
    }

    #[test]
    fn tool_use_block_roundtrips() {
        let block = ContentBlock::ToolUse {
            call_id: "abc".into(),
            name: "shell".into(),
            arguments: serde_json::json!({"cmd": "ls"}),
        };
        let json = serde_json::to_string(&block).expect("serialize");
        let back: ContentBlock = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(block, back);
    }
}
