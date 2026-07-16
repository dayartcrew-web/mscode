//! The `Tool` trait — async, Value I/O — and the registry that holds them.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};

/// The canonical tool trait. Every tool — built-in, MCP-backed, or future
/// WASM plugin — implements this surface.
///
/// Input and output are both [`serde_json::Value`] so the same trait can serve
/// in-process tools, network tools, and sandboxed WASM without ABI churn.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable identifier, unique within a registry. Lower-case ASCII preferred.
    fn name(&self) -> &str;

    /// Human-readable description, surfaced to the model and the user.
    fn description(&self) -> &str;

    /// JSON Schema describing the accepted input shape. Used to validate
    /// arguments before [`invoke`](Self::invoke) is called.
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given input. Implementations should propagate
    /// errors via [`ToolError`] rather than panicking.
    async fn invoke(&self, input: Value) -> ToolResult<Value>;
}

/// In-memory registry mapping tool names to their implementations.
///
/// Cheap to clone — tools are stored behind [`Arc`] so all clones share the
/// same underlying trait objects.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<HashMap<String, Arc<dyn Tool>>>,
}

impl ToolRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Replaces any existing tool with the same name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let map = Arc::make_mut(&mut self.tools);
        map.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// List all registered tools (sorted by name for deterministic output).
    pub fn list(&self) -> Vec<Arc<dyn Tool>> {
        let mut names: Vec<&String> = self.tools.keys().collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|n| self.tools.get(n).cloned())
            .collect()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Invoke a tool by name with the given input.
    pub async fn invoke_by_name(&self, name: &str, input: Value) -> ToolResult<Value> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        tool.invoke(input).await
    }

    /// Convenience helper: returns a JSON array of `{name, description, schema}`
    /// entries suitable for handing to a model as a tool catalog.
    pub fn catalog(&self) -> Value {
        let entries: Vec<Value> = self
            .list()
            .into_iter()
            .map(|t| {
                json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect();
        Value::Array(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echoes its input"
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn invoke(&self, input: Value) -> ToolResult<Value> {
            Ok(input)
        }
    }

    #[tokio::test]
    async fn registers_and_invokes_tool() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(EchoTool));
        assert_eq!(reg.len(), 1);
        let out = reg.invoke_by_name("echo", json!({"hi": 1})).await.unwrap();
        assert_eq!(out, json!({"hi": 1}));
    }

    #[tokio::test]
    async fn invoke_missing_tool_returns_not_found() {
        let reg = ToolRegistry::new();
        let err = reg.invoke_by_name("nope", json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)));
    }

    #[test]
    fn list_returns_tools_sorted_by_name() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(EchoTool));
        // Register a second tool with a lower-sorting name via anonymous impl.
        struct AlphaTool;
        #[async_trait]
        impl Tool for AlphaTool {
            fn name(&self) -> &str {
                "aaa"
            }
            fn description(&self) -> &str {
                ""
            }
            fn input_schema(&self) -> Value {
                json!({})
            }
            async fn invoke(&self, _: Value) -> ToolResult<Value> {
                Ok(json!({}))
            }
        }
        reg.register(Arc::new(AlphaTool));
        let names: Vec<String> = reg.list().into_iter().map(|t| t.name().into()).collect();
        assert_eq!(names, vec!["aaa", "echo"]);
    }

    #[test]
    fn catalog_is_valid_json_array() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(EchoTool));
        let cat = reg.catalog();
        let arr = cat.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "echo");
    }
}
