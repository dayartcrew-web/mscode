//! Concrete provider adapters.
//!
//! Each adapter translates between the canonical [`crate::types`] shape and a
//! specific provider's HTTP+SSE wire format. Real network adapters are gated
//! behind the `live_tests` feature so default builds stay hermetic and the
//! sub-200ms cold start is preserved.
//!
//! The shared SSE framing parser lives at [`crate::sse`] and is always
//! compiled (it has no network dependency of its own).
//!
//! # Adapters
//!
//! - [`anthropic::AnthropicProvider`] — Claude family.
//! - [`openai::OpenAiProvider`] — GPT family.
//! - [`ollama::OllamaProvider`] — direct local Ollama; no rotation per
//!   project synthesis decision.

#[cfg(feature = "live_tests")]
pub mod anthropic;
#[cfg(feature = "live_tests")]
pub mod ollama;
#[cfg(feature = "live_tests")]
pub mod openai;

#[cfg(feature = "live_tests")]
pub use anthropic::AnthropicProvider;
#[cfg(feature = "live_tests")]
pub use ollama::OllamaProvider;
#[cfg(feature = "live_tests")]
pub use openai::OpenAiProvider;
