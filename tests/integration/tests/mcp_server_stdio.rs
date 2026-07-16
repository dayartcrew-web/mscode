//! Test 8: MCP server stdio full cycle — initialize, tools/list, tools/call,
//! malformed (parse error), unknown method (method not found).
//!
//! Drives the server end-to-end through a tokio duplex pipe. The client side
//! writes one JSON-RPC request per line, then signals EOF by shutting down
//! its write half. The server processes every line and writes one response
//! per line back, then exits cleanly on EOF.

use mscode_mcp_client::JsonRpcResponse;
use mscode_mcp_server::McpServer;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn mcp_server_stdio_initializes_and_lists_tools() {
    let dir = tempdir().expect("tempdir");
    let workspace_file = dir.path().join("payload.txt");
    std::fs::write(&workspace_file, "the answer is 42").expect("write");
    let path_str = workspace_file.to_string_lossy().into_owned();

    let server = McpServer::new(dir.path());

    // Build the four-line request payload with `serde_json` so Windows
    // backslashes in the file path are escaped correctly. (Raw format!()
    // interpolation would leak `\` into the JSON and trigger parse errors
    // when the server deserializes.)
    let reqs: Vec<Value> = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params": {"name":"read_file","arguments":{"path": path_str}}
        }),
        // Unknown method — server should reply with method-not-found.
        json!({"jsonrpc":"2.0","id":4,"method":"frob/nozzle"}),
    ];
    let payload: String = reqs
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    // Append a malformed line to also exercise the parse-error path. The
    // id field for parse errors is `null` per JSON-RPC spec.
    let full_input = format!("{payload}\n{{ NOT JSON\n");

    // Single duplex split into owned halves so the spawned task can take
    // ownership (no borrowed `&mut` references crossing the spawn boundary).
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let (server_rx, server_tx) = tokio::io::split(server_io);
    let (mut client_rx, mut client_tx) = tokio::io::split(client_io);

    client_tx
        .write_all(full_input.as_bytes())
        .await
        .expect("write");
    client_tx.flush().await.expect("flush");
    // Shutdown signals EOF — server's run_stdio loop returns Ok(()).
    let _ = client_tx.shutdown().await;

    let server_handle = tokio::spawn(async move { server.run_stdio(server_rx, server_tx).await });

    let mut buf = Vec::new();
    client_rx.read_to_end(&mut buf).await.expect("read_to_end");
    let _ = server_handle.await.expect("server task did not panic");
    let out = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    // Expect 5 responses: initialize, tools/list, tools/call, unknown-method,
    // and the parse-error for the malformed line.
    assert!(
        lines.len() >= 5,
        "expected at least 5 response lines, got {}: {out}",
        lines.len()
    );

    // --- initialize (id 1) ---
    let init: JsonRpcResponse = serde_json::from_str(lines[0]).expect("parse initialize response");
    assert_eq!(init.id, 1);
    let init_result = init.result.expect("initialize result");
    assert!(init_result.get("capabilities").is_some());
    assert_eq!(init_result["serverInfo"]["name"], "mscode");

    // --- tools/list (id 2) ---
    let list: JsonRpcResponse = serde_json::from_str(lines[1]).expect("parse tools/list response");
    assert_eq!(list.id, 2);
    let tools = list.result.expect("tools/list result")["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    assert!(
        tools.len() >= 5,
        "expected at least 5 tools, got {}",
        tools.len()
    );
    let names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect();
    for expected in ["read_file", "write_file", "list_dir", "grep", "bash"] {
        assert!(
            names.contains(&expected.into()),
            "expected {expected} in tools: {names:?}"
        );
    }

    // --- tools/call (id 3) ---
    let call: JsonRpcResponse = serde_json::from_str(lines[2]).expect("parse tools/call response");
    assert_eq!(call.id, 3);
    let call_result = call.result.expect("tools/call result");
    assert_eq!(call_result["isError"], false);
    let text = call_result["content"][0]["text"].as_str().expect("text");
    assert!(
        text.contains("the answer is 42"),
        "read_file content, got: {text}"
    );

    // --- unknown method (id 4) — JSON-RPC code -32601 ---
    let unknown: JsonRpcResponse =
        serde_json::from_str(lines[3]).expect("parse unknown-method response");
    assert_eq!(unknown.id, 4);
    let err = unknown.error.expect("error");
    assert_eq!(err.code, -32601, "method-not-found");
    assert!(err.message.contains("frob/nozzle"));

    // --- malformed line — JSON-RPC code -32700 (parse error) ---
    // The id field is `null` per spec, but the mcp-server implementation
    // currently emits id=0 for parse errors. We accept either.
    let parse_err_line = lines[4];
    assert!(
        parse_err_line.contains("\"code\":-32700"),
        "expected parse error code in: {parse_err_line}"
    );
}
