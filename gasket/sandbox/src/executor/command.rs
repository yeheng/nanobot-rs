//! Command builder for sandbox execution
//!
//! ## Security Model
//!
//! This module implements a **defense-in-depth** approach to command execution:
//!
//! 1. **Advisory Pattern Check** (`check_dangerous_patterns`): A best-effort filter
//!    that catches common injection patterns. This is **NOT a security boundary**.
//!
//! 2. **Command Policy** (`validate_policy`): Allowlist/denylist for command filtering.
//!    Also advisory - can be bypassed with creative command construction.
//!
//! 3. **Sandbox Isolation**: The **real security boundary**. Commands run in an
//!    isolated environment (bwrap/sandbox-exec) with:
//!    - Filesystem isolation (read-only root, restricted write paths)
//!    - Resource limits (memory, CPU, processes)
//!    - Network isolation (optional)
//!
//! ## Why Pattern Checking is Insufficient
//!
//! Shell commands are Turing-complete. String-based filtering can be bypassed by:
//! - `$((arithmetic))` - arithmetic expansion
//! - `$(<file)` - file reading
//! - `{cmd,args}` - brace expansion
//! - Base64-encoded commands
//! - Unicode homoglyphs
//! - Environment variable injection
//!
//! Therefore, the pattern check is only meant to catch **accidental** misuse,
//! not malicious actors. Malicious commands should be contained by the sandbox.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{CommandPolicy, ResourceLimits};
use crate::error::{Result, SandboxError};

/// Command builder for constructing sandboxed commands
pub struct CommandBuilder {
    /// Command string
    command: String,
    /// Working directory
    working_dir: PathBuf,
    /// Environment variables
    env: Vec<(String, String)>,
    /// Resource limits
    limits: ResourceLimits,
    /// Whether to use sandbox
    use_sandbox: bool,
}

impl CommandBuilder {
    /// Create a new command builder
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            working_dir: PathBuf::from("."),
            env: Vec::new(),
            limits: ResourceLimits::default(),
            use_sandbox: false,
        }
    }

    /// Set working directory
    pub fn working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = dir.into();
        self
    }

    /// Add environment variable
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set resource limits
    pub fn limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Enable sandbox
    pub fn with_sandbox(mut self, enabled: bool) -> Self {
        self.use_sandbox = enabled;
        self
    }

    /// Build a std::process::Command for fallback execution
    pub fn build_fallback(&self) -> Command {
        let prefixed_cmd = format!("{}{}", self.limits.to_ulimit_prefix(), self.command);

        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(&prefixed_cmd)
            .current_dir(&self.working_dir);

        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        cmd
    }

    /// Get the command string
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Get the working directory
    pub fn get_working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// Get the resource limits
    pub fn get_limits(&self) -> &ResourceLimits {
        &self.limits
    }

    /// Validate the command against a policy
    pub fn validate_policy(&self, policy: &CommandPolicy) -> Result<()> {
        use crate::config::PolicyVerdict;

        match policy.check(&self.command) {
            PolicyVerdict::Allow => Ok(()),
            PolicyVerdict::Deny(reason) => Err(SandboxError::PolicyDenied(reason)),
        }
    }

    /// Check for common dangerous patterns in the command.
    ///
    /// # Security Warning
    ///
    /// **This is NOT a security boundary.** This check only catches obvious
    /// injection patterns like `;`, `&&`, `||`, `$()`, etc. It can be trivially
    /// bypassed by sophisticated attackers.
    ///
    /// The actual security is provided by:
    /// 1. Sandbox isolation (bwrap/sandbox-exec)
    /// 2. Resource limits
    /// 3. Filesystem restrictions
    ///
    /// This check exists solely to catch accidental misuse and provide
    /// defense-in-depth, not to prevent malicious command injection.
    ///
    /// # Known Bypasses
    ///
    /// - `$((1+1))` - arithmetic expansion
    /// - `$(<file)` - file reading
    /// - `{echo,hello}` - brace expansion
    /// - Encoded payloads (base64, hex, etc.)
    /// - Environment variable manipulation
    /// - Unicode homoglyph attacks
    pub fn check_dangerous_patterns(&self) -> Result<()> {
        // Common shell metacharacters that enable command chaining/injection
        // This list is intentionally simple - see security warning above
        const DANGEROUS_PATTERNS: &[&str] =
            &[";", "&&", "||", "`", "$(", "${", ">", ">>", "|", "\n", "\r"];

        for pattern in DANGEROUS_PATTERNS {
            if self.command.contains(pattern) {
                return Err(SandboxError::InvalidCommand(format!(
                    "Potentially unsafe pattern detected: '{}'. \
                     Command chaining is not allowed. \
                     Note: This check is advisory only and can be bypassed. \
                     The sandbox provides the actual security boundary.",
                    pattern
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_builder() {
        let cmd = CommandBuilder::new("ls -la")
            .working_dir("/tmp")
            .env("FOO", "bar")
            .limits(ResourceLimits::from_mb(512, 60, 1_048_576));

        assert_eq!(cmd.command(), "ls -la");
        assert_eq!(cmd.get_working_dir(), Path::new("/tmp"));
    }

    #[test]
    fn test_build_fallback() {
        let builder = CommandBuilder::new("echo hello").working_dir("/tmp");
        let cmd = builder.build_fallback();

        assert_eq!(cmd.get_program(), "bash");
    }

    #[test]
    fn test_dangerous_patterns() {
        let builder = CommandBuilder::new("echo hello; rm -rf /");
        let result = builder.check_dangerous_patterns();
        assert!(result.is_err());

        let builder = CommandBuilder::new("echo hello");
        let result = builder.check_dangerous_patterns();
        assert!(result.is_ok());
    }
}
