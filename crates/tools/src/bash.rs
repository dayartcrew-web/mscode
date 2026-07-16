//! `Bash` tool — shell out with sandbox + timeout + env scrubbing.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{ToolError, ToolResult};
use crate::tool::Tool;

/// Default wall-clock cap for a single command.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Environment variables that are stripped from child processes by default.
///
/// These are commonly-leaked credentials/secrets. The list is conservative —
/// extend it explicitly when new vectors are discovered.
pub const SCRUBBED_ENV_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "AZURE_API_KEY",
    "GOOGLE_API_KEY",
    "HUGGINGFACE_TOKEN",
    "HF_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GITLAB_TOKEN",
    "CI_TOKEN",
    "DATABASE_URL",
    "PGPASSWORD",
    "MSCODE_API_KEY",
];

/// Built-in tool that runs a shell command via the platform shell.
///
/// - On Windows: `cmd /C <command>`.
/// - On Unix: `sh -c <command>`.
///
/// The command is first validated against the [`mscode_sandbox::Sandbox`]
/// exec allowlist, then run with credential-bearing environment variables
/// scrubbed and a strict timeout.
pub struct BashTool {
    sandbox: Arc<mscode_sandbox::Sandbox>,
    timeout: Duration,
    extra_scrubbed: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

impl BashTool {
    /// Construct a new BashTool bound to the given sandbox.
    pub fn new(sandbox: Arc<mscode_sandbox::Sandbox>) -> Self {
        Self {
            sandbox,
            timeout: DEFAULT_TIMEOUT,
            extra_scrubbed: Vec::new(),
        }
    }

    /// Override the default timeout.
    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Add additional environment variable names to scrub.
    pub fn with_extra_scrubbed<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extra_scrubbed
            .extend(names.into_iter().map(Into::into));
        self
    }

    fn build_command(&self, raw: &str, cwd: Option<&str>) -> Command {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(raw);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(raw);
            c
        };
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Inherit the parent env, then explicitly remove scrubbed vars.
        cmd.env_clear();
        // Re-introduce a minimal set required for typical tools to function.
        if cfg!(windows) {
            for k in ["SystemRoot", "TEMP", "TMP", "PATH", "PATHEXT", "COMSPEC"] {
                if let Ok(v) = std::env::var(k) {
                    cmd.env(k, v);
                }
            }
        } else {
            for k in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"] {
                if let Ok(v) = std::env::var(k) {
                    cmd.env(k, v);
                }
            }
        }
        for k in SCRUBBED_ENV_VARS {
            cmd.env_remove(k);
        }
        for k in &self.extra_scrubbed {
            cmd.env_remove(k);
        }
        cmd
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a shell command via cmd (Windows) or sh (Unix). Subject to sandbox exec allowlist, env scrubbing, and timeout."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command line." },
                "cwd": { "type": "string", "description": "Optional working directory." },
                "timeout_ms": { "type": "integer", "description": "Optional per-call timeout in milliseconds." }
            },
            "required": ["command"]
        })
    }

    async fn invoke(&self, input: Value) -> ToolResult<Value> {
        let parsed: BashInput = serde_json::from_value(input)?;
        self.sandbox.validate_exec(&parsed.command)?;

        let per_call_timeout = parsed
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.timeout);
        let mut cmd = self.build_command(&parsed.command, parsed.cwd.as_deref());

        let child = cmd.spawn()?;
        let result = timeout(per_call_timeout, child.wait_with_output()).await;
        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let code = output.status.code().unwrap_or(-1);
                Ok(json!({
                    "exit_code": code,
                    "stdout": stdout,
                    "stderr": stderr,
                }))
            }
            Ok(Err(e)) => Err(ToolError::Exec(format!("spawn wait failed: {e}"))),
            Err(_) => Err(ToolError::Exec(format!(
                "command timed out after {:?}",
                per_call_timeout
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_sandbox(root: &Path) -> Arc<mscode_sandbox::Sandbox> {
        Arc::new(mscode_sandbox::Sandbox::new(root))
    }

    #[tokio::test]
    async fn runs_allowlisted_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new(make_sandbox(dir.path()));
        // `echo` is on the default allowlist across Windows + Unix.
        let out = tool
            .invoke(json!({"command": "echo hello-mscode"}))
            .await
            .unwrap();
        let stdout = out["stdout"].as_str().unwrap();
        assert!(stdout.contains("hello-mscode"));
        assert_eq!(out["exit_code"], 0);
    }

    #[tokio::test]
    async fn rejects_non_allowlisted_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new(make_sandbox(dir.path()));
        let err = tool
            .invoke(json!({"command": "definitely-not-real-xyz command"}))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ToolError::Sandbox(mscode_sandbox::SandboxError::ExecDenied(_))
        ));
    }

    #[tokio::test]
    async fn enforces_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new(make_sandbox(dir.path())).with_timeout(Duration::from_millis(50));
        // Sleep for 2 seconds; must be killed by the 50ms timeout.
        let cmd = if cfg!(windows) {
            "cmd /C timeout /T 2 /NOBREAK"
        } else {
            "sleep 2"
        };
        // `timeout` (Windows) and `sleep` (Unix) — wrap differently.
        let cmd = if cfg!(windows) {
            // `cmd` is on the allowlist, but the shell built-in `timeout` is
            // available via cmd /C.
            cmd.to_string()
        } else {
            cmd.to_string()
        };
        let err = tool.invoke(json!({"command": cmd})).await.unwrap_err();
        assert!(matches!(err, ToolError::Exec(_)));
    }

    #[tokio::test]
    async fn scrubbed_env_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        // Inject a scrubbed var into the parent env temporarily.
        // SAFETY: this test is single-threaded with respect to env mutation;
        // we set and remove a key uniquely owned by this test.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "should-not-leak");
        }
        let tool = BashTool::new(make_sandbox(dir.path()));
        let cmd = if cfg!(windows) {
            "echo %OPENAI_API_KEY%"
        } else {
            "echo $OPENAI_API_KEY"
        };
        let out = tool.invoke(json!({"command": cmd})).await.unwrap();
        let stdout = out["stdout"].as_str().unwrap();
        assert!(
            !stdout.contains("should-not-leak"),
            "scrubbed var leaked: {stdout}"
        );
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        }
    }
}
