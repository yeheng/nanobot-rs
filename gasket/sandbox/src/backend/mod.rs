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
use crate::error::{Result, SandboxError};

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

/// Honest classification of what a backend actually enforces.
///
/// The crate previously returned `bool is_sandboxed()` and let callers
/// figure out the difference between bwrap-namespaces and `sh -c`. That is
/// the wrong abstraction: "sandbox" means radically different things on
/// different backends. Callers should branch on this enum instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IsolationLevel {
    /// No isolation. Equivalent to calling `Command::new()` directly.
    /// `FallbackBackend` returns this. NEVER lie about this.
    None,
    /// Resource limits only (CPU/memory/wall-clock). No filesystem or
    /// network isolation. Windows Job Objects sit here.
    ResourceLimits,
    /// Filesystem isolation via a security profile (macOS sandbox-exec
    /// Seatbelt). Stronger than `ResourceLimits` but enforcement is
    /// best-effort and the Apple API is deprecated.
    SeatbeltProfile,
    /// Full namespace isolation (mount/pid/ipc/net). Linux bwrap sits here.
    Namespaces,
}

impl IsolationLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            IsolationLevel::None => "none",
            IsolationLevel::ResourceLimits => "resource_limits",
            IsolationLevel::SeatbeltProfile => "seatbelt_profile",
            IsolationLevel::Namespaces => "namespaces",
        }
    }

    /// Whether this level actually constrains filesystem access.
    pub fn isolates_filesystem(self) -> bool {
        matches!(
            self,
            IsolationLevel::SeatbeltProfile | IsolationLevel::Namespaces
        )
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

    /// What kind of isolation this backend actually provides.
    ///
    /// Default implementation returns `None` so any backend that does NOT
    /// override this is honestly labelled as "no isolation".
    fn isolation_level(&self) -> IsolationLevel {
        IsolationLevel::None
    }

    /// Whether this backend provides true filesystem isolation.
    ///
    /// Default derives from `isolation_level()`. Overriding individually is
    /// allowed but discouraged — keep the two methods consistent.
    fn provides_filesystem_isolation(&self) -> bool {
        self.isolation_level().isolates_filesystem()
    }

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
///
/// Returns an error when the requested backend is unavailable on this platform,
/// instead of silently falling back to an unsandboxed executor. Pass
/// `config.enabled = false` if you explicitly want unsandboxed execution; the
/// crate will not pick that for you behind your back.
///
/// Linus rule: never lie about isolation. If the user said "sandbox", and we
/// can't deliver, we error out so the caller can decide whether to abort.
pub fn create_backend(config: &SandboxConfig) -> Result<Box<dyn SandboxBackend>> {
    if !config.enabled {
        return Ok(Box::new(FallbackBackend::new()));
    }

    let platform = Platform::current();
    let backend_name_lower = config.backend.to_lowercase();

    // Resolve "auto" to a concrete name. On Windows there is no real sandbox,
    // so "auto" maps to host-executor + a loud warning — never silently.
    let backend_name = if backend_name_lower == "auto" {
        match platform {
            Platform::Linux => "bwrap",
            Platform::MacOS => "sandbox-exec",
            Platform::Windows => {
                tracing::warn!(
                    "Windows has no true sandbox backend; using host-executor with Job Object \
                     resource limits only. For real isolation, run gasket inside WSL2."
                );
                "host-executor"
            }
        }
    } else {
        backend_name_lower.as_str()
    };

    match backend_name {
        "fallback" => Ok(Box::new(FallbackBackend::new())),

        #[cfg(target_os = "linux")]
        "bwrap" => Ok(Box::new(LinuxBwrapBackend::new())),
        #[cfg(not(target_os = "linux"))]
        "bwrap" => Err(SandboxError::ConfigError(format!(
            "backend 'bwrap' is only available on Linux (current platform: {}); \
             set `sandbox.backend = \"auto\"` or `sandbox.enabled = false`",
            platform.as_str()
        ))),

        #[cfg(target_os = "macos")]
        "sandbox-exec" => Ok(Box::new(MacOsSandboxBackend::new())),
        #[cfg(not(target_os = "macos"))]
        "sandbox-exec" => Err(SandboxError::ConfigError(format!(
            "backend 'sandbox-exec' is only available on macOS (current platform: {}); \
             set `sandbox.backend = \"auto\"` or `sandbox.enabled = false`",
            platform.as_str()
        ))),

        #[cfg(target_os = "windows")]
        "job-objects" | "windows-fallback" | "host-executor" | "unsafe-direct" => {
            Ok(Box::new(HostExecutor::new()))
        }
        #[cfg(not(target_os = "windows"))]
        "job-objects" | "windows-fallback" | "host-executor" | "unsafe-direct" => {
            Err(SandboxError::ConfigError(format!(
                "backend '{}' is only available on Windows (current platform: {}); \
                 set `sandbox.backend = \"auto\"` or `sandbox.enabled = false`",
                backend_name,
                platform.as_str()
            )))
        }

        // Unknown name → loud error.
        _ => Err(SandboxError::ConfigError(format!(
            "unknown sandbox backend '{}'; expected one of: auto, fallback, bwrap, sandbox-exec, \
             host-executor",
            backend_name
        ))),
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
    backends.push("host-executor"); // NOT a real sandbox, just cmd.exe with Job Objects

    backends
}
