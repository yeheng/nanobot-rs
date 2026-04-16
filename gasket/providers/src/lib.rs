//! LLM Provider abstractions and implementations for gasket
//!
//! All OpenAI-compatible providers (OpenAI, DashScope, Moonshot, Zhipu, MiniMax)
//! are handled by `OpenAICompatibleProvider` with vendor-specific constructors.
//! Only providers with genuinely different API formats (DeepSeek for reasoning_content,
//! Gemini for native Google format, Copilot for OAuth token management) retain
//! their own modules.

use thiserror::Error;

/// Structured error type for LLM provider operations.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("Authentication failed: {0}")]
    AuthError(String),
    #[error("Rate limit exceeded: {0}")]
    RateLimitError(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("API error (status {status_code}): {message}")]
    ApiError { status_code: u16, message: String },
    #[error("Failed to parse response: {0}")]
    ParseError(String),
    #[error("{0}")]
    Other(String),

    /// Internal error preserving the full error chain
    #[error(transparent)]
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

impl From<anyhow::Error> for ProviderError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err.into())
    }
}

impl ProviderError {
    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::ApiError { status_code, .. } => Some(*status_code),
            _ => None,
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimitError(_) | Self::NetworkError(_) => true,
            Self::ApiError { status_code, .. } => *status_code == 429 || *status_code >= 500,
            _ => false,
        }
    }
}

mod base;
mod common;
#[cfg(feature = "provider-copilot")]
mod copilot;
#[cfg(feature = "provider-copilot")]
mod copilot_oauth;
#[cfg(feature = "provider-gemini")]
mod gemini;
mod model_spec;
pub mod streaming;

// Re-export base types
pub use base::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, ChatStreamChunk, ChatStreamDelta,
    FinishReason, FunctionCall, FunctionDefinition, LlmProvider, MessageRole, ThinkingConfig,
    ToolCall, ToolCallDelta, ToolDefinition, Usage,
};

// Re-export common types
pub use common::{
    build_http_client, parse_json_args, OpenAICompatibleProvider, ProviderBuildError,
    ProviderConfig, ProviderResult,
};

// Re-export specialized providers
#[cfg(feature = "provider-copilot")]
pub use copilot::CopilotProvider;
#[cfg(feature = "provider-copilot")]
pub use copilot_oauth::{
    CopilotOAuth, CopilotTokenResponse, DeviceCodeResponse, DEFAULT_CLIENT_ID as COPILOT_DEFAULT_CLIENT_ID,
};
#[cfg(feature = "provider-gemini")]
pub use gemini::GeminiProvider;

// Re-export model spec
pub use model_spec::ModelSpec;
