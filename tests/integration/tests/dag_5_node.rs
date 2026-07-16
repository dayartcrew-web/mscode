//! Test 3: DAG with 5 nodes — Planner → A → B → Merge → Critic — driven
//! through the Supervisor's `run_turn`.
//!
//! The supervisor does not execute a *pre-built* DAG; it builds the DAG
//! incrementally from the planner's `Plan` and runs each step through the
//! `Executor`. So this test:
//!
//! 1. Constructs a 5-step `Plan` JSON (planner, A, B, merge, critic) and
//!    serves it from a `MockLlmProvider`.
//! 2. Registers one `NodeHandler` per step label in an `Executor`.
//! 3. Drives `Supervisor::run_turn` with a goal string.
//! 4. Verifies the final `DagGraph` contains exactly 5 nodes, all
//!    `Completed`.
//! 5. Verifies idempotency: re-running `DagGraph::complete` on an
//!    already-Completed node is a no-op (returns `Ok(())` and does NOT
//!    overwrite the stored output).

use async_trait::async_trait;
use mscode_agents::{Supervisor, TurnOutcome};
use mscode_dag_runtime::{DagGraph, DagStatus};
use mscode_exec::{AgentIdentity, ExecError, Executor, HandlerSpec, NodeContext, NodeHandler};
use mscode_provider::{LlmProvider, LlmRequest, LlmResponse, StreamSink};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Mock provider that returns a queue of canned responses in order.
/// Responses are interleaved as: plan[0], critique[0], plan[1], critique[1]...
struct QueuedMock {
    queue: Arc<Mutex<Vec<String>>>,
    counter: AtomicUsize,
}

#[async_trait]
impl LlmProvider for QueuedMock {
    async fn complete(&self, _req: &LlmRequest) -> mscode_provider::Result<LlmResponse> {
        let idx = self.counter.fetch_add(1, Ordering::SeqCst);
        let next = self
            .queue
            .lock()
            .expect("queue lock")
            .get(idx)
            .cloned()
            .unwrap_or_else(|| "{}".to_string());
        Ok(LlmResponse::text("test-model", next))
    }
    async fn stream(
        &self,
        _req: &LlmRequest,
        _sink: &mut dyn StreamSink,
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

/// Handler that records the order it was invoked and returns its inputs.
struct StepHandler {
    spec: HandlerSpec,
    visits: Arc<Mutex<Vec<String>>>,
    tag: &'static str,
}

#[async_trait]
impl NodeHandler for StepHandler {
    async fn handle(&self, input: Value, _ctx: &NodeContext) -> Result<Value, ExecError> {
        self.visits
            .lock()
            .expect("visits lock")
            .push(self.tag.to_string());
        Ok(input)
    }
    fn spec(&self) -> &HandlerSpec {
        &self.spec
    }
}

fn ctx() -> NodeContext {
    NodeContext::new(PathBuf::from("/tmp/ws"), AgentIdentity::new("tester", 1))
}

#[tokio::test]
async fn dag_5_node_parallel_plan_merge_critic() {
    let visits = Arc::new(Mutex::new(Vec::new()));
    let mut exec = Executor::new();
    for tag in ["planner", "a", "b", "merge", "critic"] {
        let h = StepHandler {
            spec: HandlerSpec::new(tag, tag),
            visits: visits.clone(),
            tag,
        };
        exec.register(h);
    }

    // Plan: 5 ordered steps. The supervisor walks them in array order
    // (sequential — parallelism within the supervisor is a future concern;
    // the test verifies the *5-node lifecycle*, not parallel scheduling).
    let plan_json = json!({
        "steps": [
            {"label": "planner", "inputs": {"goal": "g"}},
            {"label": "a",        "inputs": {"v": 1}},
            {"label": "b",        "inputs": {"v": 2}},
            {"label": "merge",    "inputs": {"v": 3}},
            {"label": "critic",   "inputs": {"v": 4}}
        ],
        "goal": "g",
        "expected_output": null
    });
    let critique_json = json!({
        "decision": {"kind": "accept"},
        "reasoning": "ok"
    });
    let queue = Arc::new(Mutex::new(vec![
        plan_json.to_string(),
        critique_json.to_string(),
    ]));
    let provider = Arc::new(QueuedMock {
        queue: queue.clone(),
        counter: AtomicUsize::new(0),
    });
    let sup = Supervisor::new(provider as Arc<dyn LlmProvider>, "test-model");

    let mut dag = DagGraph::new();
    let start = Instant::now();
    let outcome = sup
        .run_turn("g", &mut dag, &exec, &ctx())
        .await
        .expect("run_turn must not error");
    let elapsed = start.elapsed();

    // Critic accepted — outcome is Success.
    assert!(
        matches!(outcome, TurnOutcome::Success(_)),
        "expected Success, got {outcome:?}"
    );

    // All 5 nodes were dispatched exactly once.
    let visits_snapshot = visits.lock().expect("visits lock").clone();
    assert_eq!(
        visits_snapshot.len(),
        5,
        "each of 5 handlers must fire exactly once, got {visits_snapshot:?}"
    );
    assert_eq!(
        visits_snapshot,
        vec!["planner", "a", "b", "merge", "critic"],
        "handlers must fire in declared order"
    );

    // Final DAG state: 5 nodes, all Completed.
    let nodes: Vec<(_, _)> = dag.nodes_iter().collect();
    assert_eq!(nodes.len(), 5, "expected 5 nodes in DAG");
    for (_, node) in &nodes {
        assert_eq!(
            node.status,
            DagStatus::Completed,
            "node '{}' is {:?}, expected Completed",
            node.label,
            node.status
        );
    }

    // Idempotency: re-running `complete` on an already-Completed node is a
    // no-op (returns Ok(()) and does NOT overwrite the stored output).
    let (first_idx, first_node) = nodes[0];
    let original_output = first_node.outputs.clone();
    let replay = dag.complete(first_idx, json!({"OVERWRITE": true}));
    assert!(
        replay.is_ok(),
        "replay complete() must be Ok, got {replay:?}"
    );
    let after_replay = dag.node(first_idx).expect("node still present");
    assert_eq!(
        after_replay.outputs, original_output,
        "idempotent complete must not overwrite the stored output"
    );

    // Performance guardrail: the turn must complete in well under a second
    // against the in-process mock provider.
    assert!(
        elapsed.as_millis() < 1000,
        "5-node turn against mock took {elapsed:?}, expected <1s"
    );
}
