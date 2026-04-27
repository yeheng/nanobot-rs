//! Context Builder: Extracted pipeline construction from AgentLoop
//!
//! This module decouples the "Pipeline building" from "LLM execution" to prevent
//! AgentLoop from becoming a God Class. The builder handles:
//!
//! 1. Hook execution (BeforeRequest, AfterHistory, BeforeLLM)
//! 2. Session loading/saving
//! 3. History processing and token budget trimming
//! 4. Prompt assembly (system prompts, skills, memory injection)
//!
//! The resulting `ChatRequest` is then passed to the executor for LLM execution.

use std::sync::Arc;

use gasket_providers::ChatMessage;
use gasket_types::{SessionEvent, SessionKey};

use crate::error::AgentError;
use crate::hooks::{HookAction, HookBuilder, HookPoint, HookRegistry, MutableContext, VaultHook};
use crate::vault::{VaultInjector, VaultStore};
use gasket_storage::process_history;
use gasket_storage::{EventStore, HistoryConfig, SessionStore};

/// Outcome of the context building pipeline.
///
/// Uses a proper enum instead of `Option<String>` to make the two
/// mutually-exclusive paths explicit at the type level.
pub enum BuildOutcome {
    /// Pipeline completed normally — ready for execution.
    Ready(ChatRequest),
    /// BeforeRequest hook aborted the pipeline with a message.
    Aborted(String),
}

/// A fully prepared chat request ready for LLM execution.
///
/// Contains all data needed for execution and post-processing,
/// extracted from the shared pre-processing steps.
pub struct ChatRequest {
    pub session_key: String,
    pub user_content: String,
    pub messages: Vec<ChatMessage>,
    /// Vault values extracted during pipeline preparation (for redaction)
    pub vault_values: Vec<String>,
    /// Estimated token count of the current context (for compaction threshold check)
    pub estimated_tokens: usize,
}

/// Builder for constructing the LLM context/pipeline.
///
/// Decouples the complex pipeline preparation logic from `AgentLoop`,
/// following the Single Responsibility Principle.
pub struct ContextBuilder {
    event_store: Arc<EventStore>,
    session_store: Arc<SessionStore>,
    system_prompt: String,
    skills_context: Option<String>,
    hooks: Arc<HookRegistry>,
    history_config: HistoryConfig,
}

impl ContextBuilder {
    /// Create a new context builder.
    pub fn new(
        event_store: Arc<EventStore>,
        session_store: Arc<SessionStore>,
        system_prompt: String,
        skills_context: Option<String>,
        hooks: Arc<HookRegistry>,
        history_config: HistoryConfig,
    ) -> Self {
        Self {
            event_store,
            session_store,
            system_prompt,
            skills_context,
            hooks,
            history_config,
        }
    }

    /// Build the complete chat request pipeline.
    ///
    /// Executes the full preparation sequence:
    /// 1. BeforeRequest hooks
    /// 2. Load summary with watermark
    /// 3. Save user event
    /// 4. Load and process history
    /// 5. Assemble prompts with system context
    /// 6. AfterHistory + BeforeLLM hooks
    pub async fn build(
        &self,
        content: &str,
        session_key: &SessionKey,
    ) -> Result<BuildOutcome, AgentError> {
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
            vault_values: Vec::new(),
        };

        match self
            .hooks
            .execute(HookPoint::BeforeRequest, &mut ctx)
            .await?
        {
            HookAction::Abort(msg) => {
                return Ok(BuildOutcome::Aborted(msg));
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

        // ── 2. Load summary with watermark (read path optimization) ─────
        let (existing_summary, watermark) = self
            .session_store
            .load_summary_with_checkpoint(session_key)
            .await
            .map_err(|e| {
                AgentError::SessionError(format!(
                    "Failed to load summary for {}: {}",
                    session_key, e
                ))
            })?;

        // ── 3. Save user event ────────────────
        let user_event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: session_key_str.clone(),
            event_type: gasket_types::EventType::UserMessage,
            content: content.clone(),
            metadata: gasket_types::EventMetadata::default(),
            created_at: chrono::Utc::now(),
            sequence: 0,
        };
        self.event_store
            .append_event(&user_event)
            .await
            .map_err(|e| {
                AgentError::SessionError(format!("Failed to persist user event: {}", e))
            })?;

        // ── 4. Load only events after the watermark ──────────────────
        let history_events = if watermark == 0 {
            self.event_store.get_session_history(session_key).await
        } else {
            self.event_store
                .get_events_after_sequence(session_key, watermark)
                .await
        }
        .map_err(|e| {
            AgentError::SessionError(format!(
                "Failed to load history for '{}': {}",
                session_key, e
            ))
        })?;

        // ── 4.5. Token-budget trimming (safety net) ──────────────────
        let processed = process_history(history_events, &self.history_config);
        let history_snapshot = processed.events;
        if processed.filtered_count > 0 {
            tracing::debug!(
                "History trimmed: {} kept, {} evicted, ~{} tokens (watermark={})",
                history_snapshot.len(),
                processed.evicted.len(),
                processed.estimated_tokens,
                watermark,
            );
        }

        // ── 5. Prompt assembly ─────────────────
        let mut system_prompts = Vec::new();
        if !self.system_prompt.is_empty() {
            system_prompts.push(self.system_prompt.clone());
        }
        if let Some(ref skills) = self.skills_context {
            system_prompts.push(skills.clone());
        }

        // ── 5.5. Memory loading removed — agent queries wiki via tools ─────

        let mut messages = Self::assemble_prompt(
            history_snapshot,
            &content,
            &system_prompts,
            if existing_summary.is_empty() {
                None
            } else {
                Some(existing_summary.as_str())
            },
        );

        // ── 6. AfterHistory + BeforeLLM hooks ─────────────────────
        let mut ctx = MutableContext {
            session_key: &session_key_str,
            messages: &mut messages,
            user_input: Some(&content),
            response: None,
            tool_calls: None,
            token_usage: None,
            vault_values: Vec::new(),
        };
        self.hooks
            .execute(HookPoint::AfterHistory, &mut ctx)
            .await?;
        self.hooks.execute(HookPoint::BeforeLLM, &mut ctx).await?;

        // Vault values are now owned by this request's context — no shared state.
        let vault_values = ctx.vault_values;

        Ok(BuildOutcome::Ready(ChatRequest {
            session_key: session_key_str,
            user_content: content,
            messages,
            vault_values,
            estimated_tokens: processed.estimated_tokens,
        }))
    }

    /// Pure, synchronous assembly of the LLM prompt sequence.
    fn assemble_prompt(
        processed_history: Vec<SessionEvent>,
        current_message: &str,
        system_prompts: &[String],
        summary: Option<&str>,
    ) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // 1. Build the system prompt (only if non-empty)
        // Static content: workspace markdown + skills. Never changes mid-session.
        if !system_prompts.is_empty() {
            let system_content = system_prompts.join("\n\n");
            if !system_content.is_empty() {
                messages.push(ChatMessage::system(system_content));
            }
        }

        // 2. Inject summary as system message with boundary markers (if exists)
        // Using System role prevents the LLM from mistaking the summary for a
        // real assistant turn. Boundary markers clearly delineate summary content.
        if let Some(summary_text) = summary {
            if !summary_text.is_empty() {
                messages.push(ChatMessage::system(format!(
                    "{}{}{}",
                    crate::session::compactor::SUMMARY_PREFIX,
                    summary_text,
                    crate::session::compactor::SUMMARY_SUFFIX,
                )));
            }
        }

        // 3. Add processed history events (convert SessionEvent to ChatMessage)
        for event in processed_history {
            match event.event_type {
                gasket_types::EventType::UserMessage => {
                    messages.push(ChatMessage::user(event.content))
                }
                gasket_types::EventType::AssistantMessage => {
                    messages.push(ChatMessage::assistant(event.content))
                }
                _ => {}
            }
        }

        // 4. Current message
        messages.push(ChatMessage::user(current_message));

        messages
    }
}

/// Build the default `HookBuilder` for main agents.
///
/// Creates:
/// - ExternalShellHook at BeforeRequest and AfterResponse
/// - VaultHook at BeforeLLM (if vault is available)
/// - HistoryRecallHook at AfterHistory (if `event_store` is provided)
///
/// With the `embedding` feature enabled, accepts an optional `EmbeddingConfig`
/// to set up the semantic recall hook instead of the keyword-based one.
///
/// Callers can append additional hooks before calling `.build_shared()`.
pub fn build_default_hooks_builder(
    #[allow(unused_variables)] event_store: Option<Arc<EventStore>>,
) -> HookBuilder {
    #[cfg(not(feature = "embedding"))]
    use crate::hooks::HistoryRecallHook;
    use crate::hooks::{ExternalHookRunner, ExternalShellHook, HookPoint};
    use std::path::PathBuf;

    let hooks_dir = dirs::home_dir()
        .map(|p| p.join(".gasket").join("hooks"))
        .unwrap_or_else(|| {
            tracing::warn!("Could not resolve home directory, disabling external hooks.");
            PathBuf::from("/dev/null")
        });

    let external_runner = ExternalHookRunner::new(hooks_dir);

    let mut builder = HookBuilder::new()
        .with_hook(Arc::new(ExternalShellHook::new(
            external_runner.clone(),
            HookPoint::BeforeRequest,
        )))
        .with_hook(Arc::new(ExternalShellHook::new(
            external_runner,
            HookPoint::AfterResponse,
        )));

    // Add vault hook if available
    if let Ok(store) = VaultStore::new() {
        tracing::debug!("[ContextBuilder] Vault initialized successfully, adding vault injector");
        let vault_hook = VaultHook::new(VaultInjector::new(Arc::new(store)));
        builder = builder.with_hook(Arc::new(vault_hook));
    } else {
        tracing::debug!("[ContextBuilder] Vault not available, skipping vault injector");
    }

    // Add history recall hook if event store is available (keyword-based, without embedding feature)
    #[cfg(not(feature = "embedding"))]
    if let Some(store) = event_store {
        builder = builder.with_hook(Arc::new(HistoryRecallHook::new(store)));
    }

    builder
}

/// Set up the embedding recall pipeline: provider → store → index → searcher → indexer.
#[cfg(feature = "embedding")]
pub async fn setup_embedding_recall(
    event_store: &Arc<EventStore>,
    config: &crate::config::EmbeddingConfig,
) -> anyhow::Result<(
    Arc<gasket_embedding::RecallSearcher>,
    gasket_embedding::EmbeddingIndexer,
)> {
    use gasket_embedding::{EmbeddingIndexer, EmbeddingStore, HnswIndex, RecallSearcher};
    use gasket_storage::EventStoreTrait;

    // Build provider from config.
    let provider = config.provider.build()?;

    // Create embedding stores sharing the same SQLite pool.
    // One for RecallSearcher (holds ownership), one for EmbeddingIndexer.
    let pool = event_store.pool();
    let emb_store_for_searcher = EmbeddingStore::new(pool.clone());
    let emb_store_for_indexer = EmbeddingStore::new(pool);

    // Run migration on one (idempotent).
    emb_store_for_indexer.run_migration().await?;

    // Create in-memory index.
    let dim = provider.dim();
    let index = Arc::new(HnswIndex::new(dim));

    // Build provider arc early so it can be reused for backfill.
    let provider_arc: Arc<dyn gasket_embedding::EmbeddingProvider> = Arc::from(provider);

    // Cold-start: load recent embeddings into the hot index (bounded by hot_limit).
    let hot_limit = config.hot_limit;
    if hot_limit > 0 {
        EmbeddingIndexer::rebuild_index(&emb_store_for_indexer, &index, Some(hot_limit)).await?;
    }

    // Backfill: if embedding store is empty, index recent historical events up to hot_limit.
    let total_in_store = emb_store_for_indexer.count().await.unwrap_or(0);
    if total_in_store == 0 && hot_limit > 0 {
        tracing::info!(
            "Embedding store is empty — backfilling up to {} recent historical events",
            hot_limit
        );
        let events = event_store
            .get_recent_events(hot_limit)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load historical events for backfill: {}", e))?;
        let mut count = 0;
        for event in events {
            if let Err(e) = EmbeddingIndexer::process_event(
                provider_arc.as_ref(),
                &emb_store_for_indexer,
                &index,
                event,
            )
            .await
            {
                tracing::warn!("Backfill failed for event: {}", e);
            } else {
                count += 1;
            }
        }
        tracing::info!(
            "Backfilled {} recent historical events into embedding index (hot_limit={})",
            count,
            hot_limit
        );
    }

    // Build searcher.
    let searcher = Arc::new(RecallSearcher::new(
        provider_arc.clone(),
        index.clone(),
        emb_store_for_searcher,
    ));

    // Subscribe to new events and start background indexer.
    let rx = event_store.as_ref().subscribe();
    let idx = EmbeddingIndexer::start(provider_arc, emb_store_for_indexer, index, rx)?;

    Ok((searcher, idx))
}

/// Build the default hook registry for main agents.
///
/// Convenience wrapper around `build_default_hooks_builder().build_shared()`.
pub fn build_default_hooks() -> Arc<HookRegistry> {
    build_default_hooks_builder(None).build_shared()
}
