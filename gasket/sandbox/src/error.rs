//! Error types for nanobot-sandbox
//!
//! Provides comprehensive error handling for sandbox operations,
//! backend failures, approval system, and audit logging.

use std::path::PathBuf;
use thiserror::Error;

/// Main error type for sandbox operations
#[derive(Debug, Error)]
pub enum SandboxError {
    /// Backend initialization failed
    #[error("Failed to initialize sandbox backend '{backend}': {reason}")]
    BackendInit { backend: String, reason: String },

    /// Backend not available on current platform
    #[error("Backend '{backend}' is not available on {platform}")]
    BackendUnavailable { backend: String, platform: String },

    /// Command execution failed
    #[error("Command execution failed: {0}")]
    ExecutionFailed(String),

    /// Command timed out
    #[error("Command timed out after {timeout_secs} seconds")]
    Timeout { timeout_secs: u64 },

    /// Resource limit exceeded
    #[error("Resource limit exceeded: {resource} (limit: {limit}, actual: {actual})")]
    ResourceLimitExceeded {
        resource: String,
        limit: u64,
        actual: u64,
    },

    /// Output size exceeded
    #[error("Output size exceeded limit of {limit} bytes")]
    OutputLimitExceeded { limit: usize },

    /// Command denied by policy
    #[error("Command denied by policy: {0}")]
    PolicyDenied(String),

    /// Permission denied by approval system
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// Approval request failed
    #[error("Approval request failed: {0}")]
    ApprovalFailed(String),

    /// Approval timeout (user did not respond)
    #[error("Approval request timed out after {timeout_secs} seconds")]
    ApprovalTimeout { timeout_secs: u64 },

    /// Permission store error
    #[error("Permission store error: {0}")]
    StoreError(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Path not allowed
    #[error("Path access denied: {path} is not in allowed directories")]
    PathNotAllowed { path: PathBuf },

    /// Invalid command
    #[error("Invalid command: {0}")]
    InvalidCommand(String),

    /// Audit log error
    #[error("Audit log error: {0}")]
    AuditError(String),
}

/// Result type alias for sandbox operations
pub type Result<T> = std::result::Result<T, SandboxError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = SandboxError::BackendInit {
            backend: "bwrap".to_string(),
            reason: "not found".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Failed to initialize sandbox backend 'bwrap': not found"
        );

        let err = SandboxError::Timeout { timeout_secs: 60 };
        assert_eq!(err.to_string(), "Command timed out after 60 seconds");
    }
}
