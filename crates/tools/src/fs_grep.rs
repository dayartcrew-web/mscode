//! `Grep` tool — regex search across files in a directory tree.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(test)]
use crate::error::ToolError;
use crate::error::ToolResult;
use crate::tool::Tool;

/// Maximum number of match records returned by a single `Grep` invocation.
pub const DEFAULT_MAX_MATCHES: usize = 200;

/// Built-in tool that runs a regex against every regular file under a root
/// directory, returning up to `max_matches` matches.
pub struct GrepTool {
    sandbox: Arc<mscode_sandbox::Sandbox>,
    max_matches: usize,
}

#[derive(Debug, Deserialize)]
struct GrepInput {
    pattern: String,
    path: String,
    #[serde(default)]
    max_matches: Option<usize>,
}

impl GrepTool {
    /// Construct a new GrepTool bound to the given sandbox.
    pub fn new(sandbox: Arc<mscode_sandbox::Sandbox>) -> Self {
        Self {
            sandbox,
            max_matches: DEFAULT_MAX_MATCHES,
        }
    }

    /// Override the default max-matches cap.
    pub fn with_max_matches(mut self, max_matches: usize) -> Self {
        self.max_matches = max_matches;
        self
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Recursively search files under a directory for a regex pattern."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Rust regex syntax." },
                "path": { "type": "string", "description": "File or directory to search." },
                "max_matches": { "type": "integer", "description": "Optional cap on total matches returned." }
            },
            "required": ["pattern", "path"]
        })
    }

    async fn invoke(&self, input: Value) -> ToolResult<Value> {
        let parsed: GrepInput = serde_json::from_value(input)?;
        let re = Regex::new(&parsed.pattern)?;
        let root = PathBuf::from(&parsed.path);
        self.sandbox.validate_read(&root)?;
        let cap = parsed.max_matches.unwrap_or(self.max_matches);

        let mut matches: Vec<Value> = Vec::new();
        walk(&root, &re, cap, &mut matches).await?;

        Ok(json!({
            "path": parsed.path,
            "pattern": parsed.pattern,
            "matches": matches,
            "truncated": matches.len() >= cap,
        }))
    }
}

async fn walk(path: &Path, re: &Regex, cap: usize, out: &mut Vec<Value>) -> ToolResult<()> {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    if meta.is_file() {
        scan_file(path, re, cap, out).await?;
        return Ok(());
    }
    if !meta.is_dir() {
        return Ok(());
    }
    let mut dir = match tokio::fs::read_dir(path).await {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };
    while let Some(entry) = dir.next_entry().await? {
        if out.len() >= cap {
            return Ok(());
        }
        let child = entry.path();
        // Skip hidden directories (.git, .svn, etc.).
        if entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with('.'))
            .unwrap_or(false)
        {
            // Still descend into hidden files at the top level? No — skip dot dirs only.
            if tokio::fs::metadata(&child)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
        }
        Box::pin(walk(&child, re, cap, out)).await?;
    }
    Ok(())
}

async fn scan_file(path: &Path, re: &Regex, cap: usize, out: &mut Vec<Value>) -> ToolResult<()> {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(());
    };
    for (lineno, line) in text.lines().enumerate() {
        if out.len() >= cap {
            return Ok(());
        }
        if re.is_match(line) {
            out.push(json!({
                "path": path.to_string_lossy(),
                "line": lineno + 1,
                "text": line,
            }));
        }
    }
    Ok(())
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
    async fn finds_matches_across_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "foo\nbar\nfoo\n").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b.rs"), "fn foo() {}\n").unwrap();
        let tool = GrepTool::new(make_sandbox(dir.path()));
        let out = tool
            .invoke(json!({"pattern": "foo", "path": dir.path().to_string_lossy()}))
            .await
            .unwrap();
        let matches = out["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 3);
    }

    #[tokio::test]
    async fn respects_max_matches() {
        let dir = tempfile::tempdir().unwrap();
        let mut content = String::new();
        for _ in 0..50 {
            content.push_str("foo\n");
        }
        fs::write(dir.path().join("a.txt"), content).unwrap();
        let tool = GrepTool::new(make_sandbox(dir.path())).with_max_matches(5);
        let out = tool
            .invoke(json!({"pattern": "foo", "path": dir.path().to_string_lossy()}))
            .await
            .unwrap();
        assert_eq!(out["matches"].as_array().unwrap().len(), 5);
        assert_eq!(out["truncated"], true);
    }

    #[tokio::test]
    async fn rejects_invalid_regex() {
        let dir = tempfile::tempdir().unwrap();
        let tool = GrepTool::new(make_sandbox(dir.path()));
        let err = tool
            .invoke(json!({"pattern": "*invalid", "path": dir.path().to_string_lossy()}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Regex(_)));
    }

    #[tokio::test]
    async fn skips_dot_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("visible.txt"), "match me").unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/hidden"), "match me").unwrap();
        let tool = GrepTool::new(make_sandbox(dir.path()));
        let out = tool
            .invoke(json!({"pattern": "match", "path": dir.path().to_string_lossy()}))
            .await
            .unwrap();
        assert_eq!(out["matches"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn skips_non_utf8_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "match").unwrap();
        fs::write(dir.path().join("bin.dat"), [0xffu8, 0xfe, b'm', b'a']).unwrap();
        let tool = GrepTool::new(make_sandbox(dir.path()));
        let out = tool
            .invoke(json!({"pattern": "match", "path": dir.path().to_string_lossy()}))
            .await
            .unwrap();
        assert_eq!(out["matches"].as_array().unwrap().len(), 1);
    }
}
