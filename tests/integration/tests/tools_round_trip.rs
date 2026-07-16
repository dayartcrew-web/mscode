//! Test 4: Tools round-trip — read/write/list/grep inside a sandbox.
//!
//! Registers the four file-system tools against one [`Sandbox`] rooted at a
//! tempdir, then exercises each tool via [`ToolRegistry::invoke_by_name`]:
//!   1. write_file  -> creates `sub/foo.txt`
//!   2. list_dir    -> lists `sub/`
//!   3. read_file   -> reads `sub/foo.txt` back
//!   4. grep        -> finds the written content
//!
//! Asserts each step returns Ok and the expected payload.

use mscode_sandbox::Sandbox;
use mscode_tools::{GrepTool, ListDirTool, ReadFileTool, ToolRegistry, WriteFileTool};
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn tools_round_trip_read_write_grep() {
    let dir = tempdir().expect("tempdir");
    let workspace_root = dir.path().to_path_buf();
    let sandbox = Arc::new(Sandbox::new(&workspace_root));

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(WriteFileTool::new(sandbox.clone())));
    registry.register(Arc::new(ListDirTool::new(sandbox.clone())));
    registry.register(Arc::new(ReadFileTool::new(sandbox.clone())));
    registry.register(Arc::new(GrepTool::new(sandbox.clone())));

    assert_eq!(registry.len(), 4, "expected 4 tools registered");

    // Sandbox requires absolute paths so the path is recognized as inside
    // the workspace. We join relative subpaths to the workspace root.
    let file_path = workspace_root.join("sub").join("foo.txt");
    let file_str = file_path.to_string_lossy().into_owned();
    let sub_str = workspace_root.join("sub").to_string_lossy().into_owned();
    let root_str = workspace_root.to_string_lossy().into_owned();

    // 1. write_file (create_dirs so the parent exists)
    let write_out = registry
        .invoke_by_name(
            "write_file",
            json!({"path": file_str, "content": "hello world\nsecond line\n", "create_dirs": true}),
        )
        .await
        .expect("write_file must succeed");
    let write_str = write_out.to_string();
    assert!(
        write_str.contains("foo.txt"),
        "write_file should echo the path, got: {write_str}"
    );

    // 2. list_dir
    let list_out = registry
        .invoke_by_name("list_dir", json!({"path": sub_str}))
        .await
        .expect("list_dir must succeed");
    let list_str = list_out.to_string();
    assert!(
        list_str.contains("foo.txt"),
        "list_dir output should mention foo.txt, got: {list_str}"
    );

    // 3. read_file
    let read_out = registry
        .invoke_by_name("read_file", json!({"path": file_str}))
        .await
        .expect("read_file must succeed");
    let read_str = read_out.to_string();
    assert!(
        read_str.contains("hello world"),
        "read_file output should contain written content, got: {read_str}"
    );

    // 4. grep — must find the line we wrote.
    let grep_out = registry
        .invoke_by_name("grep", json!({"pattern": "hello world", "path": root_str}))
        .await
        .expect("grep must succeed");
    let grep_str = grep_out.to_string();
    assert!(
        grep_str.contains("foo.txt"),
        "grep output should reference the file, got: {grep_str}"
    );
    assert!(
        grep_str.contains("hello world"),
        "grep output should contain matched line, got: {grep_str}"
    );

    // 5. Catalog exposes all four tools to the model.
    let catalog = registry.catalog();
    let names: Vec<&str> = catalog
        .as_array()
        .expect("catalog is array")
        .iter()
        .map(|e| e["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["grep", "list_dir", "read_file", "write_file"]);
}
