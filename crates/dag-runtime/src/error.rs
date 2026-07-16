//! Error type for the DAG runtime crate.
//!
//! [`DagError`] is the canonical failure mode for every operation on
//! [`crate::DagGraph`]. It is intentionally narrow: structural problems
//! (cycles, missing nodes), lifecycle violations (claiming a node already
//! claimed by another PID), and serialization failures. All other failures
//! bubble up through the executor or agents crates.

use thiserror::Error;

use crate::DagStatus;

/// Result alias used across the dag-runtime crate.
pub type DagResult<T> = std::result::Result<T, DagError>;

/// Failures raised by [`crate::DagGraph`] operations.
#[derive(Debug, Clone, Error)]
pub enum DagError {
    /// `add_edge` rejected because the resulting graph would contain a cycle.
    #[error("cycle detected when adding edge {from} -> {to}")]
    CycleDetected {
        /// Source node index of the offending edge.
        from: u32,
        /// Target node index of the offending edge.
        to: u32,
    },

    /// An operation referenced a [`crate::NodeIndex`] that does not resolve to
    /// a live node (either never existed or was removed).
    #[error("node not found: index {0}")]
    NodeNotFound(u32),

    /// A lifecycle transition was attempted against the wrong current state
    /// (e.g. calling `complete` on a `Running` node owned by another PID).
    #[error("invalid state transition: expected {expected:?}, got {current:?}")]
    InvalidState {
        /// Observed current status.
        current: DagStatus,
        /// Status the operation required.
        expected: DagStatus,
    },

    /// `claim` was called on a node already claimed by another live process.
    /// Carries the PID currently holding the claim so the caller can decide
    /// whether to wait, force-release, or surface a conflict.
    #[error("node {node} already claimed by pid {by_pid}")]
    AlreadyClaimed {
        /// Index of the contested node.
        node: u32,
        /// PID currently holding the claim.
        by_pid: u32,
    },

    /// A graph could not be (de)serialized. Used by persistence and replay
    /// paths. The string carries the underlying serializer error.
    #[error("serialization error: {0}")]
    Serialization(String),
}
