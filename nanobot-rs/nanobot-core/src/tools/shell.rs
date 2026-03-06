//! Shell execution tool with sandbox support.
//!
//! **Security model**: Defense in depth with three layers:
//! 1. **Command policy** (advisory): Allowlist/denylist to catch accidental misuse
//! 2. **Sandbox** (OS-level): bwrap namespace isolation on Linux, ulimit fallback
//! 3. **Resource limits**: Memory, CPU time, output size, wall-clock timeout

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info, instrument, warn};

use super::base::simple_schema;
use super::command_policy::{CommandPolicy, PolicyVerdict};
use super::resource_limits::ResourceLimits;
use super::sandbox::{self, SandboxExecutor};
use super::{Tool, ToolError, ToolResult};
use crate::config::ExecToolConfig;

/// Dangerous patterns that could indicate command injection attempts
const DANGEROUS_PATTERNS: &[&str] = &[";", "&&", "||", "`", "$(", "${", ">", ">>", "|", "\n", "\r"];

/// Shell execution tool with optional sandboxing.
pub struct ExecTool {
    working_dir: PathBuf,
    timeout: Duration,
    restrict_to_workspace: bool,
    enabled: bool,
    policy: CommandPolicy,
    sandbox: SandboxExecutor,
    limits: ResourceLimits,
}

impl ExecTool {
    /// Create an ExecTool from configuration.
    pub fn from_config(
        working_dir: impl Into<PathBuf>,
        config: &ExecToolConfig,
        restrict_to_workspace: bool,
    ) -> Self {
        let working_dir = working_dir.into();
        // Ensure timeout is at least 1 second to avoid immediate timeout
        let timeout_secs = if config.timeout == 0 {
            120
        } else {
            config.timeout
        };
        let timeout = Duration::from_secs(timeout_secs);
        let policy = CommandPolicy::new(&config.policy);
        let limits = ResourceLimits::from_config(&config.limits);
        let sandbox_provider = sandbox::create_provider(&working_dir, &config.sandbox);

        info!(
            "ExecTool initialized: sandbox={}, working_dir={:?}, timeout={}s{}",
            sandbox_provider.name(),
            working_dir,
            timeout_secs,
            if config.timeout == 0 {
                " (default, was 0)"
            } else {
                ""
            }
        );

        Self {
            working_dir,
            timeout,
            restrict_to_workspace,
            enabled: true,
            policy,
            sandbox: sandbox_provider,
            limits,
        }
    }

    /// Create an ExecTool with simple parameters (backward compatible).
    pub fn new(
        working_dir: impl Into<PathBuf>,
        timeout: Duration,
        restrict_to_workspace: bool,
    ) -> Self {
        let working_dir = working_dir.into();
        Self {
            policy: CommandPolicy::new(&Default::default()),
            sandbox: SandboxExecutor::Fallback(super::sandbox::FallbackExecutor),
            limits: ResourceLimits::default(),
            working_dir,
            timeout,
            restrict_to_workspace,
            enabled: true,
        }
    }

    /// Set whether the tool is enabled.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Validate that the workspace directory is resolvable.
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

    /// Validate command for potential injection attempts.
    fn validate_command(&self, command: &str) -> Result<(), ToolError> {
        for pattern in DANGEROUS_PATTERNS {
            if command.contains(pattern) {
                return Err(ToolError::InvalidArguments(format!(
                    "Potentially unsafe pattern detected: '{}'. Command injection is not allowed.",
                    pattern
                )));
            }
        }
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
        "Execute a shell command in the workspace directory. \
         Commands are subject to policy checks and resource limits. \
         Sandbox isolation is available when configured."
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

        // Step 0: Command injection validation
        self.validate_command(&args.command)?;

        // Step 1: Command policy check (advisory)
        match self.policy.check(&args.command) {
            PolicyVerdict::Allow => {}
            PolicyVerdict::Deny(reason) => {
                return Err(ToolError::ExecutionError(format!(
                    "Command denied by policy: {}",
                    reason
                )));
            }
        }

        // Step 2: Workspace containment (best-effort, uses canonicalize)
        if let Err(reason) = self.validate_workspace_access() {
            warn!("Workspace validation failed: {} ({})", args.command, reason);
            return Err(ToolError::ExecutionError(reason));
        }

        debug!(
            "Executing command via {}: {} ({:?})",
            self.sandbox.name(),
            args.command,
            args.description
        );

        // Step 3: Build sandboxed command
        let command_str = args.command;
        let working_dir = self.working_dir.clone();
        let mut cmd = self
            .sandbox
            .build_command(&command_str, &working_dir, &self.limits);
        let max_output = self.limits.max_output_bytes;
        let timeout = self.timeout;

        // Step 4: Execute with wall-clock timeout
        let result = tokio::task::spawn_blocking(move || {
            let output = cmd.output().map_err(|e| {
                ToolError::ExecutionError(format!("Failed to execute command: {}", e))
            })?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            let raw_output = if output.status.success() {
                stdout.to_string()
            } else {
                format!(
                    "Command exited with code {:?}\nStdout:\n{}\nStderr:\n{}",
                    output.status.code(),
                    stdout,
                    stderr
                )
            };

            // Step 5: Truncate output if needed
            let limits = ResourceLimits {
                max_memory_bytes: 0,
                max_cpu_secs: 0,
                max_output_bytes: max_output,
            };
            Ok(limits.truncate_output(&raw_output))
        });

        // Enforce wall-clock timeout
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

    #[test]
    fn test_from_config() {
        let config = ExecToolConfig {
            timeout: 120,
            ..Default::default()
        };
        let tool = ExecTool::from_config("/tmp", &config, false);
        assert!(tool.enabled);
        assert_eq!(tool.timeout, Duration::from_secs(120));
    }

    #[test]
    fn test_policy_blocks_denied_command() {
        let mut config = ExecToolConfig {
            timeout: 60,
            ..Default::default()
        };
        config.policy.denylist = vec!["rm -rf /".to_string()];
        let tool = ExecTool::from_config("/tmp", &config, false);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(serde_json::json!({"command": "rm -rf /"})));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("denied"));
    }

    #[test]
    fn test_output_truncation() {
        let config = ExecToolConfig {
            timeout: 60,
            limits: crate::config::ResourceLimitsConfig {
                max_memory_mb: 512,
                max_cpu_secs: 60,
                max_output_bytes: 20,
            },
            ..Default::default()
        };
        // Use echo which is more portable than python3
        let tool = ExecTool::from_config("/tmp", &config, false);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(serde_json::json!({
            "command": "echo 'aaaaaaaaaabbbbbbbbbbccccccccccdddddddddd'"
        })));
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("[OUTPUT TRUNCATED"));
    }
}
