//! Process manager for command execution

use std::path::Path;
use std::time::{Duration, Instant};

use tracing::instrument;

use super::{CommandBuilder, ExecutionResult};
use crate::backend::{create_backend, SandboxBackend};
use crate::config::{CommandPolicy, ResourceLimits, SandboxConfig};
use crate::error::{Result, SandboxError};

/// Process manager for executing commands
pub struct ProcessManager {
    /// Sandbox configuration
    config: SandboxConfig,
    /// Command policy
    policy: CommandPolicy,
    /// Sandbox backend
    backend: Box<dyn SandboxBackend>,
    /// Default timeout
    timeout: Duration,
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new(config: SandboxConfig) -> Self {
        let backend = create_backend(&config);
        let policy = CommandPolicy::from_config(&config.policy);
        let timeout = Duration::from_secs(120); // Default 2 minutes

        Self {
            config,
            policy,
            backend,
            timeout,
        }
    }

    /// Create with custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Execute a command
    #[instrument(name = "process.execute", skip(self))]
    pub async fn execute(&self, command: &str, working_dir: &Path) -> Result<ExecutionResult> {
        // Create command builder
        let builder = CommandBuilder::new(command)
            .working_dir(working_dir)
            .limits(ResourceLimits::from(&self.config.limits));

        // Validate dangerous patterns
        builder.check_dangerous_patterns()?;

        // Check policy
        builder.validate_policy(&self.policy)?;

        // Execute with backend
        let start = Instant::now();
        let result = self
            .backend
            .execute(command, working_dir, &self.config)
            .await?;
        let duration = start.elapsed();

        // Convert backend result to executor result
        Ok(ExecutionResult {
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
            timed_out: result.timed_out,
            resource_exceeded: result.resource_exceeded,
            duration_ms: duration.as_millis() as u64,
        })
    }

    /// Execute with timeout
    pub async fn execute_with_timeout(
        &self,
        command: &str,
        working_dir: &Path,
        timeout: Duration,
    ) -> Result<ExecutionResult> {
        let result = tokio::time::timeout(timeout, self.execute(command, working_dir))
            .await
            .map_err(|_| SandboxError::Timeout {
                timeout_secs: timeout.as_secs(),
            })??;

        Ok(result)
    }

    /// Execute a command builder
    pub async fn execute_builder(&self, builder: &CommandBuilder) -> Result<ExecutionResult> {
        // Validate
        builder.check_dangerous_patterns()?;
        builder.validate_policy(&self.policy)?;

        let start = Instant::now();
        let result = self
            .backend
            .execute(builder.command(), builder.get_working_dir(), &self.config)
            .await?;
        let duration = start.elapsed();

        Ok(ExecutionResult {
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
            timed_out: result.timed_out,
            resource_exceeded: result.resource_exceeded,
            duration_ms: duration.as_millis() as u64,
        })
    }

    /// Get the backend name
    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    /// Check if sandbox is enabled
    pub fn is_sandboxed(&self) -> bool {
        self.config.enabled
    }

    /// Get the command policy
    pub fn policy(&self) -> &CommandPolicy {
        &self.policy
    }

    /// Get the configuration
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_process_manager_fallback() {
        let config = SandboxConfig::fallback();
        let manager = ProcessManager::new(config);

        assert_eq!(manager.backend_name(), "fallback");
        assert!(!manager.is_sandboxed());
    }

    #[tokio::test]
    async fn test_execute_simple_command() {
        let config = SandboxConfig::fallback();
        let manager = ProcessManager::new(config);

        let result = manager.execute("echo hello", Path::new("/tmp")).await;
        assert!(result.is_ok());

        let result = result.unwrap();
        assert!(result.is_success());
        assert!(result.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_with_timeout() {
        let config = SandboxConfig::fallback();
        let manager = ProcessManager::new(config);

        let result = manager
            .execute_with_timeout("sleep 10", Path::new("/tmp"), Duration::from_millis(100))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SandboxError::Timeout { .. }));
    }
}
