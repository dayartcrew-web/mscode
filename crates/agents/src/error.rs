//! Error taxonomy for the agents crate.
//!
//! Mirrors the four-bucket PromptError taxonomy from `rig` (separated into
//! distinct variants rather than collapsed into a single "Llm" bucket) so
//! callers can drive routing decisions from the kind of failure rather than
//! just the message.

use mscode_dag_runtime::DagError;
use mscode_provider::ProviderError;

/// Result alias used across the agents crate.
pub type AgentResult<T> = std::result::Result<T, AgentError>;

/// Failures raised by the agent quartet.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// A provider call failed. The original [`ProviderError`] is preserved so
    /// callers can drive retry / rotation policy from its
    /// [`ErrorKind`](mscode_provider::ErrorKind).
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    /// An LLM prompt produced an unparseable or out-of-contract response
    /// that did not match the expected structure (not a transport-level
    /// failure — see [`Self::Provider`] for those).
    #[error("llm error: {0}")]
    Llm(String),

    /// The planner failed to produce a usable [`crate::Plan`].
    #[error("planning error: {0}")]
    Planning(String),

    /// The executor failed while running a node.
    #[error("execution error: {0}")]
    Execution(String),

    /// Supervisor hit the 3-reflection ceiling without an Accept.
    #[error("reflections exhausted (max 3 iterations)")]
    ReflectionsExhausted,

    /// The DAG itself rejected an operation (cycle, missing node, invalid
    /// state transition).
    #[error("dag error: {0}")]
    Dag(#[from] DagError),
}

/// Borrowed from `rig`'s PromptError taxonomy. Kept as a separate enum so
/// callers that already speak the rig-style classification can route on it.
///
/// The variants mirror the most common failure modes when invoking an LLM
/// with tool-use + structured output. The agents crate maps transport-level
/// failures through [`AgentError::Provider`] and treats the variants below
/// as the structured-output failure modes.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    /// Tool call was requested but malformed (missing name, bad args).
    #[error("tool use error: {0}")]
    ToolUse(String),

    /// The model's response could not be parsed into the expected JSON shape.
    #[error("json parse error: {0}")]
    JsonParse(String),

    /// The provider rate-limited the call.
    #[error("rate limit: {0}")]
    RateLimit(String),

    /// The prompt exceeded the model's context window.
    #[error("context length exceeded: {0}")]
    ContextLength(String),

    /// The provider rejected the request as invalid (bad model name, etc.).
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

impl From<PromptError> for AgentError {
    fn from(e: PromptError) -> Self {
        match e {
            PromptError::ToolUse(s)
            | PromptError::JsonParse(s)
            | PromptError::ContextLength(s)
            | PromptError::InvalidRequest(s) => AgentError::Llm(s),
            PromptError::RateLimit(s) => AgentError::Llm(format!("rate limit: {s}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mscode_provider::ProviderError;

    #[test]
    fn agent_error_from_provider_error() {
        let pe = ProviderError::Decode("bad json".into());
        let ae: AgentError = pe.into();
        assert!(matches!(ae, AgentError::Provider(_)));
    }

    #[test]
    fn agent_error_from_prompt_error_json_parse() {
        let pe = PromptError::JsonParse("missing field".into());
        let ae: AgentError = pe.into();
        assert!(matches!(ae, AgentError::Llm(_)));
    }

    #[test]
    fn agent_error_from_prompt_error_tool_use() {
        let pe = PromptError::ToolUse("unknown tool".into());
        let ae: AgentError = pe.into();
        assert!(matches!(ae, AgentError::Llm(_)));
    }

    #[test]
    fn agent_error_from_prompt_error_rate_limit() {
        let pe = PromptError::RateLimit("slow down".into());
        let ae: AgentError = pe.into();
        assert!(matches!(ae, AgentError::Llm(_)));
    }

    #[test]
    fn agent_error_from_dag_error() {
        let de = mscode_dag_runtime::DagError::NodeNotFound(5);
        let ae: AgentError = de.into();
        assert!(matches!(ae, AgentError::Dag(_)));
    }

    #[test]
    fn reflections_exhausted_displays_constant_message() {
        let e = AgentError::ReflectionsExhausted;
        let msg = e.to_string();
        assert!(msg.contains("reflections exhausted"));
    }

    #[test]
    fn prompt_error_variants_have_distinct_displays() {
        let vs = [
            PromptError::ToolUse("x".into()),
            PromptError::JsonParse("x".into()),
            PromptError::RateLimit("x".into()),
            PromptError::ContextLength("x".into()),
            PromptError::InvalidRequest("x".into()),
        ];
        let displays: Vec<String> = vs.iter().map(|e| e.to_string()).collect();
        for d in &displays {
            assert!(!d.is_empty());
        }
        // Each display should include its distinctive prefix.
        assert!(displays[0].contains("tool use"));
        assert!(displays[1].contains("json parse"));
        assert!(displays[2].contains("rate limit"));
        assert!(displays[3].contains("context length"));
        assert!(displays[4].contains("invalid request"));
    }
}
