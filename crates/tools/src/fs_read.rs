//! `ReadFile` tool — read file contents subject to sandbox + size cap.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};
use crate::tool::Tool;

/// Default maximum file size returned by `ReadFile` (1 MiB).
pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;

/// Built-in tool that reads a UTF-8 file from disk and returns its contents.
pub struct ReadFileTool {
    sandbox: Arc<mscode_sandbox::Sandbox>,
    max_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct ReadInput {
    path: String,
    #[serde(default)]
    max_bytes: Option<u64>,
}

impl ReadFileTool {
    /// Construct a new ReadFileTool bound to the given sandbox.
    pub fn new(sandbox: Arc<mscode_sandbox::Sandbox>) -> Self {
        Self {
            sandbox,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Override the default max-bytes cap.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a UTF-8 text file from disk. Subject to sandbox policy."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute or workspace-relative path." },
                "max_bytes": { "type": "integer", "description": "Optional cap on bytes returned." }
            },
            "required": ["path"]
        })
    }

    async fn invoke(&self, input: Value) -> ToolResult<Value> {
        let parsed: ReadInput = serde_json::from_value(input)?;
        let path = PathBuf::from(&parsed.path);
        self.sandbox.validate_read(&path)?;

        let meta = tokio::fs::metadata(&path).await?;
        if !meta.is_file() {
            return Err(ToolError::InvalidInput(format!(
                "path is not a regular file: {}",
                path.display()
            )));
        }
        let cap = parsed.max_bytes.unwrap_or(self.max_bytes);
        let size = meta.len();
        if size > cap {
            return Err(ToolError::InvalidInput(format!(
                "file size {size} exceeds cap {cap}"
            )));
        }

        let bytes = tokio::fs::read(&path).await?;
        let content = String::from_utf8(bytes)
            .map_err(|e| ToolError::InvalidInput(format!("file is not valid UTF-8: {e}")))?;
        Ok(json!({
            "path": parsed.path,
            "bytes": content.len(),
            "content": content,
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
    async fn reads_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hello.txt");
        fs::write(&p, "hi there").unwrap();
        let tool = ReadFileTool::new(make_sandbox(dir.path()));
        let out = tool
            .invoke(json!({"path": p.to_string_lossy()}))
            .await
            .unwrap();
        assert_eq!(out["content"], "hi there");
        assert_eq!(out["bytes"], 8);
    }

    #[tokio::test]
    async fn rejects_file_outside_workspace() {
        // Pick a workspace that is provably not under the system temp dir.
        // We use the parent of the OS temp dir as the "outside" file location
        // and a child of that same parent as the "workspace", so the system
        // temp allowance does not cover the outside file.
        let temp_root = std::env::temp_dir();
        let outside_root = temp_root.parent().unwrap_or_else(|| Path::new("/"));
        let outside = outside_root.join("mscode_read_outside_probe.txt");
        // Skip if not writable on this platform — leave a no-op pass.
        if fs::write(&outside, "x").is_err() {
            return;
        }
        // Workspace is also outside temp but distinct from the file.
        let ws = outside_root.join("mscode_read_outside_ws");
        let _ = fs::create_dir_all(&ws);
        let sandbox = Arc::new(mscode_sandbox::Sandbox::new(&ws));
        let tool = ReadFileTool::new(sandbox);
        let result = tool
            .invoke(json!({"path": outside.to_string_lossy()}))
            .await;
        // Cleanup regardless of outcome.
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            ToolError::Sandbox(mscode_sandbox::SandboxError::OutsideWorkspace(_))
        ));
    }

    #[tokio::test]
    async fn rejects_dotdot_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ReadFileTool::new(make_sandbox(dir.path()));
        let p = dir.path().join("../escape.txt");
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
    async fn rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.txt");
        fs::write(&p, "aaaa").unwrap();
        let tool = ReadFileTool::new(make_sandbox(dir.path())).with_max_bytes(2);
        let err = tool
            .invoke(json!({"path": p.to_string_lossy()}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn rejects_non_utf8_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bin.dat");
        fs::write(&p, [0xffu8, 0xfe, 0xfd]).unwrap();
        let tool = ReadFileTool::new(make_sandbox(dir.path()));
        let err = tool
            .invoke(json!({"path": p.to_string_lossy()}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
