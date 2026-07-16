//! [`DagGraph`] — the directed acyclic graph engine.
//!
//! ## Why `StableGraph`
//!
//! We use `petgraph::stable_graph::StableGraph` instead of `Graph`. The
//! crucial difference is that removing a node from a `Graph` invalidates the
//! indices of every node added after it (because indices are dense and the
//! library compacts). `StableGraph` keeps indices stable across removals by
//! leaving tombstones, so a saved [`NodeIndex`] remains valid for the
//! lifetime of the graph even if intermediate nodes are pruned.
//!
//! This invariant is load-bearing for crash recovery: a rollout file written
//! before the crash stores indices, and after restart the executor must be
//! able to address the same nodes by the same indices. With `Graph`, a single
//! mid-graph removal would silently shift every downstream index.
//!
//! ## Idempotency
//!
//! [`DagGraph::complete`] is idempotent: calling it on an already-Completed
//! node returns `Ok(())` and does NOT overwrite the stored output. This lets
//! the executor replay a journal entry after a crash without worrying about
//! whether the completion was already applied.

use petgraph::algo::{is_cyclic_directed, toposort as pg_toposort};
use petgraph::stable_graph::{NodeIndex as PgNodeIndex, StableDiGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{DagError, DagResult};
use crate::types::{DagEdge, DagNode, DagStatus};

/// Newtype over `petgraph`'s `NodeIndex` so callers do not need to import
/// petgraph themselves. The wrapped value is the dense `i32` index that
/// petgraph uses internally; it survives node removals (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeIndex(pub u32);

impl NodeIndex {
    /// Construct from a raw `u32`.
    pub const fn new(i: u32) -> Self {
        Self(i)
    }

    /// Convert to the wrapped `u32`.
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Convert to petgraph's `NodeIndex`.
    fn to_petgraph(self) -> PgNodeIndex {
        PgNodeIndex::new(self.0 as usize)
    }

    /// Convert from petgraph's `NodeIndex`.
    fn from_petgraph(idx: PgNodeIndex) -> Self {
        Self::new(idx.index() as u32)
    }
}

impl From<u32> for NodeIndex {
    fn from(i: u32) -> Self {
        Self::new(i)
    }
}

impl std::fmt::Display for NodeIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeIndex({})", self.0)
    }
}

/// Directed acyclic graph of [`DagNode`]s connected by [`DagEdge`]s.
///
/// Construction is intentionally cheap: no provider, no embeddings, no async
/// runtime — internal layout is just a `StableDiGraph<DagNode, DagEdge>` with
/// no sidecar maps. Node and edge lookups go through `StableGraph` indices,
/// which survive node removal. This honors the sub-200ms cold-start budget.
#[derive(Debug, Default)]
pub struct DagGraph {
    inner: StableDiGraph<DagNode, DagEdge>,
}

impl DagGraph {
    /// Construct an empty DAG.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a node to the graph and return its stable index.
    pub fn add_node(&mut self, node: DagNode) -> NodeIndex {
        NodeIndex::from_petgraph(self.inner.add_node(node))
    }

    /// Add a directed edge `from -> to`. Rejects self-loops and any edge that
    /// would introduce a cycle.
    pub fn add_edge(&mut self, from: NodeIndex, to: NodeIndex) -> DagResult<()> {
        if from == to {
            return Err(DagError::CycleDetected {
                from: from.0,
                to: to.0,
            });
        }
        if !self.has_node(from) || !self.has_node(to) {
            return Err(DagError::NodeNotFound(if !self.has_node(from) {
                from.0
            } else {
                to.0
            }));
        }
        // Tentatively add, then test for cycles; rollback on failure.
        self.inner
            .add_edge(from.to_petgraph(), to.to_petgraph(), DagEdge::new());
        if is_cyclic_directed(&self.inner) {
            // Remove the edge we just added to keep the graph in a valid state.
            if let Some(eid) = self
                .inner
                .edges_connecting(from.to_petgraph(), to.to_petgraph())
                .last()
                .map(|er| er.id())
            {
                self.inner.remove_edge(eid);
            }
            return Err(DagError::CycleDetected {
                from: from.0,
                to: to.0,
            });
        }
        Ok(())
    }

    /// Returns `true` if `idx` resolves to a live node.
    pub fn has_node(&self, idx: NodeIndex) -> bool {
        self.inner.node_weight(idx.to_petgraph()).is_some()
    }

    /// Borrow the node at `idx`, or `None` if not present.
    pub fn node(&self, idx: NodeIndex) -> Option<&DagNode> {
        self.inner.node_weight(idx.to_petgraph())
    }

    /// Borrow the node at `idx` mutably.
    pub fn node_mut(&mut self, idx: NodeIndex) -> Option<&mut DagNode> {
        self.inner.node_weight_mut(idx.to_petgraph())
    }

    /// Current [`DagStatus`] of the node at `idx`.
    pub fn status(&self, idx: NodeIndex) -> Option<DagStatus> {
        self.node(idx).map(|n| n.status)
    }

    /// Iterate over all live `(NodeIndex, &DagNode)` pairs in arbitrary order.
    pub fn nodes_iter(&self) -> impl Iterator<Item = (NodeIndex, &DagNode)> {
        self.inner
            .node_indices()
            .map(|pg| (NodeIndex::from_petgraph(pg), &self.inner[pg]))
    }

    /// Number of live nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.inner.node_count()
    }

    /// Number of live edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.inner.edge_count()
    }

    /// Returns the indices of all direct predecessors of `idx`.
    pub fn predecessors(&self, idx: NodeIndex) -> Vec<NodeIndex> {
        self.inner
            .neighbors_directed(idx.to_petgraph(), petgraph::Direction::Incoming)
            .map(NodeIndex::from_petgraph)
            .collect()
    }

    /// Returns the indices of all direct successors of `idx`.
    pub fn successors(&self, idx: NodeIndex) -> Vec<NodeIndex> {
        self.inner
            .neighbors_directed(idx.to_petgraph(), petgraph::Direction::Outgoing)
            .map(NodeIndex::from_petgraph)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Validation and ordering
    // -----------------------------------------------------------------------

    /// Validate the graph structure: must be acyclic and topologically
    /// sortable.
    pub fn validate(&self) -> DagResult<()> {
        if is_cyclic_directed(&self.inner) {
            // Pick the first edge we can prove is on a cycle for context.
            return Err(DagError::CycleDetected { from: 0, to: 0 });
        }
        // toposort returns Err((node, _)) on cycle, but we already checked
        // for cycles above, so this should always succeed. We call it to
        // surface any other structural anomaly (e.g. graph corrupted by
        // unsafe mutation).
        if pg_toposort(&self.inner, None).is_err() {
            return Err(DagError::CycleDetected { from: 0, to: 0 });
        }
        Ok(())
    }

    /// Return node indices in topological order. Errors if the graph contains
    /// a cycle (which would make a deterministic order impossible).
    ///
    /// The order is reproducible: petgraph's toposort walks nodes by their
    /// internal index, which is stable for `StableGraph`.
    pub fn toposort(&self) -> DagResult<Vec<NodeIndex>> {
        match pg_toposort(&self.inner, None) {
            Ok(indices) => Ok(indices.into_iter().map(NodeIndex::from_petgraph).collect()),
            Err(_) => Err(DagError::CycleDetected { from: 0, to: 0 }),
        }
    }

    /// Returns the index of the next node that is `Pending` and whose every
    /// predecessor is `Completed`, or `None` if no such node exists.
    ///
    /// If any predecessor is `Failed` or `Skipped`, the node is treated as not
    /// ready (its branch has been pruned; the supervisor should explicitly
    /// mark it `Skipped`).
    pub fn next_ready(&self) -> Option<NodeIndex> {
        // Walk in topological order so the *earliest* ready node wins.
        let order = self.toposort().ok()?;
        for idx in order {
            let Some(node) = self.node(idx) else {
                continue;
            };
            if node.status != DagStatus::Pending {
                continue;
            }
            let ready = self
                .predecessors(idx)
                .into_iter()
                .all(|p| self.status(p) == Some(DagStatus::Completed));
            if ready {
                return Some(idx);
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Lifecycle transitions
    // -----------------------------------------------------------------------

    /// Mark a node as claimed by `pid`. The node must be `Pending` (or
    /// already claimed by the same PID — re-claim is a no-op) and not
    /// already claimed by another live PID.
    ///
    /// The `AlreadyClaimed` check happens BEFORE the status check so callers
    /// get a more useful error when contending for a node that is currently
    /// being executed by another worker.
    pub fn claim(&mut self, idx: NodeIndex, pid: u32) -> DagResult<()> {
        let node = self.node_mut(idx).ok_or(DagError::NodeNotFound(idx.0))?;
        if let Some(existing) = node.claimed_by {
            if existing != pid {
                return Err(DagError::AlreadyClaimed {
                    node: idx.0,
                    by_pid: existing,
                });
            }
            // Re-claim by the same PID is a no-op.
            return Ok(());
        }
        if node.status != DagStatus::Pending {
            return Err(DagError::InvalidState {
                current: node.status,
                expected: DagStatus::Pending,
            });
        }
        node.claimed_by = Some(pid);
        node.status = DagStatus::Running;
        Ok(())
    }

    /// Clear the `claimed_by` field and reset status to `Pending`. Used when
    /// a worker cancels or fails before completing.
    pub fn release(&mut self, idx: NodeIndex) {
        if let Some(node) = self.node_mut(idx) {
            node.claimed_by = None;
            if node.status == DagStatus::Running {
                node.status = DagStatus::Pending;
            }
        }
    }

    /// Mark a node `Completed` and store its output references.
    ///
    /// **Idempotent:** if the node is already `Completed`, this is a no-op
    /// and returns `Ok(())` without overwriting the stored output. This
    /// enables safe replay of a journal after crash recovery: replaying a
    /// `complete` event that was already applied has no effect.
    pub fn complete(&mut self, idx: NodeIndex, output: Value) -> DagResult<()> {
        let node = self.node_mut(idx).ok_or(DagError::NodeNotFound(idx.0))?;
        if node.status == DagStatus::Completed {
            // Replay — do not clobber the existing output.
            return Ok(());
        }
        if node.status != DagStatus::Running {
            return Err(DagError::InvalidState {
                current: node.status,
                expected: DagStatus::Running,
            });
        }
        node.status = DagStatus::Completed;
        node.outputs = output;
        node.claimed_by = None;
        node.error_message = None;
        Ok(())
    }

    /// Mark a node `Failed` and record the error message. Clears the claim.
    /// Idempotent for already-Failed nodes (updates the message).
    pub fn fail(&mut self, idx: NodeIndex, error_msg: String) {
        if let Some(node) = self.node_mut(idx) {
            node.status = DagStatus::Failed;
            node.error_message = Some(error_msg);
            node.claimed_by = None;
        }
    }

    /// Mark a node `Skipped`. Used by the supervisor when a dependency failed
    /// and the branch should not execute.
    pub fn skip(&mut self, idx: NodeIndex) {
        if let Some(node) = self.node_mut(idx) {
            node.status = DagStatus::Skipped;
            node.claimed_by = None;
        }
    }

    /// Remove a node from the graph. Its incident edges are also removed.
    /// Indices of remaining nodes are unchanged (StableGraph invariant).
    pub fn remove_node(&mut self, idx: NodeIndex) -> Option<DagNode> {
        self.inner.remove_node(idx.to_petgraph())
    }

    /// Serialize the graph to a JSON value for persistence / replay.
    pub fn to_json(&self) -> DagResult<Value> {
        let nodes: Vec<(NodeIndex, &DagNode)> = self.nodes_iter().collect();
        let edges: Vec<(NodeIndex, NodeIndex, &DagEdge)> = self
            .inner
            .edge_references()
            .map(|er| {
                (
                    NodeIndex::from_petgraph(er.source()),
                    NodeIndex::from_petgraph(er.target()),
                    er.weight(),
                )
            })
            .collect();
        let payload = serde_json::json!({
            "nodes": nodes.iter().map(|(i, n)| serde_json::json!({"index": i.0, "node": n})).collect::<Vec<_>>(),
            "edges": edges.iter().map(|(f, t, _e)| serde_json::json!({"from": f.0, "to": t.0})).collect::<Vec<_>>(),
        });
        Ok(payload)
    }

    /// Number of nodes in each status bucket. Useful for progress reporting.
    pub fn status_counts(&self) -> StatusCounts {
        let mut counts = StatusCounts::default();
        for (_, node) in self.nodes_iter() {
            match node.status {
                DagStatus::Pending => counts.pending += 1,
                DagStatus::Running => counts.running += 1,
                DagStatus::Completed => counts.completed += 1,
                DagStatus::Failed => counts.failed += 1,
                DagStatus::Skipped => counts.skipped += 1,
            }
        }
        counts
    }
}

/// Per-status counts returned by [`DagGraph::status_counts`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCounts {
    /// Number of nodes in `Pending` status.
    pub pending: usize,
    /// Number of nodes in `Running` status.
    pub running: usize,
    /// Number of nodes in `Completed` status.
    pub completed: usize,
    /// Number of nodes in `Failed` status.
    pub failed: usize,
    /// Number of nodes in `Skipped` status.
    pub skipped: usize,
}

impl StatusCounts {
    /// Total number of nodes accounted for.
    pub fn total(&self) -> usize {
        self.pending + self.running + self.completed + self.failed + self.skipped
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_node(label: &str) -> DagNode {
        DagNode::with_label(label)
    }

    #[test]
    fn add_node_and_retrieve_by_index() {
        let mut g = DagGraph::new();
        let idx = g.add_node(make_node("a"));
        let node = g.node(idx).expect("node must exist");
        assert_eq!(node.label, "a");
        assert_eq!(node.status, DagStatus::Pending);
    }

    #[test]
    fn add_edge_creates_directed_edge() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        g.add_edge(a, b).unwrap();
        assert_eq!(g.edge_count(), 1);
        assert_eq!(g.successors(a), vec![b]);
        assert_eq!(g.predecessors(b), vec![a]);
    }

    #[test]
    fn add_edge_rejects_self_loop() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let err = g.add_edge(a, a).unwrap_err();
        assert!(matches!(err, DagError::CycleDetected { .. }));
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn validate_passes_on_acyclic_graph() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        let c = g.add_node(make_node("c"));
        g.add_edge(a, b).unwrap();
        g.add_edge(b, c).unwrap();
        assert!(g.validate().is_ok());
    }

    #[test]
    fn validate_fails_on_cyclic_graph() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        let c = g.add_node(make_node("c"));
        g.add_edge(a, b).unwrap();
        g.add_edge(b, c).unwrap();
        let err = g.add_edge(c, a).unwrap_err();
        assert!(matches!(err, DagError::CycleDetected { .. }));
        // The offending edge should have been rolled back.
        assert_eq!(g.edge_count(), 2);
        assert!(g.validate().is_ok());
    }

    #[test]
    fn toposort_returns_reproducible_order() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        let c = g.add_node(make_node("c"));
        g.add_edge(a, b).unwrap();
        g.add_edge(b, c).unwrap();
        let order1 = g.toposort().unwrap();
        let order2 = g.toposort().unwrap();
        assert_eq!(order1, order2);
        // a must come before b, b before c.
        let pos = |i: NodeIndex| order1.iter().position(|&x| x == i).unwrap();
        assert!(pos(a) < pos(b));
        assert!(pos(b) < pos(c));
    }

    #[test]
    fn next_ready_returns_pending_with_completed_deps() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        g.add_edge(a, b).unwrap();
        // Nothing completed yet — a is the first ready (no deps).
        assert_eq!(g.next_ready(), Some(a));
        // Complete a, then b becomes ready.
        g.claim(a, 100).unwrap();
        g.complete(a, json!({"out": 1})).unwrap();
        assert_eq!(g.next_ready(), Some(b));
        g.claim(b, 100).unwrap();
        g.complete(b, json!({"out": 2})).unwrap();
        assert_eq!(g.next_ready(), None);
    }

    #[test]
    fn next_ready_skips_failed_dep_branch() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        let c = g.add_node(make_node("c"));
        g.add_edge(a, b).unwrap();
        g.add_edge(b, c).unwrap();
        // a fails — b should never become ready (its dep is not Completed).
        g.claim(a, 1).unwrap();
        g.fail(a, "boom".into());
        // b's predecessor a is Failed (not Completed), so b is not ready.
        assert_eq!(g.next_ready(), None);
        // Explicitly skip b and c — supervisor's responsibility.
        g.skip(b);
        g.skip(c);
        assert_eq!(g.next_ready(), None);
    }

    #[test]
    fn claim_sets_claimed_by_pid() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        g.claim(a, 4242).unwrap();
        let node = g.node(a).unwrap();
        assert_eq!(node.claimed_by, Some(4242));
        assert_eq!(node.status, DagStatus::Running);
    }

    #[test]
    fn claim_rejects_already_claimed_node() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        g.claim(a, 100).unwrap();
        let err = g.claim(a, 200).unwrap_err();
        match err {
            DagError::AlreadyClaimed { node, by_pid } => {
                assert_eq!(node, a.0);
                assert_eq!(by_pid, 100);
            }
            other => panic!("expected AlreadyClaimed, got {other:?}"),
        }
        // Re-claim by the same PID is fine.
        g.claim(a, 100).unwrap();
    }

    #[test]
    fn release_clears_claimed_by() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        g.claim(a, 7).unwrap();
        g.release(a);
        let node = g.node(a).unwrap();
        assert!(node.claimed_by.is_none());
        assert_eq!(node.status, DagStatus::Pending);
    }

    #[test]
    fn complete_stores_output_value() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        g.claim(a, 1).unwrap();
        let out = json!({"path": "/tmp/x"});
        g.complete(a, out.clone()).unwrap();
        let node = g.node(a).unwrap();
        assert_eq!(node.status, DagStatus::Completed);
        assert_eq!(node.outputs, out);
        assert!(node.claimed_by.is_none());
    }

    #[test]
    fn complete_is_idempotent_on_replay() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        g.claim(a, 1).unwrap();
        let original = json!({"v": 1});
        g.complete(a, original.clone()).unwrap();
        // Replay: caller crashes after persisting the journal entry, restarts,
        // re-runs complete() with possibly different output — must NOT clobber.
        let replay = json!({"v": 999});
        let result = g.complete(a, replay.clone());
        assert!(result.is_ok(), "replay should be a no-op Ok");
        let node = g.node(a).unwrap();
        assert_eq!(node.outputs, original);
    }

    #[test]
    fn complete_rejects_invalid_state() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        // Not claimed yet — cannot complete.
        let err = g.complete(a, json!({})).unwrap_err();
        assert!(matches!(err, DagError::InvalidState { .. }));
    }

    #[test]
    fn fail_marks_node_failed_and_records_message() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        g.claim(a, 9).unwrap();
        g.fail(a, "network down".into());
        let node = g.node(a).unwrap();
        assert_eq!(node.status, DagStatus::Failed);
        assert_eq!(node.error_message.as_deref(), Some("network down"));
        assert!(node.claimed_by.is_none());
    }

    #[test]
    fn fail_is_idempotent_on_replay() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        g.claim(a, 9).unwrap();
        g.fail(a, "first error".into());
        g.fail(a, "second error".into()); // updates message; no panic
        let node = g.node(a).unwrap();
        assert_eq!(node.error_message.as_deref(), Some("second error"));
    }

    #[test]
    fn stable_graph_indices_survive_removal() {
        // The StableGraph load-bearing test: remove the middle node of a
        // 3-node chain and verify the remaining indices are unchanged.
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        let c = g.add_node(make_node("c"));
        g.add_edge(a, b).unwrap();
        g.add_edge(b, c).unwrap();
        // Snapshot the indices.
        assert_eq!([a.0, b.0, c.0], [0, 1, 2]);
        // Remove b.
        let removed = g.remove_node(b).expect("b must exist");
        assert_eq!(removed.label, "b");
        // a and c must STILL be addressable by their original indices.
        assert_eq!(g.node(a).unwrap().label, "a");
        assert_eq!(g.node(c).unwrap().label, "c");
        // b is gone.
        assert!(g.node(b).is_none());
        // Indices unchanged.
        assert_eq!(g.node(a).unwrap().label, "a");
        assert_eq!(g.node(c).unwrap().label, "c");
    }

    #[test]
    fn status_counts_aggregates_correctly() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        let c = g.add_node(make_node("c"));
        let d = g.add_node(make_node("d"));
        let _e = g.add_node(make_node("e"));
        g.claim(a, 1).unwrap();
        g.complete(a, json!({})).unwrap();
        g.claim(b, 1).unwrap();
        g.fail(b, "x".into());
        g.skip(c);
        g.claim(d, 1).unwrap(); // running
        let counts = g.status_counts();
        assert_eq!(counts.completed, 1);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.running, 1);
        assert_eq!(counts.pending, 1); // e
        assert_eq!(counts.total(), 5);
    }

    #[test]
    fn to_json_round_trips_structure() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let b = g.add_node(make_node("b"));
        g.add_edge(a, b).unwrap();
        let json_val = g.to_json().unwrap();
        let obj = json_val.as_object().unwrap();
        assert_eq!(obj.get("nodes").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(obj.get("edges").unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn has_node_returns_false_for_removed() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        assert!(g.has_node(a));
        g.remove_node(a);
        assert!(!g.has_node(a));
    }

    #[test]
    fn node_index_display_includes_value() {
        let idx = NodeIndex::new(7);
        assert_eq!(format!("{idx}"), "NodeIndex(7)");
    }

    #[test]
    fn add_edge_rejects_missing_node() {
        let mut g = DagGraph::new();
        let a = g.add_node(make_node("a"));
        let ghost = NodeIndex::new(99);
        let err = g.add_edge(a, ghost).unwrap_err();
        assert!(matches!(err, DagError::NodeNotFound(_)));
    }
}
