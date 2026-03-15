//! LLM Provider system
//!
//! All OpenAI-compatible providers (OpenAI, DashScope, Moonshot, Zhipu, MiniMax)
//! are handled by `OpenAICompatibleProvider` with vendor-specific constructors.
//! Only providers with genuinely different API formats (DeepSeek for reasoning_content,
//! Gemini for native Google format, Copilot for OAuth token management) retain
//! their own modules.

mod base;
mod common;
mod copilot;
mod copilot_oauth;
mod gemini;
mod model_spec;
pub mod registry;
pub mod streaming;

pub use base::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, ChatStreamChunk, ChatStreamDelta,
    FinishReason, LlmProvider, MessageRole, ThinkingConfig, ToolCall, ToolCallDelta,
    ToolDefinition, Usage,
};
pub use common::{
    parse_json_args, OpenAICompatibleProvider, ProviderConfig, ProviderError, ProviderResult,
};
pub use copilot::CopilotProvider;
pub use copilot_oauth::{CopilotOAuth, CopilotTokenResponse, DeviceCodeResponse};
pub use gemini::GeminiProvider;
pub use model_spec::ModelSpec;
pub use registry::ProviderRegistry;
