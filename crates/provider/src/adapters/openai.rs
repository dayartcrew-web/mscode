//! OpenAI Chat Completions adapter.
//!
//! Targets `POST https://api.openai.com/v1/chat/completions`. System messages
//! stay inline in the message list (OpenAI accepts a `system` role there).
//! Streaming uses OpenAI's standard `data: {...}` SSE framing with a
//! terminal `data: [DONE]` sentinel.

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

const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

/// Configuration for [`OpenAiProvider`].
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// API key. Required.
    pub api_key: String,
    /// Base URL. Defaults to the public OpenAI endpoint.
    pub endpoint: String,
}

impl OpenAiConfig {
    /// Construct with the given key and default endpoint.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        }
    }

    /// Override endpoint (e.g. for Azure OpenAI or a proxy).
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

/// OpenAI Chat Completions adapter. HTTP client built eagerly in [`new`].
#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    config: OpenAiConfig,
    client: reqwest::Client,
}

impl OpenAiProvider {
    /// Construct a new adapter. Builds the HTTP client eagerly so callers
    /// cannot hit a surprise `expect` on the first request.
    pub fn new(config: OpenAiConfig) -> Result<Self> {
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
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": req.messages.iter().map(message_to_json).collect::<Vec<_>>(),
            "stream": stream,
        });
        if let Some(max) = req.max_tokens {
            body["max_tokens"] = Value::Number(max.into());
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

fn message_to_json(m: &LlmMessage) -> Value {
    let role = match m.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    serde_json::json!({
        "role": role,
        "content": content_to_json(&m.content),
    })
}

fn content_to_json(c: &MessageContent) -> Value {
    match c {
        MessageContent::Text(s) => Value::String(s.clone()),
        MessageContent::Blocks(blocks) => {
            // For OpenAI, flatten text blocks back to a single string for
            // simplicity in the request payload.
            let mut s = String::new();
            for b in blocks {
                if let ContentBlock::Text { text } = b {
                    s.push_str(text);
                }
            }
            Value::String(s)
        }
    }
}

fn tool_to_json(t: &ToolSpec) -> Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": t.name,
            "description": t.description,
            "parameters": t.input_schema,
        }
    })
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, req: &LlmRequest) -> Result<LlmResponse> {
        let client = self.client();
        let body = self.build_body(req, false);
        let resp = client
            .post(&self.config.endpoint)
            .bearer_auth(&self.config.api_key)
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
            .map_err(|e| ProviderError::Decode(format!("openai decode: {e}")))?;
        Ok(parsed.into())
    }

    async fn stream(&self, req: &LlmRequest, sink: &mut dyn StreamSink) -> Result<()> {
        let client = self.client();
        let body = self.build_body(req, true);
        let resp = client
            .post(&self.config.endpoint)
            .bearer_auth(&self.config.api_key)
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
                if event.data == "[DONE]" {
                    continue;
                }
                if let Some(out) = decode_openai_event(&event, &mut assembled)? {
                    sink.send(out).await?;
                }
            }
        }
        sink.send(StreamEvent::MessageStop(assembled.finalize()))
            .await?;
        Ok(())
    }

    fn name(&self) -> &str {
        "openai"
    }

    fn supports_tools(&self) -> bool {
        true
    }
}

struct AssembledResponse {
    model: String,
    text: String,
    tool_calls: Vec<ToolCall>,
    tool_arg_buffers: Vec<String>,
    tool_ids: Vec<String>,
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
            tool_ids: Vec::new(),
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
        for i in 0..self.tool_arg_buffers.len() {
            let buf = &self.tool_arg_buffers[i];
            if buf.is_empty() {
                continue;
            }
            let args =
                serde_json::from_str::<Value>(buf).unwrap_or_else(|_| Value::String(buf.clone()));
            self.tool_calls.push(ToolCall {
                id: self.tool_ids.get(i).cloned().unwrap_or_default(),
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

fn decode_openai_event(
    event: &SseEvent,
    assembled: &mut AssembledResponse,
) -> Result<Option<StreamEvent>> {
    let v: Value = match serde_json::from_str(&event.data) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let choices = match v.get("choices").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return Ok(None),
    };
    let first = match choices.first() {
        Some(c) => c,
        None => return Ok(None),
    };
    if let Some(delta) = first.get("delta") {
        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
            if !content.is_empty() {
                assembled.text.push_str(content);
                return Ok(Some(StreamEvent::ContentDelta(content.to_owned())));
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
            if let Some(out) = handle_tool_call_delta(calls, assembled) {
                return Ok(Some(out));
            }
        }
    }
    if let Some(reason) = first.get("finish_reason").and_then(|f| f.as_str()) {
        assembled.stop_reason = map_stop_reason(reason);
    }
    if let Some(usage) = v.get("usage") {
        if let Some(p) = usage.get("prompt_tokens").and_then(|t| t.as_u64()) {
            assembled.usage.input_tokens = p as u32;
        }
        if let Some(c) = usage.get("completion_tokens").and_then(|t| t.as_u64()) {
            assembled.usage.output_tokens = c as u32;
        }
    }
    Ok(None)
}

/// Walk an OpenAI `delta.tool_calls` array, growing per-index buffers, then
/// forward the first arguments fragment as a `ToolCallDelta`. Returns
/// `None` when no arguments payload is present in this delta.
fn handle_tool_call_delta(
    calls: &[Value],
    assembled: &mut AssembledResponse,
) -> Option<StreamEvent> {
    for call in calls {
        let idx = call
            .get("index")
            .and_then(|i| i.as_u64())
            .map(|i| i as usize)
            .unwrap_or(0);
        while assembled.tool_arg_buffers.len() <= idx {
            assembled.tool_arg_buffers.push(String::new());
            assembled.tool_ids.push(String::new());
            assembled.tool_names.push(String::new());
        }
        if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
            assembled.tool_ids[idx] = id.to_owned();
        }
        let function = call.get("function");
        if let Some(name) = function
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
        {
            assembled.tool_names[idx].push_str(name);
        }
        if let Some(args) = function
            .and_then(|f| f.get("arguments"))
            .and_then(|a| a.as_str())
        {
            assembled.tool_arg_buffers[idx].push_str(args);
            return Some(StreamEvent::ToolCallDelta {
                id: if assembled.tool_ids[idx].is_empty() {
                    None
                } else {
                    Some(assembled.tool_ids[idx].clone())
                },
                name_delta: None,
                args_delta: Some(args.to_owned()),
            });
        }
    }
    None
}

fn map_stop_reason(s: &str) -> StopReason {
    match s {
        "stop" => StopReason::Stop,
        "length" => StopReason::Length,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::ContentFilter,
        _ => StopReason::Stop,
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ApiResponse {
    choices: Vec<ApiChoice>,
    #[serde(default)]
    usage: ApiUsage,
    model: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApiChoice {
    #[serde(default)]
    finish_reason: Option<String>,
    message: ApiMessage,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApiMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ApiToolCall>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApiToolCall {
    id: String,
    function: ApiToolFunction,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApiToolFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ApiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

impl From<ApiResponse> for LlmResponse {
    fn from(resp: ApiResponse) -> Self {
        let text = resp
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();
        let tool_calls = resp
            .choices
            .first()
            .map(|c| {
                c.message
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        let args = serde_json::from_str::<Value>(&tc.function.arguments)
                            .unwrap_or_else(|_| Value::String(tc.function.arguments.clone()));
                        ToolCall {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            args,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let stop_reason = resp
            .choices
            .first()
            .and_then(|c| c.finish_reason.as_deref())
            .map(map_stop_reason)
            .unwrap_or_default();
        LlmResponse {
            content: MessageContent::Text(text),
            stop_reason,
            tool_calls,
            usage: Usage {
                input_tokens: resp.usage.prompt_tokens,
                output_tokens: resp.usage.completion_tokens,
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
    fn build_body_preserves_system_role() {
        let provider = OpenAiProvider::new(OpenAiConfig::new("k")).unwrap();
        let req = LlmRequest {
            model: "gpt-x".into(),
            messages: vec![LlmMessage::system("sys"), LlmMessage::user("hi")],
            max_tokens: Some(10),
            temperature: None,
            tools: Vec::new(),
            system_prompt: None,
        };
        let body = provider.build_body(&req, false);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(body["max_tokens"], json!(10u32));
    }

    #[test]
    fn build_body_emits_function_tools() {
        let provider = OpenAiProvider::new(OpenAiConfig::new("k")).unwrap();
        let mut req = LlmRequest::single_user("m", "hi");
        req.tools
            .push(ToolSpec::new("t", "d", json!({"type": "object"})));
        let body = provider.build_body(&req, false);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "t");
    }

    #[test]
    fn map_stop_reason_translates_known_values() {
        assert_eq!(map_stop_reason("stop"), StopReason::Stop);
        assert_eq!(map_stop_reason("length"), StopReason::Length);
        assert_eq!(map_stop_reason("tool_calls"), StopReason::ToolUse);
        assert_eq!(map_stop_reason("content_filter"), StopReason::ContentFilter);
        assert_eq!(map_stop_reason("anything"), StopReason::Stop);
    }

    #[test]
    fn api_response_decodes_content_and_usage() {
        let raw = json!({
            "model": "gpt-x",
            "choices": [{
                "finish_reason": "stop",
                "message": {"content": "hi", "tool_calls": []}
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2}
        });
        let parsed: ApiResponse = serde_json::from_value(raw).unwrap();
        let resp: LlmResponse = parsed.into();
        assert_eq!(resp.content.as_text(), "hi");
        assert_eq!(resp.usage.total(), 5);
        assert_eq!(resp.stop_reason, StopReason::Stop);
    }

    #[test]
    fn api_response_decodes_tool_calls() {
        let raw = json!({
            "model": "gpt-x",
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {"name": "search", "arguments": "{\"q\":\"rust\"}"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 8}
        });
        let parsed: ApiResponse = serde_json::from_value(raw).unwrap();
        let resp: LlmResponse = parsed.into();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].args, json!({"q": "rust"}));
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn config_with_endpoint_overrides_default() {
        let cfg = OpenAiConfig::new("k").with_endpoint("https://azure.local");
        assert_eq!(cfg.endpoint, "https://azure.local");
    }
}
