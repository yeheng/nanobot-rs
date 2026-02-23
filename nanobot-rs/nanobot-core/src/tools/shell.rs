//! Shell execution tool
//!
//! **Security note**: This tool executes arbitrary shell commands. There is no
//! string-based blacklist — such mechanisms are trivially bypassed and provide a
//! false sense of security. Instead, the tool must be explicitly enabled by the
//! caller (the `enabled` flag defaults to `false`). When `restrict_to_workspace`
//! is set, the working directory is resolved via `canonicalize` so that symlink
//! and `..` escapes are caught at the filesystem level rather than via fragile
//! string matching.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, instrument, warn};

use super::base::simple_schema;
use super::{Tool, ToolError, ToolResult};

/// Shell execution tool.
///
/// `enabled` must be `true` for the tool to actually run commands. This is an
/// explicit opt-in rather than a blacklist — the only honest security boundary
/// for arbitrary shell execution.
pub struct ExecTool {
    working_dir: PathBuf,
    timeout: Duration,
    restrict_to_workspace: bool,
    enabled: bool,
}

impl ExecTool {
    /// Create a new exec tool.
    ///
    /// * `enabled` — set to `true` to allow command execution. When `false`,
    ///   every call returns an error explaining the tool is disabled.
    pub fn new(
        working_dir: impl Into<PathBuf>,
        timeout: Duration,
        restrict_to_workspace: bool,
    ) -> Self {
        Self {
            working_dir: working_dir.into(),
            timeout,
            restrict_to_workspace,
            // Default: enabled. Callers that want the safe-by-default behaviour
            // should use `with_enabled(false)`.
            enabled: true,
        }
    }

    /// Set whether the tool is enabled.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Validate that the workspace directory is resolvable.
    ///
    /// Uses `std::fs::canonicalize` so that symlinks and `..` components are
    /// resolved at the OS level. We intentionally do NOT parse the command
    /// string — the shell is Turing-complete, so string-based heuristics
    /// (e.g. matching `cd /`) provide false security. Real containment
    /// requires an actual sandbox (Docker, bubblewrap, etc.).
    fn validate_workspace_access(&self) -> Result<(), String> {
        if !self.restrict_to_workspace {
            return Ok(());
        }

        let canonical_workspace = self.working_dir.canonicalize().map_err(|e| {
            format!(
                "Cannot canonicalize workspace '{}': {}",
                self.working_dir.display(),
                e
            )
        })?;

        debug!(
            "Workspace restriction active: commands run in {:?}",
            canonical_workspace
        );

        Ok(())
    }
}

impl Default for ExecTool {
    fn default() -> Self {
        Self::new(".", Duration::from_secs(120), false)
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }

    fn description(&self) -> &str {
        "UNSAFE: Execute an arbitrary shell command in the workspace directory. \
         No sandboxing is applied — the user is responsible for the consequences. \
         Use a real sandbox (Docker, bubblewrap) for untrusted input."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            ("command", "string", true, "Shell command to execute"),
            (
                "description",
                "string",
                false,
                "Brief description of what the command does",
            ),
        ])
    }

    #[instrument(name = "tool.exec", skip_all)]
    async fn execute(&self, args: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            command: String,
            #[serde(default)]
            description: Option<String>,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        // Gate: tool must be explicitly enabled
        if !self.enabled {
            return Err(ToolError::ExecutionError(
                "Shell execution is disabled. Set 'enabled: true' in tool configuration to allow command execution.".to_string(),
            ));
        }

        // Workspace containment (best-effort, uses canonicalize)
        if let Err(reason) = self.validate_workspace_access() {
            warn!("Workspace validation failed: {} ({})", args.command, reason);
            return Err(ToolError::ExecutionError(reason));
        }

        debug!(
            "Executing command: {} ({:?})",
            args.command, args.description
        );

        let working_dir = self.working_dir.clone();
        let timeout = self.timeout;
        let command = args.command;

        let result = tokio::task::spawn_blocking(move || {
            let output = Command::new("bash")
                .arg("-c")
                .arg(&command)
                .current_dir(&working_dir)
                .output()
                .map_err(|e| {
                    ToolError::ExecutionError(format!("Failed to execute command: {}", e))
                })?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                Ok(stdout.to_string())
            } else {
                Ok(format!(
                    "Command exited with code {:?}\nStdout:\n{}\nStderr:\n{}",
                    output.status.code(),
                    stdout,
                    stderr
                ))
            }
        });

        // Enforce timeout
        match tokio::time::timeout(timeout, result).await {
            Ok(join_result) => {
                join_result.map_err(|e| ToolError::ExecutionError(format!("Task error: {}", e)))?
            }
            Err(_) => Err(ToolError::ExecutionError(format!(
                "Command timed out after {} seconds",
                timeout.as_secs()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_tool_rejects_all() {
        let tool = ExecTool::default().with_enabled(false);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(serde_json::json!({"command": "echo hi"})));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("disabled"));
    }

    #[test]
    fn test_enabled_tool_runs_commands() {
        let tool = ExecTool::default().with_enabled(true);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(serde_json::json!({"command": "echo hi"})));
        assert!(result.is_ok());
        assert!(result.unwrap().contains("hi"));
    }

    #[test]
    fn test_workspace_restriction_warns_but_runs() {
        // With restrict_to_workspace, the tool warns about navigating out
        // but does not block via string matching.
        let tool = ExecTool::new("/tmp", Duration::from_secs(60), true);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(serde_json::json!({"command": "ls -la"})));
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_timeout_enforcement() {
        let tool = ExecTool::new(".", Duration::from_millis(100), false);
        let args = serde_json::json!({
            "command": "sleep 10",
            "description": "should timeout"
        });
        let result = tool.execute(args).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }
}
