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
    /// Get the current platform
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
            panic!("Unsupported platform")
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

/// Create the appropriate sandbox backend based on configuration and platform.
pub fn create_backend(config: &SandboxConfig) -> Box<dyn SandboxBackend> {
    if !config.enabled {
        return Box::new(FallbackBackend::new());
    }

    let platform = Platform::current();
    let backend_name = config.backend.to_lowercase();

    // Handle "auto" backend selection
    let backend_name = if backend_name == "auto" {
        match platform {
            Platform::Linux => "bwrap",
            Platform::MacOS => "sandbox-exec",
            Platform::Windows => {
                panic!(
                    "Windows does not have a built-in sandbox backend. \
                     Please explicitly set `backend = \"unsafe-direct\"` (or \"host-executor\") \
                     in your sandbox configuration to run commands without isolation."
                )
            }
        }
    } else {
        &backend_name
    };

    // All known backend names across platforms
    const KNOWN_BACKENDS: &[&str] = &[
        "fallback",
        "bwrap",
        "sandbox-exec",
        "job-objects",
        "windows-fallback",
        "unsafe-direct",
    ];

    match backend_name {
        "fallback" => Box::new(FallbackBackend::new()),
        #[cfg(target_os = "linux")]
        "bwrap" => Box::new(LinuxBwrapBackend::new()),
        #[cfg(target_os = "macos")]
        "sandbox-exec" => Box::new(MacOsSandboxBackend::new()),
        #[cfg(target_os = "windows")]
        "job-objects" | "windows-fallback" | "host-executor" | "unsafe-direct" => {
            Box::new(HostExecutor::new())
        }
        name if KNOWN_BACKENDS.contains(&name) => {
            tracing::warn!(
                "Backend '{}' is not available on {}, using platform default instead",
                backend_name,
                platform.as_str()
            );
            match platform {
                #[cfg(target_os = "linux")]
                Platform::Linux => Box::new(LinuxBwrapBackend::new()),
                #[cfg(target_os = "macos")]
                Platform::MacOS => Box::new(MacOsSandboxBackend::new()),
                #[cfg(target_os = "windows")]
                Platform::Windows => Box::new(HostExecutor::new()),
                _ => Box::new(FallbackBackend::new()),
            }
        }
        _ => {
            tracing::warn!(
                "Unknown backend '{}', falling back to unsandboxed execution",
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
