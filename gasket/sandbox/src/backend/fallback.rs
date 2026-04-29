//! Fallback backend — direct execution with ulimit-based resource limits.

use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use tokio::process::Command as AsyncCommand;
use tracing::debug;

use super::{ExecutionResult, Platform, SandboxBackend};
use crate::config::SandboxConfig;
use crate::error::{Result, SandboxError};

/// Fallback executor — direct `sh -c` with ulimit-based resource limits.
pub struct FallbackBackend;

impl FallbackBackend {
    pub fn new() -> Self {
        Self
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
        true
    }

    fn supported_platforms(&self) -> &[Platform] {
        &[Platform::Linux, Platform::MacOS, Platform::Windows]
    }

    fn provides_filesystem_isolation(&self) -> bool {
        false
    }

    fn build_command(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<Command> {
        let prefixed_cmd = format!("{}{}", config.limits.to_ulimit_prefix(), cmd);

        let mut command = Command::new("sh");
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
        let prefixed_cmd = format!("{}{}", config.limits.to_ulimit_prefix(), cmd);

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

        let stdout = config.limits.truncate_output(&stdout);
        let stderr = config.limits.truncate_output(&stderr);

        Ok(ExecutionResult {
            exit_code: output.status.code(),
            stdout,
            stderr,
            timed_out: false,
            resource_exceeded: false,
            duration_ms: 0,
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
