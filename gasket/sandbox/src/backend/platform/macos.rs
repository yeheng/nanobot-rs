//! macOS sandbox-exec (Seatbelt) sandbox backend
//!
//! Uses Apple's built-in `sandbox-exec` tool with a custom Seatbelt profile.
//! Provides filesystem isolation on macOS.

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use tokio::process::Command as AsyncCommand;
use tracing::{debug, info, warn};

use crate::backend::{ExecutionResult, Platform, SandboxBackend};
use crate::config::{ResourceLimits, SandboxConfig};
use crate::error::{Result, SandboxError};

/// macOS sandbox-exec (Seatbelt) based sandbox.
///
/// Uses Apple's built-in `sandbox-exec` tool with a custom Seatbelt profile.
/// Profile policy:
/// - Allow all read operations (to let system binaries work)
/// - Deny all file writes except:
///   - Workspace directory (configured)
///   - /tmp and /private/tmp
///   - /dev/null and /dev/zero
///
/// Note: sandbox-exec is deprecated by Apple for App Store apps, but remains
/// the most practical solution for CLI tools requiring filesystem isolation.
pub struct MacOsSandboxBackend {
    _workspace: PathBuf,
}

impl MacOsSandboxBackend {
    /// Create a new macOS sandbox backend
    pub fn new() -> Self {
        Self {
            _workspace: PathBuf::from("."),
        }
    }

    /// Create with a specific workspace
    pub fn with_workspace(workspace: PathBuf) -> Self {
        Self {
            _workspace: workspace,
        }
    }

    /// Detect sandbox-exec binary on macOS
    fn detect_sandbox_exec() -> Option<PathBuf> {
        // sandbox-exec is typically at /usr/bin/sandbox-exec on macOS
        let path = PathBuf::from("/usr/bin/sandbox-exec");
        if path.exists() {
            info!("sandbox-exec detected at {:?}", path);
            Some(path)
        } else {
            // Try `which` command as fallback
            let output = Command::new("which").arg("sandbox-exec").output().ok()?;
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    let path = PathBuf::from(path_str);
                    info!("sandbox-exec detected at {:?}", path);
                    return Some(path);
                }
            }
            warn!("sandbox-exec not found — falling back to ulimit-based limits");
            None
        }
    }

    /// Generate a Seatbelt sandbox profile string.
    ///
    /// The profile allows all reads but restricts writes to:
    /// - The workspace directory
    /// - /tmp and /private/tmp
    /// - /dev/null and /dev/zero
    fn generate_profile(&self, workspace: &Path) -> String {
        let workspace = workspace.display();
        format!(
            r#"(version 1)
(deny default)
(allow file-read*)
(allow file-write*
  (subpath "{workspace}")
  (subpath "/tmp")
  (subpath "/private/tmp")
  (literal "/dev/null")
  (literal "/dev/zero")
)
(allow process-exec)
(allow network-outbound)
(allow signal (target self))
(allow file-read-metadata)
"#
        )
    }

    fn build_command_internal(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Command {
        let profile = self.generate_profile(working_dir);
        let limits = ResourceLimits::from(&config.limits);

        // Resource limits via ulimit (sandbox-exec doesn't handle this)
        let prefixed_cmd = format!("{}{}", limits.to_ulimit_prefix(), cmd);

        let mut command = Command::new("sandbox-exec");
        // SECURITY NOTE: Shell injection prevention is handled by CommandPolicy
        // and check_dangerous_patterns() in the CommandBuilder.
        // The sandbox-exec isolation provides additional defense-in-depth.
        command
            .arg("-p")
            .arg(profile)
            .arg("sh")
            .arg("-c")
            .arg(prefixed_cmd)
            .current_dir(working_dir);

        debug!("sandbox-exec command: {:?}", command);
        command
    }
}

impl Default for MacOsSandboxBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for MacOsSandboxBackend {
    fn name(&self) -> &str {
        "sandbox-exec"
    }

    async fn is_available(&self) -> bool {
        Self::detect_sandbox_exec().is_some()
    }

    fn supported_platforms(&self) -> &[Platform] {
        &[Platform::MacOS]
    }

    fn provides_filesystem_isolation(&self) -> bool {
        true
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
        let profile = self.generate_profile(working_dir);
        let limits = ResourceLimits::from(&config.limits);

        // Resource limits via ulimit (sandbox-exec doesn't handle this)
        let prefixed_cmd = format!("{}{}", limits.to_ulimit_prefix(), cmd);

        let mut command = AsyncCommand::new("sandbox-exec");
        command
            .arg("-p")
            .arg(&profile)
            .arg("sh")
            .arg("-c")
            .arg(&prefixed_cmd)
            .current_dir(working_dir)
            .kill_on_drop(true);

        debug!("sandbox-exec async command: {:?}", command);

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

    #[test]
    fn test_profile_generation() {
        let backend = MacOsSandboxBackend::new();
        let profile = backend.generate_profile(Path::new("/Users/test/.gasket"));

        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(subpath \"/Users/test/.gasket\")"));
        assert!(profile.contains("(subpath \"/tmp\")"));
        assert!(profile.contains("(subpath \"/private/tmp\")"));
        assert!(profile.contains("(literal \"/dev/null\")"));
        assert!(profile.contains("(literal \"/dev/zero\")"));
    }

    #[tokio::test]
    async fn test_sandbox_exec_availability() {
        let backend = MacOsSandboxBackend::new();
        // This test depends on whether sandbox-exec is available
        // Just verify it doesn't panic
        let _ = backend.is_available().await;
    }

    #[test]
    fn test_build_command() {
        let backend = MacOsSandboxBackend::new();
        let config = SandboxConfig::default();
        let cmd = backend.build_command("echo hello", Path::new("/tmp"), &config);
        assert!(cmd.is_ok());

        let cmd = cmd.unwrap();
        assert_eq!(cmd.get_program(), "sandbox-exec");

        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "-p");
        // Second arg should be the profile (starts with "(version 1)")
        let profile = args[1].to_string_lossy();
        assert!(profile.starts_with("(version 1)"));
        assert_eq!(args[2], "sh");
        assert_eq!(args[3], "-c");
    }
}
