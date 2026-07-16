//! Unit tests for the MCP server crate.
//!
//! These tests exercise the JSON-RPC dispatch table directly (initialize,
//! tools/list, tools/call, parse error, method not found) without spinning
//! up a stdio loop. Integration tests in `tests/integration/` cover the full
//! duplex stdio path.

use super::server::*;
use mscode_mcp_client::protocol::JSONRPC_VERSION;
use mscode_mcp_client::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn make_server() -> McpServer {
    let dir = tempfile::tempdir().expect("tempdir");
    McpServer::new(dir.path())
}

async fn dispatch(server: &McpServer, request: &Value) -> JsonRpcResponse {
    let parsed: JsonRpcRequest = serde_json::from_value(request.clone()).expect("parse request");
    let result = server.dispatch(&parsed).await;
    build_response(parsed.id, result)
}

#[tokio::test]
async fn initialize_returns_capabilities_with_tools() {
    let server = make_server();
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
    });
    let resp = dispatch(&server, &req).await;
    assert_eq!(resp.id, 1);
    let result = resp.result.expect("result");
    assert!(result.get("capabilities").is_some());
    assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(result["serverInfo"]["name"], "mscode");
}

#[tokio::test]
async fn tools_list_returns_all_five_built_in_tools() {
    let server = make_server();
    let req = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
    let resp = dispatch(&server, &req).await;
    let tools = resp.result.expect("result")["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    let names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_string())
        .collect();
    assert!(names.contains(&"read_file".into()));
    assert!(names.contains(&"write_file".into()));
    assert!(names.contains(&"list_dir".into()));
    assert!(names.contains(&"grep".into()));
    assert!(names.contains(&"bash".into()));
    assert_eq!(names.len(), 5);
}

#[tokio::test]
async fn tools_call_reads_file_via_registry() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "hello world").expect("write");
    let server = McpServer::new(dir.path());
    let path = dir.path().join("a.txt").to_string_lossy().into_owned();
    let req = json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params": {"name": "read_file", "arguments": {"path": path}}
    });
    let resp = dispatch(&server, &req).await;
    let result = resp.result.expect("result");
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("hello world"));
}

#[tokio::test]
async fn unknown_method_returns_method_not_found_error() {
    let server = make_server();
    let req = json!({
        "jsonrpc":"2.0","id":4,"method":"some/unknown/method"
    });
    let resp = dispatch(&server, &req).await;
    let err = resp.error.expect("error");
    assert_eq!(err.code, METHOD_NOT_FOUND);
    assert!(err.message.contains("some/unknown/method"));
}

#[tokio::test]
async fn malformed_json_returns_parse_error_via_handle_line() {
    let server = make_server();
    let resp = server
        .handle_line("{ not json")
        .await
        .expect("parse error response");
    let err = resp.error.expect("error");
    assert_eq!(err.code, PARSE_ERROR);
}

#[tokio::test]
async fn tools_call_unknown_tool_returns_invalid_params_error() {
    let server = make_server();
    let req = json!({
        "jsonrpc":"2.0","id":5,"method":"tools/call",
        "params": {"name":"does_not_exist","arguments":{}}
    });
    let resp = dispatch(&server, &req).await;
    let err = resp.error.expect("error");
    assert_eq!(err.code, INVALID_PARAMS);
    assert!(err.message.contains("does_not_exist"));
}

#[tokio::test]
async fn tools_call_with_missing_params_returns_invalid_params() {
    let server = make_server();
    let req = json!({
        "jsonrpc":"2.0","id":6,"method":"tools/call","params":{}
    });
    let resp = dispatch(&server, &req).await;
    let err = resp.error.expect("error");
    assert_eq!(err.code, INVALID_PARAMS);
}

#[tokio::test]
async fn run_stdio_handles_full_request_response_cycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("hello.txt"), "from disk").expect("write");
    let server = McpServer::new(dir.path());
    let path = dir.path().join("hello.txt").to_string_lossy().into_owned();

    // A single duplex where the client owns one end and the server owns the
    // other. The server's end is split into read+write halves via
    // tokio::io::split so the borrow checker is satisfied.
    let (client_io, server_io) = tokio::io::duplex(8 * 1024);
    let (server_rx, server_tx) = tokio::io::split(server_io);
    let (mut client_rx, mut client_tx) = tokio::io::split(client_io);

    // Two requests on separate lines; expect two responses back. Build them
    // with `serde_json` so the Windows-style file path is JSON-escaped
    // correctly (a raw `format!` interpolation leaves `\` characters intact,
    // which the JSON parser then rejects as invalid escape sequences).
    let req1 = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize"
    });
    let req2 = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "read_file", "arguments": {"path": path}}
    });
    let init = format!(
        "{}\n{}\n",
        serde_json::to_string(&req1).unwrap(),
        serde_json::to_string(&req2).unwrap(),
    );

    client_tx
        .write_all(init.as_bytes())
        .await
        .expect("write reqs");
    client_tx.flush().await.expect("flush");
    // Shutdown the client write side so the server reads EOF and exits its
    // loop, dropping server_tx and thus closing the client read side.
    let _ = client_tx.shutdown().await;

    let server_handle = tokio::spawn(async move { server.run_stdio(server_rx, server_tx).await });

    let mut buf = Vec::new();
    client_rx.read_to_end(&mut buf).await.expect("read_to_end");
    let _ = server_handle.await;
    let out = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.len() >= 2, "expected 2+ response lines, got: {out}");
    let first: JsonRpcResponse = serde_json::from_str(lines[0]).expect("parse first response");
    assert_eq!(first.id, 1);
    assert!(first.result.is_some());
    let second: JsonRpcResponse = serde_json::from_str(lines[1]).expect("parse second response");
    assert_eq!(second.id, 2);
    let second_result = second.result.expect("result");
    let second_text = second_result["content"][0]["text"].as_str().expect("text");
    assert!(second_text.contains("from disk"));
}

#[tokio::test]
async fn run_stdio_emits_parse_error_for_malformed_line() {
    let server = make_server();
    let payload = "garbage-not-json\n";

    // Use a single duplex and split it into owned halves so the spawned
    // task can take ownership (no borrowed `&mut` references crossing the
    // spawn boundary — those would either fail to compile or hang waiting
    // for references that never become inactive).
    let (client_io, server_io) = tokio::io::duplex(8 * 1024);
    let (server_rx, server_tx) = tokio::io::split(server_io);
    let (mut client_rx, mut client_tx) = tokio::io::split(client_io);

    client_tx
        .write_all(payload.as_bytes())
        .await
        .expect("write");
    client_tx.flush().await.expect("flush");
    // Shutdown signals EOF to the server's read half, exiting the loop.
    let _ = client_tx.shutdown().await;

    let server_handle = tokio::spawn(async move { server.run_stdio(server_rx, server_tx).await });

    let mut buf = Vec::new();
    client_rx.read_to_end(&mut buf).await.expect("read_to_end");
    let _ = server_handle.await;
    let out = String::from_utf8_lossy(&buf);
    assert!(
        out.contains("\"code\":-32700"),
        "expected parse error in: {out}"
    );
}

#[tokio::test]
async fn run_stdio_emits_method_not_found_for_unknown_method() {
    let server = make_server();
    let payload = "{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"frob/nozzle\"}\n";

    let (client_io, server_io) = tokio::io::duplex(8 * 1024);
    let (server_rx, server_tx) = tokio::io::split(server_io);
    let (mut client_rx, mut client_tx) = tokio::io::split(client_io);

    client_tx
        .write_all(payload.as_bytes())
        .await
        .expect("write");
    client_tx.flush().await.expect("flush");
    let _ = client_tx.shutdown().await;

    let server_handle = tokio::spawn(async move { server.run_stdio(server_rx, server_tx).await });

    let mut buf = Vec::new();
    client_rx.read_to_end(&mut buf).await.expect("read_to_end");
    let _ = server_handle.await;
    let out = String::from_utf8_lossy(&buf);
    assert!(
        out.contains("\"code\":-32601"),
        "expected method-not-found in: {out}"
    );
}

#[test]
fn json_rpc_error_code_constants_match_spec() {
    assert_eq!(PARSE_ERROR, -32700);
    assert_eq!(METHOD_NOT_FOUND, -32601);
    assert_eq!(INVALID_PARAMS, -32602);
}

#[test]
fn build_response_success_carries_result_payload() {
    let resp = build_response(99, Ok(json!({"ok": true})));
    assert_eq!(resp.id, 99);
    assert_eq!(resp.jsonrpc, JSONRPC_VERSION);
    assert_eq!(resp.result.unwrap()["ok"], true);
    assert!(resp.error.is_none());
}

#[test]
fn build_response_error_carries_error_payload() {
    let resp = build_response(
        100,
        Err(JsonRpcError {
            code: -32099,
            message: "boom".into(),
            data: None,
        }),
    );
    assert!(resp.result.is_none());
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32099);
    assert_eq!(err.message, "boom");
}
