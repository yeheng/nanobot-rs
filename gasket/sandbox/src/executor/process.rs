//! Process manager for command execution

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::instrument;

use super::ExecutionResult;
use crate::backend::{create_backend, SandboxBackend};
use crate::config::{CommandPolicy, PolicyVerdict, SandboxConfig};
use crate::error::{Result, SandboxError};

#[cfg(feature = "approval")]
use crate::approval::{ApprovalManager, ExecutionContext, OperationType};

#[cfg(feature = "audit")]
use crate::audit::AuditLog;

/// Process manager for executing commands
pub struct ProcessManager {
    config: SandboxConfig,
    policy: CommandPolicy,
    backend: Box<dyn SandboxBackend>,
    timeout: Duration,
    #[cfg(feature = "approval")]
    approval: Option<Arc<ApprovalManager>>,
    #[cfg(feature = "audit")]
    audit: Option<Arc<AuditLog>>,
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new(config: SandboxConfig) -> Self {
        let backend = create_backend(&config);
        let policy = CommandPolicy::from_config(&config.policy);
        let timeout = Duration::from_secs(120);

        Self {
            config,
            policy,
            backend,
            timeout,
            #[cfg(feature = "approval")]
            approval: None,
            #[cfg(feature = "audit")]
            audit: None,
        }
    }

    /// Create with custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Execute a command. The configured wall-clock `timeout` is always
    /// enforced; the backend future is dropped on expiry and `kill_on_drop`
    /// terminates the child.
    #[instrument(name = "process.execute", skip(self))]
    pub async fn execute(&self, command: &str, working_dir: &Path) -> Result<ExecutionResult> {
        self.execute_with_timeout(command, working_dir, self.timeout)
            .await
    }

    /// Execute with an explicit timeout, overriding the manager default.
    pub async fn execute_with_timeout(
        &self,
        command: &str,
        working_dir: &Path,
        timeout: Duration,
    ) -> Result<ExecutionResult> {
        let builder = CommandBuilder::new(command)
            .working_dir(working_dir)
            .limits(ResourceLimits::from(&self.config.limits));
        builder.validate_policy(&self.policy)?;

        let start = Instant::now();
        let result = tokio::time::timeout(
            timeout,
            self.backend.execute(command, working_dir, &self.config),
        )
        .await
        .map_err(|_| SandboxError::Timeout {
            timeout_secs: timeout.as_secs(),
        })??;
        let duration = start.elapsed();

        Ok(ExecutionResult {
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
            timed_out: result.timed_out,
            resource_exceeded: result.resource_exceeded,
            duration_ms: duration.as_millis() as u64,
        };

        // Step 5: Audit — command end
        #[cfg(feature = "audit")]
        {
            if let Some(ref audit) = self.audit {
                let _ = audit
                    .log_command_end(
                        command,
                        exec_result.exit_code,
                        exec_result.duration_ms,
                        exec_result.timed_out,
                        None,
                    )
                    .await;
            }
        }

        Ok(exec_result)
    }

    /// Execute a command builder. The configured wall-clock timeout is enforced.
    pub async fn execute_builder(&self, builder: &CommandBuilder) -> Result<ExecutionResult> {
        builder.validate_policy(&self.policy)?;

        let start = Instant::now();
        let result = tokio::time::timeout(
            self.timeout,
            self.backend
                .execute(builder.command(), builder.get_working_dir(), &self.config),
        )
        .await
        .map_err(|_| SandboxError::Timeout {
            timeout_secs: self.timeout.as_secs(),
        })??;
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

    pub fn is_sandboxed(&self) -> bool {
        self.config.enabled
    }

    pub fn provides_filesystem_isolation(&self) -> bool {
        self.backend.provides_filesystem_isolation()
    }

    pub fn policy(&self) -> &CommandPolicy {
        &self.policy
    }

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
        assert!(matches!(result.unwrap_err(), SandboxError::Timeout { .. }));
    }
}
