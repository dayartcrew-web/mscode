//! Streaming protocol for incremental LLM responses.
//!
//! Real provider backends deliver responses as Server-Sent Events (SSE) — a
//! sequence of small chunks. We abstract this into a single
//! provider-agnostic [`StreamEvent`] enum plus the [`StreamSink`] trait that
//! adapters push events into. Concrete sinks can route events to a terminal
//! renderer, an SSE re-broadcaster, or a buffer that reconstructs the final
//! [`LlmResponse`].
//!
//! Adapters never block on the full response when streaming was requested —
//! each chunk is forwarded as soon as it is parsed.

use crate::ProviderError;
use crate::types::{LlmResponse, ToolCall};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A single event in a streamed LLM response.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Emitted once at the start of a stream with a partial response
    /// skeleton (model name, empty content, zero usage). Subsequent deltas
    /// mutate this skeleton.
    MessageStart(LlmResponse),
    /// A chunk of text content appended to the response.
    ContentDelta(String),
    /// Incremental progress on a tool call. Adapters may split a single tool
    /// call across many deltas; the sink is responsible for assembling the
    /// final [`ToolCall`].
    ToolCallDelta {
        /// Provider-assigned tool call id (set on first delta for the call).
        id: Option<String>,
        /// Append to the tool name (set on first delta for the call).
        name_delta: Option<String>,
        /// Append to the raw JSON arguments string. The sink should
        /// concatenate these and parse once at end-of-stream.
        args_delta: Option<String>,
    },
    /// Emitted once at the end of a stream with the final response — full
    /// content, resolved stop reason, and complete usage.
    MessageStop(LlmResponse),
    /// Stream failed. After this event no further events will be emitted.
    Error(ProviderError),
}

/// Object-safe sink for [`StreamEvent`]s. Adapters call [`StreamSink::send`]
/// from an async context for each event they decode.
///
/// `Send + Sync` is required so the adapter can own the sink as
/// `&mut dyn StreamSink` across `.await` points inside a tokio task.
#[async_trait]
pub trait StreamSink: Send + Sync {
    /// Receive the next event. Implementations should not block indefinitely
    /// on the calling task — provider adapters rely on cooperative progress.
    async fn send(&mut self, event: StreamEvent) -> std::result::Result<(), ProviderError>;
}

/// A [`StreamSink`] that buffers every event and reconstructs the final
/// [`LlmResponse`]. Used by callers that asked for streaming semantics but
/// still want the assembled result, and in tests.
#[derive(Debug, Default)]
pub struct CapturingStreamSink {
    text: String,
    tool_calls: Vec<ToolCall>,
    tool_call_arg_buffers: Vec<String>,
    tool_call_name_buffers: Vec<String>,
    final_response: Option<LlmResponse>,
    last_error: Option<ProviderError>,
    event_count: usize,
}

impl CapturingStreamSink {
    /// Construct an empty capturing sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the concatenated text received so far.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the assembled tool calls, or `None` if the stream did not
    /// emit a terminal [`StreamEvent::MessageStop`].
    pub fn tool_calls(&self) -> &[ToolCall] {
        &self.tool_calls
    }

    /// Returns the final response if a `MessageStop` event was received.
    pub fn final_response(&self) -> Option<&LlmResponse> {
        self.final_response.as_ref()
    }

    /// Returns the last error seen, if any.
    pub fn last_error(&self) -> Option<&ProviderError> {
        self.last_error.as_ref()
    }

    /// Number of events received, including any terminal event.
    pub fn event_count(&self) -> usize {
        self.event_count
    }
}

#[async_trait]
impl StreamSink for CapturingStreamSink {
    async fn send(&mut self, event: StreamEvent) -> std::result::Result<(), ProviderError> {
        self.event_count = self.event_count.saturating_add(1);
        match event {
            StreamEvent::MessageStart(_) => {}
            StreamEvent::ContentDelta(text) => self.text.push_str(&text),
            StreamEvent::MessageStop(resp) => {
                self.final_response = Some(resp);
            }
            StreamEvent::Error(err) => {
                self.last_error = Some(err);
            }
            StreamEvent::ToolCallDelta {
                id,
                name_delta,
                args_delta,
            } => {
                let idx = match &id {
                    Some(id_str) => {
                        if let Some(pos) = self
                            .tool_calls
                            .iter()
                            .position(|existing| existing.id == *id_str)
                        {
                            pos
                        } else {
                            self.tool_calls.push(ToolCall {
                                id: id_str.clone(),
                                name: String::new(),
                                args: serde_json::Value::Null,
                            });
                            self.tool_call_arg_buffers.push(String::new());
                            self.tool_call_name_buffers.push(String::new());
                            self.tool_calls.len() - 1
                        }
                    }
                    None => self.tool_calls.len().saturating_sub(1),
                };
                if idx < self.tool_calls.len() {
                    if let Some(name) = name_delta {
                        self.tool_call_name_buffers[idx].push_str(&name);
                    }
                    if let Some(args) = args_delta {
                        self.tool_call_arg_buffers[idx].push_str(&args);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Finalize the captured state by promoting buffered name/arg strings into
/// the assembled [`ToolCall`] list. Called by callers after the stream ends.
impl CapturingStreamSink {
    /// Promote accumulated name/args buffers into the `tool_calls` list with
    /// parsed JSON arguments. Returns a clone of the assembled tool calls.
    pub fn finalize_tool_calls(&mut self) -> Vec<ToolCall> {
        for (i, buf) in self.tool_call_name_buffers.iter().enumerate() {
            if let Some(call) = self.tool_calls.get_mut(i) {
                if call.name.is_empty() {
                    call.name.clone_from(buf);
                }
            }
        }
        for (i, buf) in self.tool_call_arg_buffers.iter().enumerate() {
            if let Some(call) = self.tool_calls.get_mut(i) {
                if buf.is_empty() {
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(buf) {
                    Ok(v) => call.args = v,
                    Err(_) => {
                        call.args = serde_json::Value::String(buf.clone());
                    }
                }
            }
        }
        self.tool_calls.clone()
    }
}

/// Convenience wrapper that lets a `&mut dyn StreamSink` be shared across
/// tasks by locking it behind a `Mutex`. Rarely needed by callers but useful
/// when an adapter must dispatch events from multiple futures.
#[allow(dead_code)]
pub struct SharedSink {
    inner: Arc<Mutex<Box<dyn StreamSink>>>,
}

impl std::fmt::Debug for SharedSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSink").finish_non_exhaustive()
    }
}

impl SharedSink {
    /// Wrap an owned sink so it can be cloned and shared across tasks.
    #[allow(dead_code)]
    pub fn new(sink: Box<dyn StreamSink>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(sink)),
        }
    }

    /// Returns a handle that can be sent to another task.
    #[allow(dead_code)]
    pub fn handle(&self) -> SharedSinkHandle {
        SharedSinkHandle {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Clonable handle to a [`SharedSink`].
#[derive(Clone)]
#[allow(dead_code)]
pub struct SharedSinkHandle {
    inner: Arc<Mutex<Box<dyn StreamSink>>>,
}

impl std::fmt::Debug for SharedSinkHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSinkHandle").finish_non_exhaustive()
    }
}

#[async_trait]
impl StreamSink for SharedSinkHandle {
    async fn send(&mut self, event: StreamEvent) -> std::result::Result<(), ProviderError> {
        let mut guard = self.inner.lock().await;
        guard.send(event).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LlmResponse, StopReason, Usage};
    use serde_json::json;

    fn resp_text() -> LlmResponse {
        LlmResponse::text("m", "")
    }

    #[tokio::test]
    async fn capturing_sink_assembles_text_from_deltas() {
        let mut sink = CapturingStreamSink::new();
        sink.send(StreamEvent::MessageStart(resp_text()))
            .await
            .unwrap();
        sink.send(StreamEvent::ContentDelta("hello ".into()))
            .await
            .unwrap();
        sink.send(StreamEvent::ContentDelta("world".into()))
            .await
            .unwrap();
        sink.send(StreamEvent::MessageStop(LlmResponse {
            content: crate::types::MessageContent::Text("hello world".into()),
            stop_reason: StopReason::Stop,
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: 1,
                output_tokens: 2,
            },
            model: "m".into(),
        }))
        .await
        .unwrap();

        assert_eq!(sink.text(), "hello world");
        let final_resp = sink.final_response().expect("final response");
        assert_eq!(final_resp.usage.total(), 3);
        assert_eq!(sink.event_count(), 4);
    }

    #[tokio::test]
    async fn capturing_sink_assembles_tool_calls() {
        let mut sink = CapturingStreamSink::new();
        sink.send(StreamEvent::ToolCallDelta {
            id: Some("t1".into()),
            name_delta: Some("sea".into()),
            args_delta: Some("{\"q\":".into()),
        })
        .await
        .unwrap();
        sink.send(StreamEvent::ToolCallDelta {
            id: Some("t1".into()),
            name_delta: Some("rch".into()),
            args_delta: Some("\"rust\"}".into()),
        })
        .await
        .unwrap();

        let calls = sink.finalize_tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].args, json!({"q": "rust"}));
    }

    #[tokio::test]
    async fn capturing_sink_records_errors() {
        let mut sink = CapturingStreamSink::new();
        sink.send(StreamEvent::Error(ProviderError::Decode("bad".into())))
            .await
            .unwrap();
        assert!(sink.last_error().is_some());
    }

    #[tokio::test]
    async fn shared_sink_handle_dispatches_events() {
        let sink = SharedSink::new(Box::new(CapturingStreamSink::new()));
        let mut handle = sink.handle();
        handle
            .send(StreamEvent::ContentDelta("x".into()))
            .await
            .unwrap();
        // The capturing sink ignores the inner pointer for text retrieval;
        // we only assert the dispatch path does not deadlock or error.
    }

    #[tokio::test]
    async fn tool_call_with_malformed_args_falls_back_to_string() {
        let mut sink = CapturingStreamSink::new();
        sink.send(StreamEvent::ToolCallDelta {
            id: Some("t1".into()),
            name_delta: Some("n".into()),
            args_delta: Some("not json".into()),
        })
        .await
        .unwrap();
        let calls = sink.finalize_tool_calls();
        assert_eq!(calls[0].args, serde_json::Value::String("not json".into()));
    }
}
