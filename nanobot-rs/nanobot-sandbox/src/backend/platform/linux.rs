//! Linux Bubblewrap (bwrap) sandbox backend
//!
//! Uses bubblewrap for namespace-isolated execution on Linux.
//! Provides strong isolation through Linux namespaces.

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::backend::{ExecutionResult, Platform, SandboxBackend};
use crate::config::{ResourceLimits, SandboxConfig};
use crate::error::{Result, SandboxError};

/// Bubblewrap-based sandbox (Linux only).
///
/// Mount layout:
/// - `/`         → bind-ro from host
/// - `workspace` → bind-rw from configured workspace
/// - `/tmp`      → tmpfs (size-limited)
/// - `/dev`      → minimal devtmpfs
/// - `/proc`     → new proc namespace
pub struct LinuxBwrapBackend {
    bwrap_path: PathBuf,
    tmp_size_mb: u32,
}

impl LinuxBwrapBackend {
    /// Create a new bwrap backend, detecting the bwrap binary
    pub fn new() -> Self {
        let bwrap_path = Self::detect_bwrap().unwrap_or_else(|| PathBuf::from("bwrap"));
        Self {
            bwrap_path,
            tmp_size_mb: 64,
        }
    }

    /// Create with specific bwrap path
    pub fn with_path(bwrap_path: PathBuf) -> Self {
        Self {
            bwrap_path,
            tmp_size_mb: 64,
        }
    }

    /// Detect bwrap binary on the system
    fn detect_bwrap() -> Option<PathBuf> {
        // Try common locations
        let candidates = ["/usr/bin/bwrap", "/usr/local/bin/bwrap"];
        for path in &candidates {
            let p = PathBuf::from(path);
            if p.exists() {
                info!("bwrap detected at {:?}", p);
                return Some(p);
            }
        }

        // Try `which` command
        let output = Command::new("which").arg("bwrap").output().ok()?;
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                let p = PathBuf::from(path);
                info!("bwrap detected at {:?}", p);
                return Some(p);
            }
        }

        warn!("bwrap not found on system — sandbox unavailable");
        None
    }

    fn build_command_internal(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Command {
        let mut command = Command::new(&self.bwrap_path);
        let limits = ResourceLimits::from(&config.limits);
        let tmp_size_mb = config.tmp_size_mb;

        // Namespace isolation
        command.arg("--unshare-pid").arg("--unshare-ipc");

        // Filesystem mounts
        command
            // Read-only root
            .arg("--ro-bind")
            .arg("/")
            .arg("/")
            // Read-write workspace
            .arg("--bind")
            .arg(working_dir)
            .arg(working_dir)
            // Tmpfs for /tmp
            .arg("--tmpfs")
            .arg("/tmp")
            // Minimal /dev
            .arg("--dev")
            .arg("/dev")
            // New /proc
            .arg("--proc")
            .arg("/proc");

        // Tmpfs size limit
        command
            .arg("--size")
            .arg(format!("{}", u64::from(tmp_size_mb) * 1024 * 1024));

        // Resource limits
        for arg in limits.to_bwrap_args() {
            command.arg(arg);
        }

        // Working directory inside sandbox
        command.arg("--chdir").arg(working_dir);

        // The actual command - execute via bash
        // SECURITY NOTE: Shell injection prevention is handled by CommandPolicy
        // and check_dangerous_patterns() in the CommandBuilder.
        // The bwrap sandbox isolation provides additional defense-in-depth.
        command.arg("bash").arg("-c").arg(cmd);

        debug!("bwrap command: {:?}", command);
        command
    }
}

impl Default for LinuxBwrapBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for LinuxBwrapBackend {
    fn name(&self) -> &str {
        "bwrap"
    }

    async fn is_available(&self) -> bool {
        self.bwrap_path.exists()
    }

    fn supported_platforms(&self) -> &[Platform] {
        &[Platform::Linux]
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

        // Truncate output if needed
        let limits = ResourceLimits::from(&config.limits);
        let stdout = limits.truncate_output(&stdout);
        let stderr = limits.truncate_output(&stderr);

        Ok(ExecutionResult {
            exit_code: output.status.code(),
            stdout,
            stderr,
            timed_out: false,
            resource_exceeded: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bwrap_availability() {
        let backend = LinuxBwrapBackend::new();
        // This test depends on whether bwrap is installed
        // Just verify it doesn't panic
        let _ = backend.is_available().await;
    }

    #[test]
    fn test_build_command_structure() {
        let backend = LinuxBwrapBackend::new();
        let config = SandboxConfig::enabled();
        let result = backend.build_command("echo test", Path::new("/tmp"), &config);
        assert!(result.is_ok());
    }
}
