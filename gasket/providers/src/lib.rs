//! LLM Provider abstractions and implementations for gasket
//!
//! All chat providers (except Copilot) are backed by the generic
//! `RigCompletionProvider<C>` which wraps any rig `CompletionClient`.
//! Vendor-specific workarounds are injected via normalizer hooks or thin
//! wrapper modules:
//! - Anthropic / Gemini / OpenAI-compatible: pure `RigCompletionProvider`
//! - Minimax: message normalization (system→user, merge consecutive, sanitize)
//! - Moonshot: runtime OpenAI / Anthropic format switching
//! - Copilot: retains its own module for OAuth token management

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
mod logging_http;
mod model_spec;
pub mod rig_bridge;
pub mod rig_provider;
mod vendor_workarounds;
pub use rig_provider::RigCompletionProvider;

// Re-export base types
pub use base::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, ChatStreamChunk, ChatStreamDelta,
    FinishReason, FunctionCall, FunctionDefinition, LlmProvider, MessageRole, ModelLimits,
    ThinkingConfig, ToolCall, ToolCallDelta, ToolDefinition, Usage,
};

// Re-export common types
pub use common::{
    build_http_client, build_provider, parse_json_args, ModelConfig, OpenAICompatibleProvider,
    ProviderBuildError, ProviderConfig, ProviderResult, ProviderType,
};

// Re-export specialized providers
#[cfg(feature = "provider-copilot")]
pub use copilot::CopilotProvider;
pub use vendor_workarounds::{
    build_anthropic_provider, build_gemini_provider, build_minimax_provider,
    MoonshotProvider,
};

// Re-export model spec
pub use model_spec::ModelSpec;
