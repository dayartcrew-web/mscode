//! Model provider layer for the `mscode` CLI.
//!
//! This crate defines the LLM-domain abstraction used everywhere else in the
//! workspace. It exposes a single top-level trait, [`LlmProvider`], that each
//! backend (Anthropic, OpenAI, Ollama, mock) implements. The trait supports
//! both one-shot [`LlmProvider::complete`] and token-streamed
//! [`LlmProvider::stream`] responses via the [`StreamSink`] abstraction.
//!
//! # Design decision: local trait (Option C)
//!
//! The sibling crate `multi-account-core-rs` (`../multi-account-core-rs`) was
//! inspected for reuse. Its public surface covers only OAuth account rotation,
//! auth-header application, and HTTP error classification — there is **no**
//! chat-completion or streaming API. Reusing only its adapter trait would have
//! pulled in a daemon-feature dependency tree (axum, tower, fs4, sysinfo, etc.)
//! for very little leverage while exposing every CLI build to a sibling
//! repository's stability on Windows path-dep resolutions.
//!
//! We therefore mirror its `ErrorKind` taxonomy inside [`ProviderError`] and
//! keep this crate self-contained. If the sibling crate ever grows a
//! chat-completion layer, an integration module can lift it under a
//! `multi-account-rotation` feature flag without changing the public API
//! below.
//!
//! # Cold start
//!
//! Real HTTP adapters are gated behind the `live_tests` feature flag, so the
//! default build never compiles in `reqwest` or any network code. The mock
//! provider carries no network objects; the sub-200ms cold-start budget is
//! preserved.
//!
//! # TLS
//!
//! All real adapters depend on `reqwest` with `rustls-tls` only. No
//! `openssl` and no `native-tls` anywhere in the dependency graph.

pub mod adapters;
mod error;
mod mock;
mod provider;
mod sse;
mod stream;
mod types;

pub use error::{ErrorKind, ProviderError};
pub use mock::{MockLlmProvider, MockStreamScript};
pub use provider::LlmProvider;
pub use stream::{CapturingStreamSink, StreamEvent, StreamSink};
pub use types::{
    LlmMessage, LlmRequest, LlmResponse, MessageContent, Role, StopReason, ToolCall, ToolSpec,
    Usage,
};

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, ProviderError>;
