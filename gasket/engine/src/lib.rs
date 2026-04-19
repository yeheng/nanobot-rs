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

// NOTE: `agent/` module removed — migrated to `kernel/` + `session/` + `subagents/`
pub mod bus_adapter;
pub mod config;
pub mod cron;
pub mod error;
pub mod heartbeat;
pub mod hooks;
pub mod kernel;
pub mod plugin;
pub mod search;
pub mod session;
pub mod skills;
pub mod subagents;
pub mod token_tracker;
pub mod tools;
pub mod vault;
pub mod wiki;

// ── Session (replaces agent/core) ───────────────────────────
pub use session::{
    AgentConfig, AgentContext, AgentResponse, ContextCompactor, MemoryContext, MemoryManager,
    MemoryProvider, MemoryStore, PersistentContext,
};
// Backward-compatible alias
pub use session::AgentSession as AgentLoop;

// ── Kernel ─────────────────────────────────────────────────
pub use kernel::{
    BufferedEvents, ExecutionResult, ExecutorOptions, KernelExecutor, StreamEvent, ToolExecutor,
};

// ── Subagents ──────────────────────────────────────────────
pub use subagents::{run_subagent, ModelResolver, SimpleSpawner, SubagentTracker, TrackerError};

// Re-export from gasket_types for convenience
pub use gasket_types::SubagentSpawner;

// ── History (re-export from storage) ───────────────────────
pub use gasket_storage::{
    count_tokens, process_history, HistoryConfig, HistoryQuery, HistoryQueryBuilder, HistoryResult,
    HistoryRetriever, ProcessedHistory, QueryOrder, ResultMeta, SemanticQuery, TimeRange,
};

// ── Indexing (from session/history) ────────────────────────
pub use session::history::indexing::{IndexingQueue, IndexingService, Priority, QueueError};

// ── Bus Adapter ────────────────────────────────────────────
pub use bus_adapter::EngineHandler;

// ── Broker Outbound Dispatcher ─────────────────────────────
pub use broker_outbound::OutboundDispatcher;

// ── Session ────────────────────────────────────────────────
pub use session::AgentSession;

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
    SpawnParallelTool, SpawnTool, ToolRegistry, WebFetchTool, WebSearchTool, WriteFileTool,
};

// ── Vault ──────────────────────────────────────────────────
pub use vault::{
    contains_placeholders, contains_secrets, extract_keys, redact_message_secrets, redact_secrets,
    replace_placeholders, scan_placeholders, AtomicTimestamp, EncryptedData, InjectionReport,
    KdfParams, Placeholder, VaultCrypto, VaultEntryV2, VaultError, VaultFileV2, VaultInjector,
    VaultMetadata, VaultStore,
};

// ── Facade re-exports (merged from gasket-core) ─────────────

// Broker (topic-based message broker)
pub mod broker {
    pub use gasket_broker::*;
}

// OutboundDispatcher (in engine, not broker — uses ImProviders)
pub mod broker_outbound;

// Channels
pub mod channels {
    #[cfg(feature = "dingtalk")]
    pub use gasket_channels::dingtalk;
    #[cfg(feature = "discord")]
    pub use gasket_channels::discord;

    #[cfg(feature = "feishu")]
    pub use gasket_channels::feishu;
    #[cfg(feature = "slack")]
    pub use gasket_channels::slack;
    #[cfg(feature = "telegram")]
    pub use gasket_channels::telegram;
    #[cfg(any(
        feature = "websocket",
        feature = "dingtalk",
        feature = "feishu",
        feature = "wecom"
    ))]
    pub use gasket_channels::webhook;
    #[cfg(feature = "websocket")]
    pub use gasket_channels::websocket;
    #[cfg(feature = "wecom")]
    pub use gasket_channels::wecom;
    pub use gasket_channels::{
        adapter, log_inbound, middleware, ChannelConfigError, ChannelType, ChannelsConfig,
        DingTalkConfig, DiscordConfig, FeishuConfig, ImAdapter, ImProvider, ImProviders,
        InboundMessage, InboundSender, MediaAttachment, OutboundMessage, SessionKey,
        SessionKeyParseError, SimpleAuthChecker, SimpleRateLimiter, SlackConfig, TelegramConfig,
        WeComConfig, WebSocketMessage,
    };
    pub use gasket_types::events::ChatEvent;
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
        COPILOT_DEFAULT_CLIENT_ID,
    };
}

// Memory
pub mod memory {
    pub use crate::session::MemoryStore;
    pub use gasket_storage::memory::{
        memory_base_dir, AutoIndexHandler, FileMemoryStore, FrequencyManager, RefreshReport,
    };
    pub use gasket_storage::memory::{EmbeddingStore, MetadataStore};
    pub use gasket_storage::{EventStore, SqliteStore, StoreError};
}
