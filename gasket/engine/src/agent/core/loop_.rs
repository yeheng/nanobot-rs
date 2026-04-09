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

use super::context::AgentContext;
use crate::agent::execution::prompt;
use crate::agent::streaming::stream::{self};
use crate::agent::HistoryConfig;
use crate::error::AgentError;
use crate::hooks::{HookPoint, HookRegistry, MutableContext};
use crate::tools::{SubagentSpawner, ToolRegistry};
use crate::vault::redact_secrets;
use gasket_providers::{ChatMessage, LlmProvider};
use gasket_types::{EventMetadata, EventType, SessionEvent, SessionKey};

use crate::agent::history::indexing::IndexingService;
use crate::agent::memory::compactor::ContextCompactor;
use crate::agent::memory::manager::MemoryManager;
use crate::agent::memory::store::MemoryStore;
use gasket_storage::EventStore;

use super::config::AgentConfig;

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
    pub token_usage: Option<gasket_types::TokenUsage>,
    /// Cost for this request (if pricing configured)
    pub cost: f64,
}

/// Owned snapshot of fields needed for post-response finalization.
///
/// Extracted *before* `messages` is moved into the executor.
/// Owns its data so the borrow checker doesn't tie it to the request's lifetime.
struct FinalizeContext {
    session_key_str: String,
    content: String,
    local_vault_values: Vec<String>,
    /// Estimated token count — used to decide if compaction should be triggered.
    estimated_tokens: usize,
}

impl FinalizeContext {
    fn from_request(req: &crate::agent::history::builder::ChatRequest) -> Self {
        Self {
            session_key_str: req.session_key.clone(),
            content: req.user_content.clone(),
            local_vault_values: req.vault_values.clone(),
            estimated_tokens: req.estimated_tokens,
        }
    }
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
    compactor: Arc<ContextCompactor>,
    memory_manager: Option<Arc<MemoryManager>>,
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
/// They are collected in `HookContext::vault_values` during BeforeLLM hook
/// execution, then snapshot-cloned into `PipelineState` for the request duration.
/// No shared mutable state is involved — each request owns its values.
pub struct AgentLoop {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: AgentConfig,
    workspace: PathBuf,
    /// History truncator configuration (token-budget-aware trimming).
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
    /// Subagent spawner for spawn/spawn_parallel tools
    spawner: Option<Arc<dyn SubagentSpawner>>,
    /// Token tracker for budget enforcement across parent and subagents
    token_tracker: Option<Arc<crate::token_tracker::TokenTracker>>,
    /// Synchronous context compactor — runs after response, before next request.
    /// Replaces the previous async fire-and-forget background compression.
    compactor: Option<Arc<ContextCompactor>>,
    /// Long-term memory manager (optional — only active if ~/.gasket/memory/ exists).
    memory_manager: Option<Arc<MemoryManager>>,
    /// Semantic indexing service for embeddings.
    indexing_service: Option<Arc<IndexingService>>,
}

impl AgentLoop {
    pub async fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Result<Self, AgentError> {
        let memory_store = Arc::new(MemoryStore::new().await);
        Self::with_services(provider, workspace, config, tools, memory_store, None).await
    }

    async fn with_services(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        memory_store: Arc<MemoryStore>,
        pricing: Option<crate::token_tracker::ModelPricing>,
    ) -> Result<Self, AgentError> {
        let sqlite_store = Arc::new(memory_store.sqlite_store().clone());
        let event_store = Arc::new(EventStore::new(memory_store.sqlite_store().pool()));

        // Create and configure IndexingService
        let mut indexing_service = IndexingService::new(sqlite_store.clone());

        #[cfg(feature = "local-embedding")]
        {
            // Try to create embedder with default config
            if let Ok(embedder) = gasket_storage::TextEmbedder::new() {
                indexing_service.set_embedder(Arc::new(embedder));
            }
        }

        // Enable async queue and start worker
        indexing_service.enable_queue(10000);
        indexing_service.start_worker();

        let indexing_service = Arc::new(indexing_service);

        let history_config = HistoryConfig {
            max_events: config.memory_window,
            ..Default::default()
        };

        let AgentInitState {
            context,
            system_prompt,
            skills_context,
            hooks,
            compactor,
            memory_manager,
        } = Self::build_internal(
            event_store,
            sqlite_store,
            &workspace,
            provider.clone(),
            config.model.clone(),
            history_config.token_budget,
            config.summarization_prompt.clone(),
        )
        .await?;

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config,
            context,
            system_prompt,
            skills_context,
            hooks,
            pricing,
            spawner: None,
            token_tracker: None,
            compactor: Some(compactor),
            memory_manager,
            indexing_service: Some(indexing_service),
        })
    }

    pub async fn with_pricing(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: ToolRegistry,
        memory_store: Arc<MemoryStore>,
        pricing: Option<crate::token_tracker::ModelPricing>,
    ) -> Result<Self, AgentError> {
        Self::with_services(
            provider,
            workspace,
            config,
            Arc::new(tools),
            memory_store,
            pricing,
        )
        .await
    }

    async fn load_prompts(workspace: &Path) -> Result<(String, Option<String>), AgentError> {
        let system_prompt =
            prompt::load_system_prompt(workspace, prompt::BOOTSTRAP_FILES_FULL).await?;
        let skills_context = prompt::load_skills_context(workspace).await;
        Ok((system_prompt, skills_context))
    }

    /// Build hooks registry for main agents.
    ///
    /// Delegates to `context_builder::build_default_hooks()` to keep
    /// hook construction logic in one place.
    fn build_hooks() -> Arc<HookRegistry> {
        crate::agent::history::builder::build_default_hooks()
    }

    /// Internal builder: common initialization for all constructors.
    ///
    /// Extracts shared logic from `with_services()` and `with_memory_store_and_pricing()`.
    async fn build_internal(
        event_store: Arc<EventStore>,
        sqlite_store: Arc<gasket_storage::SqliteStore>,
        workspace: &Path,
        provider: Arc<dyn LlmProvider>,
        model: String,
        token_budget: usize,
        summarization_prompt: Option<String>,
    ) -> Result<AgentInitState, AgentError> {
        let context = AgentContext::persistent(event_store.clone(), sqlite_store.clone());

        let mut compactor = ContextCompactor::new(
            provider,
            event_store.clone(),
            sqlite_store.clone(),
            model,
            token_budget,
        );
        if let Some(prompt) = summarization_prompt {
            compactor = compactor.with_summarization_prompt(prompt);
        }
        let compactor = Arc::new(compactor);

        let (system_prompt, skills_context) = Self::load_prompts(workspace).await?;

        let hooks = Self::build_hooks();

        // Try to initialize long-term memory manager (graceful if not available)
        let memory_manager = Self::try_init_memory_manager(&sqlite_store).await;

        Ok(AgentInitState {
            context,
            system_prompt,
            skills_context,
            hooks,
            compactor,
            memory_manager,
        })
    }

    pub fn for_subagent(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Result<Self, AgentError> {
        let history_config = HistoryConfig {
            max_events: config.memory_window,
            ..Default::default()
        };

        Ok(Self {
            provider,
            tools,
            config,
            workspace,
            history_config,
            context: AgentContext::Stateless,
            system_prompt: String::new(),
            skills_context: None,
            hooks: HookRegistry::empty(),
            pricing: None,
            spawner: None,
            token_tracker: None,
            compactor: None,
            memory_manager: None,
            indexing_service: None,
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

    /// Set the token tracker for budget enforcement across parent and subagents.
    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.token_tracker = Some(tracker);
        self
    }

    /// Get a reference to the hook registry.
    pub fn hooks(&self) -> &Arc<HookRegistry> {
        &self.hooks
    }

    /// Get the model name
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the workspace path
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get a reference to the indexing service.
    pub fn indexing_service(&self) -> Option<&Arc<IndexingService>> {
        self.indexing_service.as_ref()
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

    /// Try to initialize the long-term memory manager.
    /// Returns None if the memory directory doesn't exist or init fails.
    async fn try_init_memory_manager(
        sqlite_store: &gasket_storage::SqliteStore,
    ) -> Option<Arc<MemoryManager>> {
        use gasket_storage::memory::{memory_base_dir, Embedder, NoopEmbedder};

        let base_dir = memory_base_dir();
        if !base_dir.exists() {
            debug!(
                "Memory directory {:?} does not exist, skipping memory manager",
                base_dir
            );
            return None;
        }

        // Use TextEmbedder if local-embedding feature is enabled, otherwise use NoopEmbedder
        let embedder: Box<dyn Embedder> = {
            #[cfg(feature = "local-embedding")]
            {
                match gasket_storage::TextEmbedder::new() {
                    Ok(embedder) => {
                        info!("Memory manager using TextEmbedder (local-embedding enabled)");
                        Box::new(embedder) as Box<dyn Embedder>
                    }
                    Err(e) => {
                        warn!(
                            "Failed to initialize TextEmbedder, falling back to NoopEmbedder: {}",
                            e
                        );
                        Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
                    }
                }
            }
            #[cfg(not(feature = "local-embedding"))]
            {
                info!("Memory manager using NoopEmbedder (local-embedding disabled)");
                Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
            }
        };

        match MemoryManager::new(base_dir, &sqlite_store.pool(), embedder).await {
            Ok(mgr) => {
                if let Err(e) = mgr.init().await {
                    warn!("Failed to initialize memory manager: {}", e);
                    return None;
                }
                debug!("Memory manager initialized successfully");
                Some(Arc::new(mgr))
            }
            Err(e) => {
                warn!("Failed to create memory manager: {}", e);
                None
            }
        }
    }
}

// ── Common Pipeline ──────────────────────────────────────────

impl AgentLoop {
    /// Common pre-processing pipeline for both direct and streaming execution.
    ///
    /// Delegates to `ContextBuilder` to decouple pipeline construction from execution.
    /// Returns `BuildOutcome::Aborted` if the BeforeRequest hook aborts.
    async fn prepare_pipeline(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<crate::agent::history::builder::BuildOutcome, AgentError> {
        use crate::agent::history::builder::ContextBuilder;

        // Create memory loader closure if memory manager is available
        let memory_loader = if let Some(ref mgr) = self.memory_manager {
            let mgr = mgr.clone();
            Some(
                move |content: &str| -> crate::agent::history::builder::MemoryLoaderFuture {
                    let mgr = mgr.clone();
                    let content = content.to_string();
                    Box::pin(async move {
                        use gasket_storage::memory::MemoryQuery;
                        let query = MemoryQuery::new().with_text(&content);
                        match mgr.load_for_context(&query).await {
                            Ok(ctx) if !ctx.memories.is_empty() => {
                                let mut sections = Vec::new();
                                sections.push("## Long-Term Memory".to_string());
                                sections.push(format!(
                                    "The following memories were loaded ({} tokens):",
                                    ctx.tokens_used
                                ));
                                sections.push(String::new());
                                for mem in &ctx.memories {
                                    sections.push(format!(
                                        "### {} [{}]",
                                        mem.metadata.title, mem.metadata.scenario
                                    ));
                                    sections.push(mem.content.clone());
                                    sections.push(String::new());
                                }
                                Some(sections.join("\n"))
                            }
                            _ => None,
                        }
                    })
                },
            )
        } else {
            None
        };

        // Build context builder
        let mut builder = ContextBuilder::new(
            self.context.clone(),
            self.system_prompt.clone(),
            self.skills_context.clone(),
            self.hooks.clone(),
            self.history_config.clone(),
        );

        if let Some(loader) = memory_loader {
            builder = builder.with_memory_loader(loader);
        }

        builder.build(content, session_key).await
    }

    /// Process a message and return response.
    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<AgentResponse, AgentError> {
        use crate::agent::history::builder::BuildOutcome;

        let outcome = self.prepare_pipeline(content, session_key).await?;

        let request = match outcome {
            BuildOutcome::Aborted(msg) => {
                return Ok(AgentResponse {
                    content: msg,
                    reasoning_content: None,
                    tools_used: vec![],
                    model: Some(self.config.model.clone()),
                    token_usage: None,
                    cost: 0.0,
                });
            }
            BuildOutcome::Ready(req) => req,
        };

        let model = self.config.model.clone();
        let fctx = FinalizeContext::from_request(&request);
        let vault_values = request.vault_values.clone();
        let result = self.run_agent_loop(request.messages, &vault_values).await?;

        Ok(finalize_response(
            result,
            &fctx,
            &self.context,
            &self.hooks,
            &model,
            self.compactor.as_ref(),
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
        use crate::agent::history::builder::BuildOutcome;

        let outcome = self.prepare_pipeline(content, session_key).await?;

        // Handle abort — return closed channel + immediate response
        let request = match outcome {
            BuildOutcome::Aborted(msg) => {
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
            BuildOutcome::Ready(req) => req,
        };

        // Clone Arc fields for the spawned task
        let provider = self.provider.clone();
        let tools = self.tools.clone();
        let config = self.config.clone();
        let pricing = self.pricing.clone();
        let hooks = self.hooks.clone();
        let context = self.context.clone();
        let spawner = self.spawner.clone();
        let token_tracker = self.token_tracker.clone();
        let compactor = self.compactor.clone();

        let (event_tx, event_rx) = tokio::sync::mpsc::channel(64);

        // Extract finalize context before moving request into the task
        let fctx = FinalizeContext::from_request(&request);
        let vault_values = request.vault_values.clone();
        let messages = request.messages;

        let result_handle = tokio::spawn(async move {
            use crate::agent::execution::{AgentExecutor, ExecutorOptions};

            let executor = AgentExecutor::with_spawner(provider, tools, &config, spawner);

            let mut options = ExecutorOptions::new().with_vault_values(&vault_values);
            if let Some(ref p) = pricing {
                options = options.with_pricing(crate::token_tracker::ModelPricing {
                    price_input_per_million: p.price_input_per_million,
                    price_output_per_million: p.price_output_per_million,
                    currency: p.currency.clone(),
                });
            }
            if let Some(ref tracker) = token_tracker {
                options = options.with_token_tracker(tracker.clone());
            }

            let result = executor
                .execute_stream_with_options(messages, event_tx, &options)
                .await?;

            Ok(finalize_response(
                result,
                &fctx,
                &context,
                &hooks,
                &config.model,
                compactor.as_ref(),
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
    ) -> Result<crate::agent::execution::ExecutionResult, AgentError> {
        use crate::agent::execution::{AgentExecutor, ExecutorOptions};

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
        if let Some(ref tracker) = self.token_tracker {
            options = options.with_token_tracker(tracker.clone());
        }

        let result = executor.execute_with_options(messages, &options).await?;
        Ok(result)
    }

    // Note: handle_tool_calls was moved to executor_core.rs as part of the AgentExecutor refactoring.
}

// ── Post-processing ─────────────────────────────────────────

/// Shared post-processing for both direct and streaming execution.
///
/// Handles: save assistant event → compaction → AfterResponse hooks → token logging → build response.
/// Errors in save_event and AfterResponse are logged and swallowed — the expensive LLM
/// response must not be lost because SQLite had a hiccup or a hook misbehaved.
async fn finalize_response(
    result: crate::agent::execution::ExecutionResult,
    ctx: &FinalizeContext,
    context: &AgentContext,
    hooks: &HookRegistry,
    model: &str,
    compactor: Option<&Arc<ContextCompactor>>,
) -> AgentResponse {
    let session_key_str = &ctx.session_key_str;
    let local_vault_values = &ctx.local_vault_values;

    // ── Save assistant event (critical data safety) ────────
    let history_content = redact_secrets(&result.content, local_vault_values);
    let assistant_event = SessionEvent {
        id: uuid::Uuid::now_v7(),
        session_key: session_key_str.to_string(),
        event_type: EventType::AssistantMessage,
        content: history_content,
        embedding: None,
        metadata: EventMetadata {
            tools_used: result.tools_used.clone(),
            ..Default::default()
        },
        created_at: chrono::Utc::now(),
        sequence: 0,
    };
    if let Err(e) = context.save_event(assistant_event).await {
        warn!("Failed to persist assistant event: {}", e);
    }

    // ── Non-blocking post-response compaction ─────────────
    // Compaction uses watermark-based background processing.
    // The compactor internally checks the AtomicBool guard, threshold,
    // and spawns the actual compression task via tokio::spawn.
    // Zero user-facing latency impact.
    if ctx.estimated_tokens > 0 {
        if let Some(compactor) = compactor {
            compactor.try_compact(session_key_str, ctx.estimated_tokens, local_vault_values);
        }
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
        user_input: Some(&ctx.content),
        response: Some(&result.content),
        tool_calls: Some(&tools_used),
        token_usage: token_usage_for_hooks.as_ref(),
        vault_values: Vec::new(),
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

// ── Stream Event Conversion ──────────────────────────────────────────────────

/// Idiomatic `From` conversion from engine StreamEvent to bus StreamEvent.
///
/// Replaces the previous standalone `convert_stream_event` function.
/// The two enums are structurally similar but live in different crates
/// (engine vs bus), so the conversion is a thin field-mapping.
impl From<crate::agent::streaming::stream::StreamEvent> for crate::bus::StreamEvent {
    fn from(event: crate::agent::streaming::stream::StreamEvent) -> Self {
        use crate::agent::streaming::stream::StreamEvent as Src;

        match event {
            Src::Content(content) => Self::Content(content),
            Src::Reasoning(content) => Self::Reasoning(content),
            Src::ToolStart { name, arguments } => Self::ToolStart {
                name,
                arguments: arguments.unwrap_or_default(),
            },
            Src::ToolEnd { name, output } => Self::ToolEnd { name, output },
            Src::Done => Self::Done,
            Src::TokenStats {
                input_tokens,
                output_tokens,
                total_tokens,
                cost: _,
                currency: _,
            } => Self::TokenStats {
                prompt: input_tokens,
                completion: output_tokens,
                total: total_tokens,
            },
        }
    }
}

// ── MessageHandler Implementation ───────────────────────────────────────────

#[async_trait::async_trait]
impl crate::bus::MessageHandler for AgentLoop {
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
            tokio::sync::mpsc::Receiver<crate::bus::StreamEvent>,
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
                if event_tx.send(event.into()).await.is_err() {
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
