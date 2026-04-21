//! Session management layer — wraps the kernel with stateful lifecycle.
//!
//! AgentSession owns session state (events, prompts, memory, compaction)
//! and delegates the core LLM loop to `kernel::execute()`.

pub mod builder;
pub mod compactor;
pub mod config;
pub mod context;
pub mod history;
pub mod prompt;
pub mod store;

pub use compactor::{ContextCompactor, UsageStats, WatermarkInfo};
pub use config::{AgentConfig, EvolutionConfig};
pub use context::{AgentContext, PersistentContext};
pub use store::MemoryStore;

use crate::wiki::{PageIndex, PageStore, WikiLog};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::error::AgentError;
use crate::hooks::{HookPoint, HookRegistry, MutableContext};
use crate::kernel::{self, ExecutionResult, RuntimeContext, StreamEvent};
use crate::token_tracker::ModelPricing;
use crate::tools::{SubagentSpawner, ToolRegistry};
use crate::vault::redact_secrets;
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
///
/// Resolution order:
/// 1. `GASKET_SKILLS_DIR` environment variable
/// 2. Executable-relative heuristic (for dev builds)
/// 3. Current working directory fallback
pub fn find_builtin_skills_dir() -> Option<PathBuf> {
    // 1. Environment variable override (production deployments)
    if let Ok(env_dir) = std::env::var("GASKET_SKILLS_DIR") {
        let candidate = PathBuf::from(env_dir);
        if candidate.exists() {
            info!(
                "Found builtin skills from GASKET_SKILLS_DIR at {:?}",
                candidate
            );
            return Some(candidate);
        }
        warn!(
            "GASKET_SKILLS_DIR set to {:?} but directory does not exist",
            candidate
        );
    }

    // 2. Executable-relative heuristic (development builds)
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

    // 3. Current working directory fallback
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

/// Wiki knowledge system components — always exist together or not at all.
struct WikiComponents {
    page_store: Arc<PageStore>,
    page_index: Arc<PageIndex>,
    wiki_log: Arc<WikiLog>,
}

/// Stateful session management — wraps the kernel, adds session lifecycle.
///
/// Owns session state (events, prompts, wiki knowledge, compaction) and delegates
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
    indexing_service: Option<Arc<IndexingService>>,
    /// Wiki knowledge system. None when wiki is disabled.
    wiki: Option<WikiComponents>,
    /// Optional pricing configuration for cost calculation.
    pricing: Option<ModelPricing>,
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
        builder::build_session(provider, workspace, config, tools, memory_store, pricing).await
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

    /// Get the wiki page store.
    pub fn page_store(&self) -> Option<&Arc<PageStore>> {
        self.wiki.as_ref().map(|w| &w.page_store)
    }

    /// Get the wiki page index.
    pub fn page_index(&self) -> Option<&Arc<PageIndex>> {
        self.wiki.as_ref().map(|w| &w.page_index)
    }

    /// Get the wiki log.
    pub fn wiki_log(&self) -> Option<&Arc<WikiLog>> {
        self.wiki.as_ref().map(|w| &w.wiki_log)
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
        let pricing = self.pricing.clone();

        let (kernel_tx, mut kernel_rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);
        let (chat_tx, chat_rx) = tokio::sync::mpsc::channel(64);

        // Translate kernel StreamEvents into clean ChatEvents for consumers.
        // System events (TokenStats, subagent lifecycle) are dropped here;
        // TokenStats are already handled by the kernel's TokenTracker.
        // Tracked by pending_done for graceful shutdown.
        self.pending_done.spawn(async move {
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
            let result = match kernel::execute_streaming(&runtime_ctx, messages, kernel_tx).await {
                Ok(r) => r,
                Err(crate::kernel::KernelError::MaxIterations(n)) => {
                    crate::kernel::ExecutionResult {
                        content: format!("Maximum iterations ({}) reached.", n),
                        reasoning_content: None,
                        tools_used: vec![],
                        token_usage: None,
                        cost: 0.0,
                    }
                }
                Err(e) => return Err(e.into()),
            };

            let response = finalize_response(
                result,
                &fctx,
                &context,
                &hooks,
                &model,
                compactor.as_ref(),
                pricing.as_ref(),
            )
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

        // TODO: Phase 3 - Add wiki-based memory loader
        // For now, don't set any memory loader
        let builder = ContextBuilder::new(
            self.context.clone(),
            self.system_prompt.clone(),
            self.skills_context.clone(),
            self.hooks.clone(),
            self.history_config.clone(),
        );

        builder.build(content, session_key).await
    }
}

// ── Post-processing (shared between direct and streaming) ──────────────────

/// Save the assistant's response as a session event.
async fn save_assistant_event(
    context: &AgentContext,
    result: &ExecutionResult,
    ctx: &FinalizeContext,
    vault_values: &[String],
) {
    let history_content = redact_secrets(&result.content, vault_values);
    let assistant_event = SessionEvent {
        id: uuid::Uuid::now_v7(),
        session_key: ctx.session_key_str.to_string(),
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
}

/// Trigger non-blocking context compaction if token budget is exceeded.
fn trigger_compaction(
    compactor: Option<&Arc<ContextCompactor>>,
    ctx: &FinalizeContext,
    vault_values: &[String],
) {
    if ctx.estimated_tokens > 0 {
        if let Some(comp) = compactor {
            comp.try_compact(&ctx.session_key, ctx.estimated_tokens, vault_values);
        }
    }
}

/// Execute AfterResponse hooks with the result context.
async fn execute_after_response_hooks(
    hooks: &HookRegistry,
    result: &ExecutionResult,
    ctx: &FinalizeContext,
) {
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
        session_key: &ctx.session_key_str,
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
}

/// Calculate the cost of the response based on token usage.
fn calculate_cost(
    token_usage: &Option<gasket_types::TokenUsage>,
    pricing: Option<&ModelPricing>,
) -> f64 {
    match (token_usage, pricing) {
        (Some(usage), Some(p)) => p.calculate_cost(usage.input_tokens, usage.output_tokens),
        _ => 0.0,
    }
}

/// Log token usage statistics.
fn log_token_stats(usage: &Option<gasket_types::TokenUsage>, cost: f64) {
    if let Some(u) = usage {
        info!(
            "[Token] Input: {} | Output: {} | Total: {} | Cost: ${:.4}",
            u.input_tokens, u.output_tokens, u.total_tokens, cost
        );
    }
}

async fn finalize_response(
    result: ExecutionResult,
    ctx: &FinalizeContext,
    context: &AgentContext,
    hooks: &HookRegistry,
    model: &str,
    compactor: Option<&Arc<ContextCompactor>>,
    pricing: Option<&ModelPricing>,
) -> AgentResponse {
    let vault_values = &ctx.local_vault_values;

    save_assistant_event(context, &result, ctx, vault_values).await;
    trigger_compaction(compactor, ctx, vault_values);
    execute_after_response_hooks(hooks, &result, ctx).await;

    let cost = calculate_cost(&result.token_usage, pricing);
    log_token_stats(&result.token_usage, cost);

    AgentResponse {
        content: result.content,
        reasoning_content: result.reasoning_content,
        tools_used: result.tools_used,
        model: Some(model.to_string()),
        token_usage: result.token_usage,
        cost,
    }
}
