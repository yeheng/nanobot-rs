//! LLM Provider system
//!
//! All OpenAI-compatible providers (OpenAI, DashScope, Moonshot, Zhipu, MiniMax)
//! are handled by `OpenAICompatibleProvider` with vendor-specific constructors.
//! Only providers with genuinely different API formats (DeepSeek for reasoning_content,
//! Gemini for native Google format) retain their own modules.

mod base;
mod common;
mod gemini;
mod model_spec;
mod registry;
pub mod streaming;

pub use base::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, ChatStreamChunk, ChatStreamDelta,
    FinishReason, LlmProvider, ThinkingConfig, ToolCall, ToolCallDelta, ToolDefinition,
};
pub use common::{parse_json_args, OpenAICompatibleProvider, ProviderConfig};
pub use gemini::GeminiProvider;
pub use model_spec::ModelSpec;
pub use registry::{ProviderMetadata, ProviderRegistry};
