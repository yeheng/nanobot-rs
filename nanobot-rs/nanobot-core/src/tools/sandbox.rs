//! Sandbox execution providers for shell commands.
//!
//! - `BwrapSandbox`: Uses bubblewrap for namespace-isolated execution (Linux only)
//! - `MacOsSandbox`: Uses sandbox-exec (Seatbelt) for filesystem isolation (macOS only)
//! - `FallbackExecutor`: Direct `bash -c` with ulimit prefix (all platforms)

use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{debug, info, warn};

use super::resource_limits::ResourceLimits;
use crate::config::SandboxConfig;

/// Bubblewrap-based sandbox (Linux only).
///
/// Mount layout:
/// - `/`         → bind-ro from host
/// - `workspace` → bind-rw from configured workspace
/// - `/tmp`      → tmpfs (size-limited)
/// - `/dev`      → minimal devtmpfs
/// - `/proc`     → new proc namespace
pub struct BwrapSandbox {
    bwrap_path: PathBuf,
    workspace: PathBuf,
    tmp_size_mb: u32,
}

impl BwrapSandbox {
    /// Detect bwrap binary and create a sandbox.
    /// Returns `None` if bwrap is not available.
    pub fn detect(workspace: &Path, config: &SandboxConfig) -> Option<Self> {
        let bwrap_path = which_bwrap()?;
        info!("bwrap detected at {:?}", bwrap_path);
        Some(Self {
            bwrap_path,
            workspace: workspace.to_path_buf(),
            tmp_size_mb: config.tmp_size_mb,
        })
    }

    fn build_command(&self, cmd: &str, _working_dir: &Path, limits: &ResourceLimits) -> Command {
        let mut command = Command::new(&self.bwrap_path);

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
            .arg(&self.workspace)
            .arg(&self.workspace)
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
            .arg(format!("{}", u64::from(self.tmp_size_mb) * 1024 * 1024));

        // Resource limits
        for arg in limits.to_bwrap_args() {
            command.arg(arg);
        }

        // Working directory inside sandbox
        command.arg("--chdir").arg(&self.workspace);

        // The actual command
        command.arg("bash").arg("-c").arg(cmd);

        debug!("bwrap command: {:?}", command);
        command
    }
}

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
#[cfg(target_os = "macos")]
pub struct MacOsSandbox {
    workspace: PathBuf,
    base_profile: String,
}

#[cfg(target_os = "macos")]
impl MacOsSandbox {
    /// Create a new macOS sandbox executor.
    /// Loads the base Seatbelt profile and adds workspace-specific rules.
    pub fn new(workspace: PathBuf) -> Self {
        let base_profile = load_base_profile();
        Self {
            workspace,
            base_profile,
        }
    }

    /// Generate a Seatbelt sandbox profile string.
    ///
    /// The profile allows all reads but restricts writes to:
    /// - The workspace directory
    /// - /tmp and /private/tmp
    /// - /dev/null and /dev/zero
    fn generate_profile(&self) -> String {
        let workspace = self.workspace.display();
        format!(
            r#"{}

; Workspace-specific write permissions
(allow file-write*
  (subpath "{}")
)

; Additional tmp paths
(allow file-write-data
  (require-all
    (path "/tmp")
    (vnode-type DIRECTORY)
  )
)
(allow file-write-data
  (require-all
    (path "/private/tmp")
    (vnode-type DIRECTORY)
  )
)
"#,
            self.base_profile, workspace
        )
    }

    fn build_command(&self, cmd: &str, _working_dir: &Path, limits: &ResourceLimits) -> Command {
        let profile = self.generate_profile();

        // Resource limits via ulimit (sandbox-exec doesn't handle this)
        let prefixed_cmd = format!("{}{}", limits.to_ulimit_prefix(), cmd);

        let mut command = Command::new("sandbox-exec");
        command
            .arg("-p")
            .arg(profile)
            .arg("bash")
            .arg("-c")
            .arg(prefixed_cmd)
            .current_dir(&self.workspace);

        debug!("sandbox-exec command: {:?}", command);
        command
    }
}

/// Load the base Seatbelt profile from the embedded file.
#[cfg(target_os = "macos")]
fn load_base_profile() -> String {
    // Try to load from the same directory as the executable
    let mut path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    path.push("seatbelt_base_policy.sbpl");

    if path.exists() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            return content;
        }
    }

    // Fallback: try relative to current working directory
    let cwd_path = PathBuf::from("seatbelt_base_policy.sbpl");
    if cwd_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&cwd_path) {
            return content;
        }
    }

    // Fallback to hardcoded minimal profile if file not found
    String::from(
        r#"(version 1)
(deny default)
(allow file-read*)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow file-write-data
  (require-all
    (path "/dev/null")
    (vnode-type CHARACTER-DEVICE)))
"#,
    )
}

/// Fallback executor — direct `bash -c` with ulimit-based resource limits.
///
/// Used when bwrap is unavailable or sandbox is disabled.
pub struct FallbackExecutor;

impl FallbackExecutor {
    fn build_command(&self, cmd: &str, working_dir: &Path, limits: &ResourceLimits) -> Command {
        let prefixed_cmd = format!("{}{}", limits.to_ulimit_prefix(), cmd);

        let mut command = Command::new("bash");
        command.arg("-c").arg(prefixed_cmd).current_dir(working_dir);

        command
    }
}

/// Sandbox executor — statically dispatched enum replacing the old
/// `Box<dyn SandboxProvider>` dynamic dispatch.
///
/// Only two variants exist and are known at compile time, so an enum
/// eliminates the unnecessary heap allocation and vtable indirection.
pub enum SandboxExecutor {
    Bwrap(BwrapSandbox),
    #[cfg(target_os = "macos")]
    MacOs(MacOsSandbox),
    Fallback(FallbackExecutor),
}

impl SandboxExecutor {
    /// Build a `Command` that will execute `cmd` in the given `working_dir`
    /// with the specified `limits`.
    pub fn build_command(&self, cmd: &str, working_dir: &Path, limits: &ResourceLimits) -> Command {
        match self {
            Self::Bwrap(s) => s.build_command(cmd, working_dir, limits),
            #[cfg(target_os = "macos")]
            Self::MacOs(s) => s.build_command(cmd, working_dir, limits),
            Self::Fallback(s) => s.build_command(cmd, working_dir, limits),
        }
    }

    /// Human-readable name for logging.
    pub fn name(&self) -> &str {
        match self {
            Self::Bwrap(_) => "bwrap",
            #[cfg(target_os = "macos")]
            Self::MacOs(_) => "sandbox-exec",
            Self::Fallback(_) => "fallback",
        }
    }
}

/// Detect bwrap binary on the system.
fn which_bwrap() -> Option<PathBuf> {
    // Try common locations
    let candidates = ["/usr/bin/bwrap", "/usr/local/bin/bwrap"];
    for path in &candidates {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try `which` command
    let output = Command::new("which").arg("bwrap").output().ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }

    warn!("bwrap not found on system — sandbox unavailable, falling back to ulimit-based limits");
    None
}

/// Detect sandbox-exec binary on macOS.
#[cfg(target_os = "macos")]
fn which_sandbox_exec() -> Option<PathBuf> {
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

/// Create the appropriate sandbox executor based on configuration.
pub fn create_provider(workspace: &Path, config: &SandboxConfig) -> SandboxExecutor {
    if !config.enabled {
        debug!("Sandbox disabled by config");
        return SandboxExecutor::Fallback(FallbackExecutor);
    }

    // macOS: use sandbox-exec (Seatbelt)
    #[cfg(target_os = "macos")]
    {
        if which_sandbox_exec().is_some() {
            info!("Sandbox enabled: macOS sandbox-exec");
            return SandboxExecutor::MacOs(MacOsSandbox::new(workspace.to_path_buf()));
        }
        warn!("sandbox-exec not available — falling back to ulimit-based limits");
        SandboxExecutor::Fallback(FallbackExecutor)
    }

    // Linux: use bwrap
    #[cfg(not(target_os = "macos"))]
    {
        // Only bwrap backend is supported
        if config.backend != "bwrap" {
            warn!(
                "Unknown sandbox backend '{}', falling back to unsandboxed",
                config.backend
            );
            return SandboxExecutor::Fallback(FallbackExecutor);
        }

        match BwrapSandbox::detect(workspace, config) {
            Some(sandbox) => {
                info!("Sandbox enabled: bwrap at {:?}", sandbox.bwrap_path);
                SandboxExecutor::Bwrap(sandbox)
            }
            None => {
                warn!("bwrap not available — running without sandbox (ulimit-based limits only)");
                SandboxExecutor::Fallback(FallbackExecutor)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_executor_builds_command() {
        let executor = FallbackExecutor;
        let limits = ResourceLimits::default();
        let cmd = executor.build_command("echo hello", Path::new("/tmp"), &limits);

        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(cmd.get_program(), "bash");
        assert_eq!(args[0], "-c");
        // Second arg should contain ulimit prefix + actual command
        let full_cmd = args[1].to_string_lossy();
        assert!(full_cmd.contains("ulimit"));
        assert!(full_cmd.contains("echo hello"));
    }

    #[test]
    fn test_create_provider_disabled() {
        let config = SandboxConfig::default(); // enabled: false
        let provider = create_provider(Path::new("/tmp"), &config);
        assert_eq!(provider.name(), "fallback");
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn test_create_provider_unknown_backend() {
        // On macOS, backend config is ignored and sandbox-exec is always used
        let config = SandboxConfig {
            enabled: true,
            backend: "docker".to_string(),
            tmp_size_mb: 64,
        };
        let provider = create_provider(Path::new("/tmp"), &config);
        assert_eq!(provider.name(), "fallback");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_create_provider_macos_ignores_backend() {
        // On macOS, sandbox-exec is used regardless of backend config
        let config = SandboxConfig {
            enabled: true,
            backend: "docker".to_string(),
            tmp_size_mb: 64,
        };
        let provider = create_provider(Path::new("/tmp"), &config);
        assert_eq!(provider.name(), "sandbox-exec");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_sandbox_profile_generation() {
        let workspace = PathBuf::from("/Users/test/.nanobot");
        let sandbox = MacOsSandbox::new(workspace);
        let profile = sandbox.generate_profile();

        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(subpath \"/Users/test/.nanobot\")"));
        assert!(profile.contains("/tmp"));
        assert!(profile.contains("/private/tmp"));
        // Base profile should contain /dev/null permission
        assert!(profile.contains("/dev/null"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_sandbox_builds_command() {
        let workspace = PathBuf::from("/Users/test/.nanobot");
        let sandbox = MacOsSandbox::new(workspace);
        let limits = ResourceLimits::default();
        let cmd = sandbox.build_command("echo hello", Path::new("/tmp"), &limits);

        assert_eq!(cmd.get_program(), "sandbox-exec");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "-p");
        // Second arg should be the profile (starts with "(version 1)")
        let profile = args[1].to_string_lossy();
        assert!(profile.starts_with("(version 1)"));
        assert_eq!(args[2], "bash");
        assert_eq!(args[3], "-c");
    }
}
