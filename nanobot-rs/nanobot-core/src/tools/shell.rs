//! Shell execution tool

use std::process::Command;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use std::path::PathBuf;

use super::base::simple_schema;
use super::{Tool, ToolError, ToolResult};

/// Shell execution tool
#[allow(dead_code)]
pub struct ExecTool {
    working_dir: PathBuf,
    timeout: Duration,
    restrict_to_workspace: bool,
}

impl ExecTool {
    /// Create a new exec tool
    pub fn new(
        working_dir: impl Into<PathBuf>,
        timeout: Duration,
        restrict_to_workspace: bool,
    ) -> Self {
        Self {
            working_dir: working_dir.into(),
            timeout,
            restrict_to_workspace,
        }
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
        "Execute a shell command in the workspace directory"
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            ("command", "string", true),
            ("description", "string", false),
        ])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            command: String,
            #[serde(default)]
            description: Option<String>,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        debug!(
            "Executing command: {} ({:?})",
            args.command, args.description
        );

        // Execute the command using tokio
        let working_dir = self.working_dir.clone();
        let _timeout = self.timeout;
        let command = args.command;

        let result = tokio::task::spawn_blocking(move || {
            // Use bash -c for consistent behavior
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
        })
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Task error: {}", e)))??;

        Ok(result)
    }
}
