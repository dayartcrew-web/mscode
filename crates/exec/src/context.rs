//! [`NodeContext`] — execution-time context passed to every handler.
//!
//! The context carries everything a handler needs that is NOT in the node's
//! `inputs` payload: workspace path, agent identity, retry counter. This
//! separation keeps payloads serializable and reproducible while still
//! giving handlers access to runtime-only resources.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Identity of the agent executing the node. Used for audit logging and for
/// routing per-agent resources (rate limits, scratch dirs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    /// Stable identifier (e.g. `"planner"`, `"executor-3"`).
    pub name: String,
    /// PID of the process executing the node. Matches `claimed_by` on the
    /// [`crate::DagNode`].
    pub pid: u32,
}

impl AgentIdentity {
    /// Construct a new identity.
    pub fn new(name: impl Into<String>, pid: u32) -> Self {
        Self {
            name: name.into(),
            pid,
        }
    }
}

/// Execution-time context handed to every [`crate::NodeHandler::handle`]
/// invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeContext {
    /// Absolute path to the workspace root.
    pub workspace: PathBuf,
    /// Identity of the executing agent.
    pub identity: AgentIdentity,
    /// Number of times this node has been retried (0 on first attempt).
    pub retry_count: u32,
}

impl NodeContext {
    /// Construct a new context.
    pub fn new(workspace: PathBuf, identity: AgentIdentity) -> Self {
        Self {
            workspace,
            identity,
            retry_count: 0,
        }
    }

    /// Return a new context with the retry counter incremented.
    #[must_use]
    pub fn with_retry_incremented(&self) -> Self {
        Self {
            workspace: self.workspace.clone(),
            identity: self.identity.clone(),
            retry_count: self.retry_count.saturating_add(1),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_context_carries_workspace_and_identity() {
        let ctx = NodeContext::new(
            PathBuf::from("/tmp/ws"),
            AgentIdentity::new("planner", 1234),
        );
        assert_eq!(ctx.workspace, PathBuf::from("/tmp/ws"));
        assert_eq!(ctx.identity.name, "planner");
        assert_eq!(ctx.identity.pid, 1234);
        assert_eq!(ctx.retry_count, 0);
    }

    #[test]
    fn with_retry_incremented_returns_new_context() {
        let ctx = NodeContext::new(
            PathBuf::from("/tmp/ws"),
            AgentIdentity::new("planner", 1234),
        );
        let next = ctx.with_retry_incremented();
        assert_eq!(next.retry_count, 1);
        // Original unchanged.
        assert_eq!(ctx.retry_count, 0);
    }

    #[test]
    fn retry_increment_is_saturating() {
        let ctx = NodeContext {
            workspace: PathBuf::from("/"),
            identity: AgentIdentity::new("x", 1),
            retry_count: u32::MAX,
        };
        let next = ctx.with_retry_incremented();
        assert_eq!(next.retry_count, u32::MAX);
    }

    #[test]
    fn agent_identity_new_stores_fields() {
        let id = AgentIdentity::new("sup", 9);
        assert_eq!(id.name, "sup");
        assert_eq!(id.pid, 9);
    }
}
