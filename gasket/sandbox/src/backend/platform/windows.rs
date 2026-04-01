//! Windows unsafe direct execution backend (NOT a sandbox)
//!
//! **CRITICAL WARNING**: This backend provides ZERO isolation. Commands run
//! with the same privileges as the parent process — no sandboxing, no resource
//! limits, no filesystem restrictions. For proper sandboxing on Windows, use
//! WSL2 with bwrap.

use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use tokio::process::Command as AsyncCommand;
use tracing::{debug, warn};

use crate::backend::{ExecutionResult, Platform, SandboxBackend};
use crate::config::SandboxConfig;
use crate::error::{Result, SandboxError};

/// Unsafe direct execution backend — runs commands via cmd.exe with NO isolation.
///
/// This backend does **not** sandbox commands. It exists only so that Windows
/// users can still execute tools, but every command runs with full user
/// privileges. The name is deliberately chosen to make the lack of safety
/// obvious at every call site.
///
/// For proper sandboxing on Windows, consider using WSL2 with bwrap.
pub struct UnsafeDirectExecution {
    // No isolation mechanism — this is intentional
}

impl UnsafeDirectExecution {
    /// Create a new unsafe direct execution backend.
    ///
    /// Logs a **warning** on every construction to remind operators that
    /// commands will run without any isolation.
    pub fn new() -> Self {
        warn!(
            "UnsafeDirectExecution: Commands will run WITHOUT isolation or \
             resource limits. For proper sandboxing, use WSL2 with bwrap."
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

impl Default for UnsafeDirectExecution {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for UnsafeDirectExecution {
    fn name(&self) -> &str {
        "unsafe-direct" // Name reflects reality: no sandboxing at all
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
        // Build async command with kill_on_drop to ensure process termination on timeout
        let mut command = AsyncCommand::new("cmd");
        command
            .arg("/C")
            .arg(cmd)
            .current_dir(working_dir)
            .kill_on_drop(true);

        debug!("Windows async command (unsandboxed): {:?}", command);

        let output = command
            .output()
            .await
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
                original_len, max_output
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
        let backend = UnsafeDirectExecution::new();
        assert!(backend.is_available().await);
    }

    #[test]
    fn test_build_command() {
        let backend = UnsafeDirectExecution::new();
        let config = SandboxConfig::default();
        let cmd = backend.build_command("echo hello", Path::new("C:\\"), &config);
        assert!(cmd.is_ok());

        let cmd = cmd.unwrap();
        assert_eq!(cmd.get_program(), "cmd");

        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "/C");
    }
}
