//! Executor for DAG nodes.
//!
//! This crate is intentionally separated from
//! [`mscode_dag_runtime`](../../mscode_dag_runtime) (which decides the
//! ordering) and from [`mscode_agents`](../../mscode_agents) (which decides
//! *what* to run via the LLM). The executor's single job is to dispatch a
//! prepared [`DagNode`] to the registered [`NodeHandler`] for its `label`,
//! enforcing the idempotency contract documented on [`handler`].
//!
//! ## Cold start
//!
//! Construction is O(1) — just an empty `HashMap`. No provider, no async
//! runtime spawn, no I/O. The sub-200ms budget is preserved.
//!
//! ## Trait object pattern
//!
//! Handlers are stored as `Arc<dyn NodeHandler>` so the executor can hold a
//! heterogeneous registry without monomorphizing per handler type.

pub mod context;
pub mod error;
pub mod handler;

pub use context::{AgentIdentity, NodeContext};
pub use error::{ExecError, ExecResult};
pub use handler::{BoxedHandler, Executor, HandlerSpec, NodeHandler};

// Re-export the node type at the crate root so callers can avoid importing
// dag-runtime separately for the most common case.
pub use mscode_dag_runtime::DagNode;
