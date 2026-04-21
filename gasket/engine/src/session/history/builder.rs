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

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use gasket_providers::ChatMessage;
use gasket_types::{SessionEvent, SessionKey};

use crate::error::AgentError;
use crate::hooks::{HookAction, HookBuilder, HookPoint, HookRegistry, MutableContext, VaultHook};
use crate::session::context::AgentContext;
use crate::vault::{VaultInjector, VaultStore};
use gasket_storage::process_history;
use gasket_storage::HistoryConfig;

/// Type alias for memory loader futures.
pub type MemoryLoaderFuture = Pin<Box<dyn Future<Output = Option<String>> + Send>>;

/// Memory loader function type.
pub type MemoryLoader = Arc<dyn Fn(&str) -> MemoryLoaderFuture + Send + Sync>;

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
    context: AgentContext,
    system_prompt: String,
    skills_context: Option<String>,
    hooks: Arc<HookRegistry>,
    history_config: HistoryConfig,
    /// Optional memory loader function
    memory_loader: Option<MemoryLoader>,
}

impl ContextBuilder {
    /// Create a new context builder.
    pub fn new(
        context: AgentContext,
        system_prompt: String,
        skills_context: Option<String>,
        hooks: Arc<HookRegistry>,
        history_config: HistoryConfig,
    ) -> Self {
        Self {
            context,
            system_prompt,
            skills_context,
            hooks,
            history_config,
            memory_loader: None,
        }
    }

    /// Set the memory loader for injecting long-term memory context.
    pub fn with_memory_loader<F>(mut self, loader: F) -> Self
    where
        F: Fn(&str) -> MemoryLoaderFuture + Send + Sync + 'static,
    {
        self.memory_loader = Some(Arc::new(loader));
        self
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
        let (existing_summary, watermark) =
            self.context.load_summary_with_watermark(session_key).await?;

        // ── 3. Save user event ────────────────
        let user_event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: session_key_str.clone(),
            event_type: gasket_types::EventType::UserMessage,
            content: content.clone(),
            embedding: None,
            metadata: gasket_types::EventMetadata::default(),
            created_at: chrono::Utc::now(),
            sequence: 0,
        };
        self.context.save_event(user_event).await?;

        // ── 4. Load only events after the watermark ──────────────────
        let history_events = self
            .context
            .get_events_after_watermark(session_key, watermark)
            .await?;

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

        // ── 5.5. Long-term memory loading (injected as User Message) ─────
        // Memory content varies per turn (on-demand semantic search depends on
        // user input). Injecting it as User Message preserves Prompt Cache on
        // the static System Prompt, reducing API costs by 90%+ on long sessions.
        // See: https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching
        let memory_context = if let Some(ref loader) = self.memory_loader {
            loader(&content).await
        } else {
            None
        };

        let mut messages = Self::assemble_prompt(
            history_snapshot,
            &content,
            &system_prompts,
            if existing_summary.is_empty() {
                None
            } else {
                Some(existing_summary.as_str())
            },
            memory_context.as_deref(),
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
    ///
    /// # Architecture Note: User Message Injection
    ///
    /// `memory_context` is injected as a **User Message** rather than appended
    /// to the System Prompt. This preserves Prompt Cache on Anthropic models
    /// (and similar caching mechanisms on other providers), because the System
    /// Prompt remains static across turns while the dynamic memory content
    /// varies per request. For long sessions, this reduces API costs by 90%+.
    fn assemble_prompt(
        processed_history: Vec<SessionEvent>,
        current_message: &str,
        system_prompts: &[String],
        summary: Option<&str>,
        memory_context: Option<&str>,
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

        // 4. Long-term memory as User Message (preserves System Prompt cache)
        // The [SYSTEM] prefix elevates authority without breaking the cache.
        if let Some(memory_text) = memory_context {
            if !memory_text.is_empty() {
                messages.push(ChatMessage::user(format!(
                    "[SYSTEM: Relevant long-term memories loaded for this turn. \
                     Consider them in your response.]\n\n{}",
                    memory_text
                )));
            }
        }

        // 5. Current message
        messages.push(ChatMessage::user(current_message));

        messages
    }
}

/// Build the default `HookBuilder` for main agents.
///
/// Creates:
/// - ExternalShellHook at BeforeRequest and AfterResponse
/// - VaultHook at BeforeLLM (if vault is available)
///
/// Callers can append additional hooks before calling `.build_shared()`.
pub fn build_default_hooks_builder() -> HookBuilder {
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

    builder
}

/// Build the default hook registry for main agents.
///
/// Convenience wrapper around `build_default_hooks_builder().build_shared()`.
pub fn build_default_hooks() -> Arc<HookRegistry> {
    build_default_hooks_builder().build_shared()
}
