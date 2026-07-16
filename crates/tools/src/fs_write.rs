//! `WriteFile` tool — atomic file writes (tmp + rename).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(test)]
use crate::error::ToolError;
use crate::error::ToolResult;
use crate::tool::Tool;

/// Built-in tool that writes UTF-8 content to disk atomically.
///
/// Atomicity strategy: write content to `<path>.mscode-tmp-<pid>`, `fsync` is
/// omitted for portability, then rename to the final path. On Windows and
/// Unix alike, the rename target is replaced atomically when the OS supports
/// it; if it does not, the rename still fails cleanly without corrupting the
/// original file.
pub struct WriteFileTool {
    sandbox: Arc<mscode_sandbox::Sandbox>,
}

#[derive(Debug, Deserialize)]
struct WriteInput {
    path: String,
    content: String,
    #[serde(default)]
    create_dirs: Option<bool>,
}

impl WriteFileTool {
    /// Construct a new WriteFileTool bound to the given sandbox.
    pub fn new(sandbox: Arc<mscode_sandbox::Sandbox>) -> Self {
        Self { sandbox }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write text content to a file atomically (tmp + rename). Subject to sandbox policy."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute or workspace-relative path." },
                "content": { "type": "string", "description": "UTF-8 content to write." },
                "create_dirs": { "type": "boolean", "description": "Create parent directories if missing (default false)." }
            },
            "required": ["path", "content"]
        })
    }

    async fn invoke(&self, input: Value) -> ToolResult<Value> {
        let parsed: WriteInput = serde_json::from_value(input)?;
        let path = PathBuf::from(&parsed.path);
        self.sandbox.validate_write(&path)?;

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && parsed.create_dirs.unwrap_or(false) {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let tmp_path = atomic_tmp_path(&path);
        tokio::fs::write(&tmp_path, parsed.content.as_bytes()).await?;
        // Atomic replace on Unix; best-effort replace on Windows (>= NTFS).
        if let Err(e) = tokio::fs::rename(&tmp_path, &path).await {
            // Clean up the temp file; surface the original error.
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e.into());
        }

        Ok(json!({
            "path": parsed.path,
            "bytes": parsed.content.len(),
        }))
    }
}

/// Construct a sibling tmp path for atomic writes.
fn atomic_tmp_path(target: &std::path::Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    name.push_str(".mscode-tmp");
    let mut buf = target.to_path_buf();
    buf.set_file_name(name);
    buf
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
    async fn writes_new_file_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.txt");
        let tool = WriteFileTool::new(make_sandbox(dir.path()));
        let out = tool
            .invoke(json!({"path": p.to_string_lossy(), "content": "hello"}))
            .await
            .unwrap();
        assert_eq!(out["bytes"], 5);
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello");
        // Tmp file should be gone.
        assert!(!atomic_tmp_path(&p).exists());
    }

    #[tokio::test]
    async fn overwrites_existing_file_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("out.txt");
        fs::write(&p, "old").unwrap();
        let tool = WriteFileTool::new(make_sandbox(dir.path()));
        tool.invoke(json!({"path": p.to_string_lossy(), "content": "new"}))
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
    }

    #[tokio::test]
    async fn create_dirs_flag_makes_parents() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/c.txt");
        let tool = WriteFileTool::new(make_sandbox(dir.path()));
        tool.invoke(json!({
            "path": p.to_string_lossy(),
            "content": "x",
            "create_dirs": true
        }))
        .await
        .unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "x");
    }

    #[tokio::test]
    async fn rejects_dotdot_path() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("../escape.txt");
        let tool = WriteFileTool::new(make_sandbox(dir.path()));
        let err = tool
            .invoke(json!({"path": p.to_string_lossy(), "content": ""}))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ToolError::Sandbox(mscode_sandbox::SandboxError::DotDotEscape(_))
        ));
    }

    #[tokio::test]
    async fn rejects_path_outside_workspace() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let tool = WriteFileTool::new(make_sandbox(inside.path()));
        let p = outside.path().join("out.txt");
        let err = tool
            .invoke(json!({"path": p.to_string_lossy(), "content": "x"}))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ToolError::Sandbox(mscode_sandbox::SandboxError::OutsideWorkspace(_))
        ));
    }
}
