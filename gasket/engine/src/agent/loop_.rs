//! Agent loop: the core processing engine
//!
//! ## Execution Flow
//!
//! The main pipeline in 'process_direct_with_callback' is a straight-line sequence:
//!
//! 1. external_hook(pre_request)  → shell script can abort or modify input
//! 2. load_session                → context.load_session() (enum dispatch)
//! 3. save_user_event             → context.save_event() (enum dispatch)
//! 4. process_history             → pure: truncate history, compute evictions
//! 5. load_summary + bg_compress  → load existing summary (fast), spawn background compression if events were evicted (non-blocking)
//! 6. inject_system_prompts       → direct: bootstrap + skills
//! 7. assemble_prompt             → pure: build Vec<ChatMessage>
//!    7.5. vault_injection            → inject secrets from vault (optional)
//!    7.6. history_recall              → semantic recall of old messages (optional)
//! 8. run_agent_loop              → LLM iteration (with inline logging)
//! 9. external_hook(post_response) → shell script for audit/alerting
//! 10. save_assistant_event        → context.save_event() (enum dispatch)
//!
//! All steps are **direct method calls** or pure functions — no hidden hook dispatch.
//! External shell hooks (if present) are called via subprocess at steps 1 and 10.
//! Step 5's background compression uses `tokio::spawn` — zero user-facing latency.
//! Step 7.5 injects vault secrets directly via `VaultInjector`.
//! Step 7.6 recalls relevant history via semantic embedding search.
//!
//! ## AgentContext Enum Pattern
//!
//! The agent uses the `AgentContext` enum for state management, eliminating
//! `Arc<dyn Trait>` overhead in the core loop:
//! - **AgentContext::Persistent** — for main agents with full persistence
//! - **AgentContext::Stateless** — for subagents without persistence
//!
//! This pattern allows enum dispatch at initialization time rather than
//! runtime vtable lookup on every message.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::agent::context::AgentContext;
use crate::agent::prompt;
use crate::agent::stream::{self};
use crate::agent::HistoryConfig;
use crate::error::AgentError;
use crate::hooks::{
    ExternalHookRunner, ExternalShellHook, HookAction, HookBuilder, HookPoint, HookRegistry,
    MutableContext, VaultHook,
};
use crate::tools::{SubagentSpawner, ToolRegistry};
use crate::vault::{redact_secrets, VaultInjector, VaultStore};
use gasket_providers::{ChatMessage, LlmProvider};
use gasket_types::SessionKey;
use tokio::sync::RwLock;

use crate::agent::memory::MemoryStore;
use gasket_storage::EventStore;
use gasket_types::{EventMetadata, EventType, SessionEvent};

/// Default model for agent
const DEFAULT_MODEL: &str = "gpt-4o";
/// Default maximum iterations for agent loop
const DEFAULT_MAX_ITERATIONS: u32 = 20;
/// Default temperature for generation
const DEFAULT_TEMPERATURE: f32 = 1.0;
/// Default maximum tokens for generation
const DEFAULT_MAX_TOKENS: u32 = 65536;
/// Default memory window size
const DEFAULT_MEMORY_WINDOW: usize = 50;
/// Default maximum characters for tool result output
const DEFAULT_MAX_TOOL_RESULT_CHARS: usize = 8000;
/// Default subagent execution timeout in seconds (10 minutes)
pub const DEFAULT_SUBAGENT_TIMEOUT_SECS: u64 = 600;
/// Default session idle timeout in seconds (1 hour)
pub const DEFAULT_SESSION_IDLE_TIMEOUT_SECS: u64 = 3600;
/// Default wait timeout for subagent results in seconds (12 minutes)
pub const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 720;

/// Agent loop configuration
#[derive(Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_iterations: u32,
    pub temperature: f32,
    pub max_tokens: u32,
    pub memory_window: usize,
    /// Maximum characters for tool result output (0 = unlimited)
    pub max_tool_result_chars: usize,
    /// Enable thinking/reasoning mode for deep reasoning models
    pub thinking_enabled: bool,
    /// Enable streaming mode for progressive output
    pub streaming: bool,
    /// Subagent execution timeout in seconds
    pub subagent_timeout_secs: u64,
    /// Session idle timeout in seconds
    pub session_idle_timeout_secs: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            temperature: DEFAULT_TEMPERATURE,
            max_tokens: DEFAULT_MAX_TOKENS,
            memory_window: DEFAULT_MEMORY_WINDOW,
            max_tool_result_chars: DEFAULT_MAX_TOOL_RESULT_CHARS,
            thinking_enabled: false,
            streaming: true,
            subagent_timeout_secs: DEFAULT_SUBAGENT_TIMEOUT_SECS,
            session_idle_timeout_secs: DEFAULT_SESSION_IDLE_TIMEOUT_SECS,
        }
    }
}

/// Response from agent processing
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// Main response content
    pub content: String,
    /// Reasoning/thinking content (if thinking mode enabled)
    pub reasoning_content: Option<String>,
    /// Tools used during processing
    pub tools_used: Vec<String>,
    /// Model name used for this response
    pub model: Option<String>,
    /// Token usage for this request (if tracking enabled)
    pub token_usage: Option<crate::token_tracker::TokenUsage>,
    /// Cost for this request (if pricing configured)
    pub cost: f64,
}

/// State produced by the common pipeline preparation.
///
/// Contains all data needed for execution and post-processing,
/// extracted from the shared pre-processing steps (hooks, history, prompt assembly).
struct PipelineState {
    session_key_str: String,
    content: String,
    messages: Vec<ChatMessage>,
    local_vault_values: Vec<String>,
}

/// Outcome of the pipeline preparation.
///
/// Uses a proper enum instead of `Option<String>` to make the two
/// mutually-exclusive paths explicit at the type level.
enum PipelineOutcome {
    /// Pipeline completed normally — ready for execution.
    Ready(PipelineState),
    /// BeforeRequest hook aborted the pipeline with a message.
    Aborted(String),
}

/// Initialization state returned by `build_internal()`.
///
/// Groups all values created during agent initialization,
/// avoiding a multi-tuple return type.
struct AgentInitState {
    context: AgentContext,
    system_prompt: String,
    skills_context: Option<String>,
    hooks: Arc<crate::hooks::HookRegistry>,
    vault_values: Arc<tokio::sync::RwLock<Vec<String>>>,
    compression_handle: Option<tokio::task::JoinHandle<()>>,
}

// ── AgentLoop ───────────────────────────────────────────────

/// The agent loop - core processing engine.
///
/// Uses **AgentContext trait** for state management (polymorphic dispatch):
/// - **PersistentContext** — main agents with session persistence and compression
/// - **StatelessContext** — subagents without persistence
///
/// This pattern eliminates `Option<T>` checks in the hot path — the context
/// is determined at initialization, not at every message.
///
/// Explicit long-term memory lives in `~/.gasket/memory/*.md` files (SSOT).
/// SQLite only stores machine-state (sessions, summaries, cron, kv).
///
/// System prompt and skills context are loaded **once** at initialization
/// and stored as plain 'String' fields — no dynamic dispatch.
///
/// Lifecycle hooks are managed via **HookRegistry** — a unified mechanism for:
/// - External shell hooks (pre_request / post_response)
/// - Vault secret injection (before LLM)
/// - Semantic history recall (after history load)
///
/// ## Hook Architecture
///
/// The hook registry provides a unified interface for all pipeline hooks:
/// - `BeforeRequest`: External shell hooks can modify/abort requests
/// - `AfterHistory`: Semantic recall of relevant context
/// - `BeforeLLM`: Vault secret injection
/// - `AfterResponse`: External shell hooks for auditing
///
/// ## Security Note: Vault Values Lifecycle
///
/// Injected vault values (plaintext secrets) are scoped to **single requests**.
/// They are collected in `vault_values` field, passed through the agent loop,
/// and used for log redaction. They persist across the request lifetime.
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: AgentConfig,
    workspace: PathBuf,
    /// History truncator configuration.
    #[allow(dead_code)]
    history_config: HistoryConfig,
    /// Agent context — handles session persistence and compression.
    /// Uses enum dispatch instead of Arc<dyn Trait> for zero overhead.
    context: AgentContext,
    /// Pre-loaded system prompt (from workspace bootstrap files).
    system_prompt: String,
    /// Pre-loaded skills context (from workspace skills).
    skills_context: Option<String>,
    /// Unified hook registry for all lifecycle hooks.
    /// Replaces external_hooks, vault_injector, embedder, history_recall_k.
    hooks: Arc<HookRegistry>,
    /// Pricing configuration for cost calculation (optional)
    pricing: Option<crate::token_tracker::ModelPricing>,
    /// Injected vault values for log redaction (shared with VaultHook)
    vault_values: Arc<RwLock<Vec<String>>>,
    /// Subagent spawner for spawn/spawn_parallel tools
    spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Handle for the compression background actor (for graceful shutdown)
    compression_handle: Option<tokio::task::JoinHandle<()>>,
}

impl AgentLoop {
    /// Create a new agent loop with a pre-built tool registry.
    ///
    /// Uses **PersistentContext** for full session persistence and compression.
    ///
    /// Loads system prompt and skills context **once** at initialization.
    /// Logging is inlined directly — no hook indirection.
    /// External shell hooks are loaded from '~/.gasket/hooks/'.
    ///
    /// # Errors
    ///
    /// Returns an error if workspace bootstrap files exist but cannot be read.
    pub async fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Result<Self, AgentError> {
        let memory_store = Arc::new(MemoryStore::new().await);
        Self::with_services(provider, workspace, config, tools, memory_store).await
    }

    /// Internal helper: create AgentLoop with pre-created services.
    async fn with_services(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        memory_store: Arc<MemoryStore>,
    ) -> Result<Self, AgentError> {
        let event_store = Arc::new(EventStore::new(memory_store.sqlite_store().pool()));

        let AgentInitState {
            context,
            system_prompt,
            skills_context,
            hooks,
            vault_values,
            compression_handle,
        } = Self::build_internal(event_store, &workspace).await?;

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            context,
            system_prompt,
            skills_context,
            hooks,
            pricing: None,
            vault_values,
            spawner: None,
            compression_handle,
        })
    }

    /// Load system prompt and skills context from workspace.
    async fn load_prompts(workspace: &Path) -> Result<(String, Option<String>), AgentError> {
        let system_prompt =
            prompt::load_system_prompt(workspace, prompt::BOOTSTRAP_FILES_FULL).await?;
        let skills_context = prompt::load_skills_context(workspace).await;
        Ok((system_prompt, skills_context))
    }

    /// Build hooks registry for main agents.
    ///
    /// Creates:
    /// - ExternalShellHook at BeforeRequest and AfterResponse
    /// - VaultHook at BeforeLLM (if vault is available)
    fn build_hooks() -> (Arc<HookRegistry>, Arc<RwLock<Vec<String>>>) {
        let vault_values = Arc::new(RwLock::new(Vec::new()));

        let hooks_dir = dirs::home_dir()
            .map(|p| p.join(".gasket").join("hooks"))
            .unwrap_or_else(|| {
                tracing::warn!("Could not resolve home directory, disabling external hooks.");
                PathBuf::from("/dev/null")
            });

        let external_runner = ExternalHookRunner::new(hooks_dir);

        let mut builder = HookBuilder::new()
            // External shell hooks at BeforeRequest and AfterResponse
            .with_hook(Arc::new(ExternalShellHook::new(
                external_runner.clone(),
                HookPoint::BeforeRequest,
            )))
            .with_hook(Arc::new(ExternalShellHook::new(
                external_runner,
                HookPoint::AfterResponse,
            )));

        // Add vault hook if available
        let vault_values = if let Ok(store) = VaultStore::new() {
            debug!("[Agent] Vault initialized successfully, adding vault injector");
            let vault_hook = VaultHook::new(VaultInjector::new(Arc::new(store)));
            // Get the injected values handle for log redaction
            let values = vault_hook.injected_values();
            builder = builder.with_hook(Arc::new(vault_hook));
            values
        } else {
            debug!("[Agent] Vault not available, skipping vault injector");
            vault_values
        };

        (builder.build_shared(), vault_values)
    }

    /// Internal builder: common initialization for all constructors.
    ///
    /// Extracts shared logic from `with_services()` and `with_memory_store_and_pricing()`.
    async fn build_internal(
        event_store: Arc<EventStore>,
        workspace: &Path,
    ) -> Result<AgentInitState, AgentError> {
        // Spawn the compression actor (returns the sender for submitting tasks)
        let compression_tx = Self::spawn_compression_actor(event_store.clone());

        // Create persistent context for main agents
        let context = AgentContext::persistent(event_store, compression_tx);

        // Load system prompt and skills context
        let (system_prompt, skills_context) = Self::load_prompts(workspace).await?;

        // Build hooks using unified registry
        let (hooks, vault_values) = Self::build_hooks();

        Ok(AgentInitState {
            context,
            system_prompt,
            skills_context,
            hooks,
            vault_values,
            compression_handle: None,
        })
    }

    /// Spawn the compression actor with placeholder services.
    ///
    /// NOTE: Placeholder implementations are used for summarization and embedding.
    /// Real implementations require external service integration.
    fn spawn_compression_actor(
        event_store: Arc<gasket_storage::EventStore>,
    ) -> tokio::sync::mpsc::Sender<crate::agent::context::CompressionTask> {
        // Placeholder implementations
        struct PlaceholderSummarizer;
        #[async_trait::async_trait]
        impl crate::agent::compression::SummarizationService for PlaceholderSummarizer {
            async fn summarize(
                &self,
                events: &[gasket_types::SessionEvent],
            ) -> anyhow::Result<String> {
                Ok(format!("Compressed {} events (placeholder)", events.len()))
            }
        }

        struct PlaceholderEmbedder;
        #[async_trait::async_trait]
        impl crate::agent::compression::EmbeddingService for PlaceholderEmbedder {
            async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
                Ok(vec![0.0_f32; 384])
            }
        }

        let summarizer = std::sync::Arc::new(PlaceholderSummarizer);
        let embedder = std::sync::Arc::new(PlaceholderEmbedder);

        crate::agent::compression::CompressionActor::spawn(event_store, summarizer, embedder)
    }

    /// Create a new agent loop with MemoryStore and pricing configuration.
    ///
    /// Uses **PersistentContext** for full session persistence and compression.
    pub async fn with_memory_store_and_pricing(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
        memory_store: Arc<MemoryStore>,
        pricing: Option<crate::token_tracker::ModelPricing>,
    ) -> Result<Self, AgentError> {
        let event_store = Arc::new(EventStore::new(memory_store.sqlite_store().pool()));

        let AgentInitState {
            context,
            system_prompt,
            skills_context,
            hooks,
            vault_values,
            compression_handle,
        } = Self::build_internal(event_store, &workspace).await?;

        Ok(Self {
            provider,
            tools: Arc::new(tools),
            config,
            workspace,
            history_config: HistoryConfig::default(),
            context,
            system_prompt,
            skills_context,
            hooks,
            pricing,
            vault_values,
            spawner: None,
            compression_handle,
        })
    }

    /// Create a new agent loop for subagents without default hooks or services.
    ///
    /// Uses **AgentContext::Stateless** — no persistence, all operations are no-ops.
    /// System prompt is empty by default; use 'set_system_prompt()' to configure.
    /// No hooks for subagents by default; use 'with_hooks()' to customize.
    pub fn builder(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Result<Self, AgentError> {
        // Use stateless context for subagents
        let context = AgentContext::Stateless;

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config: HistoryConfig::default(),
            context,
            system_prompt: String::new(),
            skills_context: None,
            hooks: HookRegistry::empty(), // Empty hooks for subagents
            pricing: None,
            vault_values: Arc::new(RwLock::new(Vec::new())),
            spawner: None,
            compression_handle: None,
        })
    }

    /// Set the system prompt (used by subagents to configure identity).
    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    /// Set custom hooks for this agent.
    ///
    /// Used by subagents to inherit hooks from the main agent.
    pub fn with_hooks(mut self, hooks: Arc<HookRegistry>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Set the subagent spawner for spawn/spawn_parallel tools.
    pub fn with_spawner(mut self, spawner: Arc<dyn SubagentSpawner>) -> Self {
        self.spawner = Some(spawner);
        self
    }

    /// Get a reference to the hook registry.
    pub fn hooks(&self) -> &Arc<HookRegistry> {
        &self.hooks
    }

    /// Get mutable access to vault values (for log redaction).
    pub fn vault_values(&self) -> &Arc<RwLock<Vec<String>>> {
        &self.vault_values
    }

    /// Get the model name
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the workspace path
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Clear the session for the given key (used by CLI for '/new' command).
    ///
    /// This resets the conversation history so the next message starts fresh.
    /// For stateless contexts (subagents), this is a no-op.
    pub async fn clear_session(&self, session_key: &SessionKey) {
        if self.context.is_persistent() {
            match self.context.clear_session(&session_key.to_string()).await {
                Ok(_) => tracing::info!("Session '{}' cleared", session_key),
                Err(e) => tracing::warn!("Failed to clear session '{}': {}", session_key, e),
            }
        }
    }
}

impl Drop for AgentLoop {
    fn drop(&mut self) {
        if let Some(handle) = self.compression_handle.take() {
            handle.abort();
        }
    }
}

// ── Common Pipeline ──────────────────────────────────────────

impl AgentLoop {
    /// Common pre-processing pipeline for both direct and streaming execution.
    ///
    /// Executes the shared steps: BeforeRequest hook, history load/save,
    /// prompt assembly, AfterHistory/BeforeLLM hooks, vault value extraction.
    /// Returns `PipelineOutcome::Aborted` if the BeforeRequest hook aborts.
    async fn prepare_pipeline(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<PipelineOutcome, AgentError> {
        let session_key_str = session_key.to_string();

        // ── 1. BeforeRequest hooks (can modify input or abort) ─────
        let mut messages: Vec<ChatMessage> = vec![ChatMessage::user(content)];
        let mut ctx = MutableContext {
            session_key: &session_key_str,
            messages: &mut messages,
            user_input: Some(content),
            response: None,
            tool_calls: None,
            token_usage: None,
        };

        match self
            .hooks
            .execute(HookPoint::BeforeRequest, &mut ctx)
            .await?
        {
            HookAction::Abort(msg) => {
                return Ok(PipelineOutcome::Aborted(msg));
            }
            HookAction::Continue => {}
        }

        // Get the (possibly modified) user content
        let content: String = ctx
            .messages
            .iter()
            .find(|m| m.role == gasket_providers::MessageRole::User)
            .and_then(|m| m.content.clone())
            .unwrap_or_else(|| content.to_string());

        // ── 2. Load session history (enum dispatch) ─────
        let history_events = self.context.get_history(&session_key_str, None).await;

        // ── 3. Save user event ────────────────
        let user_event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: session_key_str.clone(),
            parent_id: None,
            event_type: EventType::UserMessage,
            content: content.clone(),
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: chrono::Utc::now(),
        };
        self.context.save_event(user_event).await?;

        // ── 4. Truncate history (pure computation) ─────────────────
        let history_snapshot: Vec<SessionEvent> = history_events
            .into_iter()
            .rev()
            .take(self.config.memory_window)
            .rev()
            .collect();

        // ── 5. System prompts + prompt assembly ─────────────────────
        let mut system_prompts = Vec::new();
        if !self.system_prompt.is_empty() {
            system_prompts.push(self.system_prompt.clone());
        }
        if let Some(ref skills) = self.skills_context {
            system_prompts.push(skills.clone());
        }

        let mut messages = Self::assemble_prompt(
            history_snapshot,
            &content,
            &system_prompts,
            None, // Summary: TODO - query for summary events
            None, // History recall handled by hooks
        );

        // ── 6. AfterHistory + BeforeLLM hooks ─────────────────────
        let mut ctx = MutableContext {
            session_key: &session_key_str,
            messages: &mut messages,
            user_input: Some(&content),
            response: None,
            tool_calls: None,
            token_usage: None,
        };
        self.hooks
            .execute(HookPoint::AfterHistory, &mut ctx)
            .await?;
        self.hooks.execute(HookPoint::BeforeLLM, &mut ctx).await?;

        let local_vault_values = self.vault_values.read().await.clone();

        Ok(PipelineOutcome::Ready(PipelineState {
            session_key_str,
            content,
            messages,
            local_vault_values,
        }))
    }

    /// Process a message and return response.
    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<AgentResponse, AgentError> {
        let state = self.prepare_pipeline(content, session_key).await?;

        let state = match state {
            PipelineOutcome::Aborted(msg) => {
                return Ok(AgentResponse {
                    content: msg,
                    reasoning_content: None,
                    tools_used: vec![],
                    model: Some(self.config.model.clone()),
                    token_usage: None,
                    cost: 0.0,
                });
            }
            PipelineOutcome::Ready(s) => s,
        };

        let model = self.config.model.clone();
        let result = self
            .run_agent_loop(state.messages, &state.local_vault_values)
            .await?;

        Ok(finalize_response(
            result,
            &state.session_key_str,
            &state.content,
            &state.local_vault_values,
            &self.context,
            &self.hooks,
            &model,
        )
        .await)
    }

    /// Process a message with streaming and return a channel for events.
    ///
    /// This is the preferred method for streaming. It returns:
    /// - `mpsc::Receiver<StreamEvent>` - for consuming stream events with .await
    /// - `JoinHandle<Result<AgentResponse>>` - final result after streaming completes
    ///
    /// The caller can now await each event send, providing proper backpressure.
    pub async fn process_direct_streaming_with_channel(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<
        (
            tokio::sync::mpsc::Receiver<stream::StreamEvent>,
            tokio::task::JoinHandle<Result<AgentResponse, AgentError>>,
        ),
        AgentError,
    > {
        let outcome = self.prepare_pipeline(content, session_key).await?;

        // Handle abort — return closed channel + immediate response
        let state = match outcome {
            PipelineOutcome::Aborted(msg) => {
                let (_tx, rx) = tokio::sync::mpsc::channel(1);
                let model = self.config.model.clone();
                let handle = tokio::spawn(async move {
                    Ok(AgentResponse {
                        content: msg,
                        reasoning_content: None,
                        tools_used: vec![],
                        model: Some(model),
                        token_usage: None,
                        cost: 0.0,
                    })
                });
                return Ok((rx, handle));
            }
            PipelineOutcome::Ready(s) => s,
        };

        // Clone Arc fields for the spawned task
        let provider = self.provider.clone();
        let tools = self.tools.clone();
        let config = self.config.clone();
        let pricing = self.pricing.clone();
        let hooks = self.hooks.clone();
        let context = self.context.clone();
        let spawner = self.spawner.clone();

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);

        let result_handle = tokio::spawn(async move {
            use crate::agent::executor_core::{AgentExecutor, ExecutorOptions};

            let executor = AgentExecutor::with_spawner(provider, tools, &config, spawner);

            let mut options = ExecutorOptions::new().with_vault_values(&state.local_vault_values);
            if let Some(ref p) = pricing {
                options = options.with_pricing(crate::token_tracker::ModelPricing {
                    price_input_per_million: p.price_input_per_million,
                    price_output_per_million: p.price_output_per_million,
                    currency: p.currency.clone(),
                });
            }

            let result = executor
                .execute_stream_with_options(state.messages, event_tx, &options)
                .await?;

            Ok(finalize_response(
                result,
                &state.session_key_str,
                &state.content,
                &state.local_vault_values,
                &context,
                &hooks,
                &config.model,
            )
            .await)
        });

        Ok((event_rx, result_handle))
    }

    // ── Agent Loop Internals ────────────────────────────────
    // Note: calculate_token_usage and handle_tool_calls were moved to executor_core.rs
    // as part of the AgentExecutor refactoring.

    /// Unified agent iteration loop.
    ///
    /// Delegates to `AgentExecutor` for the core LLM loop.
    /// Handles session stats tracking after execution completes.
    ///
    /// # Security: Vault Values Scoping
    ///
    /// `vault_values` is passed as a parameter (not stored in self) to ensure
    /// plaintext secrets are scoped to single requests. This prevents memory
    /// accumulation and limits the exposure window for sensitive data.
    async fn run_agent_loop(
        &self,
        messages: Vec<ChatMessage>,
        vault_values: &[String],
    ) -> Result<crate::agent::executor_core::ExecutionResult, AgentError> {
        use crate::agent::executor_core::{AgentExecutor, ExecutorOptions};

        let executor = AgentExecutor::with_spawner(
            self.provider.clone(),
            self.tools.clone(),
            &self.config,
            self.spawner.clone(),
        );

        let mut options = ExecutorOptions::new().with_vault_values(vault_values);
        if let Some(ref pricing) = self.pricing {
            options = options.with_pricing(crate::token_tracker::ModelPricing {
                price_input_per_million: pricing.price_input_per_million,
                price_output_per_million: pricing.price_output_per_million,
                currency: pricing.currency.clone(),
            });
        }

        let result = executor.execute_with_options(messages, &options).await?;
        Ok(result)
    }

    // Note: handle_tool_calls was moved to executor_core.rs as part of the AgentExecutor refactoring.
}

// ── Post-processing ─────────────────────────────────────────

/// Shared post-processing for both direct and streaming execution.
///
/// Handles: save assistant event → AfterResponse hooks → token logging → build response.
/// Errors in save_event and AfterResponse are logged and swallowed — the expensive LLM
/// response must not be lost because SQLite had a hiccup or a hook misbehaved.
async fn finalize_response(
    result: crate::agent::executor_core::ExecutionResult,
    session_key_str: &str,
    content: &str,
    local_vault_values: &[String],
    context: &AgentContext,
    hooks: &HookRegistry,
    model: &str,
) -> AgentResponse {
    // ── Save assistant event (critical data safety) ────────
    let history_content = redact_secrets(&result.content, local_vault_values);
    let assistant_event = SessionEvent {
        id: uuid::Uuid::now_v7(),
        session_key: session_key_str.to_string(),
        parent_id: None,
        event_type: EventType::AssistantMessage,
        content: history_content,
        embedding: None,
        metadata: EventMetadata {
            tools_used: result.tools_used.clone(),
            ..Default::default()
        },
        created_at: chrono::Utc::now(),
    };
    if let Err(e) = context.save_event(assistant_event).await {
        warn!("Failed to persist assistant event: {}", e);
    }

    // ── AfterResponse hooks (audit, logging, etc.) ────────
    let tools_used: Vec<crate::hooks::ToolCallInfo> = result
        .tools_used
        .iter()
        .map(|name| crate::hooks::ToolCallInfo {
            id: name.clone(),
            name: name.clone(),
            arguments: None,
        })
        .collect();

    let token_usage_for_hooks =
        result
            .token_usage
            .as_ref()
            .map(|usage| crate::token_tracker::TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                total_tokens: usage.total_tokens,
            });

    let mut ctx = MutableContext {
        session_key: session_key_str,
        messages: &mut vec![],
        user_input: Some(content),
        response: Some(&result.content),
        tool_calls: Some(&tools_used),
        token_usage: token_usage_for_hooks.as_ref(),
    };
    if let Err(e) = hooks.execute(HookPoint::AfterResponse, &mut ctx).await {
        warn!("AfterResponse hook failed (ignored): {}", e);
    }

    // ── Log token usage ────────────────────────────────────
    if let Some(ref usage) = result.token_usage {
        info!(
            "[Token] Input: {} | Output: {} | Total: {} | Cost: ${:.4}",
            usage.input_tokens, usage.output_tokens, usage.total_tokens, result.cost
        );
    }

    AgentResponse {
        content: result.content,
        reasoning_content: result.reasoning_content,
        tools_used: result.tools_used,
        model: Some(model.to_string()),
        token_usage: result.token_usage,
        cost: result.cost,
    }
}

// ── Helpers ─────────────────────────────────────────────────

impl AgentLoop {
    /// Pure, synchronous assembly of the LLM prompt sequence.
    fn assemble_prompt(
        processed_history: Vec<SessionEvent>,
        current_message: &str,
        system_prompts: &[String],
        summary: Option<&str>,
        recalled_history: Option<&[String]>,
    ) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // 1. Build the system prompt (only if non-empty)
        if !system_prompts.is_empty() {
            let system_content = system_prompts.join("\n\n");
            if !system_content.is_empty() {
                messages.push(ChatMessage::system(system_content));
            }
        }

        // 2. Inject summary as assistant message (if exists)
        if let Some(summary_text) = summary {
            if !summary_text.is_empty() {
                messages.push(ChatMessage::assistant(format!(
                    "{}{}",
                    crate::agent::summarization::SUMMARY_PREFIX,
                    summary_text
                )));
            }
        }

        // 2.5. Inject recalled history (semantic recall of old conversations)
        if let Some(recalled) = recalled_history {
            if !recalled.is_empty() {
                let recall_content = format!(
                    "{}\n{}",
                    crate::agent::summarization::RECALL_PREFIX,
                    recalled.join("\n")
                );
                messages.push(ChatMessage::assistant(recall_content));
                debug!("Injected {} recalled history messages", recalled.len());
            }
        }

        // 3. Add processed history events (convert SessionEvent to ChatMessage)
        let history_count = processed_history.len();
        for event in processed_history {
            // Only include User and Assistant messages
            match event.event_type {
                EventType::UserMessage => messages.push(ChatMessage::user(event.content)),
                EventType::AssistantMessage => messages.push(ChatMessage::assistant(event.content)),
                _ => {
                    // Skip other event types (tool calls, summaries, etc.)
                }
            }
        }

        // 4. Current message
        messages.push(ChatMessage::user(current_message));

        debug!(
            "Built messages: {} history events, summary: {}, recalled: {}",
            history_count,
            summary.is_some(),
            recalled_history.map(|r| r.len()).unwrap_or(0)
        );

        messages
    }
}

// ── Stream Event Conversion ──────────────────────────────────────────────────

/// Convert an engine StreamEvent to a bus StreamEvent.
///
/// Extracted as a standalone function to keep the spawned task flat
/// and avoid exceeding the 3-layer indentation rule.
fn convert_stream_event(event: crate::agent::stream::StreamEvent) -> gasket_bus::StreamEvent {
    use crate::agent::stream::StreamEvent as AgentStreamEvent;
    use gasket_bus::StreamEvent as BusStreamEvent;

    match event {
        AgentStreamEvent::Content(content) => BusStreamEvent::Content(content),
        AgentStreamEvent::Reasoning(content) => BusStreamEvent::Reasoning(content),
        AgentStreamEvent::ToolStart { name, arguments } => BusStreamEvent::ToolStart {
            name,
            arguments: arguments.unwrap_or_default(),
        },
        AgentStreamEvent::ToolEnd { name, output } => BusStreamEvent::ToolEnd { name, output },
        AgentStreamEvent::Done => BusStreamEvent::Done,
        AgentStreamEvent::TokenStats {
            input_tokens,
            output_tokens,
            total_tokens,
            cost: _,
            currency: _,
        } => BusStreamEvent::TokenStats {
            prompt: input_tokens,
            completion: output_tokens,
            total: total_tokens,
        },
    }
}

// ── MessageHandler Implementation ───────────────────────────────────────────

#[async_trait::async_trait]
impl gasket_bus::MessageHandler for AgentLoop {
    async fn handle_message(
        &self,
        session_key: &gasket_types::events::SessionKey,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let response = self.process_direct(message, session_key).await?;
        Ok(response.content)
    }

    async fn handle_streaming_message(
        &self,
        message: &str,
        session_key: &gasket_types::events::SessionKey,
    ) -> Result<
        (
            tokio::sync::mpsc::Receiver<gasket_bus::StreamEvent>,
            tokio::sync::oneshot::Receiver<
                Result<
                    gasket_types::events::OutboundMessage,
                    Box<dyn std::error::Error + Send + Sync>,
                >,
            >,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        use tokio::sync::mpsc;

        let (event_tx, event_rx) = mpsc::channel(32);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        // Clone session_key for the spawned task
        let session_key_owned = session_key.clone();

        // Get the streaming result from AgentLoop
        let (mut agent_event_rx, result_handle) = self
            .process_direct_streaming_with_channel(message, session_key)
            .await?;

        // Spawn a task to convert AgentLoop StreamEvents to gasket_bus StreamEvents
        tokio::spawn(async move {
            while let Some(event) = agent_event_rx.recv().await {
                if event_tx.send(convert_stream_event(event)).await.is_err() {
                    break;
                }
            }
        });

        // Spawn a task to wrap the final result
        tokio::spawn(async move {
            match result_handle.await {
                Ok(Ok(response)) => {
                    // Create an OutboundMessage from the response
                    let outbound_msg = gasket_types::events::OutboundMessage {
                        channel: gasket_types::events::ChannelType::Cli,
                        chat_id: session_key_owned.to_string(),
                        content: response.content,
                        metadata: None,
                        trace_id: None,
                        ws_message: None,
                    };
                    let _ = result_tx.send(Ok(outbound_msg));
                }
                Ok(Err(e)) => {
                    let _ = result_tx
                        .send(Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>));
                }
                Err(e) => {
                    let _ = result_tx
                        .send(Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>));
                }
            }
        });

        Ok((event_rx, result_rx))
    }
}
