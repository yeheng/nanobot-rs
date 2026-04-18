//! Session management layer — wraps the kernel with stateful lifecycle.
//!
//! AgentSession owns session state (events, prompts, memory, compaction)
//! and delegates the core LLM loop to `kernel::execute()`.

pub mod compactor;
pub mod config;
pub mod context;
pub mod history;
pub mod memory;
pub mod prompt;
pub mod store;

pub use compactor::{ContextCompactor, UsageStats, WatermarkInfo};
pub use config::AgentConfig;
pub use context::{AgentContext, PersistentContext};
pub use memory::{MemoryContext, MemoryManager};
pub use store::{MemoryProvider, MemoryStore};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::error::AgentError;
use crate::hooks::{HookPoint, HookRegistry, MutableContext};
use crate::kernel::{self, ExecutionResult, RuntimeContext, StreamEvent};
use crate::token_tracker::ModelPricing;
use crate::tools::{SubagentSpawner, ToolRegistry};
use crate::vault::redact_secrets;
use config::AgentConfigExt;
use gasket_storage::EventStore;
use gasket_types::events::ChatEvent;
use gasket_types::{EventMetadata, EventType, SessionEvent, SessionKey};

use history::builder::BuildOutcome;
use history::indexing::IndexingService;

/// Response from agent processing
#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub tools_used: Vec<String>,
    pub model: Option<String>,
    pub token_usage: Option<gasket_types::TokenUsage>,
    pub cost: f64,
}

/// Owned snapshot for post-response finalization.
struct FinalizeContext {
    session_key: SessionKey,
    session_key_str: String,
    content: String,
    local_vault_values: Vec<String>,
    estimated_tokens: usize,
}

impl FinalizeContext {
    fn from_request(req: &history::builder::ChatRequest) -> Self {
        let session_key = SessionKey::parse(&req.session_key)
            .unwrap_or_else(|| SessionKey::new(gasket_types::ChannelType::Cli, &req.session_key));
        Self {
            session_key,
            session_key_str: req.session_key.clone(),
            content: req.user_content.clone(),
            local_vault_values: req.vault_values.clone(),
            estimated_tokens: req.estimated_tokens,
        }
    }
}

/// Conditional type for shared embedder.
#[cfg(feature = "local-embedding")]
type SharedEmbedder = Arc<gasket_storage::TextEmbedder>;

#[cfg(not(feature = "local-embedding"))]
type SharedEmbedder = ();

/// Thin wrapper to share a single `Arc<TextEmbedder>` as a `Box<dyn Embedder>`.
#[cfg(feature = "local-embedding")]
struct SharedTextEmbedder(Arc<gasket_storage::TextEmbedder>);

#[cfg(feature = "local-embedding")]
#[async_trait::async_trait]
impl gasket_storage::memory::Embedder for SharedTextEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.0.embed(text)
    }
    fn dimension(&self) -> usize {
        self.0.dimension()
    }
    fn clone_box(&self) -> Box<dyn gasket_storage::memory::Embedder> {
        Box::new(Self(self.0.clone()))
    }
}

// ── Skill loading (inlined from agent/core/mod.rs) ──

use crate::skills::{SkillsLoader, SkillsRegistry};

/// Load skills from builtin and user directories.
///
/// Returns a context summary string if any skills were loaded, or None otherwise.
pub async fn load_skills(workspace: &Path) -> Option<String> {
    let user_skills_dir = workspace.join("skills");
    let builtin_skills_dir = find_builtin_skills_dir();

    if builtin_skills_dir.is_none() {
        debug!("Built-in skills directory not found, loading user skills only");
        if !user_skills_dir.exists() {
            debug!("No skills directories found");
            return None;
        }
    }

    let loader = SkillsLoader::new(user_skills_dir, builtin_skills_dir);
    match SkillsRegistry::from_loader(loader).await {
        Ok(registry) => {
            let summary = registry.generate_context_summary().await;
            if summary.is_empty() {
                info!("No skills loaded");
                None
            } else {
                info!(
                    "Loaded {} skills ({} available)",
                    registry.len(),
                    registry.list_available().len()
                );
                Some(summary)
            }
        }
        Err(e) => {
            warn!("Failed to load skills: {}", e);
            None
        }
    }
}

/// Find the builtin skills directory.
pub fn find_builtin_skills_dir() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(project_root) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            let candidate = project_root.join("engine").join("skills");
            if candidate.exists() {
                debug!("Found builtin skills at {:?}", candidate);
                return Some(candidate);
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("engine").join("skills");
        if candidate.exists() {
            debug!("Found builtin skills at {:?}", candidate);
            return Some(candidate);
        }
        let candidate = cwd.join("skills");
        if candidate.exists() {
            debug!("Found builtin skills at {:?}", candidate);
            return Some(candidate);
        }
    }

    None
}

/// Stateful session management — wraps the kernel, adds session lifecycle.
///
/// Owns session state (events, prompts, memory, compaction) and delegates
/// the core LLM loop to `kernel::execute()`.
pub struct AgentSession {
    runtime_ctx: RuntimeContext,
    context: AgentContext,
    config: AgentConfig,
    workspace: PathBuf,
    system_prompt: String,
    skills_context: Option<String>,
    hooks: Arc<HookRegistry>,
    history_config: gasket_storage::HistoryConfig,
    compactor: Option<Arc<ContextCompactor>>,
    memory_manager: Option<Arc<MemoryManager>>,
    indexing_service: Option<Arc<IndexingService>>,
    /// Task tracker for graceful shutdown of spawned finalization tasks.
    /// `TaskTracker` is lock-free and purpose-built for "spawn N tasks, then
    /// await all" patterns. Replaces the previous `Mutex<Vec<oneshot::Receiver>>`.
    pending_done: tokio_util::task::TaskTracker,
}

impl AgentSession {
    /// Create a new session with default services.
    pub async fn new(
        provider: Arc<dyn gasket_providers::LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Result<Self, AgentError> {
        let memory_store = Arc::new(MemoryStore::new().await);
        Self::with_services(provider, workspace, config, tools, memory_store, None).await
    }

    /// Create a session with custom services.
    async fn with_services(
        provider: Arc<dyn gasket_providers::LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        memory_store: Arc<MemoryStore>,
        pricing: Option<ModelPricing>,
    ) -> Result<Self, AgentError> {
        let sqlite_store = Arc::new(memory_store.sqlite_store().clone());
        let event_store = Arc::new(EventStore::new(memory_store.sqlite_store().pool()));

        // Create IndexingService
        let mut indexing_service = IndexingService::new(sqlite_store.clone());

        // Shared embedder
        #[cfg(feature = "local-embedding")]
        let shared_embedder: Option<SharedEmbedder> = {
            let embedder_config = config
                .embedding_config
                .as_ref()
                .map(|c| gasket_storage::EmbeddingConfig::from(c.clone()))
                .unwrap_or_default();
            match gasket_storage::TextEmbedder::with_config(embedder_config) {
                Ok(embedder) => {
                    info!("TextEmbedder initialized successfully");
                    let arc: SharedEmbedder = Arc::new(embedder);
                    indexing_service.set_embedder(arc.clone());
                    Some(arc)
                }
                Err(e) => {
                    warn!("Failed to initialize TextEmbedder: {}", e);
                    None
                }
            }
        };
        #[cfg(not(feature = "local-embedding"))]
        let shared_embedder: Option<SharedEmbedder> = None;

        indexing_service.enable_queue(10000);
        indexing_service.start_worker();
        let indexing_service = Arc::new(indexing_service);

        let history_config = gasket_storage::HistoryConfig {
            max_events: config.memory_window,
            ..Default::default()
        };

        let kernel_config = config.to_kernel_config();
        let runtime_ctx = RuntimeContext {
            provider: provider.clone(),
            tools: tools.clone(),
            config: kernel_config,
            spawner: None,
            token_tracker: None,
            pricing: pricing.clone(),
        };

        let context = AgentContext::persistent(event_store.clone(), sqlite_store.clone());

        let mut compactor = ContextCompactor::new(
            provider,
            event_store,
            sqlite_store,
            config.model.clone(),
            history_config.token_budget,
        );
        if let Some(ref prompt) = config.summarization_prompt {
            compactor = compactor.with_summarization_prompt(prompt.clone());
        }
        let compactor = Arc::new(compactor);

        let (system_prompt, skills_context) = Self::load_prompts(&workspace).await?;
        let hooks = Self::build_hooks();
        let memory_manager =
            Self::try_init_memory_manager(&memory_store, shared_embedder, config.memory_budget)
                .await;

        Ok(Self {
            runtime_ctx,
            context,
            config,
            workspace,
            system_prompt,
            skills_context,
            hooks,
            history_config,
            compactor: Some(compactor),
            memory_manager,
            indexing_service: Some(indexing_service),
            pending_done: tokio_util::task::TaskTracker::new(),
        })
    }

    /// Create with pricing configuration.
    pub async fn with_pricing(
        provider: Arc<dyn gasket_providers::LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        memory_store: Arc<MemoryStore>,
        pricing: Option<ModelPricing>,
    ) -> Result<Self, AgentError> {
        Self::with_services(provider, workspace, config, tools, memory_store, pricing).await
    }

    async fn load_prompts(workspace: &Path) -> Result<(String, Option<String>), AgentError> {
        let system_prompt =
            prompt::load_system_prompt(workspace, prompt::BOOTSTRAP_FILES_FULL).await?;
        let skills_context = prompt::load_skills_context(workspace).await;
        Ok((system_prompt, skills_context))
    }

    fn build_hooks() -> Arc<HookRegistry> {
        history::builder::build_default_hooks()
    }

    /// Set the subagent spawner.
    pub fn with_spawner(mut self, spawner: Arc<dyn SubagentSpawner>) -> Self {
        self.runtime_ctx.spawner = Some(spawner);
        self
    }

    /// Set the token tracker.
    pub fn with_token_tracker(mut self, tracker: Arc<crate::token_tracker::TokenTracker>) -> Self {
        self.runtime_ctx.token_tracker = Some(tracker);
        self
    }

    /// Get the model name.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get the hook registry.
    pub fn hooks(&self) -> &Arc<HookRegistry> {
        &self.hooks
    }

    /// Get the indexing service.
    pub fn indexing_service(&self) -> Option<&Arc<IndexingService>> {
        self.indexing_service.as_ref()
    }

    /// Clear session for the given key.
    pub async fn clear_session(&self, session_key: &SessionKey) {
        if self.context.is_persistent() {
            match self.context.clear_session(session_key).await {
                Ok(_) => info!("Session '{}' cleared", session_key),
                Err(e) => warn!("Failed to clear session '{}': {}", session_key, e),
            }
        }
    }

    /// Force-trigger context compaction.
    pub fn force_compact(&self, session_key: &SessionKey, vault_values: &[String]) -> bool {
        if let Some(ref compactor) = self.compactor {
            compactor.force_compact(session_key, vault_values)
        } else {
            false
        }
    }

    /// Force-trigger context compaction and await completion.
    pub async fn force_compact_and_wait(
        &self,
        session_key: &SessionKey,
        vault_values: &[String],
    ) -> Result<(), AgentError> {
        if let Some(ref compactor) = self.compactor {
            compactor
                .force_compact_and_wait(session_key, vault_values)
                .await
                .map_err(|e| AgentError::SessionError(e.to_string()))
        } else {
            Err(AgentError::SessionError(
                "No compactor available".to_string(),
            ))
        }
    }

    /// Check if context compaction is currently in progress.
    pub fn is_compacting(&self) -> bool {
        if let Some(ref compactor) = self.compactor {
            compactor
                .is_compressing_flag()
                .load(std::sync::atomic::Ordering::Acquire)
        } else {
            false
        }
    }

    /// Get context usage statistics.
    pub async fn get_context_stats(
        &self,
        session_key: &SessionKey,
    ) -> Option<crate::session::compactor::UsageStats> {
        if let Some(ref compactor) = self.compactor {
            compactor.get_usage_stats(session_key).await.ok()
        } else {
            None
        }
    }

    /// Get watermark information.
    pub async fn get_watermark_info(
        &self,
        session_key: &SessionKey,
    ) -> Option<crate::session::compactor::WatermarkInfo> {
        if let Some(ref compactor) = self.compactor {
            compactor.get_watermark_info(session_key).await.ok()
        } else {
            None
        }
    }

    /// Gracefully shut down the session, awaiting all in-flight finalization tasks.
    ///
    /// Call this before dropping the session or shutting down the gateway to ensure
    /// all pending `finalize_response` calls complete. This prevents data loss where
    /// an assistant message has been generated but not yet persisted to the EventStore.
    pub async fn graceful_shutdown(&self) {
        // Flush memory manager access log
        if let Some(ref mgr) = self.memory_manager {
            if let Err(e) = mgr.shutdown_flush().await {
                warn!("Memory manager shutdown flush failed: {}", e);
            }
        }

        // Close the tracker (no new tasks accepted) and await all in-flight work.
        self.pending_done.close();
        if !self.pending_done.is_empty() {
            info!(
                "Graceful shutdown: awaiting {} pending finalization task(s)",
                self.pending_done.len()
            );
        }
        self.pending_done.wait().await;
    }

    /// Process a message and return response.
    ///
    /// Thin wrapper around the streaming pipeline — events are silently discarded
    /// and only the final `AgentResponse` is returned.
    pub async fn process_direct(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<AgentResponse, AgentError> {
        if self.is_compacting() {
            return Err(AgentError::SessionError(
                "Context compaction in progress, please wait.".to_string(),
            ));
        }
        let (_event_rx, handle) = self
            .process_direct_streaming_with_channel(content, session_key)
            .await?;

        // Discard streaming events, await final result
        handle
            .await
            .map_err(|e| AgentError::SessionError(format!("Task join error: {}", e)))?
    }

    /// Process a message with streaming.
    ///
    /// Returns a receiver of `ChatEvent` (user-facing data-plane events) and a
    /// join handle for the final response. System events (`TokenStats`, subagent
    /// lifecycle) are consumed internally and do not flow to the returned channel.
    pub async fn process_direct_streaming_with_channel(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<
        (
            tokio::sync::mpsc::Receiver<ChatEvent>,
            tokio::task::JoinHandle<Result<AgentResponse, AgentError>>,
        ),
        AgentError,
    > {
        if self.is_compacting() {
            return Err(AgentError::SessionError(
                "Context compaction in progress, please wait.".to_string(),
            ));
        }
        let outcome = self.prepare_pipeline(content, session_key).await?;

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

        let fctx = FinalizeContext::from_request(&request);
        let messages = request.messages;

        // Clone Arc fields for spawned task
        let runtime_ctx = self.runtime_ctx.clone();
        let hooks = self.hooks.clone();
        let context = self.context.clone();
        let model = self.config.model.clone();
        let compactor = self.compactor.clone();

        let (kernel_tx, mut kernel_rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);
        let (chat_tx, chat_rx) = tokio::sync::mpsc::channel(64);

        // Translate kernel StreamEvents into clean ChatEvents for consumers.
        // System events (TokenStats, subagent lifecycle) are dropped here;
        // TokenStats are already handled by the kernel's TokenTracker.
        tokio::spawn(async move {
            while let Some(event) = kernel_rx.recv().await {
                if let Some(chat) = event.to_chat_event() {
                    if chat_tx.send(chat).await.is_err() {
                        break;
                    }
                }
            }
        });

        // Spawn via TaskTracker so graceful shutdown can await this task.
        let result_handle = self.pending_done.spawn(async move {
            let result = kernel::execute_streaming(&runtime_ctx, messages, kernel_tx).await?;

            let response =
                finalize_response(result, &fctx, &context, &hooks, &model, compactor.as_ref())
                    .await;

            Ok(response)
        });

        Ok((chat_rx, result_handle))
    }

    /// Common pre-processing pipeline.
    async fn prepare_pipeline(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<history::builder::BuildOutcome, AgentError> {
        use history::builder::ContextBuilder;

        let memory_loader = if let Some(ref mgr) = self.memory_manager {
            let mgr = mgr.clone();
            Some(
                move |content: &str| -> history::builder::MemoryLoaderFuture {
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

    /// Try to initialize the long-term memory manager.
    async fn try_init_memory_manager(
        memory_store: &MemoryStore,
        _shared_embedder: Option<SharedEmbedder>,
        memory_budget: Option<gasket_storage::memory::TokenBudget>,
    ) -> Option<Arc<MemoryManager>> {
        use gasket_storage::memory::{memory_base_dir, Embedder, NoopEmbedder};

        let base_dir = memory_base_dir();
        if !base_dir.exists() {
            debug!("Memory directory {:?} does not exist", base_dir);
            return None;
        }

        let embedder: Box<dyn Embedder> = {
            #[cfg(feature = "local-embedding")]
            {
                if let Some(arc_embedder) = _shared_embedder {
                    info!("Memory manager reusing shared TextEmbedder");
                    Box::new(SharedTextEmbedder(arc_embedder)) as Box<dyn Embedder>
                } else {
                    Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
                }
            }
            #[cfg(not(feature = "local-embedding"))]
            {
                Box::new(NoopEmbedder::new(384)) as Box<dyn Embedder>
            }
        };

        let pool = memory_store.sqlite_store().pool();
        let mgr_result = MemoryManager::new(base_dir, &pool, embedder).await;
        let mgr_result = mgr_result.map(|m| m.with_budget(memory_budget));

        match mgr_result {
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

// ── Post-processing (shared between direct and streaming) ──────────────────

async fn finalize_response(
    result: ExecutionResult,
    ctx: &FinalizeContext,
    context: &AgentContext,
    hooks: &HookRegistry,
    model: &str,
    compactor: Option<&Arc<ContextCompactor>>,
) -> AgentResponse {
    let session_key_str = &ctx.session_key_str;
    let local_vault_values = &ctx.local_vault_values;

    // Save assistant event
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

    // Non-blocking compaction
    if ctx.estimated_tokens > 0 {
        if let Some(compactor) = compactor {
            compactor.try_compact(&ctx.session_key, ctx.estimated_tokens, local_vault_values);
        }
    }

    // AfterResponse hooks
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

    let mut hook_ctx = MutableContext {
        session_key: session_key_str,
        messages: &mut vec![],
        user_input: Some(&ctx.content),
        response: Some(&result.content),
        tool_calls: Some(&tools_used),
        token_usage: token_usage_for_hooks.as_ref(),
        vault_values: Vec::new(),
    };
    if let Err(e) = hooks.execute(HookPoint::AfterResponse, &mut hook_ctx).await {
        warn!("AfterResponse hook failed (ignored): {}", e);
    }

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
