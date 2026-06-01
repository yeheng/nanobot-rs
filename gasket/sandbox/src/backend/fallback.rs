//! Fallback backend — direct execution with ulimit-based resource limits.

use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::process::Command as AsyncCommand;
use tracing::debug;

use super::{ExecutionResult, IsolationLevel, Platform, SandboxBackend};
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

    fn isolation_level(&self) -> IsolationLevel {
        // Fallback runs `sh -c` directly. No isolation. Be honest about it.
        IsolationLevel::None
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

    async fn execute_with_timeout(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
        timeout: Duration,
    ) -> Result<ExecutionResult> {
        let prefixed_cmd = format!("{}{}", config.limits.to_ulimit_prefix(), cmd);

        let mut command = AsyncCommand::new("sh");
        command
            .arg("-c")
            .arg(&prefixed_cmd)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(unix)]
        {
            unsafe {
                command.pre_exec(|| {
                    if libc::setpgid(0, 0) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
        }

        debug!("Fallback async command with timeout: {:?}", command);

        let mut child = command
            .spawn()
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?;

        let pid = child.id();
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();

        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(ref mut out) = stdout {
                out.read_to_end(&mut buf).await?;
            }
            Ok::<Vec<u8>, std::io::Error>(buf)
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(ref mut err) = stderr {
                err.read_to_end(&mut buf).await?;
            }
            Ok::<Vec<u8>, std::io::Error>(buf)
        });

        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => status.map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?,
            Err(_) => {
                #[cfg(unix)]
                if let Some(pid) = pid {
                    unsafe {
                        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                    }
                }
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(SandboxError::Timeout {
                    timeout_secs: timeout.as_secs(),
                });
            }
        };

        let stdout = stdout_task
            .await
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?;
        let stderr = stderr_task
            .await
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&stdout).to_string();
        let stderr = String::from_utf8_lossy(&stderr).to_string();

        let stdout = config.limits.truncate_output(&stdout);
        let stderr = config.limits.truncate_output(&stderr);

        Ok(ExecutionResult {
            exit_code: status.code(),
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
