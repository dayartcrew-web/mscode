//! MCP transports — a trait plus a stdio implementation.
//!
//! Only stdio is implemented in v1.1; the trait abstraction exists so HTTP+SSE
//! or WebSocket transports can plug in without rewriting the client.

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::error::{McpError, McpResult};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Async, framed transport abstraction for the MCP wire protocol.
///
/// Each call to [`send`](Self::send) emits one JSON-RPC message terminated by
/// a newline. Each call to [`recv`](Self::recv) reads one message.
#[async_trait]
pub trait McpTransport: Send {
    /// Send a request to the server.
    async fn send(&mut self, msg: &JsonRpcRequest) -> McpResult<()>;

    /// Receive the next response from the server.
    async fn recv(&mut self) -> McpResult<JsonRpcResponse>;

    /// Close the transport, releasing any child process resources.
    async fn close(&mut self) -> McpResult<()>;
}

/// Stdio transport: spawns a child process and exchanges newline-delimited
/// JSON-RPC messages over its stdin/stdout pipes.
pub struct StdioTransport {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl StdioTransport {
    /// Spawn the child process and take ownership of its stdin/stdout pipes.
    pub async fn spawn(command: &str, args: &[&str]) -> McpResult<Self> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Spawn(format!("spawn `{command}` failed: {e}")))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let (stdin, stdout) = match (stdin, stdout) {
            (Some(i), Some(o)) => (i, o),
            _ => {
                return Err(McpError::Spawn(
                    "child did not expose stdin/stdout pipes".into(),
                ));
            }
        };
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&mut self, msg: &JsonRpcRequest) -> McpResult<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| McpError::TransportClosed)?;
        let line = serde_json::to_string(msg)?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> McpResult<JsonRpcResponse> {
        let mut buf = String::new();
        let n = self.stdout.read_line(&mut buf).await?;
        if n == 0 {
            return Err(McpError::TransportClosed);
        }
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            return Err(McpError::Malformed("empty line".into()));
        }
        let resp: JsonRpcResponse = serde_json::from_str(trimmed)?;
        Ok(resp)
    }

    async fn close(&mut self) -> McpResult<()> {
        // Drop stdin first to signal EOF.
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.shutdown().await;
        }
        if let Some(mut child) = self.child.take() {
            // Try to wait politely; kill on the next tick if needed.
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), child.wait()).await;
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Test support — exposed via `pub(crate)` so other modules can use the mock
// without leaking it through the crate's public API.
// ---------------------------------------------------------------------------

/// A mock transport that runs entirely in memory — used to test the client
/// without spawning real processes.
#[cfg(test)]
pub(crate) struct MockTransport {
    pub sent: Vec<JsonRpcRequest>,
    pub recv_queue: Vec<JsonRpcResponse>,
    pub closed: bool,
}

#[cfg(test)]
impl MockTransport {
    /// Construct a new mock with a pre-loaded response queue.
    pub fn with_responses(responses: Vec<JsonRpcResponse>) -> Self {
        Self {
            sent: Vec::new(),
            recv_queue: responses,
            closed: false,
        }
    }
}

#[cfg(test)]
#[async_trait]
impl McpTransport for MockTransport {
    async fn send(&mut self, msg: &JsonRpcRequest) -> McpResult<()> {
        self.sent.push(msg.clone());
        Ok(())
    }

    async fn recv(&mut self) -> McpResult<JsonRpcResponse> {
        self.recv_queue.pop().ok_or(McpError::TransportClosed)
    }

    async fn close(&mut self) -> McpResult<()> {
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn mock_transport_round_trips() {
        let mut t = MockTransport::with_responses(vec![JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: Some(json!({"ok": true})),
            error: None,
        }]);
        let req = JsonRpcRequest::new(1, "ping", None);
        t.send(&req).await.unwrap();
        let resp = t.recv().await.unwrap();
        assert_eq!(resp.id, 1);
        assert_eq!(resp.result.unwrap()["ok"], true);
        t.close().await.unwrap();
        assert!(t.closed);
        assert_eq!(t.sent.len(), 1);
    }
}
