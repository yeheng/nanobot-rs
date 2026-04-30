//! Session management layer — wraps the kernel with stateful lifecycle.
//!
//! AgentSession owns session state (events, prompts, memory, compaction)
//! and delegates the core LLM loop to `kernel::execute()`.

pub mod builder;
pub mod compactor;
pub mod config;
pub mod finalizer;
pub mod history;
pub mod prompt;

pub use compactor::{ContextCompactor, UsageStats, WatermarkInfo};
pub use config::{AgentConfig, EvolutionConfig};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::error::AgentError;
use crate::hooks::HookRegistry;
use crate::kernel::{self, ExecutionResult, RuntimeContext, StreamEvent};
use crate::session::finalizer::ResponseFinalizer;
use crate::token_tracker::ModelPricing;
use crate::tools::{SubagentSpawner, ToolRegistry};
use async_trait::async_trait;
use gasket_providers::ChatMessage;
use gasket_storage::SqliteStore;
use gasket_types::events::ChatEvent;
use gasket_types::SessionKey;

use futures_util::StreamExt;
use history::builder::BuildOutcome;
use tokio_stream::wrappers::ReceiverStream;

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

impl AgentResponse {
    /// Create from a kernel `ExecutionResult` + resolved model name.
    ///
    /// Cost is initialized to 0 — the finalizer calculates actual cost from pricing.
    pub(crate) fn from_execution(result: ExecutionResult, model: Option<String>) -> Self {
        Self {
            content: result.content,
            reasoning_content: result.reasoning_content,
            tools_used: result.tools_used,
            model,
            token_usage: result.token_usage,
            cost: 0.0,
        }
    }
}

/// Owned snapshot for post-response finalization.
pub(crate) struct FinalizeContext {
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

/// Async checkpoint callback implementation for AgentSession.
///
/// Bridges the kernel's checkpoint hook to the session's compactor,
/// eliminating the need for `block_in_place` + `block_on` hacks.
struct SessionCheckpointCallback {
    session_key: SessionKey,
    compactor: Arc<ContextCompactor>,
    event_store: gasket_storage::EventStore,
}

#[async_trait]
impl crate::kernel::CheckpointCallback for SessionCheckpointCallback {
    async fn get_checkpoint(&self, msg_len: usize) -> Option<String> {
        // Only check after a minimum number of messages to avoid
        // checkpoint noise at the start of a conversation.
        if msg_len < 3 {
            return None;
        }
        let max_seq = match self.event_store.get_max_sequence(&self.session_key).await {
            Ok(seq) => seq,
            Err(e) => {
                tracing::debug!("Checkpoint: get_max_sequence failed: {}", e);
                return None;
            }
        };
        match self.compactor.checkpoint(&self.session_key, max_seq).await {
            Ok(Some(summary)) => {
                tracing::info!(
                    "Checkpoint injected for {} at seq {} ({} chars)",
                    self.session_key,
                    max_seq,
                    summary.len()
                );
                Some(summary)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!("Checkpoint generation failed: {}", e);
                None
            }
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

/// Stateful session management — wraps the kernel, adds session lifecycle.
///
/// Owns session state (events, prompts, compaction) and delegates
/// the core LLM loop to `kernel::execute()`.
/// Optional subsystems (wiki, cost tracking) are stored as direct Option fields
/// to avoid the indirection and dynamic dispatch of a plugin trait.
pub struct AgentSession {
    runtime_ctx: RuntimeContext,
    config: AgentConfig,
    context_builder: history::builder::ContextBuilder,
    compactor: Option<Arc<ContextCompactor>>,
    /// Pricing config for cost tracking. None when cost tracking is disabled.
    pricing: Option<ModelPricing>,
    /// Response finalizer — owns post-processing logic.
    finalizer: ResponseFinalizer,
    /// Task tracker for graceful shutdown of spawned finalization tasks.
    /// `TaskTracker` is lock-free and purpose-built for "spawn N tasks, then
    /// await all" patterns. Replaces the previous `Mutex<Vec<oneshot::Receiver>>`.
    pending_done: tokio_util::task::TaskTracker,
    /// Background embedding indexer (kept alive for the session lifetime).
    #[allow(dead_code)]
    #[cfg(feature = "embedding")]
    embedding_indexer: Option<gasket_embedding::EmbeddingIndexer>,
}

/// Context carried through the PreProcess → Execute → PostProcess pipeline.
struct PipelineContext {
    runtime_ctx: RuntimeContext,
    messages: Vec<ChatMessage>,
    fctx: FinalizeContext,
    model: String,
    finalizer: ResponseFinalizer,
}

impl AgentSession {
    /// Create a new session with default services.
    pub async fn new(
        provider: Arc<dyn gasket_providers::LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Result<Self, AgentError> {
        let sqlite_store = Arc::new(
            SqliteStore::new()
                .await
                .expect("Failed to open SqliteStore"),
        );
        Self::with_sqlite_store(provider, workspace, config, tools, sqlite_store).await
    }

    /// Create a session with custom services.
    pub async fn with_sqlite_store(
        provider: Arc<dyn gasket_providers::LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        sqlite_store: Arc<SqliteStore>,
    ) -> Result<Self, AgentError> {
        builder::build_session(provider, workspace, config, tools, sqlite_store).await
    }

    /// Create a session with embedding recall support.
    #[cfg(feature = "embedding")]
    pub async fn with_sqlite_store_and_embedding(
        provider: Arc<dyn gasket_providers::LlmProvider>,
        workspace: PathBuf,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        sqlite_store: Arc<SqliteStore>,
        embedding: builder::EmbeddingContext,
    ) -> Result<Self, AgentError> {
        builder::build_session_with_embedding(
            provider,
            workspace,
            config,
            tools,
            sqlite_store,
            embedding,
        )
        .await
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

    /// Attach cost-tracking with the given pricing config.
    pub fn with_pricing(mut self, pricing: Option<ModelPricing>) -> Self {
        self.pricing = pricing;
        self
    }

    /// Get the model name.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the hook registry.
    pub fn hooks(&self) -> &Arc<HookRegistry> {
        self.context_builder.hooks()
    }

    /// Clear session for the given key.
    pub async fn clear_session(&self, session_key: &SessionKey) {
        match self
            .context_builder
            .event_store()
            .clear_session(session_key)
            .await
        {
            Ok(_) => info!("Session '{}' cleared", session_key),
            Err(e) => warn!("Failed to clear session '{}': {}", session_key, e),
        }
    }

    /// Force-trigger context compaction.
    pub fn force_compact(&self, session_key: &SessionKey, vault_values: &[String]) -> bool {
        self.compactor
            .as_ref()
            .is_some_and(|c| c.force_compact(session_key, vault_values))
    }

    /// Force-trigger context compaction and await completion.
    pub async fn force_compact_and_wait(
        &self,
        session_key: &SessionKey,
        vault_values: &[String],
    ) -> Result<(), AgentError> {
        match self.compactor.as_ref() {
            Some(c) => c
                .force_compact_and_wait(session_key, vault_values)
                .await
                .map_err(|e| AgentError::SessionError(e.to_string())),
            None => Err(AgentError::SessionError(
                "No compactor available".to_string(),
            )),
        }
    }

    /// Check if context compaction is currently in progress.
    pub fn is_compacting(&self) -> bool {
        self.compactor.as_ref().is_some_and(|c| {
            c.is_compressing_flag()
                .load(std::sync::atomic::Ordering::Acquire)
        })
    }

    /// Get context usage statistics.
    pub async fn get_context_stats(
        &self,
        session_key: &SessionKey,
    ) -> Option<crate::session::compactor::UsageStats> {
        match self.compactor.as_ref() {
            Some(c) => c.get_usage_stats(session_key).await.ok(),
            None => None,
        }
    }

    /// Get watermark information.
    pub async fn get_watermark_info(
        &self,
        session_key: &SessionKey,
    ) -> Option<crate::session::compactor::WatermarkInfo> {
        match self.compactor.as_ref() {
            Some(c) => c.get_watermark_info(session_key).await.ok(),
            None => None,
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
        let (mut ctx, aborted) = self.preprocess(content, session_key).await?;

        if let Some(msg) = aborted {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            let model = ctx.model;
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

        let (kernel_tx, kernel_rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);
        let (chat_tx, chat_rx) = tokio::sync::mpsc::channel(64);

        // Bridge: outbound messages from tools → ChatEvent → frontend
        let (outbound_tx, mut outbound_rx) =
            tokio::sync::mpsc::channel::<gasket_types::events::OutboundMessage>(64);
        let chat_tx_bridge = chat_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = outbound_rx.recv().await {
                if let gasket_types::events::OutboundPayload::Stream(chat_event) = msg.payload {
                    let _ = chat_tx_bridge.send(chat_event).await;
                }
            }
        });
        ctx.runtime_ctx.outbound_tx = Some(outbound_tx);

        // Cancel any previous aggregator for this session turn
        if ctx.runtime_ctx.aggregator_cancel.is_none() {
            ctx.runtime_ctx.aggregator_cancel = Some(Arc::new(tokio::sync::Mutex::new(None)));
        }
        if let Some(ref cancel) = ctx.runtime_ctx.aggregator_cancel {
            if let Ok(mut guard) = cancel.try_lock() {
                if let Some(ref token) = *guard {
                    token.cancel();
                }
                *guard = None;
            }
        }

        // Extract messages so we can move them into the kernel without cloning.
        let messages = std::mem::take(&mut ctx.messages);

        // Spawn via TaskTracker so graceful shutdown can await this task.
        // T3: Stream combinator replaces manual loop + extra spawn.
        let result_handle = self.pending_done.spawn(async move {
            let stream_future = ReceiverStream::new(kernel_rx)
                .filter_map(|event| futures_util::future::ready(event.to_chat_event()))
                .for_each(|chat| {
                    let chat_tx = chat_tx.clone();
                    async move {
                        let _ = chat_tx.send(chat).await;
                    }
                });

            let exec_future = Self::execute(&ctx.runtime_ctx, messages, kernel_tx);
            let (result, _) = tokio::join!(exec_future, stream_future);
            let result = result?;

            let response = Self::postprocess(result, &ctx).await;

            Ok(response)
        });

        Ok((chat_rx, result_handle))
    }

    // ── Pipeline stages ──────────────────────────────────────────────────────

    /// Stage 1: PreProcess — build request, wire checkpoint callback.
    async fn preprocess(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<(PipelineContext, Option<String>), AgentError> {
        let outcome = self.prepare_pipeline(content, session_key).await?;
        let request = match outcome {
            BuildOutcome::Aborted(msg) => {
                let ctx = PipelineContext {
                    runtime_ctx: self.runtime_ctx.clone(),
                    messages: vec![],
                    fctx: FinalizeContext {
                        session_key: session_key.clone(),
                        session_key_str: session_key.to_string(),
                        content: content.to_string(),
                        local_vault_values: vec![],
                        estimated_tokens: 0,
                    },
                    model: self.config.model.clone(),
                    finalizer: self.finalizer.clone(),
                };
                return Ok((ctx, Some(msg)));
            }
            BuildOutcome::Ready(req) => req,
        };

        let fctx = FinalizeContext::from_request(&request);
        let messages = request.messages;
        let mut runtime_ctx = self.runtime_ctx.clone();
        runtime_ctx.session_key = Some(session_key.clone());

        if let Some(ref compactor) = &self.compactor {
            runtime_ctx.checkpoint_callback = Some(Arc::new(SessionCheckpointCallback {
                session_key: fctx.session_key.clone(),
                compactor: compactor.clone(),
                event_store: self.context_builder.event_store().clone(),
            }));
        }

        let ctx = PipelineContext {
            runtime_ctx,
            messages,
            fctx,
            model: self.config.model.clone(),
            finalizer: self.finalizer.clone(),
        };
        Ok((ctx, None))
    }

    /// Stage 2: Execute — run the kernel streaming loop.
    async fn execute(
        runtime_ctx: &RuntimeContext,
        messages: Vec<ChatMessage>,
        kernel_tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ExecutionResult, AgentError> {
        if runtime_ctx.config.phased_execution {
            let executor = crate::kernel::phased::PhasedExecutor::new(runtime_ctx.clone());
            return match executor.run(messages, kernel_tx).await {
                Ok(r) => Ok(r),
                Err(crate::kernel::KernelError::MaxIterations(n)) => Ok(ExecutionResult {
                    content: format!("Maximum iterations ({}) reached.", n),
                    reasoning_content: None,
                    tools_used: vec![],
                    token_usage: None,
                }),
                Err(e) => Err(e.into()),
            };
        }
        match kernel::execute_streaming(runtime_ctx, messages, kernel_tx).await {
            Ok(r) => Ok(r),
            Err(crate::kernel::KernelError::MaxIterations(n)) => Ok(ExecutionResult {
                content: format!("Maximum iterations ({}) reached.", n),
                reasoning_content: None,
                tools_used: vec![],
                token_usage: None,
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Stage 3: PostProcess — finalize response, persist events, trigger compaction.
    async fn postprocess(result: ExecutionResult, ctx: &PipelineContext) -> AgentResponse {
        ctx.finalizer.finalize(result, &ctx.fctx, &ctx.model).await
    }

    /// Common pre-processing pipeline.
    async fn prepare_pipeline(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<history::builder::BuildOutcome, AgentError> {
        self.context_builder.build(content, session_key).await
    }
}

// Post-processing logic lives in `session::finalizer::ResponseFinalizer`.
