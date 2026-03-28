//! LLM Provider system
//!
//! This module re-exports types from the `gasket-providers` crate.

pub use gasket_providers::{
    build_http_client, parse_json_args, streaming, ChatMessage, ChatRequest, ChatResponse,
    ChatStream, ChatStreamChunk, ChatStreamDelta, FinishReason, FunctionCall, FunctionDefinition,
    LlmProvider, MessageRole, ModelSpec, OpenAICompatibleProvider, ProviderBuildError,
    ProviderConfig, ProviderResult, ThinkingConfig, ToolCall, ToolCallDelta, ToolDefinition, Usage,
};

#[cfg(feature = "provider-gemini")]
pub use gasket_providers::GeminiProvider;
#[cfg(feature = "provider-copilot")]
pub use gasket_providers::{
    CopilotOAuth, CopilotProvider, CopilotTokenResponse, DeviceCodeResponse,
};

// Re-export ProviderRegistry from config module for backward compatibility
pub use crate::config::ProviderRegistry;
