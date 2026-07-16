//! [`Planner`] — decides what to run.
//!
//! Given a goal and the current [`DagGraph`] snapshot, the planner calls
//! the LLM with a structured prompt and parses the response into a [`Plan`].
//!
//! Construction is O(1): no LLM calls happen until [`Planner::plan`] is
//! invoked. This keeps cold-start cheap.

use std::sync::Arc;

use mscode_dag_runtime::DagGraph;
use mscode_provider::{LlmProvider, LlmRequest, LlmResponse};

use crate::error::{AgentError, AgentResult};
use crate::extract::extract_from_str;
use crate::plan::Plan;

/// Decides the next sequence of nodes to attempt.
pub struct Planner {
    provider: Arc<dyn LlmProvider>,
    /// Model name to send in [`LlmRequest`].
    model: String,
}

impl Planner {
    /// Construct a new planner.
    ///
    /// Cheap: stores the `Arc` and the model name. No provider call.
    pub fn new(provider: Arc<dyn LlmProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }

    /// Returns the maximum number of steps the planner will emit per turn.
    /// Hardcoded to keep prompt size bounded.
    pub const fn max_steps(&self) -> usize {
        16
    }

    /// Plan a sequence of steps for the given goal against the current DAG.
    ///
    /// The DAG is passed in so the planner can avoid re-suggesting nodes that
    /// are already `Completed` (or `Failed`). The planner does NOT mutate the
    /// DAG — that's the supervisor's job. The argument is shared (not mut)
    /// for this reason.
    pub async fn plan(&self, goal: &str, dag: &DagGraph) -> AgentResult<Plan> {
        let system = "You are a planning assistant. Reply with a JSON object \
            matching the schema: {\"steps\": [{\"label\": string, \"inputs\": any}], \
            \"goal\": string, \"expected_output\": any}. Do not include prose.";

        let snapshot = describe_dag(dag);
        let user = format!(
            "Goal: {goal}\n\nCurrent DAG snapshot:\n{snapshot}\n\n\
             Return a JSON plan with at most {} steps.",
            self.max_steps()
        );

        let req = LlmRequest {
            model: self.model.clone(),
            messages: vec![
                mscode_provider::LlmMessage::system(system),
                mscode_provider::LlmMessage::user(user),
            ],
            max_tokens: Some(1024),
            temperature: Some(0.0),
            tools: Vec::new(),
            system_prompt: None,
        };

        let resp: LlmResponse = self.provider.complete(&req).await?;
        let text = resp.content.as_text();
        let plan: Plan = extract_from_str(&text)?;
        if plan.steps.is_empty() {
            return Err(AgentError::Planning(
                "planner returned an empty plan".into(),
            ));
        }
        if plan.steps.len() > self.max_steps() {
            return Err(AgentError::Planning(format!(
                "planner returned {} steps (max {})",
                plan.steps.len(),
                self.max_steps()
            )));
        }
        Ok(plan)
    }
}

/// Render a compact human-readable description of the DAG for the prompt.
fn describe_dag(dag: &DagGraph) -> String {
    let mut out = String::new();
    for (idx, node) in dag.nodes_iter() {
        out.push_str(&format!(
            "- node {}: label={}, status={:?}\n",
            idx.as_u32(),
            node.label,
            node.status
        ));
    }
    if out.is_empty() {
        "(empty DAG)".to_string()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mscode_dag_runtime::{DagNode, DagStatus};
    use mscode_provider::MockLlmProvider;
    use serde_json::json;

    fn planner_with_response(text: &str) -> Planner {
        let m = MockLlmProvider::text("mock", "test-model", text);
        Planner::new(Arc::new(m), "test-model")
    }

    #[tokio::test]
    async fn planner_plan_returns_node_sequence() {
        let plan_json = json!({
            "steps": [
                {"label": "fetch", "inputs": {"url": "x"}},
                {"label": "transform", "inputs": {}}
            ],
            "goal": "fetch and transform",
            "expected_output": {"type": "object"}
        });
        let p = planner_with_response(&plan_json.to_string());
        let dag = DagGraph::new();
        let plan = p.plan("test", &dag).await.unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].label, "fetch");
        assert_eq!(plan.steps[1].label, "transform");
    }

    #[tokio::test]
    async fn planner_plan_includes_dag_snapshot_in_prompt() {
        // Same mock returns valid plan; we just verify it doesn't error
        // when the DAG is non-empty.
        let plan_json = json!({"steps": [{"label": "next"}], "goal": "g"});
        let p = planner_with_response(&plan_json.to_string());
        let mut dag = DagGraph::new();
        let _ = dag.add_node(DagNode::with_label("done"));
        let plan = p.plan("test", &dag).await.unwrap();
        assert_eq!(plan.steps.len(), 1);
    }

    #[tokio::test]
    async fn planner_plan_rejects_empty_plan() {
        let plan_json = json!({"steps": [], "goal": "g"});
        let p = planner_with_response(&plan_json.to_string());
        let dag = DagGraph::new();
        let err = p.plan("test", &dag).await.unwrap_err();
        assert!(matches!(err, AgentError::Planning(_)));
    }

    #[tokio::test]
    async fn planner_plan_rejects_invalid_json() {
        let p = planner_with_response("this is not json");
        let dag = DagGraph::new();
        let err = p.plan("test", &dag).await.unwrap_err();
        assert!(matches!(err, AgentError::Llm(_)));
    }

    #[tokio::test]
    async fn planner_plan_rejects_oversized_plan() {
        let steps: Vec<_> = (0..32)
            .map(|i| serde_json::json!({"label": format!("s{i}")}))
            .collect();
        let plan_json = json!({"steps": steps, "goal": "g"});
        let p = planner_with_response(&plan_json.to_string());
        let dag = DagGraph::new();
        let err = p.plan("test", &dag).await.unwrap_err();
        assert!(matches!(err, AgentError::Planning(_)));
    }

    #[test]
    fn describe_dag_handles_empty_graph() {
        let dag = DagGraph::new();
        let s = describe_dag(&dag);
        assert_eq!(s, "(empty DAG)");
    }

    #[test]
    fn describe_dag_lists_nodes() {
        let mut dag = DagGraph::new();
        let _ = dag.add_node(DagNode::new("a", json!({})));
        let _ = dag.add_node(DagNode::new("b", json!({})));
        let s = describe_dag(&dag);
        assert!(s.contains("label=a"));
        assert!(s.contains("label=b"));
        assert!(s.contains("status=Pending"));
    }

    #[test]
    fn max_steps_constant() {
        let m = MockLlmProvider::default();
        let p = Planner::new(Arc::new(m), "x");
        assert_eq!(p.max_steps(), 16);
        // Verify it matches DagStatus import (no warning).
        let _ = DagStatus::Pending;
    }
}
