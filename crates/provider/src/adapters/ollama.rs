//! Direct Ollama adapter.
//!
//! Talks to a local Ollama server at `http://localhost:11434/api/chat` using
//! its native NDJSON streaming format. Per the project synthesis decision,
//! this adapter does NOT participate in any multi-account rotation — Ollama
//! is a local single-instance server, so rotation would be meaningless. The
//! adapter holds no rotation state and reads no account store.
//!
//! Wire format (streaming): each line is a JSON object with `message.content`
//! deltas and a final `done: true` marker. Non-streaming returns a single
//! JSON object with the full message.

use crate::provider::LlmProvider;
use crate::stream::{StreamEvent, StreamSink};
use crate::types::{
    LlmMessage, LlmRequest, LlmResponse, MessageContent, Role, StopReason, ToolCall, ToolSpec,
    Usage,
};
use crate::{ProviderError, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_ENDPOINT: &str = "http://localhost:11434/api/chat";

/// Configuration for [`OllamaProvider`].
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// Base URL. Defaults to `http://localhost:11434/api/chat`.
    pub endpoint: String,
}

impl OllamaConfig {
    /// Construct with the default local endpoint.
    pub fn new() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_owned(),
        }
    }

    /// Override the endpoint (e.g. for a remote Ollama instance).
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Direct Ollama adapter. HTTP client built eagerly in [`new`].
#[derive(Debug, Clone)]
pub struct OllamaProvider {
    config: OllamaConfig,
    client: reqwest::Client,
}

impl OllamaProvider {
    /// Construct a new adapter. Builds the HTTP client eagerly so callers
    /// cannot hit a surprise `expect` on the first request.
    pub fn new(config: OllamaConfig) -> Result<Self> {
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
            body["options"]["num_predict"] = Value::Number(max.into());
        }
        if let Some(temp) = req.temperature {
            if let Some(n) = serde_json::Number::from_f64(f64::from(temp)) {
                body["options"]["temperature"] = Value::Number(n);
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
        "content": m.content.as_text(),
    })
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
impl LlmProvider for OllamaProvider {
    async fn complete(&self, req: &LlmRequest) -> Result<LlmResponse> {
        let client = self.client();
        let body = self.build_body(req, false);
        let resp = client
            .post(&self.config.endpoint)
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
            .map_err(|e| ProviderError::Decode(format!("ollama decode: {e}")))?;
        Ok(parsed.into())
    }

    async fn stream(&self, req: &LlmRequest, sink: &mut dyn StreamSink) -> Result<()> {
        let client = self.client();
        let body = self.build_body(req, true);
        let resp = client
            .post(&self.config.endpoint)
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
        let mut buf = String::new();
        let mut assembled = AssembledResponse::new(&req.model);
        sink.send(StreamEvent::MessageStart(assembled.skeleton()))
            .await?;
        while let Some(chunk) = stream.next().await {
            let bytes: Bytes =
                chunk.map_err(|e| ProviderError::StreamParse(format!("transport: {e}")))?;
            buf.push_str(
                std::str::from_utf8(&bytes)
                    .map_err(|_| ProviderError::StreamParse("non-utf8 chunk".into()))?,
            );
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let parsed: StreamLine = match serde_json::from_str(trimmed) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if !parsed.message.content.is_empty() {
                    assembled.text.push_str(&parsed.message.content);
                    sink.send(StreamEvent::ContentDelta(parsed.message.content.clone()))
                        .await?;
                }
                assembled.done = parsed.done;
                if parsed.done {
                    if let Some(eval) = parsed.eval_count {
                        // Heuristic: assume ~1 token of prompt overhead.
                        assembled.usage.input_tokens = 1;
                        assembled.usage.output_tokens = eval.saturating_sub(1);
                    }
                }
            }
        }
        assembled.stop_reason = StopReason::Stop;
        sink.send(StreamEvent::MessageStop(assembled.finalize()))
            .await?;
        Ok(())
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn supports_tools(&self) -> bool {
        true
    }
}

struct AssembledResponse {
    model: String,
    text: String,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
    stop_reason: StopReason,
    done: bool,
}

impl AssembledResponse {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            text: String::new(),
            tool_calls: Vec::new(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            done: false,
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

    fn finalize(self) -> LlmResponse {
        LlmResponse {
            content: MessageContent::Text(self.text),
            stop_reason: self.stop_reason,
            tool_calls: self.tool_calls,
            usage: self.usage,
            model: self.model,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct StreamLine {
    #[serde(default)]
    message: StreamMessage,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct StreamMessage {
    #[serde(default)]
    content: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApiResponse {
    message: StreamMessage,
    #[serde(default)]
    eval_count: Option<u32>,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
}

impl From<ApiResponse> for LlmResponse {
    fn from(resp: ApiResponse) -> Self {
        let input_tokens = resp.prompt_eval_count.unwrap_or(0);
        let output_tokens = resp.eval_count.unwrap_or(0);
        LlmResponse {
            content: MessageContent::Text(resp.message.content),
            stop_reason: StopReason::Stop,
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens,
                output_tokens,
            },
            model: String::new(),
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
    fn build_body_uses_default_endpoint_when_unset() {
        let provider = OllamaProvider::new(OllamaConfig::new()).unwrap();
        let body = provider.build_body(&LlmRequest::single_user("llama3", "hi"), true);
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["model"], "llama3");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["content"], "hi");
    }

    #[test]
    fn build_body_passes_options_through() {
        let provider = OllamaProvider::new(OllamaConfig::new()).unwrap();
        let req = LlmRequest {
            model: "m".into(),
            messages: vec![LlmMessage::user("hi")],
            max_tokens: Some(50),
            temperature: Some(0.7),
            tools: Vec::new(),
            system_prompt: None,
        };
        let body = provider.build_body(&req, false);
        assert_eq!(body["options"]["num_predict"], json!(50u32));
        assert!(body["options"]["temperature"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn config_with_endpoint_overrides_default() {
        let cfg = OllamaConfig::new().with_endpoint("https://remote.local/api/chat");
        assert_eq!(cfg.endpoint, "https://remote.local/api/chat");
    }

    #[test]
    fn config_default_is_localhost() {
        assert_eq!(OllamaConfig::default().endpoint, DEFAULT_ENDPOINT);
    }

    #[test]
    fn api_response_decodes_message_and_usage() {
        let raw = json!({
            "message": {"content": "hi"},
            "eval_count": 5,
            "prompt_eval_count": 2
        });
        let parsed: ApiResponse = serde_json::from_value(raw).unwrap();
        let resp: LlmResponse = parsed.into();
        assert_eq!(resp.content.as_text(), "hi");
        assert_eq!(resp.usage.input_tokens, 2);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn tool_spec_serializes_to_function_shape() {
        let t = ToolSpec::new("n", "d", json!({"type": "object"}));
        let v = tool_to_json(&t);
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "n");
    }
}
