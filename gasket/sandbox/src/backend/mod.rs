//! Sandbox backend abstraction
//!
//! Provides a trait-based abstraction for different sandbox backends,
//! supporting multiple platforms and isolation levels.

mod fallback;
mod platform;

pub use fallback::FallbackBackend;
pub use platform::*;

use std::path::Path;
use std::process::Command;

use async_trait::async_trait;

use crate::config::SandboxConfig;
use crate::error::Result;

/// Platform enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// Linux
    Linux,
    /// macOS
    MacOS,
    /// Windows
    Windows,
}

impl Platform {
    /// Get the current platform. On unsupported targets this returns
    /// `Platform::Linux` as a best-effort default — combined with the
    /// `FallbackBackend` selection in `create_backend`, that lets the crate
    /// still compile and load on BSDs/illumos without crashing the host.
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Platform::Linux
        }
        #[cfg(target_os = "macos")]
        {
            Platform::MacOS
        }
        #[cfg(target_os = "windows")]
        {
            Platform::Windows
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Platform::Linux
        }
    }

    /// Get platform name as string
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::MacOS => "macos",
            Platform::Windows => "windows",
        }
    }
}

/// Execution result from sandbox backend
///
/// This is a re-export of `executor::ExecutionResult` to avoid duplication.
/// All backends should use this type for consistency.
pub use crate::executor::ExecutionResult;

/// Sandbox backend trait
///
/// Defines the interface for sandbox execution backends.
/// Each backend implements platform-specific isolation mechanisms.
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    /// Get the backend name
    fn name(&self) -> &str;

    /// Check if the backend is available on this system
    async fn is_available(&self) -> bool;

    /// Get supported platforms
    fn supported_platforms(&self) -> &[Platform];

    /// Whether this backend provides true filesystem isolation.
    ///
    /// Used by callers to decide whether redirection patterns (`>`, `<`)
    /// need to be blocked at the command-policy level.
    fn provides_filesystem_isolation(&self) -> bool;

    /// Build a Command for execution
    fn build_command(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<Command>;

    /// Execute a command in the sandbox
    async fn execute(
        &self,
        cmd: &str,
        working_dir: &Path,
        config: &SandboxConfig,
    ) -> Result<ExecutionResult>;
}

/// Build the platform-default sandbox backend, falling back to
/// `FallbackBackend` when the native backend isn't installed (so misconfigured
/// hosts surface a warning instead of crashing at execution time).
fn platform_default_backend() -> Box<dyn SandboxBackend> {
    #[cfg(target_os = "linux")]
    {
        let backend = LinuxBwrapBackend::new();
        if backend.is_installed() {
            return Box::new(backend);
        }
        tracing::warn!("bwrap not installed; using fallback (unsandboxed) backend");
        Box::new(FallbackBackend::new())
    }
    #[cfg(target_os = "macos")]
    {
        let backend = MacOsSandboxBackend::new();
        if backend.is_installed() {
            return Box::new(backend);
        }
        tracing::warn!("sandbox-exec not found; using fallback (unsandboxed) backend");
        Box::new(FallbackBackend::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(HostExecutor::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Box::new(FallbackBackend::new())
    }
}

/// Create the appropriate sandbox backend based on configuration and platform.
///
/// This function is infallible: unknown backend names, unavailable native
/// backends, and misconfigured platform/backend combinations all degrade to
/// `FallbackBackend` with a warning. (Previously, Windows + `backend = "auto"`
/// would `panic!` at startup.)
pub fn create_backend(config: &SandboxConfig) -> Box<dyn SandboxBackend> {
    if !config.enabled {
        return Box::new(FallbackBackend::new());
    }

    let platform = Platform::current();
    let backend_name = config.backend.to_lowercase();

    // "auto" → platform default (fallback if native backend isn't installed).
    if backend_name == "auto" {
        return platform_default_backend();
    }

    // All known backend names across platforms
    const KNOWN_BACKENDS: &[&str] = &[
        "fallback",
        "bwrap",
        "sandbox-exec",
        "job-objects",
        "windows-fallback",
        "host-executor",
        "unsafe-direct",
    ];

    match backend_name.as_str() {
        "fallback" => Box::new(FallbackBackend::new()),
        #[cfg(target_os = "linux")]
        "bwrap" => {
            let backend = LinuxBwrapBackend::new();
            if backend.is_installed() {
                Box::new(backend)
            } else {
                tracing::warn!("bwrap not installed; using fallback backend");
                Box::new(FallbackBackend::new())
            }
        }
        #[cfg(target_os = "macos")]
        "sandbox-exec" => {
            let backend = MacOsSandboxBackend::new();
            if backend.is_installed() {
                Box::new(backend)
            } else {
                tracing::warn!("sandbox-exec not found; using fallback backend");
                Box::new(FallbackBackend::new())
            }
        }
        #[cfg(target_os = "windows")]
        "job-objects" | "windows-fallback" | "host-executor" | "unsafe-direct" => {
            Box::new(HostExecutor::new())
        }
        name if KNOWN_BACKENDS.contains(&name) => {
            tracing::warn!(
                "Backend '{}' is not available on {}; using platform default instead",
                backend_name,
                platform.as_str()
            );
            platform_default_backend()
        }
        _ => {
            tracing::warn!(
                "Unknown backend '{}'; falling back to unsandboxed execution",
                backend_name
            );
            Box::new(FallbackBackend::new())
        }
    }
}

/// Get list of available backends on current platform
pub fn available_backends() -> Vec<&'static str> {
    let mut backends = vec!["fallback"];

    #[cfg(target_os = "linux")]
    backends.push("bwrap");

    #[cfg(target_os = "macos")]
    backends.push("sandbox-exec");

    #[cfg(target_os = "windows")]
    backends.push("unsafe-direct"); // NOT a real sandbox, just cmd.exe

    backends
}
