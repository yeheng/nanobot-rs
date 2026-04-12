//! Fallback backend — direct execution with ulimit-based resource limits.
//!
//! Used when no sandbox is available or when sandbox is disabled.
//! Provides basic resource limiting via shell `ulimit` command.

use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use tokio::process::Command as AsyncCommand;
use tracing::debug;

use super::{ExecutionResult, Platform, SandboxBackend};
use crate::config::{ResourceLimits, SandboxConfig};
use crate::error::{Result, SandboxError};

/// Fallback executor — direct `sh -c` with ulimit-based resource limits.
pub struct FallbackBackend {
    _limits: ResourceLimits,
}

impl FallbackBackend {
    /// Create a new fallback backend with default limits
    pub fn new() -> Self {
        Self {
            _limits: ResourceLimits::default(),
        }
    }

    /// Create with custom resource limits
    pub fn with_limits(limits: ResourceLimits) -> Self {
        Self { _limits: limits }
    }
}

impl Default for FallbackBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for FallbackBackend {
    fn name(&self) -> &str {
        "fallback"
    }

    async fn is_available(&self) -> bool {
        // Fallback is always available
        true
    }

    fn supported_platforms(&self) -> &[Platform] {
        // Fallback works on all platforms
        &[Platform::Linux, Platform::MacOS, Platform::Windows]
    }

    fn build_command(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<Command> {
        let limits = ResourceLimits::from(&config.limits);
        let prefixed_cmd = format!("{}{}", limits.to_ulimit_prefix(), cmd);

        let mut command = Command::new("sh");
        // Use sh -c with the command string.
        // SECURITY NOTE: Shell injection prevention is handled by CommandPolicy
        // and check_dangerous_patterns() in the CommandBuilder.
        // The sandbox isolation (bwrap/sandbox-exec) provides additional defense.
        command
            .arg("-c")
            .arg(&prefixed_cmd)
            .current_dir(working_dir);

        debug!("Fallback command: {:?}", command);
        Ok(command)
    }

    async fn execute(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<ExecutionResult> {
        let limits = ResourceLimits::from(&config.limits);
        let prefixed_cmd = format!("{}{}", limits.to_ulimit_prefix(), cmd);

        // Build async command with kill_on_drop to ensure process termination on timeout
        let mut command = AsyncCommand::new("sh");
        command
            .arg("-c")
            .arg(&prefixed_cmd)
            .current_dir(working_dir)
            .kill_on_drop(true);

        debug!("Fallback async command: {:?}", command);

        let output = command
            .output()
            .await
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Truncate output if needed
        let stdout = limits.truncate_output(&stdout);
        let stderr = limits.truncate_output(&stderr);

        Ok(ExecutionResult {
            exit_code: output.status.code(),
            stdout,
            stderr,
            timed_out: false,
            resource_exceeded: false,
            duration_ms: 0, // Duration is tracked by ProcessManager
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fallback_is_available() {
        let backend = FallbackBackend::new();
        assert!(backend.is_available().await);
    }

    #[tokio::test]
    async fn test_fallback_execute() {
        let backend = FallbackBackend::new();
        let config = SandboxConfig::default();
        let result = backend
            .execute("echo hello", Path::new("/tmp"), &config)
            .await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.is_success());
        assert!(result.stdout.contains("hello"));
    }

    #[test]
    fn test_build_command() {
        let backend = FallbackBackend::new();
        let config = SandboxConfig::default();
        let cmd = backend.build_command("echo test", Path::new("/tmp"), &config);
        assert!(cmd.is_ok());
        let cmd = cmd.unwrap();
        assert_eq!(cmd.get_program(), "sh");
    }
}
