//! # Gasket Engine
//!
//! This crate serves as the **orchestration facade** for the gasket system.
//!
//! ## Architecture
//!
//! - **engine**: Orchestration layer (AgentSession, executor, tools, hooks)
//! - **core**: Convenience re-export layer (pub use of common types)
//! - **providers**: LLM provider abstractions
//! - **storage**: Event sourcing and persistence
//!
//! ## Design Principles
//!
//! - **Direct store refs**: Components hold `Arc<EventStore>` directly
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
pub mod session;
pub mod skills;
pub mod subagents;
pub mod token_tracker;
pub mod tools;
pub mod vault;
pub mod wiki;

// ── Session (replaces agent/core) ───────────────────────────
pub use session::{AgentConfig, AgentResponse, AgentSession, ContextCompactor};

// ── Kernel ─────────────────────────────────────────────────
pub use kernel::{
    BufferedEvents, ExecutionResult, ExecutorOptions, KernelExecutor, StreamEvent, ToolExecutor,
};

// ── Subagents ──────────────────────────────────────────────
pub use gasket_types::SubagentSpawner;
pub use subagents::{run_subagent, ModelResolver, SimpleSpawner, SubagentTracker, TrackerError};

// ── Storage (top-level re-exports) ─────────────────────────
pub use gasket_storage::{
    count_tokens, process_history, CronStore, EventStore, HistoryConfig, HistoryQuery,
    HistoryQueryBuilder, HistoryResult, KvStore, MaintenanceStore, ProcessedHistory, QueryOrder,
    ResultMeta, SemanticQuery, SessionStore, SqliteStore, StoreError, TimeRange,
};

// ── Indexing (from session/history) ────────────────────────
pub use session::history::indexing::{IndexingQueue, IndexingService, Priority, QueueError};

// ── Bus Adapter ────────────────────────────────────────────
pub use bus_adapter::EngineHandler;


// ── Config ─────────────────────────────────────────────────
pub use config::{
    config_dir, load_config, CommandPolicyConfig, Config, ConfigLoader, ExecToolConfig,
    ModelConfig, ModelProfile, ModelRegistry, ProviderConfig, ProviderRegistry, ProviderType,
    ResourceLimitsConfig, SandboxConfig, ToolsConfig, WebToolsConfig,
};

// ── Cron ───────────────────────────────────────────────────
pub use cron::{CronJob, CronService};

// ── Error ──────────────────────────────────────────────────
pub use error::{AgentError, ChannelError, ConfigValidationError, ProviderError};

// ── Hooks ──────────────────────────────────────────────────
pub use hooks::{
    ExecutionStrategy, ExternalHookInput, ExternalHookOutput, ExternalHookRunner,
    ExternalShellHook, HookAction, HookBuilder, HookContext, HookPoint, HookRegistry,
    MutableContext, PipelineHook, ReadonlyContext, ToolCallInfo, VaultHook,
};

// ── Skills ─────────────────────────────────────────────────
pub use skills::{parse_skill_file, Skill, SkillMetadata, SkillsLoader, SkillsRegistry};

// ── Token Tracker ──────────────────────────────────────────
pub use token_tracker::{
    calculate_cost, estimate_tokens, format_cost, format_request_stats, format_token_usage,
    ModelPricing, SessionTokenStats, TokenUsage,
};

// ── Tools ──────────────────────────────────────────────────
pub use tools::{
    CronTool, EditFileTool, ExecTool, ListDirTool, MessageTool, ReadFileTool, SpawnParallelTool,
    SpawnTool, ToolRegistry, WebFetchTool, WebSearchTool, WriteFileTool,
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

// Providers
pub mod providers {
    pub use crate::config::app_config::ProviderRegistry;
    #[cfg(feature = "provider-anthropic")]
    pub use gasket_providers::build_anthropic_provider;
    #[cfg(feature = "provider-gemini")]
    pub use gasket_providers::build_gemini_provider;
    #[cfg(feature = "provider-minimax")]
    pub use gasket_providers::build_minimax_provider;
    #[cfg(feature = "provider-moonshot")]
    pub use gasket_providers::MoonshotProvider;
    pub use gasket_providers::{
        build_http_client, parse_json_args, ChatMessage, ChatRequest, ChatResponse,
        ChatStream, ChatStreamChunk, ChatStreamDelta, FinishReason, FunctionCall,
        FunctionDefinition, LlmProvider, MessageRole, ModelSpec, OpenAICompatibleProvider,
        ProviderBuildError, ProviderConfig, ProviderResult, ThinkingConfig, ToolCall,
        ToolCallDelta, ToolDefinition, Usage,
    };
    #[cfg(feature = "provider-copilot")]
    pub use gasket_providers::CopilotProvider;
}

// Wiki re-exports are now in the wiki module (pub mod wiki).
pub use wiki::create_wiki_tables;

// Embedding (re-exported for CLI when feature is enabled)
#[cfg(feature = "embedding")]
pub mod embedding {
    pub use gasket_embedding::vector_store;
    pub use gasket_embedding::{
        EmbeddingIndexer, EmbeddingProvider, MemoryIndex, RecallConfig, RecallSearcher, VectorStore,
    };
}
