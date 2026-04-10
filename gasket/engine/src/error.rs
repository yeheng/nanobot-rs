//! Error types for gasket engine public APIs
//!
//! Defines `AgentError`, `ChannelError`, `PipelineError`, and re-exports
//! `ProviderError` from `gasket_providers`. Uses `thiserror` for structured
//! error handling. `anyhow::Error` is only used internally.

use thiserror::Error;

/// Errors that can occur during agent processing
#[derive(Debug, Error)]
pub enum AgentError {
    /// Error from the LLM provider
    #[error("LLM provider error: {0}")]
    ProviderError(#[from] ProviderError),

    /// Error during tool execution
    #[error("Tool execution error: {0}")]
    ToolError(String),

    /// Error during session management
    #[error("Session error: {0}")]
    SessionError(String),

    /// Error during context preparation
    #[error("Context preparation error: {0}")]
    ContextError(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// I/O error
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Hook execution error
    #[error("Hook '{name}' failed: {message}")]
    HookFailed { name: String, message: String },

    /// Request aborted by hook
    #[error("Request aborted by hook: {0}")]
    AbortedByHook(String),

    /// Generic error with message
    #[error("{0}")]
    Other(String),

    /// Internal error preserving the full error chain
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

/// Errors from LLM providers — canonical definition lives in `gasket_providers`.
pub use gasket_providers::ProviderError;

/// Errors from channel operations
#[derive(Debug, Error)]
pub enum ChannelError {
    /// Channel not configured
    #[error("Channel '{0}' not configured")]
    NotConfigured(String),

    /// Authentication failed
    #[error("Channel authentication failed: {0}")]
    AuthError(String),

    /// Send message error
    #[error("Failed to send message: {0}")]
    SendError(String),

    /// Receive message error
    #[error("Failed to receive message: {0}")]
    ReceiveError(String),

    /// Invalid message format
    #[error("Invalid message format: {0}")]
    InvalidFormat(String),

    /// Internal error preserving the full error chain
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

/// Configuration validation errors
#[derive(Debug, Error)]
pub enum ConfigValidationError {
    /// Provider not available (missing API key for non-local providers)
    #[error("Provider '{0}' is not available (missing API key)")]
    ProviderNotAvailable(String),

    /// Invalid channel configuration
    #[error("Channel '{0}' has invalid configuration: {1}")]
    InvalidChannelConfig(String, String),
}

/// Errors from the multi-agent pipeline subsystem
#[derive(Debug, Error)]
pub enum PipelineError {
    /// Pipeline is not enabled in config
    #[error("Pipeline is not enabled")]
    NotEnabled,

    /// Task not found
    #[error("Pipeline task not found: {0}")]
    TaskNotFound(String),

    /// Illegal state transition
    #[error("Invalid state transition from {from} to {to}")]
    InvalidTransition { from: String, to: String },

    /// Caller not allowed to delegate to target
    #[error("Permission denied: role '{caller}' cannot delegate to '{target}'")]
    PermissionDenied { caller: String, target: String },

    /// Too many review round-trips
    #[error("Review limit exceeded for task {0} (max {1})")]
    ReviewLimitExceeded(String, u32),

    /// Task stalled (no heartbeat within timeout)
    #[error("Stall detected for task {0}")]
    StallDetected(String),

    /// Persistence layer error
    #[error("Pipeline store error: {0}")]
    StoreError(String),
}

// ============================================================================
// From<anyhow::Error> — preserve full error chain via Internal variant
// ============================================================================

impl From<crate::kernel::KernelError> for AgentError {
    fn from(err: crate::kernel::KernelError) -> Self {
        AgentError::Other(err.to_string())
    }
}

impl From<anyhow::Error> for AgentError {
    fn from(err: anyhow::Error) -> Self {
        AgentError::Internal(err.into())
    }
}

impl From<anyhow::Error> for ChannelError {
    fn from(err: anyhow::Error) -> Self {
        ChannelError::Internal(err.into())
    }
}

impl From<gasket_channels::ChannelConfigError> for ConfigValidationError {
    fn from(err: gasket_channels::ChannelConfigError) -> Self {
        match err {
            gasket_channels::ChannelConfigError::InvalidChannelConfig(ch, msg) => {
                ConfigValidationError::InvalidChannelConfig(ch, msg)
            }
        }
    }
}
