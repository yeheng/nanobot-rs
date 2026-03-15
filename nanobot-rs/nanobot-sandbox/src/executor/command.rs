//! Command builder for sandbox execution

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

    /// Check for dangerous patterns in the command
    pub fn check_dangerous_patterns(&self) -> Result<()> {
        const DANGEROUS_PATTERNS: &[&str] =
            &[";", "&&", "||", "`", "$(", "${", ">", ">>", "|", "\n", "\r"];

        for pattern in DANGEROUS_PATTERNS {
            if self.command.contains(pattern) {
                return Err(SandboxError::InvalidCommand(format!(
                    "Potentially unsafe pattern detected: '{}'. Command injection is not allowed.",
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
