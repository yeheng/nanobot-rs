//! Error types for nanobot-core public APIs
//!
//! This module defines specific error types using `thiserror` for better
//! error handling and API contracts. Library crates should NOT expose
//! `anyhow::Error` in their public APIs - it's only for internal use.

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

    /// Generic error with message
    #[error("{0}")]
    Other(String),
}

/// Errors from LLM providers
#[derive(Debug, Error)]
pub enum ProviderError {
    /// API authentication failed
    #[error("Authentication failed: {0}")]
    AuthError(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded: {0}")]
    RateLimitError(String),

    /// Invalid request
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Model not found
    #[error("Model not found: {0}")]
    ModelNotFound(String),

    /// Network error
    #[error("Network error: {0}")]
    NetworkError(String),

    /// API error with status code
    #[error("API error (status {status_code}): {message}")]
    ApiError { status_code: u16, message: String },

    /// Response parsing error
    #[error("Failed to parse response: {0}")]
    ParseError(String),

    /// Generic provider error
    #[error("{0}")]
    Other(String),
}

/// Errors from MCP (Model Context Protocol) operations
#[derive(Debug, Error)]
pub enum McpError {
    /// Server not found
    #[error("MCP server '{0}' not found")]
    ServerNotFound(String),

    /// Connection error
    #[error("Failed to connect to MCP server: {0}")]
    ConnectionError(String),

    /// Tool call error
    #[error("MCP tool call error: {0}")]
    ToolCallError(String),

    /// Timeout error
    #[error("MCP operation timed out: {0}")]
    TimeoutError(String),

    /// JSON-RPC error
    #[error("JSON-RPC error (code {code}): {message}")]
    JsonRpcError { code: i64, message: String },
}

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
}

// Implement From<anyhow::Error> for conversion at module boundaries
impl From<anyhow::Error> for AgentError {
    fn from(err: anyhow::Error) -> Self {
        AgentError::Other(err.to_string())
    }
}

impl From<anyhow::Error> for ProviderError {
    fn from(err: anyhow::Error) -> Self {
        ProviderError::Other(err.to_string())
    }
}

impl From<anyhow::Error> for McpError {
    fn from(err: anyhow::Error) -> Self {
        McpError::ToolCallError(err.to_string())
    }
}

impl From<anyhow::Error> for ChannelError {
    fn from(err: anyhow::Error) -> Self {
        ChannelError::SendError(err.to_string())
    }
}
