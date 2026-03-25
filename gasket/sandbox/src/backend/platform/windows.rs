//! Windows fallback sandbox backend
//!
//! **WARNING**: This is NOT a real sandbox. Commands run with the same
//! privileges as the parent process without isolation or resource limits.
//! Consider using WSL2 with bwrap for proper sandboxing on Windows.

use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::backend::{ExecutionResult, Platform, SandboxBackend};
use crate::config::SandboxConfig;
use crate::error::{Result, SandboxError};

/// Windows fallback executor — direct cmd.exe execution.
///
/// **WARNING**: This is NOT a real sandbox. Commands run with the same
/// privileges as the parent process without isolation or resource limits.
/// Full Job Objects integration would require unsafe Win32 API calls.
///
/// For proper sandboxing on Windows, consider using WSL2 with bwrap.
pub struct WindowsFallbackBackend {
    // No Job Object handle - this is intentional (not implemented)
}

impl WindowsFallbackBackend {
    /// Create a new Windows fallback backend
    ///
    /// **WARNING**: Logs a warning that this is not a real sandbox.
    pub fn new() -> Self {
        warn!(
            "WindowsFallbackBackend: This is NOT a sandbox. \
             Commands run without isolation. Consider using WSL2 with bwrap."
        );
        Self {}
    }

    fn build_command_internal(
        &self,
        cmd: &str,
        working_dir: &Path,
        _config: &SandboxConfig,
    ) -> Command {
        // On Windows, we use cmd.exe for command execution
        // NOTE: This provides NO sandboxing - commands run with full privileges
        // Full Job Objects integration would require unsafe Win32 API calls

        let mut command = Command::new("cmd");
        command.arg("/C").arg(cmd).current_dir(working_dir);

        debug!("Windows command (unsandboxed): {:?}", command);
        command
    }
}

impl Default for WindowsFallbackBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for WindowsFallbackBackend {
    fn name(&self) -> &str {
        "fallback" // Changed from "job-objects" to be honest about capabilities
    }

    async fn is_available(&self) -> bool {
        // Always available on Windows (it's just cmd.exe)
        true
    }

    fn supported_platforms(&self) -> &[Platform] {
        &[Platform::Windows]
    }

    fn build_command(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<Command> {
        Ok(self.build_command_internal(cmd, working_dir, config))
    }

    async fn execute(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<ExecutionResult> {
        let mut command = self.build_command(cmd, working_dir, config)?;

        let output = tokio::task::spawn_blocking(move || command.output())
            .await
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Note: Full Job Objects resource limiting would require Win32 API
        // For now, we just truncate output
        let max_output = config.limits.max_output_bytes;
        let stdout = if stdout.len() > max_output {
            let original_len = stdout.len();
            let mut truncated = stdout;
            truncated.truncate(max_output);
            truncated.push_str(&format!(
                "\n\n[OUTPUT TRUNCATED: {} bytes exceeded limit of {} bytes]",
                original_len,
                max_output
            ));
            truncated
        } else {
            stdout
        };

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
    async fn test_fallback_availability() {
        let backend = WindowsFallbackBackend::new();
        assert!(backend.is_available().await);
    }

    #[test]
    fn test_build_command() {
        let backend = WindowsFallbackBackend::new();
        let config = SandboxConfig::default();
        let cmd = backend.build_command("echo hello", Path::new("C:\\"), &config);
        assert!(cmd.is_ok());

        let cmd = cmd.unwrap();
        assert_eq!(cmd.get_program(), "cmd");

        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "/C");
    }
}
