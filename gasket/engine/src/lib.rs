//! Core execution engine for gasket AI assistant

pub mod agent;
pub mod bus_adapter;
pub mod config;
pub mod cron;
pub mod error;
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
    // Pipeline & stream
    process_message,
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
    CompressionActor,
    CompressionTask,
    EmbeddingService,
    ExecutionResult,
    ExecutorOptions,
    HistoryConfig,
    HistoryQuery,
    HistoryQueryBuilder,
    HistoryResult,
    HistoryRetriever,
    // Memory & compression
    MemoryStore,
    ModelResolver,
    PersistentContext,
    PipelineContext,
    ProcessedHistory,
    QueryOrder,
    ResultMeta,
    SemanticQuery,
    SessionKeyGuard,
    StreamEvent,
    SubagentManager,
    SubagentTracker,
    SummarizationService,
    TimeRange,
    ToolExecutor,
    TrackerError,
};

// ── Bus Adapter ────────────────────────────────────────────
pub use bus_adapter::EngineHandler;

// ── Config ─────────────────────────────────────────────────
pub use config::{
    config_dir, CommandPolicyConfig, ExecToolConfig, ResourceLimitsConfig, SandboxConfig,
    ToolsConfig, WebToolsConfig,
};

// ── Cron ───────────────────────────────────────────────────
pub use cron::{CronJob, CronService};

// ── Error ──────────────────────────────────────────────────
pub use error::{AgentError, ChannelError, ConfigValidationError, PipelineError, ProviderError};

// ── Hooks ──────────────────────────────────────────────────
pub use hooks::{
    ExecutionStrategy, ExternalHookInput, ExternalHookOutput, ExternalHookRunner,
    ExternalShellHook, HistoryRecallHook, HookAction, HookBuilder, HookContext, HookPoint,
    HookRegistry, MutableContext, PipelineHook, ReadonlyContext, ToolCallInfo, VaultHook,
};

// ── Search ─────────────────────────────────────────────────
pub use search::{
    bytes_to_embedding, cosine_similarity, embedding_to_bytes, top_k_similar, TextEmbedder,
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
    CronTool, EditFileTool, ExecTool, HistorySearchTool, ListDirTool, MemorySearchTool,
    MessageTool, ReadFileTool, SpawnParallelTool, SpawnTool, ToolRegistry, WebFetchTool,
    WebSearchTool, WriteFileTool,
};

// ── Vault ──────────────────────────────────────────────────
pub use vault::{
    contains_placeholders, contains_secrets, extract_keys, redact_message_secrets, redact_secrets,
    replace_placeholders, scan_placeholders, AtomicTimestamp, EncryptedData, InjectionReport,
    KdfParams, Placeholder, VaultCrypto, VaultEntryV2, VaultError, VaultFileV2, VaultInjector,
    VaultMetadata, VaultStore,
};
