//! # Gasket Engine
//!
//! This crate serves as the **orchestration facade** for the gasket system.
//!
//! ## Architecture
//!
//! - **engine**: Orchestration layer (AgentLoop, executor, tools, hooks)
//! - **core**: Convenience re-export layer (pub use of common types)
//! - **providers**: LLM provider abstractions
//! - **storage**: Event sourcing and persistence
//!
//! ## Design Principles
//!
//! - **Enum-based dispatch**: `AgentContext` enum instead of trait objects
//! - **Event sourcing**: All state changes persisted as events
//! - **Streaming-first**: SSE streaming with backpressure support

pub mod agent;
pub mod bus_adapter;
pub mod config;
pub mod cron;
pub mod error;
pub mod heartbeat;
pub mod hooks;
pub mod search;
pub mod skills;
pub mod token_tracker;
pub mod tools;
pub mod vault;

// ── Agent ──────────────────────────────────────────────────
pub use agent::{
    // History
    count_tokens,
    process_history,
    // Stream
    // Subagents
    run_subagent,
    // Core loop
    AgentConfig,
    // Context (enum dispatch)
    AgentContext,
    // Execution
    AgentExecutor,
    AgentLoop,
    AgentResponse,
    BufferedEvents,
    ExecutionResult,
    ExecutorOptions,
    HistoryConfig,
    HistoryQuery,
    HistoryQueryBuilder,
    HistoryResult,
    HistoryRetriever,
    IndexingService,
    // Memory & compression
    MemoryStore,
    ModelResolver,
    PersistentContext,
    ProcessedHistory,
    QueryOrder,
    ResultMeta,
    SemanticQuery,
    SessionKeyGuard,
    StreamEvent,
    SubagentManager,
    SubagentTracker,
    TimeRange,
    ToolExecutor,
    TrackerError,
};

// ── Bus Adapter ────────────────────────────────────────────
pub use bus_adapter::EngineHandler;

// ── Config ─────────────────────────────────────────────────
pub use config::{
    config_dir, load_config, CommandPolicyConfig, Config, ConfigLoader, EmbeddingConfig,
    ExecToolConfig, ModelConfig, ModelProfile, ModelRegistry, ProviderConfig, ProviderRegistry,
    ProviderType, ResourceLimitsConfig, SandboxConfig, ToolsConfig, WebToolsConfig,
};

// ── Cron ───────────────────────────────────────────────────
pub use cron::{CronJob, CronService};

// ── Error ──────────────────────────────────────────────────
pub use error::{AgentError, ChannelError, ConfigValidationError, PipelineError, ProviderError};

// ── Hooks ──────────────────────────────────────────────────
#[cfg(feature = "local-embedding")]
pub use hooks::HistoryRecallHook;
pub use hooks::{
    ExecutionStrategy, ExternalHookInput, ExternalHookOutput, ExternalHookRunner,
    ExternalShellHook, HookAction, HookBuilder, HookContext, HookPoint, HookRegistry,
    MutableContext, PipelineHook, ReadonlyContext, ToolCallInfo, VaultHook,
};

// ── Search ─────────────────────────────────────────────────
#[cfg(feature = "local-embedding")]
pub use search::{bytes_to_embedding, embedding_to_bytes, TextEmbedder};
pub use search::{cosine_similarity, top_k_similar};

// ── Memory (re-export from storage) ────────────────────────
pub use gasket_storage::memory::{Embedder, NoopEmbedder};

// ── Skills ─────────────────────────────────────────────────
pub use skills::{parse_skill_file, Skill, SkillMetadata, SkillsLoader, SkillsRegistry};

// ── Token Tracker ──────────────────────────────────────────
pub use token_tracker::{
    calculate_cost, estimate_tokens, format_cost, format_request_stats, format_token_usage,
    ModelPricing, SessionTokenStats, TokenUsage,
};

// ── Tools ──────────────────────────────────────────────────
pub use tools::{
    CronTool, EditFileTool, ExecTool, ListDirTool, MemorySearchTool, MessageTool, ReadFileTool,
    SpawnParallelTool, SpawnTool, SubagentSpawner, ToolRegistry, WebFetchTool, WebSearchTool,
    WriteFileTool,
};

// ── Vault ──────────────────────────────────────────────────
pub use vault::{
    contains_placeholders, contains_secrets, extract_keys, redact_message_secrets, redact_secrets,
    replace_placeholders, scan_placeholders, AtomicTimestamp, EncryptedData, InjectionReport,
    KdfParams, Placeholder, VaultCrypto, VaultEntryV2, VaultError, VaultFileV2, VaultInjector,
    VaultMetadata, VaultStore,
};

// ── Facade re-exports (merged from gasket-core) ─────────────

// Bus
pub mod bus {
    pub use gasket_bus::*;
}

// Channels
pub mod channels {
    #[cfg(feature = "dingtalk")]
    pub use gasket_channels::dingtalk;
    #[cfg(feature = "discord")]
    pub use gasket_channels::discord;
    #[cfg(feature = "email")]
    pub use gasket_channels::email;
    #[cfg(feature = "feishu")]
    pub use gasket_channels::feishu;
    #[cfg(feature = "slack")]
    pub use gasket_channels::slack;
    #[cfg(feature = "telegram")]
    pub use gasket_channels::telegram;
    #[cfg(any(
        feature = "webhook",
        feature = "dingtalk",
        feature = "feishu",
        feature = "wecom"
    ))]
    pub use gasket_channels::webhook;
    #[cfg(feature = "webhook")]
    pub use gasket_channels::websocket;
    #[cfg(feature = "wecom")]
    pub use gasket_channels::wecom;
    pub use gasket_channels::{
        base, log_inbound, middleware, outbound, Channel, ChannelConfigError, ChannelType,
        ChannelsConfig, DingTalkConfig, DiscordConfig, EmailConfig, FeishuConfig, InboundMessage,
        InboundSender, MediaAttachment, OutboundMessage, OutboundSender, OutboundSenderRegistry,
        SessionKey, SessionKeyParseError, SimpleAuthChecker, SimpleRateLimiter, SlackConfig,
        TelegramConfig, WebSocketMessage,
    };
}

// Providers
pub mod providers {
    pub use crate::config::app_config::ProviderRegistry;
    #[cfg(feature = "provider-gemini")]
    pub use gasket_providers::GeminiProvider;
    pub use gasket_providers::{
        build_http_client, parse_json_args, streaming, ChatMessage, ChatRequest, ChatResponse,
        ChatStream, ChatStreamChunk, ChatStreamDelta, FinishReason, FunctionCall,
        FunctionDefinition, LlmProvider, MessageRole, ModelSpec, OpenAICompatibleProvider,
        ProviderBuildError, ProviderConfig, ProviderResult, ThinkingConfig, ToolCall,
        ToolCallDelta, ToolDefinition, Usage,
    };
    #[cfg(feature = "provider-copilot")]
    pub use gasket_providers::{
        CopilotOAuth, CopilotProvider, CopilotTokenResponse, DeviceCodeResponse,
    };
}

// Memory
pub mod memory {
    pub use crate::agent::MemoryStore;
    pub use gasket_storage::memory::{memory_base_dir, AutoIndexHandler, RefreshReport};
    pub use gasket_storage::memory::{EmbeddingStore, MetadataStore};
    pub use gasket_storage::{EventStore, SqliteStore, StoreError};
}
