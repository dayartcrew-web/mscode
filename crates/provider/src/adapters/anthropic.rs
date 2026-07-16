//! Anthropic Claude adapter.
//!
//! Targets the Anthropic Messages API at `https://api.anthropic.com/v1/messages`.
//! System prompts are lifted out of the message list and sent as the top-level
//! `system` field — Anthropic rejects a `system` role in `messages`.
//!
//! SSE event names used: `message_start`, `content_block_start`,
//! `content_block_delta`, `content_block_stop`, `message_delta`,
//! `message_stop`. Only the deltas we need to assemble the canonical
//! [`crate::types::LlmResponse`] are decoded; unknown event types are logged
//! and skipped.

use crate::provider::LlmProvider;
use crate::sse::{SseEvent, SseParser};
use crate::stream::{StreamEvent, StreamSink};
use crate::types::{
    ContentBlock, LlmMessage, LlmRequest, LlmResponse, MessageContent, Role, StopReason, ToolCall,
    ToolSpec, Usage,
};
use crate::{ProviderError, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Configuration for [`AnthropicProvider`].
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    /// API key. Required.
    pub api_key: String,
    /// Base URL. Defaults to the public Anthropic endpoint.
    pub endpoint: String,
}

impl AnthropicConfig {
    /// Construct a config with the given API key and the default endpoint.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        }
    }

    /// Override the endpoint URL (e.g. for a corporate proxy).
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

/// Anthropic Claude adapter. The HTTP client is built eagerly in [`new`].
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: reqwest::Client,
}

impl AnthropicProvider {
    /// Construct a new adapter with the given configuration. Builds the
    /// underlying HTTP client eagerly so callers cannot hit a surprise
    /// `expect` on the first request.
    pub fn new(config: AnthropicConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| ProviderError::Transient {
                status: 0,
                detail: format!("client build failed: {e}"),
            })?;
        Ok(Self { config, client })
    }

    fn client(&self) -> &reqwest::Client {
        &self.client
    }

    fn build_body(&self, req: &LlmRequest, stream: bool) -> Value {
        let (system, messages) = split_system(&req.messages, req.system_prompt.as_deref());
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": messages,
            "stream": stream,
        });
        if let Some(sys) = system {
            body["system"] = Value::String(sys);
        }
        if let Some(max) = req.max_tokens {
            body["max_tokens"] = Value::Number(max.into());
        } else {
            // Anthropic requires max_tokens; default to a sane ceiling.
            body["max_tokens"] = Value::Number(4096u32.into());
        }
        if let Some(temp) = req.temperature {
            if let Some(n) = serde_json::Number::from_f64(f64::from(temp)) {
                body["temperature"] = Value::Number(n);
            }
        }
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(req.tools.iter().map(tool_to_json).collect());
        }
        body
    }
}

fn split_system(messages: &[LlmMessage], explicit: Option<&str>) -> (Option<String>, Vec<Value>) {
    let mut system = explicit.map(String::from);
    let mut out = Vec::with_capacity(messages.len());
    for m in messages {
        match m.role {
            Role::System => {
                if system.is_none() {
                    system = Some(m.content.as_text());
                }
            }
            _ => out.push(serde_json::json!({
                "role": match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "user",
                    Role::System => "user",
                },
                "content": content_to_json(&m.content),
            })),
        }
    }
    (system, out)
}

fn content_to_json(c: &MessageContent) -> Value {
    match c {
        MessageContent::Text(s) => Value::String(s.clone()),
        MessageContent::Blocks(blocks) => Value::Array(blocks.iter().map(block_to_json).collect()),
    }
}

fn block_to_json(b: &ContentBlock) -> Value {
    match b {
        ContentBlock::Text { text } => serde_json::json!({"type": "text", "text": text}),
        ContentBlock::ToolUse { id, name, input } => serde_json::json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
        } => serde_json::json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
        }),
    }
}

fn tool_to_json(t: &ToolSpec) -> Value {
    serde_json::json!({
        "name": t.name,
        "description": t.description,
        "input_schema": t.input_schema,
    })
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, req: &LlmRequest) -> Result<LlmResponse> {
        let client = self.client();
        let body = self.build_body(req, false);
        let resp = client
            .post(&self.config.endpoint)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transient {
                status: 0,
                detail: format!("send failed: {e}"),
            })?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let bytes = resp.bytes().await.unwrap_or_default();
            return Err(ProviderError::from_http_status(status, &bytes));
        }
        let parsed: ApiResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Decode(format!("anthropic decode: {e}")))?;
        Ok(parsed.into())
    }

    async fn stream(&self, req: &LlmRequest, sink: &mut dyn StreamSink) -> Result<()> {
        let client = self.client();
        let body = self.build_body(req, true);
        let resp = client
            .post(&self.config.endpoint)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transient {
                status: 0,
                detail: format!("send failed: {e}"),
            })?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let bytes = resp.bytes().await.unwrap_or_default();
            let err = ProviderError::from_http_status(status, &bytes);
            sink.send(StreamEvent::Error(err)).await?;
            return Ok(());
        }
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        let mut assembled = AssembledResponse::new(&req.model);
        sink.send(StreamEvent::MessageStart(assembled.skeleton()))
            .await?;
        while let Some(chunk) = stream.next().await {
            let bytes: Bytes =
                chunk.map_err(|e| ProviderError::StreamParse(format!("transport: {e}")))?;
            parser.feed(&bytes);
            while let Some(event) = parser.next_event() {
                if let Some(out) = decode_anthropic_event(&event, &mut assembled)? {
                    sink.send(out).await?;
                }
            }
        }
        sink.send(StreamEvent::MessageStop(assembled.finalize()))
            .await?;
        Ok(())
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn supports_tools(&self) -> bool {
        true
    }
}

/// Accumulator that progressively builds the final [`LlmResponse`].
struct AssembledResponse {
    model: String,
    text: String,
    tool_calls: Vec<ToolCall>,
    tool_arg_buffers: Vec<String>,
    tool_names: Vec<String>,
    usage: Usage,
    stop_reason: StopReason,
}

impl AssembledResponse {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            text: String::new(),
            tool_calls: Vec::new(),
            tool_arg_buffers: Vec::new(),
            tool_names: Vec::new(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
        }
    }

    fn skeleton(&self) -> LlmResponse {
        LlmResponse {
            content: MessageContent::Text(String::new()),
            stop_reason: StopReason::Stop,
            tool_calls: Vec::new(),
            usage: Usage::default(),
            model: self.model.clone(),
        }
    }

    fn finalize(mut self) -> LlmResponse {
        for (i, buf) in self.tool_arg_buffers.iter().enumerate() {
            if buf.is_empty() {
                continue;
            }
            let args =
                serde_json::from_str::<Value>(buf).unwrap_or_else(|_| Value::String(buf.clone()));
            self.tool_calls.push(ToolCall {
                id: format!("toolu_stream_{i}"),
                name: self.tool_names.get(i).cloned().unwrap_or_default(),
                args,
            });
        }
        LlmResponse {
            content: MessageContent::Text(std::mem::take(&mut self.text)),
            stop_reason: self.stop_reason,
            tool_calls: self.tool_calls,
            usage: self.usage,
            model: self.model,
        }
    }
}

fn decode_anthropic_event(
    event: &SseEvent,
    assembled: &mut AssembledResponse,
) -> Result<Option<StreamEvent>> {
    match event.name.as_str() {
        "content_block_start" => handle_content_block_start(event, assembled),
        "content_block_delta" => handle_content_block_delta(event, assembled),
        "message_delta" => handle_message_delta(event, assembled),
        // All other event names (message_start, content_block_stop, ping,
        // message_stop, unknown) carry no data we accumulate.
        _ => Ok(None),
    }
}

/// Allocate per-block buffers when Anthropic starts a new content block and
/// record tool-use ids/names so later `input_json_delta` deltas can reattach.
fn handle_content_block_start(
    event: &SseEvent,
    assembled: &mut AssembledResponse,
) -> Result<Option<StreamEvent>> {
    let v: Value = match serde_json::from_str(&event.data) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if let Some(b) = v.get("index").and_then(|i| i.as_u64()) {
        let b = b as usize;
        while assembled.tool_arg_buffers.len() <= b {
            assembled.tool_arg_buffers.push(String::new());
            assembled.tool_names.push(String::new());
        }
    }
    if let Some(cb) = v.get("content_block") {
        if cb.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
            if let Some(id) = cb.get("id").and_then(|i| i.as_str()) {
                let idx = v
                    .get("index")
                    .and_then(|i| i.as_u64())
                    .map(|i| i as usize)
                    .unwrap_or(0);
                while assembled.tool_arg_buffers.len() <= idx {
                    assembled.tool_arg_buffers.push(String::new());
                    assembled.tool_names.push(String::new());
                }
                assembled.tool_names[idx] = cb
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_owned();
                assembled.tool_arg_buffers[idx] = format!("__id:{}__", id);
            }
        }
    }
    Ok(None)
}

/// Decode `content_block_delta` events: forward text deltas to the caller and
/// accumulate tool argument fragments, preserving the id marker injected by
/// `content_block_start`.
fn handle_content_block_delta(
    event: &SseEvent,
    assembled: &mut AssembledResponse,
) -> Result<Option<StreamEvent>> {
    let v: Value = match serde_json::from_str(&event.data) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let delta = match v.get("delta") {
        Some(d) => d,
        None => return Ok(None),
    };
    match delta.get("type").and_then(|t| t.as_str()) {
        Some("text_delta") => Ok(handle_text_delta(delta, assembled)),
        Some("input_json_delta") => Ok(handle_input_json_delta(&v, delta, assembled)),
        _ => Ok(None),
    }
}

/// Forward a `text_delta` to the caller and accumulate it into the response.
fn handle_text_delta(delta: &Value, assembled: &mut AssembledResponse) -> Option<StreamEvent> {
    let text = delta.get("text").and_then(|t| t.as_str()).unwrap_or("");
    if !text.is_empty() {
        assembled.text.push_str(text);
        return Some(StreamEvent::ContentDelta(text.to_owned()));
    }
    None
}

/// Append an `input_json_delta` to the indexed tool buffer, preserving any
/// `__id:...__` marker injected by `content_block_start`, and emit a
/// `ToolCallDelta` carrying the new fragment.
fn handle_input_json_delta(
    v: &Value,
    delta: &Value,
    assembled: &mut AssembledResponse,
) -> Option<StreamEvent> {
    let partial = delta
        .get("partial_json")
        .and_then(|p| p.as_str())
        .unwrap_or("");
    let idx = v
        .get("index")
        .and_then(|i| i.as_u64())
        .map(|i| i as usize)
        .unwrap_or(0);
    if idx >= assembled.tool_arg_buffers.len() {
        return None;
    }
    // Preserve any id marker prefix from content_block_start.
    let existing = std::mem::take(&mut assembled.tool_arg_buffers[idx]);
    let stripped = existing
        .strip_prefix("__id:")
        .and_then(|s| s.strip_suffix("__"))
        .map(|id| (id.to_owned(), String::new()))
        .unwrap_or((String::new(), existing));
    assembled.tool_arg_buffers[idx] = format!("{}{}", stripped.1, partial);
    Some(StreamEvent::ToolCallDelta {
        id: if stripped.0.is_empty() {
            None
        } else {
            Some(stripped.0)
        },
        name_delta: None,
        args_delta: Some(partial.to_owned()),
    })
}

/// Apply trailing `message_delta` updates: stop reason and final output
/// token accounting. Emits no stream events of its own.
fn handle_message_delta(
    event: &SseEvent,
    assembled: &mut AssembledResponse,
) -> Result<Option<StreamEvent>> {
    let v: Value = match serde_json::from_str(&event.data) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if let Some(reason) = v
        .get("delta")
        .and_then(|d| d.get("stop_reason"))
        .and_then(|s| s.as_str())
    {
        assembled.stop_reason = map_stop_reason(reason);
    }
    if let Some(usage) = v.get("usage") {
        if let Some(out) = usage.get("output_tokens").and_then(|t| t.as_u64()) {
            assembled.usage.output_tokens = out as u32;
        }
    }
    Ok(None)
}

fn map_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" | "stop_sequence" => StopReason::Stop,
        "max_tokens" => StopReason::Length,
        "tool_use" => StopReason::ToolUse,
        _ => StopReason::Stop,
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ApiResponse {
    content: Vec<ApiContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: ApiUsage,
    model: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ApiContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

impl From<ApiResponse> for LlmResponse {
    fn from(resp: ApiResponse) -> Self {
        let mut text = String::new();
        let mut tool_calls = Vec::new();
        for block in resp.content {
            match block {
                ApiContentBlock::Text { text: t } => text.push_str(&t),
                ApiContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        args: input,
                    });
                }
            }
        }
        let stop_reason = resp
            .stop_reason
            .as_deref()
            .map(map_stop_reason)
            .unwrap_or_default();
        LlmResponse {
            content: MessageContent::Text(text),
            stop_reason,
            tool_calls,
            usage: Usage {
                input_tokens: resp.usage.input_tokens,
                output_tokens: resp.usage.output_tokens,
            },
            model: resp.model,
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
    fn split_system_lifts_leading_system_message() {
        let msgs = vec![LlmMessage::system("be nice"), LlmMessage::user("hi")];
        let (sys, out) = split_system(&msgs, None);
        assert_eq!(sys.as_deref(), Some("be nice"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
    }

    #[test]
    fn split_system_prefers_explicit_over_message() {
        let msgs = vec![LlmMessage::system("from-msg")];
        let (sys, _) = split_system(&msgs, Some("explicit"));
        assert_eq!(sys.as_deref(), Some("explicit"));
    }

    #[test]
    fn build_body_includes_max_tokens_default() {
        let provider = AnthropicProvider::new(AnthropicConfig::new("k")).unwrap();
        let body = provider.build_body(&LlmRequest::single_user("m", "hi"), false);
        assert_eq!(body["max_tokens"], json!(4096u32));
        assert_eq!(body["model"], "m");
        assert_eq!(body["stream"], json!(false));
    }

    #[test]
    fn build_body_includes_tools_when_present() {
        let provider = AnthropicProvider::new(AnthropicConfig::new("k")).unwrap();
        let mut req = LlmRequest::single_user("m", "hi");
        req.tools.push(ToolSpec::new(
            "search",
            "search docs",
            json!({"type": "object"}),
        ));
        let body = provider.build_body(&req, true);
        assert_eq!(body["tools"][0]["name"], "search");
        assert_eq!(body["stream"], json!(true));
    }

    #[test]
    fn map_stop_reason_translates_known_values() {
        assert_eq!(map_stop_reason("end_turn"), StopReason::Stop);
        assert_eq!(map_stop_reason("max_tokens"), StopReason::Length);
        assert_eq!(map_stop_reason("tool_use"), StopReason::ToolUse);
        assert_eq!(map_stop_reason("unknown"), StopReason::Stop);
    }

    #[test]
    fn api_response_decodes_text_and_tool_use() {
        let raw = json!({
            "content": [
                {"type": "text", "text": "hi "},
                {"type": "tool_use", "id": "t1", "name": "n", "input": {"x": 1}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 5, "output_tokens": 7},
            "model": "claude-x"
        });
        let parsed: ApiResponse = serde_json::from_value(raw).unwrap();
        let resp: LlmResponse = parsed.into();
        assert_eq!(resp.content.as_text(), "hi ");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.usage.total(), 12);
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn config_with_endpoint_overrides_default() {
        let cfg = AnthropicConfig::new("k").with_endpoint("https://proxy.local");
        assert_eq!(cfg.endpoint, "https://proxy.local");
    }
}
