//! `ListDir` tool — list directory entries.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};
use crate::tool::Tool;

/// Built-in tool that lists the entries of a directory.
pub struct ListDirTool {
    sandbox: Arc<mscode_sandbox::Sandbox>,
}

#[derive(Debug, Deserialize)]
struct ListInput {
    path: String,
}

impl ListDirTool {
    /// Construct a new ListDirTool bound to the given sandbox.
    pub fn new(sandbox: Arc<mscode_sandbox::Sandbox>) -> Self {
        Self { sandbox }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List entries in a directory. Each entry has a name and an is_dir flag."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to list." }
            },
            "required": ["path"]
        })
    }

    async fn invoke(&self, input: Value) -> ToolResult<Value> {
        let parsed: ListInput = serde_json::from_value(input)?;
        let path = PathBuf::from(&parsed.path);
        self.sandbox.validate_read(&path)?;

        let meta = tokio::fs::metadata(&path).await?;
        if !meta.is_dir() {
            return Err(ToolError::InvalidInput(format!(
                "path is not a directory: {}",
                path.display()
            )));
        }

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            entries.push(json!({"name": name, "is_dir": is_dir}));
        }
        entries.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        Ok(json!({
            "path": parsed.path,
            "entries": entries,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn make_sandbox(root: &Path) -> Arc<mscode_sandbox::Sandbox> {
        Arc::new(mscode_sandbox::Sandbox::new(root))
    }

    #[tokio::test]
    async fn lists_entries_with_is_dir_flag() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "x").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        let tool = ListDirTool::new(make_sandbox(dir.path()));
        let out = tool
            .invoke(json!({"path": dir.path().to_string_lossy()}))
            .await
            .unwrap();
        let entries = out["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let by_name: std::collections::HashMap<&str, bool> = entries
            .iter()
            .map(|e| (e["name"].as_str().unwrap(), e["is_dir"].as_bool().unwrap()))
            .collect();
        assert!(!by_name["a.txt"]);
        assert!(by_name["subdir"]);
    }

    #[tokio::test]
    async fn rejects_non_directory() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("file.txt");
        fs::write(&p, "x").unwrap();
        let tool = ListDirTool::new(make_sandbox(dir.path()));
        let err = tool
            .invoke(json!({"path": p.to_string_lossy()}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn rejects_dotdot() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ListDirTool::new(make_sandbox(dir.path()));
        let p = dir.path().join("..");
        let err = tool
            .invoke(json!({"path": p.to_string_lossy()}))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ToolError::Sandbox(mscode_sandbox::SandboxError::DotDotEscape(_))
        ));
    }

    #[tokio::test]
    async fn empty_directory_returns_empty_array() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ListDirTool::new(make_sandbox(dir.path()));
        let out = tool
            .invoke(json!({"path": dir.path().to_string_lossy()}))
            .await
            .unwrap();
        assert_eq!(out["entries"].as_array().unwrap().len(), 0);
    }
}
