//! Platform-specific sandbox implementations
//!
//! This module re-exports platform-specific backends based on the target OS.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::LinuxBwrapBackend;

#[cfg(target_os = "macos")]
pub use macos::MacOsSandboxBackend;

#[cfg(target_os = "windows")]
pub use windows::HostExecutor;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::config::SandboxConfig;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::error::{Result, SandboxError};

/// Canonicalize `working_dir` and, when a `workspace` is configured on the
/// `SandboxConfig`, enforce that `working_dir` is contained within it.
///
/// Returns the canonicalized path on success. Used by Linux/macOS backends so
/// callers cannot pass arbitrary host paths through the sandbox.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn validate_workspace(working_dir: &Path, config: &SandboxConfig) -> Result<PathBuf> {
    if !working_dir.exists() || !working_dir.is_dir() {
        return Err(SandboxError::PathNotAllowed {
            path: working_dir.to_path_buf(),
        });
    }

    let canonical = working_dir
        .canonicalize()
        .map_err(|_| SandboxError::PathNotAllowed {
            path: working_dir.to_path_buf(),
        })?;

    if let Some(ref workspace) = config.workspace {
        let ws_canonical = workspace
            .canonicalize()
            .map_err(|_| SandboxError::PathNotAllowed {
                path: workspace.clone(),
            })?;
        if !canonical.starts_with(&ws_canonical) {
            tracing::warn!(
                "working_dir {:?} is outside workspace {:?}; blocking",
                canonical,
                ws_canonical
            );
            return Err(SandboxError::PathNotAllowed { path: canonical });
        }
    }

    Ok(canonical)
}
