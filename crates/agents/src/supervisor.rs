//! [`Supervisor`] — orchestrates the plan → execute → critique loop.
//!
//! The supervisor owns the reflection cap (max 3 iterations, hardcoded per
//! industry consensus — see [`MAX_REFLECTIONS`]). After the cap is hit
//! without an Accept, the supervisor returns
//! [`TurnOutcome::ReflectionsExhausted`].
//!
//! ## Why 3?
//!
//! Empirically, reflection loops past 3 iterations almost never converge:
//! either the model is stuck in a fixation or the task is genuinely
//! impossible. Better to surface `ReflectionsExhausted` so the caller can
//! escalate (e.g. prompt the user) than to spin in a tight loop burning
//! tokens. This value is intentionally NOT configurable to prevent callers
//! from quietly bumping the ceiling.

use std::sync::Arc;

use mscode_dag_runtime::{DagGraph, DagNode, DagStatus};
use mscode_exec::{Executor, NodeContext};
use mscode_provider::LlmProvider;
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::critic::Critic;
use crate::error::{AgentError, AgentResult};
use crate::plan::{CritiqueDecision, Plan};
use crate::planner::Planner;

/// Hardcoded ceiling on reflection iterations. See module docs for the
/// reasoning.
pub const MAX_REFLECTIONS: u8 = 3;

/// Outcome of a single supervisor turn.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    /// The supervisor's plan executed successfully and the critic accepted
    /// the results. The value is the output of the LAST step (the most
    /// useful single artifact; the supervisor does not aggregate intermediate
    /// outputs by default).
    Success(Value),
    /// The critic returned `Reject`, or an executor error occurred that the
    /// supervisor chose not to retry. The string is the failure reason.
    Failed(String),
    /// The reflection cap was hit without an Accept.
    ReflectionsExhausted,
}

/// Orchestrates the agent quartet.
pub struct Supervisor {
    provider: Arc<dyn LlmProvider>,
    planner: Planner,
    critic: Critic,
    model: String,
}

impl Supervisor {
    /// Construct a new supervisor. Owns the planner and critic internally;
    /// they share the same provider.
    pub fn new(provider: Arc<dyn LlmProvider>, model: impl Into<String>) -> Self {
        let model_str = model.into();
        Self {
            provider: provider.clone(),
            planner: Planner::new(provider.clone(), &model_str),
            critic: Critic::new(provider, &model_str),
            model: model_str,
        }
    }

    /// Returns the reflection cap. Always [`MAX_REFLECTIONS`].
    pub fn max_reflections(&self) -> u8 {
        MAX_REFLECTIONS
    }

    /// Returns the provider reference (for tests and for callers that want
    /// to drive the provider directly).
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Returns the planner reference (for tests that want to drive it
    /// directly).
    pub fn planner(&self) -> &Planner {
        &self.planner
    }

    /// Returns the critic reference.
    pub fn critic(&self) -> &Critic {
        &self.critic
    }

    /// Returns the model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Run a single turn: plan → execute → critique, with up to
    /// [`MAX_REFLECTIONS`] reflection iterations.
    ///
    /// The DAG is mutated in-place: each plan step is added as a node (or
    /// reuses an existing node with the same label if present) and marked
    /// through its lifecycle. The executor is invoked via the supplied
    /// [`NodeContext`].
    pub async fn run_turn(
        &self,
        goal: &str,
        dag: &mut DagGraph,
        executor: &Executor,
        ctx: &NodeContext,
    ) -> AgentResult<TurnOutcome> {
        let mut iteration: u8 = 0;
        let mut last_feedback: Option<String> = None;

        loop {
            iteration = iteration.saturating_add(1);
            if iteration > MAX_REFLECTIONS {
                info!(iteration, "reflections exhausted");
                return Ok(TurnOutcome::ReflectionsExhausted);
            }

            // ----- PLAN -----
            let mut plan: Plan = self.planner.plan(goal, dag).await?;
            if let Some(fb) = last_feedback.take() {
                // Annotate the plan goal with feedback from the prior
                // reflection so the planner (or executor) has visibility.
                // We don't change the steps themselves — the planner already
                // adjusted them in this iteration.
                plan.goal = format!("{}\n[prior feedback: {fb}]", plan.goal);
            }
            debug!(iteration, steps = plan.steps.len(), "planned");

            // ----- EXECUTE -----
            let results = match self.execute_plan(&plan, dag, executor, ctx).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(iteration, error = %e, "execution failed");
                    return Ok(TurnOutcome::Failed(format!("execution error: {e}")));
                }
            };

            // ----- CRITIQUE -----
            let critique = self.critic.critique(&plan, &results).await?;
            match critique.decision {
                CritiqueDecision::Accept => {
                    info!(iteration, "critic accepted");
                    let final_value = results.last().cloned().unwrap_or(Value::Null);
                    return Ok(TurnOutcome::Success(final_value));
                }
                CritiqueDecision::Reject(reason) => {
                    warn!(iteration, reason = %reason, "critic rejected");
                    return Ok(TurnOutcome::Failed(format!("critic rejected: {reason}")));
                }
                CritiqueDecision::Reflect(feedback) => {
                    debug!(iteration, feedback = %feedback, "critic reflected");
                    last_feedback = Some(feedback);
                    // Loop back to plan with feedback.
                }
            }
        }
    }

    /// Execute every step in `plan` against the executor, threading state
    /// through the DAG. Returns the parallel array of outputs.
    async fn execute_plan(
        &self,
        plan: &Plan,
        dag: &mut DagGraph,
        executor: &Executor,
        ctx: &NodeContext,
    ) -> AgentResult<Vec<Value>> {
        let mut results = Vec::with_capacity(plan.steps.len());
        for step in &plan.steps {
            // Reuse an existing Pending node with the same label, or add a
            // new one. This keeps the DAG honest about what actually ran.
            let idx = find_or_add_node(dag, &step.label, step.inputs.clone());
            // Claim + execute + complete.
            dag.claim(idx, ctx.identity.pid)?;
            let node_id = dag
                .node(idx)
                .ok_or_else(|| AgentError::Execution(format!("node {idx} vanished under claim")))?
                .id;
            match executor
                .execute(
                    &DagNode {
                        id: node_id,
                        label: step.label.clone(),
                        status: DagStatus::Running,
                        claimed_by: Some(ctx.identity.pid),
                        inputs: step.inputs.clone(),
                        outputs: Value::Null,
                        error_message: None,
                    },
                    ctx,
                )
                .await
            {
                Ok(out) => {
                    dag.complete(idx, out.clone())?;
                    results.push(out);
                }
                Err(e) => {
                    dag.fail(idx, e.to_string());
                    return Err(AgentError::Execution(format!(
                        "step '{}' failed: {e}",
                        step.label
                    )));
                }
            }
        }
        Ok(results)
    }
}

/// Find the first Pending node with the given label, or add a new node.
fn find_or_add_node(
    dag: &mut DagGraph,
    label: &str,
    inputs: Value,
) -> mscode_dag_runtime::NodeIndex {
    for (idx, node) in dag.nodes_iter() {
        if node.label == label && node.status == DagStatus::Pending {
            return idx;
        }
    }
    dag.add_node(DagNode::new(label, inputs))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use mscode_dag_runtime::DagGraph;
    use mscode_exec::{AgentIdentity, ExecError, Executor, HandlerSpec, NodeHandler};
    use mscode_provider::MockLlmProvider;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn ctx() -> NodeContext {
        NodeContext::new(PathBuf::from("/tmp/ws"), AgentIdentity::new("tester", 1))
    }

    /// A handler that echoes its inputs.
    struct EchoHandler {
        spec: HandlerSpec,
    }
    #[async_trait]
    impl NodeHandler for EchoHandler {
        async fn handle(&self, input: Value, _ctx: &NodeContext) -> Result<Value, ExecError> {
            Ok(input)
        }
        fn spec(&self) -> &HandlerSpec {
            &self.spec
        }
    }

    fn make_executor() -> Executor {
        let mut e = Executor::new();
        e.register(EchoHandler {
            spec: HandlerSpec::new("echo", "echo"),
        });
        e
    }

    /// Mock provider whose response mutates per call. We use a shared
    /// `Arc<Mutex<Vec<String>>>` queue of canned JSON responses.
    fn supervisor_with_responses(
        plan_responses: Vec<String>,
        critique_responses: Vec<String>,
    ) -> Supervisor {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // We use the same MockLlmProvider for both planner and critic. Since
        // the supervisor constructs both from the same Arc, both will see the
        // same canned response. To interleave plan + critique responses,
        // we encode them as a single queue and let the order do the work.
        // The queue must be: plan[0], critique[0], plan[1], critique[1], ...
        let mut interleaved: Vec<String> = Vec::new();
        let max = plan_responses.len().max(critique_responses.len());
        for i in 0..max {
            if i < plan_responses.len() {
                interleaved.push(plan_responses[i].clone());
            }
            if i < critique_responses.len() {
                interleaved.push(critique_responses[i].clone());
            }
        }
        let queue = Arc::new(Mutex::new(interleaved));
        let counter = Arc::new(AtomicUsize::new(0));
        struct QueuedMock {
            queue: Arc<Mutex<Vec<String>>>,
            counter: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl LlmProvider for QueuedMock {
            async fn complete(
                &self,
                _req: &mscode_provider::LlmRequest,
            ) -> mscode_provider::Result<mscode_provider::LlmResponse> {
                let idx = self.counter.fetch_add(1, Ordering::SeqCst);
                let next = self
                    .queue
                    .lock()
                    .unwrap()
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| "{}".to_string());
                Ok(mscode_provider::LlmResponse::text("test-model", next))
            }
            async fn stream(
                &self,
                _req: &mscode_provider::LlmRequest,
                _sink: &mut dyn mscode_provider::StreamSink,
            ) -> mscode_provider::Result<()> {
                Ok(())
            }
            fn name(&self) -> &str {
                "queued-mock"
            }
            fn supports_tools(&self) -> bool {
                false
            }
        }
        let m = Arc::new(QueuedMock {
            queue: queue.clone(),
            counter: counter.clone(),
        });
        Supervisor::new(m as Arc<dyn LlmProvider>, "test-model")
    }

    #[test]
    fn supervisor_max_reflections_is_three() {
        let m = MockLlmProvider::default();
        let sup = Supervisor::new(Arc::new(m), "x");
        assert_eq!(sup.max_reflections(), 3);
        assert_eq!(MAX_REFLECTIONS, 3);
    }

    #[tokio::test]
    async fn supervisor_run_turn_succeeds_on_first_attempt() {
        let plan_json = json!({
            "steps": [{"label": "echo", "inputs": {"v": 42}}],
            "goal": "test",
            "expected_output": null
        });
        let critique_json = json!({
            "decision": {"kind": "accept"},
            "reasoning": "good"
        });
        let sup =
            supervisor_with_responses(vec![plan_json.to_string()], vec![critique_json.to_string()]);
        let exec = make_executor();
        let mut dag = DagGraph::new();
        let outcome = sup.run_turn("test", &mut dag, &exec, &ctx()).await.unwrap();
        match outcome {
            TurnOutcome::Success(v) => assert_eq!(v, json!({"v": 42})),
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn supervisor_reflects_on_suboptimal_result() {
        // Iteration 1: plan + reflect.
        // Iteration 2: plan + accept.
        let plans = vec![
            json!({"steps": [{"label": "echo", "inputs": 1}], "goal": "g"}).to_string(),
            json!({"steps": [{"label": "echo", "inputs": 2}], "goal": "g"}).to_string(),
        ];
        let critiques = vec![
            json!({"decision": {"kind": "reflect", "feedback": "again"}, "reasoning": "weak"})
                .to_string(),
            json!({"decision": {"kind": "accept"}, "reasoning": "ok"}).to_string(),
        ];
        let sup = supervisor_with_responses(plans, critiques);
        let exec = make_executor();
        let mut dag = DagGraph::new();
        let outcome = sup.run_turn("g", &mut dag, &exec, &ctx()).await.unwrap();
        assert!(matches!(outcome, TurnOutcome::Success(_)));
    }

    #[tokio::test]
    async fn supervisor_returns_reflections_exhausted_after_3_iterations() {
        // All 3 critiques reflect, the 4th call would exceed the cap.
        let plans = vec![
            json!({"steps": [{"label": "echo"}], "goal": "g"}).to_string(),
            json!({"steps": [{"label": "echo"}], "goal": "g"}).to_string(),
            json!({"steps": [{"label": "echo"}], "goal": "g"}).to_string(),
            json!({"steps": [{"label": "echo"}], "goal": "g"}).to_string(),
        ];
        let critiques = vec![
            json!({"decision": {"kind": "reflect", "feedback": "f1"}, "reasoning": "r1"})
                .to_string(),
            json!({"decision": {"kind": "reflect", "feedback": "f2"}, "reasoning": "r2"})
                .to_string(),
            json!({"decision": {"kind": "reflect", "feedback": "f3"}, "reasoning": "r3"})
                .to_string(),
            json!({"decision": {"kind": "accept"}, "reasoning": "would-be-4th"}).to_string(),
        ];
        let sup = supervisor_with_responses(plans, critiques);
        let exec = make_executor();
        let mut dag = DagGraph::new();
        let outcome = sup.run_turn("g", &mut dag, &exec, &ctx()).await.unwrap();
        assert_eq!(outcome, TurnOutcome::ReflectionsExhausted);
    }

    #[tokio::test]
    async fn supervisor_returns_failed_on_reject() {
        let plan = json!({"steps": [{"label": "echo"}], "goal": "g"}).to_string();
        let critique = json!({
            "decision": {"kind": "reject", "reason": "wrong"},
            "reasoning": "bad"
        })
        .to_string();
        let sup = supervisor_with_responses(vec![plan], vec![critique]);
        let exec = make_executor();
        let mut dag = DagGraph::new();
        let outcome = sup.run_turn("g", &mut dag, &exec, &ctx()).await.unwrap();
        match outcome {
            TurnOutcome::Failed(msg) => assert!(msg.contains("critic rejected")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn supervisor_returns_failed_on_executor_error() {
        // Plan calls a handler that is NOT registered in the executor.
        let plan = json!({"steps": [{"label": "ghost"}], "goal": "g"}).to_string();
        let critique = json!({"decision": {"kind": "accept"}, "reasoning": "x"}).to_string();
        let sup = supervisor_with_responses(vec![plan], vec![critique]);
        let exec = Executor::new(); // no handlers
        let mut dag = DagGraph::new();
        let outcome = sup.run_turn("g", &mut dag, &exec, &ctx()).await.unwrap();
        match outcome {
            TurnOutcome::Failed(msg) => assert!(msg.contains("execution error")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn supervisor_marks_dag_node_failed_on_executor_error() {
        let plan = json!({"steps": [{"label": "ghost"}], "goal": "g"}).to_string();
        let critique = json!({"decision": {"kind": "accept"}, "reasoning": "x"}).to_string();
        let sup = supervisor_with_responses(vec![plan], vec![critique]);
        let exec = Executor::new();
        let mut dag = DagGraph::new();
        let _ = sup.run_turn("g", &mut dag, &exec, &ctx()).await.unwrap();
        // The ghost node should be present and Failed.
        let failed_count = dag
            .nodes_iter()
            .filter(|(_, n)| n.status == DagStatus::Failed)
            .count();
        assert_eq!(failed_count, 1);
    }

    #[test]
    fn max_reflections_constant_value() {
        assert_eq!(MAX_REFLECTIONS, 3);
    }

    #[test]
    fn supervisor_exposes_provider_and_quartet_parts() {
        let m: Arc<dyn LlmProvider> = Arc::new(MockLlmProvider::default());
        let m_clone = m.clone();
        let sup = Supervisor::new(m, "m");
        assert_eq!(sup.model(), "m");
        assert!(Arc::ptr_eq(sup.provider(), &m_clone));
        // Planner and Critic are accessible.
        let _ = sup.planner();
        let _ = sup.critic();
    }
}
