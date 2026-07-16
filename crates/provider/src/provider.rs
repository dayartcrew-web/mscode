//! Top-level [`LlmProvider`] trait.
//!
//! Each backend implements this trait against the canonical LLM-domain types
//! in [`crate::types`]. The trait supports both one-shot completion and
//! streaming via [`crate::StreamSink`].
//!
//! Concurrency: implementations must be `Send + Sync` so they can be stored
//! as `Arc<dyn LlmProvider>` and shared across tokio tasks.

use crate::Result;
use crate::stream::StreamSink;
use crate::types::{LlmRequest, LlmResponse};
use async_trait::async_trait;

/// Provider-agnostic LLM completion API.
///
/// Implementations:
/// - [`crate::adapters::AnthropicProvider`] (when `live_tests` is enabled)
/// - [`crate::adapters::OpenAiProvider`] (when `live_tests` is enabled)
/// - [`crate::adapters::OllamaProvider`] (when `live_tests` is enabled)
/// - [`crate::MockLlmProvider`] (always available)
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Run a one-shot completion and return the assembled response.
    ///
    /// Adapters that also support streaming typically implement this in terms
    /// of [`LlmProvider::stream`] against a [`crate::CapturingStreamSink`],
    /// but are free to use the provider's native non-streaming endpoint when
    /// that is simpler or cheaper.
    async fn complete(&self, req: &LlmRequest) -> Result<LlmResponse>;

    /// Stream a completion into `sink`. The implementation must emit at least
    /// a terminal [`crate::StreamEvent::MessageStop`] or
    /// [`crate::StreamEvent::Error`].
    async fn stream(&self, req: &LlmRequest, sink: &mut dyn StreamSink) -> Result<()>;

    /// Stable provider name used in logs, errors, and provider routing.
    fn name(&self) -> &str;

    /// Whether this provider can satisfy requests with non-empty
    /// [`crate::types::LlmRequest::tools`]. Returning `false` causes the
    /// dispatcher to short-circuit tool-bearing requests with
    /// [`crate::ProviderError::ToolUnsupported`] before any HTTP call.
    fn supports_tools(&self) -> bool;
}
