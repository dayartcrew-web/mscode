//! [`Critic`] — evaluates executor results.
//!
//! Given the [`Plan`] and the actual results produced by the executor, the
//! critic asks the LLM whether to accept, reflect (retry with feedback), or
//! reject. The supervisor uses the [`CritiqueDecision`] to drive its
//! reflection loop.

use std::sync::Arc;

use mscode_provider::{LlmProvider, LlmRequest, LlmResponse};
use serde::Deserialize;
use serde_json::Value;

use crate::error::{AgentError, AgentResult};
use crate::extract::extract_from_str;
use crate::plan::{Critique, CritiqueDecision, Plan};

/// Evaluates executor results and returns a [`Critique`].
pub struct Critic {
    provider: Arc<dyn LlmProvider>,
    model: String,
}

impl Critic {
    /// Construct a new critic.
    pub fn new(provider: Arc<dyn LlmProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }

    /// Evaluate the results of executing `plan`.
    ///
    /// `results` is parallel to `plan.steps`: `results[i]` is the output of
    /// `plan.steps[i]`. If lengths differ, the critic raises a
    /// [`AgentError::Execution`] error before calling the LLM.
    pub async fn critique(&self, plan: &Plan, results: &[Value]) -> AgentResult<Critique> {
        if results.len() != plan.steps.len() {
            return Err(AgentError::Execution(format!(
                "results length {} does not match plan steps {}",
                results.len(),
                plan.steps.len()
            )));
        }
        let system = "You are a critic. Reply with a JSON object: \
            {\"decision\": {\"kind\": \"accept\"} | \
            {\"kind\": \"reflect\", \"feedback\": string} | \
            {\"kind\": \"reject\", \"reason\": string}, \
            \"reasoning\": string}. \
            decision.kind MUST be exactly one of: accept, reflect, reject.";

        let user = format!(
            "Goal: {}\nSteps: {}\nResults: {}",
            plan.goal,
            serde_json::to_string(&plan.steps).unwrap_or_default(),
            serde_json::to_string(results).unwrap_or_default()
        );

        let req = LlmRequest {
            model: self.model.clone(),
            messages: vec![
                mscode_provider::LlmMessage::system(system),
                mscode_provider::LlmMessage::user(user),
            ],
            max_tokens: Some(512),
            temperature: Some(0.0),
            tools: Vec::new(),
            system_prompt: None,
        };

        let resp: LlmResponse = self.provider.complete(&req).await?;
        let text = resp.content.as_text();
        let wire: CritiqueWire = extract_from_str(&text)?;
        wire.into_critique()
    }
}

/// Wire shape produced by the LLM. We parse into this intermediate type
/// because the externally-tagged enum `CritiqueDecision` uses `tag = "kind"`
/// internally, but the LLM is asked for a tagged-union shape that serde
/// reads as `{"kind": "accept"}` or `{"kind": "reflect", "feedback": "..."}`.
/// We model that explicitly here.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CritiqueWireDecision {
    Accept,
    Reflect { feedback: String },
    Reject { reason: String },
}

#[derive(Debug, Deserialize)]
struct CritiqueWire {
    decision: CritiqueWireDecision,
    reasoning: String,
}

impl CritiqueWire {
    fn into_critique(self) -> AgentResult<Critique> {
        if self.reasoning.trim().is_empty() {
            return Err(AgentError::Llm("critic returned empty reasoning".into()));
        }
        let decision = match self.decision {
            CritiqueWireDecision::Accept => CritiqueDecision::Accept,
            CritiqueWireDecision::Reflect { feedback } => CritiqueDecision::Reflect(feedback),
            CritiqueWireDecision::Reject { reason } => CritiqueDecision::Reject(reason),
        };
        Ok(Critique {
            decision,
            reasoning: self.reasoning,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mscode_provider::MockLlmProvider;
    use serde_json::json;

    fn critic_with_response(text: &str) -> Critic {
        let m = MockLlmProvider::text("mock", "test-model", text);
        Critic::new(Arc::new(m), "test-model")
    }

    fn two_step_plan() -> Plan {
        Plan::new("g")
            .with_step(crate::plan::PlanStep::new("a"))
            .with_step(crate::plan::PlanStep::new("b"))
    }

    #[tokio::test]
    async fn critic_critique_returns_accept_on_good_result() {
        let resp = json!({
            "decision": {"kind": "accept"},
            "reasoning": "results look good"
        });
        let c = critic_with_response(&resp.to_string());
        let plan = two_step_plan();
        let results = vec![json!("ra"), json!("rb")];
        let cr = c.critique(&plan, &results).await.unwrap();
        assert!(matches!(cr.decision, CritiqueDecision::Accept));
        assert_eq!(cr.reasoning, "results look good");
    }

    #[tokio::test]
    async fn critic_critique_returns_reject_on_bad_result() {
        let resp = json!({
            "decision": {"kind": "reject", "reason": "outputs are null"},
            "reasoning": "expected numbers"
        });
        let c = critic_with_response(&resp.to_string());
        let plan = two_step_plan();
        let results = vec![Value::Null, Value::Null];
        let cr = c.critique(&plan, &results).await.unwrap();
        match cr.decision {
            CritiqueDecision::Reject(r) => assert_eq!(r, "outputs are null"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn critic_critique_returns_reflect_with_feedback() {
        let resp = json!({
            "decision": {"kind": "reflect", "feedback": "retry with cleanup"},
            "reasoning": "needs another pass"
        });
        let c = critic_with_response(&resp.to_string());
        let plan = two_step_plan();
        let results = vec![json!({}), json!({})];
        let cr = c.critique(&plan, &results).await.unwrap();
        match cr.decision {
            CritiqueDecision::Reflect(fb) => assert_eq!(fb, "retry with cleanup"),
            other => panic!("expected Reflect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn critic_rejects_length_mismatch() {
        let c = critic_with_response("{\"decision\":{\"kind\":\"accept\"},\"reasoning\":\"ok\"}");
        let plan = two_step_plan();
        let results = vec![json!("only one")]; // missing one
        let err = c.critique(&plan, &results).await.unwrap_err();
        assert!(matches!(err, AgentError::Execution(_)));
    }

    #[tokio::test]
    async fn critic_rejects_invalid_json() {
        let c = critic_with_response("not json");
        let plan = two_step_plan();
        let results = vec![json!(1), json!(2)];
        let err = c.critique(&plan, &results).await.unwrap_err();
        assert!(matches!(err, AgentError::Llm(_)));
    }

    #[tokio::test]
    async fn critic_rejects_empty_reasoning() {
        let resp = json!({
            "decision": {"kind": "accept"},
            "reasoning": "   "
        });
        let c = critic_with_response(&resp.to_string());
        let plan = two_step_plan();
        let results = vec![json!(1), json!(2)];
        let err = c.critique(&plan, &results).await.unwrap_err();
        assert!(matches!(err, AgentError::Llm(_)));
    }

    #[tokio::test]
    async fn critic_rejects_unknown_decision_kind() {
        let resp = json!({
            "decision": {"kind": "maybe"},
            "reasoning": "unknown"
        });
        let c = critic_with_response(&resp.to_string());
        let plan = two_step_plan();
        let results = vec![json!(1), json!(2)];
        let err = c.critique(&plan, &results).await.unwrap_err();
        assert!(matches!(err, AgentError::Llm(_)));
    }
}
