//! DAG domain types: [`DagNode`], [`DagEdge`], [`DagStatus`].
//!
//! Design notes (Airflow XCom lesson):
//!
//! Nodes hold *references* to payloads (`serde_json::Value`) — not the
//! payloads themselves — when those payloads are large. Inputs and outputs
//! here ARE the values themselves because DAG nodes are lightweight plan
//! entries; the heavy payloads they describe are stored out-of-line by the
//! executor / rollout layer (see `mscode-rollout`). Storing big blobs inside
//! the graph would (a) make serialization for crash-recovery expensive and
//! (b) tempt the planner to read sibling payloads, recreating Airflow's
//! XCom hidden-dependency problem.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Lifecycle status of a [`DagNode`].
///
/// Transitions are driven by the executor:
///   `Pending`   -> `Running`   (via `claim`)
///   `Running`   -> `Completed` (via `complete`)
///   `Running`   -> `Failed`    (via `fail`)
///   `*`         -> `Skipped`   (when a dependency failed and the branch is
///                               pruned; set explicitly by the supervisor)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DagStatus {
    /// Node has not yet been claimed by any executor.
    #[default]
    Pending,
    /// A worker has claimed the node and is executing its body.
    Running,
    /// Node body finished successfully and `output` has been recorded.
    Completed,
    /// Node body raised an error; `error_message` has been recorded.
    Failed,
    /// Node will not run (dependency failed or supervisor pruned the branch).
    Skipped,
}

impl DagStatus {
    /// Returns `true` if the status is terminal (no further transitions).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Skipped)
    }
}

/// A single unit of work in the DAG.
///
/// `inputs` and `outputs` carry *references* (typically a path, hash, or
/// XCom-style key as a `Value::String`). They are NOT the payload itself —
/// see the module docs for the rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DagNode {
    /// Stable identifier (survives replays).
    pub id: Uuid,
    /// Handler name used by the executor to dispatch (`node.label` is the key
    /// into the executor's handler registry).
    pub label: String,
    /// Current lifecycle status.
    pub status: DagStatus,
    /// PID that has claimed this node for execution, or `None` if not claimed.
    /// Used by crash-recovery to detect orphaned nodes.
    pub claimed_by: Option<u32>,
    /// Input references (paths, keys, hashes). Never the payload body.
    pub inputs: Value,
    /// Output references, populated when `status == Completed`.
    pub outputs: Value,
    /// Error message recorded when `status == Failed`.
    pub error_message: Option<String>,
}

impl DagNode {
    /// Construct a new pending node with the given label and inputs.
    pub fn new(label: impl Into<String>, inputs: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            status: DagStatus::Pending,
            claimed_by: None,
            inputs,
            outputs: Value::Null,
            error_message: None,
        }
    }

    /// Construct a node with `Null` inputs.
    pub fn with_label(label: impl Into<String>) -> Self {
        Self::new(label, Value::Null)
    }
}

/// An edge in the DAG: `from` must complete before `to` may start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagEdge {
    /// Optional human-readable label (e.g. "data_ready").
    pub label: Option<String>,
}

impl DagEdge {
    /// Construct an unlabeled edge.
    pub fn new() -> Self {
        Self { label: None }
    }

    /// Construct an edge with a label.
    pub fn with_label(label: impl Into<String>) -> Self {
        Self {
            label: Some(label.into()),
        }
    }
}

impl Default for DagEdge {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dag_status_default_is_pending() {
        assert_eq!(DagStatus::default(), DagStatus::Pending);
    }

    #[test]
    fn dag_status_is_terminal_classifies_correctly() {
        assert!(!DagStatus::Pending.is_terminal());
        assert!(!DagStatus::Running.is_terminal());
        assert!(DagStatus::Completed.is_terminal());
        assert!(DagStatus::Failed.is_terminal());
        assert!(DagStatus::Skipped.is_terminal());
    }

    #[test]
    fn dag_node_new_initializes_fields() {
        let node = DagNode::new("fetch", serde_json::json!({"url": "x"}));
        assert_eq!(node.label, "fetch");
        assert_eq!(node.status, DagStatus::Pending);
        assert!(node.claimed_by.is_none());
        assert!(node.error_message.is_none());
        assert_eq!(node.outputs, Value::Null);
    }

    #[test]
    fn dag_node_with_label_uses_null_inputs() {
        let node = DagNode::with_label("noop");
        assert_eq!(node.label, "noop");
        assert_eq!(node.inputs, Value::Null);
    }

    #[test]
    fn dag_node_round_trips_through_json() {
        let node = DagNode::new("fetch", serde_json::json!({"k": 1}));
        let v = serde_json::to_value(&node).unwrap();
        let back: DagNode = serde_json::from_value(v).unwrap();
        assert_eq!(node, back);
    }

    #[test]
    fn dag_status_serializes_as_lowercase_snake() {
        let v = serde_json::to_value(DagStatus::Running).unwrap();
        assert_eq!(v, serde_json::json!("running"));
        let v = serde_json::to_value(DagStatus::Skipped).unwrap();
        assert_eq!(v, serde_json::json!("skipped"));
    }

    #[test]
    fn dag_edge_default_is_unlabeled() {
        let e = DagEdge::default();
        assert!(e.label.is_none());
    }

    #[test]
    fn dag_edge_with_label_stores_label() {
        let e = DagEdge::with_label("data_ready");
        assert_eq!(e.label.as_deref(), Some("data_ready"));
    }
}
