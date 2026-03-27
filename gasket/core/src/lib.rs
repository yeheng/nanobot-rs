//! gasket-core: Facade for gasket AI assistant framework
//!
//! This crate is a facade that re-exports all gasket crates for backward
//! compatibility. It provides a single entry point for all gasket functionality.

// Re-export types first (canonical source)
pub use gasket_types::*;

// Re-export other crates (avoiding glob imports to prevent ambiguity)
pub use gasket_bus::{
    events as bus_events, run_outbound_actor, run_router_actor, run_session_actor, MessageBus,
    MessageHandler, StreamEvent,
};

pub use gasket_history::{
    count_tokens, process_history, HistoryConfig, HistoryQuery, HistoryQueryBuilder, HistoryResult,
    HistoryRetriever, ProcessedHistory, QueryOrder, ResultMeta, SemanticQuery, TimeRange,
};

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

// Re-export channels (avoiding name conflicts with local modules)
pub use gasket_channels::{
    base, log_inbound, middleware, outbound, Channel, ChannelConfigError, ChannelType,
    ChannelsConfig, DingTalkConfig, DiscordConfig, EmailConfig, FeishuConfig, InboundMessage,
    InboundSender, MediaAttachment, OutboundMessage, OutboundSender, OutboundSenderRegistry,
    SessionKey, SessionKeyParseError, SimpleAuthChecker, SimpleRateLimiter, SlackConfig,
    TelegramConfig, WebSocketMessage,
};

// Re-export webhook (feature-gated)
#[cfg(any(feature = "webhook", feature = "dingtalk", feature = "feishu", feature = "wecom"))]
pub use gasket_channels::webhook;

// Re-export vault base types from gasket_vault crate
pub use gasket_vault::{
    contains_placeholders, contains_secrets, extract_keys, redact_message_secrets, redact_secrets,
    replace_placeholders, scan_placeholders, AtomicTimestamp, EncryptedData, KdfParams,
    Placeholder, VaultCrypto, VaultEntryV2, VaultError, VaultFileV2, VaultMetadata, VaultStore,
};

// Re-export semantic for local embedding support
pub use gasket_semantic as semantic;

// Re-export storage
pub use gasket_storage as storage;

// Keep local modules that contain core business logic or re-exports
pub mod agent;
pub mod bus;
pub mod channels;
pub mod config;
pub mod cron;
pub mod error;
pub mod heartbeat;
pub mod hooks;
pub mod memory;
pub mod providers;
pub mod search;
pub mod skills;
pub mod token_tracker;
pub mod tools;
pub mod vault;

// Re-export error types for backward compatibility
pub use error::{AgentError, PipelineError, ProviderError};

// Re-export config
pub use config::Config;

// Re-export skills types
pub use skills::{Skill, SkillMetadata, SkillsLoader, SkillsRegistry};

// Re-export tool types
pub use tools::{MessageTool, Tool, ToolRegistry};

// Re-export vault types (including local injector types)
pub use vault::{InjectionReport, VaultInjector};
