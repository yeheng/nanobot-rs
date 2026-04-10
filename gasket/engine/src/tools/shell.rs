//! Shell execution tool with sandbox support.
//!
//! **Security model**: Defense in depth with three layers:
//! 1. **Command policy** (advisory): Allowlist/denylist to catch accidental misuse
//! 2. **Sandbox** (OS-level): bwrap namespace isolation on Linux, sandbox-exec on macOS
//! 3. **Resource limits**: Memory, CPU time, output size, wall-clock timeout
//!
//! This module delegates to `gasket-sandbox` for all sandbox execution,
//! eliminating code duplication and ensuring consistent security behavior.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info, instrument, warn};

use super::base::{simple_schema, ToolContext};
use super::{Tool, ToolError, ToolResult};
use crate::config::ExecToolConfig;

// Re-export types from gasket-sandbox for external use
pub use gasket_sandbox::ProcessManager;
// Use alias to avoid name conflict with core's SandboxConfig
pub use gasket_sandbox::SandboxConfig as SandboxExecutorConfig;

/// Dangerous patterns that could indicate command injection attempts.
///
/// Note: `>` and `>>` are intentionally excluded — they are output redirections,
/// not injection vectors. The sandbox layer already constrains file system access.
/// Harmless fd-duplication patterns like `2>&1` must be allowed for normal use.
const DANGEROUS_PATTERNS: &[&str] = &[";", "&&", "||", "`", "$(", "${", "|", "\n", "\r"];

/// Shell execution tool with optional sandboxing.
///
/// Uses `gasket-sandbox::ProcessManager` for command execution,
/// providing consistent sandbox behavior across all platforms.
pub struct ExecTool {
    working_dir: PathBuf,
    timeout: Duration,
    restrict_to_workspace: bool,
    enabled: bool,
    process_manager: ProcessManager,
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

        // Convert gasket-core config to gasket-sandbox config
        let sandbox_config = build_sandbox_config(config, &working_dir);

        // Create process manager with the sandbox configuration
        let process_manager = ProcessManager::new(sandbox_config).with_timeout(timeout);

        info!(
            "ExecTool initialized: sandbox={}, working_dir={:?}, timeout={}s{}",
            if process_manager.is_sandboxed() {
                process_manager.backend_name()
            } else {
                "disabled"
            },
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
            process_manager,
        }
    }

    /// Create an ExecTool with simple parameters (backward compatible).
    pub fn new(
        working_dir: impl Into<PathBuf>,
        timeout: Duration,
        restrict_to_workspace: bool,
    ) -> Self {
        let working_dir = working_dir.into();
        let sandbox_config = SandboxExecutorConfig::fallback();
        let process_manager = ProcessManager::new(sandbox_config).with_timeout(timeout);

        Self {
            working_dir,
            timeout,
            restrict_to_workspace,
            enabled: true,
            process_manager,
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
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
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

        // Step 1: Workspace containment (best-effort, uses canonicalize)
        if let Err(reason) = self.validate_workspace_access() {
            warn!("Workspace validation failed: {} ({})", args.command, reason);
            return Err(ToolError::ExecutionError(reason));
        }

        debug!(
            "Executing command via {}: {} ({:?})",
            self.process_manager.backend_name(),
            args.command,
            args.description
        );

        // Step 2: Execute via gasket-sandbox ProcessManager
        let result = self
            .process_manager
            .execute_with_timeout(&args.command, &self.working_dir, self.timeout)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        // Step 3: Format output
        if result.is_success() {
            Ok(result.stdout)
        } else if result.timed_out {
            Err(ToolError::ExecutionError(format!(
                "Command timed out after {} seconds",
                self.timeout.as_secs()
            )))
        } else {
            Ok(format!(
                "Command exited with code {:?}\nStdout:\n{}\nStderr:\n{}",
                result.exit_code, result.stdout, result.stderr
            ))
        }
    }
}

/// Build a gasket-sandbox SandboxConfig from gasket-core ExecToolConfig.
///
/// This function maps the core configuration types to the sandbox configuration,
/// handling differences in field names and structure.
fn build_sandbox_config(config: &ExecToolConfig, workspace: &Path) -> SandboxExecutorConfig {
    use gasket_sandbox::config::{
        ApprovalConfig, AuditConfig, CommandPolicyConfig as SandboxPolicyConfig,
        ResourceLimitsConfig as SandboxLimitsConfig,
    };

    // Build resource limits
    let limits = SandboxLimitsConfig {
        max_memory_mb: config.limits.max_memory_mb,
        max_cpu_secs: config.limits.max_cpu_secs,
        max_output_bytes: config.limits.max_output_bytes,
        ..Default::default()
    };

    // Build command policy
    let policy = SandboxPolicyConfig {
        allowlist: config.policy.allowlist.clone(),
        denylist: config.policy.denylist.clone(),
    };

    // Build full sandbox config
    SandboxExecutorConfig {
        enabled: config.sandbox.enabled,
        backend: config.sandbox.backend.clone(),
        tmp_size_mb: config.sandbox.tmp_size_mb,
        workspace: Some(workspace.to_path_buf()),
        limits,
        policy,
        approval: ApprovalConfig::default(),
        audit: AuditConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_tool_rejects_all() {
        let tool = ExecTool::default().with_enabled(false);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(
            serde_json::json!({"command": "echo hi"}),
            &ToolContext::default(),
        ));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("disabled"));
    }

    #[test]
    fn test_enabled_tool_runs_commands() {
        let tool = ExecTool::default().with_enabled(true);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(
            serde_json::json!({"command": "echo hi"}),
            &ToolContext::default(),
        ));
        assert!(result.is_ok());
        assert!(result.unwrap().contains("hi"));
    }

    #[test]
    fn test_workspace_restriction_warns_but_runs() {
        let tool = ExecTool::new("/tmp", Duration::from_secs(60), true);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(tool.execute(
            serde_json::json!({"command": "ls -la"}),
            &ToolContext::default(),
        ));
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_timeout_enforcement() {
        let tool = ExecTool::new(".", Duration::from_millis(100), false);
        let args = serde_json::json!({
            "command": "sleep 10",
            "description": "should timeout"
        });
        let result = tool.execute(args, &ToolContext::default()).await;
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
        let result = rt.block_on(tool.execute(
            serde_json::json!({"command": "rm -rf /"}),
            &ToolContext::default(),
        ));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("denied"));
    }

    #[test]
    fn test_command_injection_blocked() {
        let tool = ExecTool::default().with_enabled(true);
        let rt = tokio::runtime::Runtime::new().unwrap();

        // Test various injection patterns (command chaining/substitution)
        let injection_attempts = vec![
            "echo hello && cat /etc/passwd",
            "echo hello; cat /etc/passwd",
            "echo hello || cat /etc/passwd",
            "echo `cat /etc/passwd`",
            "echo $(cat /etc/passwd)",
            "echo ${PATH}",
            "echo hello | cat",
        ];

        for cmd in injection_attempts {
            let result = rt.block_on(
                tool.execute(serde_json::json!({"command": cmd}), &ToolContext::default()),
            );
            assert!(
                result.is_err(),
                "Command '{}' should have been blocked",
                cmd
            );
            assert!(
                result.unwrap_err().to_string().contains("unsafe pattern"),
                "Command '{}' should have been blocked as unsafe",
                cmd
            );
        }
    }

    #[test]
    fn test_redirect_patterns_allowed() {
        let tool = ExecTool::default().with_enabled(true);
        let rt = tokio::runtime::Runtime::new().unwrap();

        // Redirect patterns are NOT injection vectors and should pass validation
        let redirect_commands = vec![
            "gasket memory reindex 2>&1",
            "echo hello > /tmp/test_output.txt",
            "echo hello >> /tmp/test_append.txt",
        ];

        for cmd in redirect_commands {
            let result = rt.block_on(
                tool.execute(serde_json::json!({"command": cmd}), &ToolContext::default()),
            );
            // Should NOT be blocked by injection check (may still fail on execution)
            if let Err(e) = &result {
                let err_msg = e.to_string();
                assert!(
                    !err_msg.contains("unsafe pattern"),
                    "Redirect command '{}' should not be blocked as unsafe, got: {}",
                    cmd,
                    err_msg
                );
            }
        }
    }
}
