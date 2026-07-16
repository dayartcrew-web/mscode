//! DAG runtime for the mscode agentic CLI.
//!
//! This crate provides the directed-acyclic-graph engine used by the agent
//! quartet to plan and track multi-step work. It is deliberately
//! infrastructure-light:
//!
//! - No async runtime (`petgraph` is sync, lifecycle transitions are sync).
//! - No LLM provider, no embeddings, no I/O.
//! - Construction is O(1): [`DagGraph::new`] just allocates an empty
//!   `StableDiGraph`. This honors the sub-200ms cold-start budget.
//!
//! ## Crash recovery
//!
//! Every node carries a `claimed_by: Option<u32>` storing the PID of the
//! process executing it. On restart, the recovery code scans for nodes stuck
//! in `Running` whose `claimed_by` PID no longer exists, and either requeues
//! them (`release`) or marks them `Failed`.
//!
//! ## Idempotency
//!
//! [`DagGraph::complete`] is idempotent: replaying a journal entry that was
//! already applied is a no-op. This is essential for at-least-once delivery
//! of completion events after a crash.
//!
//! ## Stable indices
//!
//! We use `petgraph::stable_graph::StableGraph` so that removing a node never
//! invalidates the indices of any other node. See [`graph`] module docs.

pub mod error;
pub mod graph;
pub mod types;

pub use error::{DagError, DagResult};
pub use graph::{DagGraph, NodeIndex, StatusCounts};
pub use types::{DagEdge, DagNode, DagStatus};
