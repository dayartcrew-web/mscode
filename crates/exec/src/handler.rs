//! [`NodeHandler`] trait, [`HandlerSpec`], and [`Executor`].
//!
//! ## Idempotency contract
//!
//! [`NodeHandler::handle`] MUST be idempotent: the same `input` MUST produce
//! the same `output`. This is a hard requirement because the supervisor may
//! retry a node after a transient failure (network blip, crash) and the
//! executor's replay logic assumes that re-running a handler is safe.
//!
//! Handlers that are expensive or have side effects (file writes, network
//! calls) can opt into a replay cache by overriding
//! [`NodeHandler::idempotency_key`]: when the key returns `Some(k)`, the
//! executor (or an outer caching layer) may cache `(k -> output)` and skip
//! future calls with the same key.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use mscode_dag_runtime::DagNode;

use crate::context::NodeContext;
use crate::error::{ExecError, ExecResult};

/// Static description of a handler. Returned by [`NodeHandler::spec`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandlerSpec {
    /// Matches [`DagNode::label`] — this is the key used by the executor to
    /// dispatch.
    pub name: String,
    /// Human-readable description shown in tool listings and logs.
    pub description: String,
    /// Optional JSON schema describing accepted input shape. `Null` when the
    /// handler accepts any JSON.
    #[serde(default)]
    pub input_schema: Value,
}

impl HandlerSpec {
    /// Construct a new spec with a null input schema.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: Value::Null,
        }
    }

    /// Replace the input schema.
    #[must_use]
    pub fn with_schema(mut self, schema: Value) -> Self {
        self.input_schema = schema;
        self
    }
}

/// Async handler for a single DAG node body.
///
/// Implementations MUST be `Send + Sync` because they are stored inside an
/// `Arc<dyn NodeHandler>` shared across tokio tasks.
///
/// **Idempotency contract:** see the module docs.
#[async_trait]
pub trait NodeHandler: Send + Sync {
    /// Run the handler.
    ///
    /// # Arguments
    /// * `input` — the node's `inputs` payload (references, not large blobs).
    /// * `ctx` — runtime context (workspace, identity, retry count).
    async fn handle(&self, input: Value, ctx: &NodeContext) -> ExecResult<Value>;

    /// Static metadata for this handler. Returning a reference lets the
    /// executor list specs cheaply without copying.
    fn spec(&self) -> &HandlerSpec;

    /// Optional idempotency key for replay caching. Returning `None` (the
    /// default) means the executor will always call [`handle`]. Returning
    /// `Some(k)` lets an outer layer cache `(k -> output)` and skip future
    /// invocations with the same key.
    fn idempotency_key(&self, _input: &Value) -> Option<String> {
        None
    }
}

/// Type-erased handler with its name precomputed for cheap lookup.
pub type BoxedHandler = Arc<dyn NodeHandler>;

/// Registry of handlers keyed by `HandlerSpec::name`. Owns the dispatch from
/// `DagNode::label` to the handler implementation.
pub struct Executor {
    handlers: HashMap<String, BoxedHandler>,
}

impl Executor {
    /// Construct an empty executor.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler by value. The handler's `spec().name` is used as
    /// the registry key.
    pub fn register<H>(&mut self, handler: H)
    where
        H: NodeHandler + 'static,
    {
        let name = handler.spec().name.clone();
        self.handlers.insert(name, Arc::new(handler));
    }

    /// Register an already-boxed handler under an explicit name. Useful when
    /// constructing handlers dynamically (e.g. from a plugin loader).
    pub fn register_boxed(&mut self, name: impl Into<String>, handler: Box<dyn NodeHandler>) {
        self.handlers.insert(name.into(), Arc::from(handler));
    }

    /// Returns references to the specs of every registered handler.
    pub fn list_handlers(&self) -> Vec<&HandlerSpec> {
        self.handlers.values().map(|h| h.spec()).collect()
    }

    /// Execute a single DAG node by dispatching to the registered handler.
    pub async fn execute(&self, node: &DagNode, ctx: &NodeContext) -> ExecResult<Value> {
        let handler = self
            .handlers
            .get(&node.label)
            .ok_or_else(|| ExecError::HandlerNotFound(node.label.clone()))?;
        handler.handle(node.inputs.clone(), ctx).await
    }

    /// Returns the number of registered handlers.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Returns `true` if no handlers are registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl Default for Executor {
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
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::PathBuf;

    use crate::context::AgentIdentity;

    /// A minimal echo handler used by multiple tests.
    struct EchoHandler {
        spec: HandlerSpec,
    }
    impl EchoHandler {
        fn new() -> Self {
            Self {
                spec: HandlerSpec::new("echo", "echoes the input"),
            }
        }
    }
    #[async_trait]
    impl NodeHandler for EchoHandler {
        async fn handle(&self, input: Value, _ctx: &NodeContext) -> ExecResult<Value> {
            Ok(input)
        }
        fn spec(&self) -> &HandlerSpec {
            &self.spec
        }
    }

    /// A handler that always fails.
    struct FailHandler {
        spec: HandlerSpec,
    }
    impl FailHandler {
        fn new() -> Self {
            Self {
                spec: HandlerSpec::new("fail", "always fails"),
            }
        }
    }
    #[async_trait]
    impl NodeHandler for FailHandler {
        async fn handle(&self, _input: Value, _ctx: &NodeContext) -> ExecResult<Value> {
            Err(ExecError::HandlerFailed("intentional failure".into()))
        }
        fn spec(&self) -> &HandlerSpec {
            &self.spec
        }
    }

    /// A handler that returns a deterministic idempotency key.
    struct CachedHandler {
        spec: HandlerSpec,
    }
    impl CachedHandler {
        fn new() -> Self {
            Self {
                spec: HandlerSpec::new("cached", "idempotent"),
            }
        }
    }
    #[async_trait]
    impl NodeHandler for CachedHandler {
        async fn handle(&self, input: Value, _ctx: &NodeContext) -> ExecResult<Value> {
            Ok(json!({"echoed": input}))
        }
        fn spec(&self) -> &HandlerSpec {
            &self.spec
        }
        fn idempotency_key(&self, input: &Value) -> Option<String> {
            Some(format!("cached:{input}"))
        }
    }

    fn ctx() -> NodeContext {
        NodeContext::new(PathBuf::from("/tmp/ws"), AgentIdentity::new("tester", 1))
    }

    #[test]
    fn executor_starts_empty() {
        let e = Executor::new();
        assert!(e.is_empty());
        assert_eq!(e.len(), 0);
        assert!(e.list_handlers().is_empty());
    }

    #[test]
    fn register_adds_handler_to_registry() {
        let mut e = Executor::new();
        e.register(EchoHandler::new());
        assert_eq!(e.len(), 1);
        assert!(!e.is_empty());
        let names: Vec<&str> = e.list_handlers().iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"echo"));
    }

    #[tokio::test]
    async fn execute_dispatches_to_registered_handler() {
        let mut e = Executor::new();
        e.register(EchoHandler::new());
        let node = DagNode::new("echo", json!({"hello": "world"}));
        let out = e.execute(&node, &ctx()).await.unwrap();
        assert_eq!(out, json!({"hello": "world"}));
    }

    #[tokio::test]
    async fn execute_returns_error_for_unknown_handler() {
        let e = Executor::new();
        let node = DagNode::with_label("ghost");
        let err = e.execute(&node, &ctx()).await.unwrap_err();
        match err {
            ExecError::HandlerNotFound(name) => assert_eq!(name, "ghost"),
            other => panic!("expected HandlerNotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_propagates_handler_error() {
        let mut e = Executor::new();
        e.register(FailHandler::new());
        let node = DagNode::with_label("fail");
        let err = e.execute(&node, &ctx()).await.unwrap_err();
        match err {
            ExecError::HandlerFailed(msg) => assert_eq!(msg, "intentional failure"),
            other => panic!("expected HandlerFailed, got {other:?}"),
        }
    }

    #[test]
    fn idempotency_key_default_returns_none() {
        let h = EchoHandler::new();
        assert!(h.idempotency_key(&json!({})).is_none());
    }

    #[test]
    fn idempotency_key_override_returns_some() {
        let h = CachedHandler::new();
        let key = h.idempotency_key(&json!(42)).unwrap();
        assert_eq!(key, "cached:42");
    }

    #[test]
    fn list_handlers_returns_all_specs() {
        let mut e = Executor::new();
        e.register(EchoHandler::new());
        e.register(FailHandler::new());
        e.register(CachedHandler::new());
        let specs = e.list_handlers();
        assert_eq!(specs.len(), 3);
        let names: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
        assert!(names.contains(&"echo".to_string()));
        assert!(names.contains(&"fail".to_string()));
        assert!(names.contains(&"cached".to_string()));
    }

    #[test]
    fn handler_spec_with_schema_stores_schema() {
        let spec = HandlerSpec::new("x", "d").with_schema(json!({"type": "object"}));
        assert_eq!(spec.input_schema, json!({"type": "object"}));
    }

    #[tokio::test]
    async fn cached_handler_round_trip() {
        let mut e = Executor::new();
        e.register(CachedHandler::new());
        let node = DagNode::new("cached", json!("input"));
        let out = e.execute(&node, &ctx()).await.unwrap();
        assert_eq!(out, json!({"echoed": "input"}));
    }
}
