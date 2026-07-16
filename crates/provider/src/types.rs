//! LLM-domain types shared by every provider adapter.
//!
//! These types model the chat-completion surface common to Anthropic, OpenAI,
//! and Ollama. Each adapter is responsible for translating between this
//! canonical shape and the provider's wire format. All types are
//! `Serialize + Deserialize` because they cross the rollout persistence
//! boundary and may be replayed.

use serde::{Deserialize, Serialize};

/// A single chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmMessage {
    /// Author role (system, user, assistant, or tool result).
    pub role: Role,
    /// Either plain text or a sequence of structured content blocks.
    pub content: MessageContent,
}

impl LlmMessage {
    /// Construct a plain-text message with the given role.
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: MessageContent::Text(text.into()),
        }
    }

    /// Construct a system message.
    pub fn system(text: impl Into<String>) -> Self {
        Self::text(Role::System, text)
    }

    /// Construct a user message.
    pub fn user(text: impl Into<String>) -> Self {
        Self::text(Role::User, text)
    }

    /// Construct an assistant message.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::text(Role::Assistant, text)
    }
}

/// Author role for an [`LlmMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Top-level system instructions. Some providers (Anthropic) treat this
    /// specially rather than as a regular message in the history.
    #[default]
    System,
    /// User-supplied input.
    User,
    /// Model-generated output.
    Assistant,
    /// Returned tool call result fed back into the conversation.
    Tool,
}

/// Either a plain-text string or a sequence of structured content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain text — the common case for prompts and replies.
    Text(String),
    /// Structured blocks (text runs, tool calls, tool results). Used when a
    /// message needs to carry multiple parts, e.g. an assistant reply with
    /// both text and a tool call.
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Convenience constructor for plain text.
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    /// Flatten to a single concatenated text representation. Useful for
    /// mocks and tests where structured blocks are not relevant.
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => {
                let mut out = String::new();
                for b in blocks {
                    if let ContentBlock::Text { text } = b {
                        out.push_str(text);
                    }
                }
                out
            }
        }
    }
}

/// A single structured content block inside a [`MessageContent::Blocks`]
/// payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// A run of plain text.
    Text {
        /// The text body.
        text: String,
    },
    /// A tool call emitted by the assistant.
    ToolUse {
        /// Provider-assigned tool call id (e.g. `toolu_01abc...`).
        id: String,
        /// Tool name, must match a [`ToolSpec::name`] from the request.
        name: String,
        /// Raw JSON arguments payload.
        input: serde_json::Value,
    },
    /// A tool result returned to the model.
    ToolResult {
        /// Id of the originating tool call.
        tool_use_id: String,
        /// Result payload (provider-specific shape).
        content: serde_json::Value,
    },
}

/// An LLM completion request in the canonical shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Model identifier (e.g. `"claude-3-5-sonnet-20241022"`).
    pub model: String,
    /// Conversation history. The adapter may lift a leading system message
    /// out into `system_prompt` if the provider requires it.
    pub messages: Vec<LlmMessage>,
    /// Maximum output tokens. `None` lets the provider apply its default.
    pub max_tokens: Option<u32>,
    /// Sampling temperature in `[0.0, 2.0]`. `None` lets the provider apply
    /// its default.
    pub temperature: Option<f32>,
    /// Tools the model may call. Empty when tool-use is not requested.
    pub tools: Vec<ToolSpec>,
    /// Optional explicit system prompt. When present, adapters that require
    /// a separate system field (Anthropic) will use this and strip any
    /// leading system message from `messages`.
    pub system_prompt: Option<String>,
}

impl LlmRequest {
    /// Build a minimal request with a single user message and no tools.
    pub fn single_user(model: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: vec![LlmMessage::user(prompt)],
            max_tokens: None,
            temperature: None,
            tools: Vec::new(),
            system_prompt: None,
        }
    }

    /// Returns `true` if any tools were requested.
    pub fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }
}

/// A tool the model may invoke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name (must be unique within a request).
    pub name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// JSON schema describing the accepted input shape.
    pub input_schema: serde_json::Value,
}

impl ToolSpec {
    /// Construct a tool spec with the given name, description, and schema.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// A canonical completion response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    /// The model-generated content. May be empty if the model only emitted
    /// tool calls.
    pub content: MessageContent,
    /// Why generation stopped.
    pub stop_reason: StopReason,
    /// Tool calls emitted by the model. Empty unless `stop_reason ==
    /// StopReason::ToolUse`.
    pub tool_calls: Vec<ToolCall>,
    /// Token accounting.
    pub usage: Usage,
    /// Echoed model identifier (provider may rewrite aliases).
    pub model: String,
}

impl LlmResponse {
    /// Construct a minimal text response with zero usage.
    pub fn text(model: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            content: MessageContent::Text(text.into()),
            stop_reason: StopReason::Stop,
            tool_calls: Vec::new(),
            usage: Usage::default(),
            model: model.into(),
        }
    }
}

/// Reason the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model reached a natural stop or stop sequence.
    #[default]
    Stop,
    /// Output hit `max_tokens`.
    Length,
    /// Model emitted one or more tool calls; the caller should execute them
    /// and continue the conversation.
    ToolUse,
    /// Provider content filter blocked the response.
    ContentFilter,
}

/// A single tool call extracted from a model response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned tool call id.
    pub id: String,
    /// Tool name (matches a [`ToolSpec::name`] from the request).
    pub name: String,
    /// Parsed JSON arguments. `Null` if the model emitted malformed JSON.
    pub args: serde_json::Value,
}

/// Token usage accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens consumed by the prompt.
    pub input_tokens: u32,
    /// Tokens generated by the model.
    pub output_tokens: u32,
}

impl Usage {
    /// Total tokens billed for this exchange.
    pub fn total(&self) -> u32 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// Add the fields of `other` into `self`, returning a new value.
    #[must_use]
    pub fn add(&self, other: &Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_helpers_construct_expected_roles() {
        assert_eq!(LlmMessage::system("hi").role, Role::System);
        assert_eq!(LlmMessage::user("hi").role, Role::User);
        assert_eq!(LlmMessage::assistant("hi").role, Role::Assistant);
    }

    #[test]
    fn as_text_concatenates_block_text_runs() {
        let c = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "hello ".into(),
            },
            ContentBlock::Text {
                text: "world".into(),
            },
        ]);
        assert_eq!(c.as_text(), "hello world");
    }

    #[test]
    fn as_text_returns_text_for_text_variant() {
        assert_eq!(MessageContent::Text("abc".into()).as_text(), "abc");
    }

    #[test]
    fn message_content_text_constructor() {
        assert_eq!(
            MessageContent::text("xyz"),
            MessageContent::Text("xyz".into())
        );
    }

    #[test]
    fn single_user_request_has_no_tools() {
        let req = LlmRequest::single_user("m", "hi");
        assert!(!req.has_tools());
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, Role::User);
    }

    #[test]
    fn has_tools_true_when_tools_nonempty() {
        let mut req = LlmRequest::single_user("m", "hi");
        req.tools.push(ToolSpec::new(
            "search",
            "search docs",
            json!({"type": "object"}),
        ));
        assert!(req.has_tools());
    }

    #[test]
    fn usage_total_and_add_are_saturating() {
        let u = Usage {
            input_tokens: 10,
            output_tokens: 5,
        };
        assert_eq!(u.total(), 15);

        let combined = u.add(&Usage {
            input_tokens: u32::MAX,
            output_tokens: 1,
        });
        assert_eq!(combined.input_tokens, u32::MAX);
        assert_eq!(combined.output_tokens, 6);
    }

    #[test]
    fn content_block_serializes_with_type_tag() {
        let block = ContentBlock::Text { text: "x".into() };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "x");
    }

    #[test]
    fn tool_use_block_round_trips() {
        let block = ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "search".into(),
            input: json!({"q": "rust"}),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(v["type"], "tool_use");
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(block, back);
    }

    #[test]
    fn role_default_is_system() {
        assert_eq!(Role::default(), Role::System);
    }

    #[test]
    fn llm_response_text_constructor() {
        let r = LlmResponse::text("m", "hi");
        assert_eq!(r.stop_reason, StopReason::Stop);
        assert!(r.tool_calls.is_empty());
    }

    #[test]
    fn request_round_trips_through_json() {
        let req = LlmRequest {
            model: "m".into(),
            messages: vec![LlmMessage::user("hi")],
            max_tokens: Some(100),
            temperature: Some(0.5),
            tools: vec![ToolSpec::new("t", "d", json!({}))],
            system_prompt: Some("be nice".into()),
        };
        let v = serde_json::to_value(&req).unwrap();
        let back: LlmRequest = serde_json::from_value(v).unwrap();
        assert_eq!(req, back);
    }
}
