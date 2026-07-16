//! Deterministic mock provider for tests and offline development.
//!
//! [`MockLlmProvider`] returns either a canned [`LlmResponse`] for one-shot
//! [`LlmProvider::complete`] calls, or replays a recorded sequence of
//! [`StreamEvent`]s for [`LlmProvider::stream`] calls. The mock owns no
//! network resources and constructs no HTTP client, preserving the
//! sub-200ms cold-start budget when only the mock is in use.

use crate::provider::LlmProvider;
use crate::stream::{CapturingStreamSink, StreamEvent, StreamSink};
use crate::types::{LlmRequest, LlmResponse};
use crate::{ProviderError, Result};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// Scripted behavior for a [`MockLlmProvider`].
#[derive(Debug, Clone)]
pub enum MockStreamScript {
    /// Emit a `MessageStart`, one or more `ContentDelta`s, then `MessageStop`
    /// containing the assembled text.
    Text {
        /// The model name to echo in the final response.
        model: String,
        /// The text to deliver, in order.
        chunks: Vec<String>,
        /// Token accounting for the final `MessageStop`.
        usage: crate::types::Usage,
    },
    /// Replay an exact sequence of events verbatim. The list must already
    /// include the terminal `MessageStop` or `Error`.
    Events(Vec<StreamEvent>),
    /// Always fail with the given error on the next `stream` call.
    Fail(ProviderError),
}

impl MockStreamScript {
    /// Construct a `Text` script from a single string. The text is split on
    /// word boundaries into a handful of chunks to exercise delta handling.
    pub fn from_text(model: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let chunks: Vec<String> = if text.is_empty() {
            Vec::new()
        } else {
            text.split_whitespace().map(|w| format!("{w} ")).collect()
        };
        let output_tokens = chunks.len() as u32;
        Self::Text {
            model: model.into(),
            chunks,
            usage: crate::types::Usage {
                input_tokens: 1,
                output_tokens,
            },
        }
    }
}

/// Mock LLM provider. Thread-safe; the script is shared via `Arc<Mutex<>>`
/// so it can be updated between calls from tests.
#[derive(Debug, Clone)]
pub struct MockLlmProvider {
    name: String,
    supports_tools: bool,
    /// Canned response for `complete`.
    complete_response: Arc<Mutex<LlmResponse>>,
    /// Script for `stream`.
    stream_script: Arc<Mutex<MockStreamScript>>,
}

impl MockLlmProvider {
    /// Construct a mock that returns the given canned response and streams
    /// the same text via word-boundary chunks.
    pub fn new(name: impl Into<String>, response: LlmResponse) -> Self {
        let model = response.model.clone();
        let text = response.content.as_text();
        let script = MockStreamScript::from_text(model.clone(), text);
        Self {
            name: name.into(),
            supports_tools: true,
            complete_response: Arc::new(Mutex::new(response)),
            stream_script: Arc::new(Mutex::new(script)),
        }
    }

    /// Construct a mock that always returns the given text response.
    pub fn text(
        name: impl Into<String>,
        model: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self::new(name, LlmResponse::text(model, text))
    }

    /// Override the `supports_tools` flag. Defaults to `true`.
    pub fn with_supports_tools(self, supports: bool) -> Self {
        Self {
            supports_tools: supports,
            ..self
        }
    }

    /// Replace the canned `complete` response.
    pub fn set_complete_response(&self, response: LlmResponse) {
        *self
            .complete_response
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = response;
    }

    /// Replace the `stream` script.
    pub fn set_stream_script(&self, script: MockStreamScript) {
        *self.stream_script.lock().unwrap_or_else(|e| e.into_inner()) = script;
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn complete(&self, req: &LlmRequest) -> Result<LlmResponse> {
        if req.has_tools() && !self.supports_tools {
            return Err(ProviderError::ToolUnsupported {
                provider: self.name.clone(),
            });
        }
        let resp = self
            .complete_response
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Ok(resp)
    }

    async fn stream(&self, req: &LlmRequest, sink: &mut dyn StreamSink) -> Result<()> {
        if req.has_tools() && !self.supports_tools {
            return Err(ProviderError::ToolUnsupported {
                provider: self.name.clone(),
            });
        }
        let script = self
            .stream_script
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match script {
            MockStreamScript::Fail(err) => {
                sink.send(StreamEvent::Error(err)).await?;
                Ok(())
            }
            MockStreamScript::Events(events) => {
                for ev in events {
                    sink.send(ev).await?;
                }
                Ok(())
            }
            MockStreamScript::Text {
                model,
                chunks,
                usage,
            } => {
                let mut assembled = String::new();
                sink.send(StreamEvent::MessageStart(LlmResponse::text(&model, "")))
                    .await?;
                for chunk in &chunks {
                    assembled.push_str(chunk);
                    sink.send(StreamEvent::ContentDelta(chunk.clone())).await?;
                }
                let trimmed = assembled.trim().to_owned();
                sink.send(StreamEvent::MessageStop(LlmResponse {
                    content: crate::types::MessageContent::Text(trimmed),
                    stop_reason: crate::types::StopReason::Stop,
                    tool_calls: Vec::new(),
                    usage,
                    model,
                }))
                .await?;
                Ok(())
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn supports_tools(&self) -> bool {
        self.supports_tools
    }
}

impl Default for MockLlmProvider {
    fn default() -> Self {
        Self::text("mock", "mock-model", "ok")
    }
}

/// Drive a `stream` call against a fresh [`CapturingStreamSink`] and return
/// the assembled text. Convenience for tests that just want the output.
#[allow(dead_code)]
pub async fn capture_stream(
    provider: &dyn LlmProvider,
    req: &LlmRequest,
) -> Result<(String, Option<LlmResponse>)> {
    let mut sink = CapturingStreamSink::new();
    provider.stream(req, &mut sink).await?;
    Ok((sink.text().to_owned(), sink.final_response().cloned()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LlmResponse, Usage};
    use serde_json::json;

    #[tokio::test]
    async fn complete_returns_canned_response() {
        let m = MockLlmProvider::text("mock", "m", "hi");
        let resp = m
            .complete(&LlmRequest::single_user("m", "ping"))
            .await
            .unwrap();
        assert_eq!(resp.content.as_text(), "hi");
    }

    #[tokio::test]
    async fn complete_rejects_tools_when_unsupported() {
        let m = MockLlmProvider::text("mock", "m", "hi").with_supports_tools(false);
        let mut req = LlmRequest::single_user("m", "ping");
        req.tools.push(crate::types::ToolSpec::new(
            "t",
            "d",
            json!({"type": "object"}),
        ));
        let err = m.complete(&req).await.unwrap_err();
        assert!(matches!(err, ProviderError::ToolUnsupported { .. }));
    }

    #[tokio::test]
    async fn stream_replays_text_chunks() {
        let m = MockLlmProvider::text("mock", "m", "hello world");
        let (text, resp) = capture_stream(&m, &LlmRequest::single_user("m", "ping"))
            .await
            .unwrap();
        assert_eq!(text, "hello world ");
        assert!(resp.is_some());
    }

    #[tokio::test]
    async fn stream_replays_event_list_verbatim() {
        let m = MockLlmProvider::text("mock", "m", "unused");
        m.set_stream_script(MockStreamScript::Events(vec![
            StreamEvent::MessageStart(LlmResponse::text("m", "")),
            StreamEvent::ContentDelta("custom".into()),
            StreamEvent::MessageStop(LlmResponse {
                content: crate::types::MessageContent::Text("custom".into()),
                stop_reason: crate::types::StopReason::Stop,
                tool_calls: Vec::new(),
                usage: Usage::default(),
                model: "m".into(),
            }),
        ]));
        let (text, _) = capture_stream(&m, &LlmRequest::single_user("m", "ping"))
            .await
            .unwrap();
        assert_eq!(text, "custom");
    }

    #[tokio::test]
    async fn stream_emits_error_on_fail_script() {
        let m = MockLlmProvider::text("mock", "m", "unused");
        m.set_stream_script(MockStreamScript::Fail(ProviderError::Decode("boom".into())));
        let mut sink = CapturingStreamSink::new();
        let res = m
            .stream(&LlmRequest::single_user("m", "ping"), &mut sink)
            .await;
        // The adapter dispatches an Error event but does not return Err
        // itself; the sink's last_error records the failure.
        assert!(res.is_ok());
        assert!(sink.last_error().is_some());
    }

    #[tokio::test]
    async fn set_complete_response_overrides_canned_value() {
        let m = MockLlmProvider::text("mock", "m", "old");
        m.set_complete_response(LlmResponse::text("m", "new"));
        let resp = m
            .complete(&LlmRequest::single_user("m", "ping"))
            .await
            .unwrap();
        assert_eq!(resp.content.as_text(), "new");
    }

    #[test]
    fn default_provider_returns_ok_text() {
        let m = MockLlmProvider::default();
        assert_eq!(m.name(), "mock");
    }

    #[tokio::test]
    async fn name_and_supports_tools_reported() {
        let m = MockLlmProvider::text("custom", "m", "x");
        assert_eq!(m.name(), "custom");
        assert!(m.supports_tools());

        let m2 = m.clone().with_supports_tools(false);
        assert!(!m2.supports_tools());
    }
}
