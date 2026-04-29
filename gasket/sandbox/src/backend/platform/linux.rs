//! Linux Bubblewrap (bwrap) sandbox backend
//!
//! Uses bubblewrap for namespace-isolated execution on Linux.
//! Provides strong isolation through Linux namespaces.

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use tokio::process::Command as AsyncCommand;
use tracing::{debug, info, warn};

use super::validate_workspace;
use crate::backend::{ExecutionResult, Platform, SandboxBackend};
use crate::config::SandboxConfig;
use crate::error::{Result, SandboxError};

/// Bubblewrap-based sandbox (Linux only).
///
/// Mount layout:
/// - `/`: bind-ro from host
/// - `working_dir`: bind-rw, validated to live within
///   `SandboxConfig.workspace` when configured
/// - `/tmp`: tmpfs, size-limited via `--size` placed *before* `--tmpfs /tmp`
///   (bwrap applies `--size` to the *next* tmpfs mount)
/// - `/dev`: minimal devtmpfs
/// - `/proc`: new proc namespace
///
/// Note: `--ro-bind / /` exposes the host's read-only filesystem inside the
/// sandbox. Anything readable to the calling user (e.g. `~/.ssh/`) is also
/// readable here. Combine with a tighter mount layout if that matters.
pub struct LinuxBwrapBackend {
    bwrap_path: PathBuf,
}

impl LinuxBwrapBackend {
    /// Create a new bwrap backend, detecting the bwrap binary
    pub fn new() -> Self {
        let bwrap_path = Self::detect_bwrap().unwrap_or_else(|| PathBuf::from("bwrap"));
        Self { bwrap_path }
    }

    /// Create with specific bwrap path
    pub fn with_path(bwrap_path: PathBuf) -> Self {
        Self { bwrap_path }
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

    /// Whether `bwrap` is installed at the resolved path. Used by the
    /// backend factory to fall back when the binary is missing instead of
    /// failing at execution time with a confusing "No such file" error.
    pub(crate) fn is_installed(&self) -> bool {
        self.bwrap_path.exists()
    }

    /// Build the bwrap argument list for either the sync or async command.
    /// The argument order is significant: bwrap's `--size BYTES` flag modifies
    /// the *next* `--tmpfs` mount, so it must immediately precede `--tmpfs /tmp`.
    fn bwrap_args(&self, validated_dir: &Path, config: &SandboxConfig) -> Vec<String> {
        let limits = ResourceLimits::from(&config.limits);
        let tmp_size_bytes = u64::from(config.tmp_size_mb) * 1024 * 1024;
        let dir = validated_dir.display().to_string();

        let mut args: Vec<String> = vec![
            // Namespace isolation
            "--unshare-pid".into(),
            "--unshare-ipc".into(),
            // Read-only root
            "--ro-bind".into(),
            "/".into(),
            "/".into(),
            // Read-write working dir
            "--bind".into(),
            dir.clone(),
            dir.clone(),
            // Size for the next tmpfs (must precede `--tmpfs /tmp`)
            "--size".into(),
            tmp_size_bytes.to_string(),
            "--tmpfs".into(),
            "/tmp".into(),
            // Minimal /dev
            "--dev".into(),
            "/dev".into(),
            // New /proc
            "--proc".into(),
            "/proc".into(),
        ];

        args.extend(limits.to_bwrap_args());
        args.push("--chdir".into());
        args.push(dir);
        // SECURITY: Shell injection prevention is handled by CommandPolicy.
        // bwrap isolation provides additional defense-in-depth.
        args.push("sh".into());
        args.push("-c".into());
        args
    }

    fn build_command_internal(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<Command> {
        let validated = validate_workspace(working_dir, config)?;
        let mut command = Command::new(&self.bwrap_path);
        for arg in self.bwrap_args(&validated, config) {
            command.arg(arg);
        }
        command.arg(cmd);
        debug!("bwrap command: {:?}", command);
        Ok(command)
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
        self.is_installed()
    }

    fn supported_platforms(&self) -> &[Platform] {
        &[Platform::Linux]
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
        self.build_command_internal(cmd, working_dir, config)
    }

    async fn execute(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<ExecutionResult> {
        let validated = validate_workspace(working_dir, config)?;
        let limits = ResourceLimits::from(&config.limits);

        let mut command = AsyncCommand::new(&self.bwrap_path);
        for arg in self.bwrap_args(&validated, config) {
            command.arg(arg);
        }
        command.arg(cmd).kill_on_drop(true);

        debug!("bwrap async command: {:?}", command);

        let output = command
            .output()
            .await
            .map_err(|e| SandboxError::ExecutionFailed(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

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

    #[test]
    fn test_size_precedes_tmpfs() {
        // Regression test: bwrap's `--size N` flag modifies the *next* tmpfs
        // mount. If `--size` lands after `--tmpfs /tmp`, the size limit is
        // silently dropped.
        let backend = LinuxBwrapBackend::new();
        let config = SandboxConfig::enabled();
        let args = backend.bwrap_args(Path::new("/tmp"), &config);
        let size_idx = args.iter().position(|a| a == "--size").expect("--size");
        let tmpfs_idx = args.iter().position(|a| a == "--tmpfs").expect("--tmpfs");
        assert!(
            size_idx < tmpfs_idx,
            "--size must precede --tmpfs to take effect (got size={size_idx}, tmpfs={tmpfs_idx})"
        );
        // And `--tmpfs` must be followed by `/tmp` so the size applies to it.
        assert_eq!(args.get(tmpfs_idx + 1).map(String::as_str), Some("/tmp"));
    }

    #[test]
    fn test_workspace_enforcement_blocks_outside_paths() {
        let backend = LinuxBwrapBackend::new();
        let workspace = std::env::temp_dir().join("gasket-workspace-test");
        std::fs::create_dir_all(&workspace).unwrap();
        let config = SandboxConfig::enabled().with_workspace(&workspace);
        // /etc isn't inside the workspace, must be rejected.
        let err = backend
            .build_command("echo test", Path::new("/etc"), &config)
            .unwrap_err();
        assert!(matches!(err, SandboxError::PathNotAllowed { .. }));
        std::fs::remove_dir_all(&workspace).ok();
    }
}
