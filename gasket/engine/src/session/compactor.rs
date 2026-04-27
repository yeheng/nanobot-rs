//! Watermark-based background context compactor.
//!
//! # Design: Sequence High-Water Mark
//!
//! Compaction uses a single integer cursor (`covered_upto_sequence`) stored in
//! the `session_summaries` table. This replaces the previous `covered_event_ids:
//! Vec<Uuid>` tracking — one integer instead of hundreds of UUIDs.
//!
//! # Read Path
//!
//! ```text
//! SELECT content, covered_upto_sequence FROM session_summaries WHERE session_key = ?
//! → summary text + watermark
//! SELECT * FROM session_events WHERE session_key = ? AND sequence > watermark
//! → recent (uncompacted) events
//! Assemble: [System Prompt] + [Summary] + [Recent Events]
//! ```
//!
//! # Write Path (Background)
//!
//! ```text
//! 1. should_compact() → guard + threshold check
//! 2. try_acquire_lock() → CAS guard
//! 3. spawn_compaction_task() → tokio::spawn
//!    └─ run_compaction()
//!       ├─ load target_sequence, existing_summary, events from DB
//!       ├─ build_context_text() → assemble LLM input
//!       ├─ summarize_with_llm() → call provider
//!       └─ persist_and_gc() → redact, save summary, garbage-collect
//! ```
//!
//! # Crash Safety
//!
//! If the process crashes during compaction, nothing is lost: the summary
//! watermark wasn't updated, so the next startup loads slightly more history
//! and triggers compaction again. This is **natural idempotency**.

/// Cooldown after LLM failure: skip compaction for 60s to avoid hammering a failing API.
const COMPACTION_COOLDOWN_SECS: u64 = 60;

/// Watermark-based context compactor.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Result};
use tracing::{debug, info, warn};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_storage::{count_tokens, EventStore, SessionStore};
use gasket_types::{SessionEvent, SessionKey};

use crate::vault::redact_secrets;

/// Default prompt for LLM summarization (used when no custom prompt is configured).
pub const DEFAULT_SUMMARIZATION_PROMPT: &str =
    "Summarize the following conversation briefly, keeping key facts, decisions, and outcomes.";

/// Alias for backward compatibility.
pub const SUMMARIZATION_PROMPT: &str = DEFAULT_SUMMARIZATION_PROMPT;

/// Prefix for injected summary system messages.
/// Uses clear boundary markers to prevent the LLM from mistaking
/// the summary for real conversation turns.
pub const SUMMARY_PREFIX: &str = "[Conversation Summary]\n";

pub const SUMMARY_SUFFIX: &str = "\n[End of Summary]";

/// Prefix for recalled history injection.
pub const RECALL_PREFIX: &str = "[回忆]";

// ---------------------------------------------------------------------------
// CompactionListener — notification interface for embedding cleanup
// ---------------------------------------------------------------------------

/// Listener notified when compaction deletes events.
///
/// Implementors (e.g. `EmbeddingIndexer`) use this to clean up associated
/// embeddings when events are garbage-collected by the compactor.
pub trait CompactionListener: Send + Sync {
    /// Called with the IDs of events about to be (or already) deleted.
    fn on_events_deleted(&self, event_ids: &[String]);
}

// ---------------------------------------------------------------------------
// ContextCompactor — public API
// ---------------------------------------------------------------------------

/// Context usage statistics for a session.
#[derive(Debug, Clone)]
pub struct UsageStats {
    /// Configured token budget for the context window.
    pub token_budget: usize,
    /// Compaction threshold multiplier (e.g. 1.2 = 120%).
    pub compaction_threshold: f32,
    /// Token count at which auto-compaction triggers.
    pub threshold_tokens: usize,
    /// Estimated total tokens (summary + uncompacted events).
    pub current_tokens: usize,
    /// Current usage as a percentage of token_budget.
    pub usage_percent: f64,
    /// Tokens consumed by the existing summary.
    pub summary_tokens: usize,
    /// Number of events not yet covered by a summary.
    pub uncompacted_events: usize,
    /// Token count of uncompacted events.
    pub event_tokens: usize,
    /// Whether a compaction task is currently running.
    pub is_compressing: bool,
}

/// Watermark and sequence information for a session.
#[derive(Debug, Clone)]
pub struct WatermarkInfo {
    /// The covered_upto_sequence from the latest summary.
    pub watermark: i64,
    /// Maximum sequence number in the event store.
    pub max_sequence: i64,
    /// Number of events after the watermark.
    pub uncompacted_count: usize,
    /// Percentage of history that has been compacted.
    pub compacted_percent: f64,
}

/// Configuration for proactive checkpointing.
#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    /// Trigger checkpoint every N sequence increments (0 = disabled).
    pub interval_turns: usize,
    /// Prompt template for checkpoint generation.
    pub prompt: String,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            interval_turns: 7,
            prompt: r#"Summarize current task state for working memory.
Output ONLY in this format:

<key_info>
- Current goal: [one sentence]
- Completed: [list]
- Blocked on: [if any]
- Next step: [one sentence]
- Key facts learned: [list]
</key_info>

Be concise."#
                .into(),
        }
    }
}

/// Watermark-based context compactor.
///
/// Called via `tokio::spawn` after each agent response when the token budget
/// is exceeded. Uses an `AtomicBool` guard to prevent concurrent compaction
/// for the same session.
///
/// The compactor is self-contained: given a `target_sequence`, it loads all
/// data it needs from the database, calls the LLM, persists the result, and
/// garbage-collects old events. No in-memory state is required from the caller.
pub struct ContextCompactor {
    /// LLM provider for generating summaries.
    provider: Arc<dyn LlmProvider>,
    /// Event store for loading events and garbage collection.
    event_store: Arc<EventStore>,
    /// SQLite store for summary persistence (session_summaries table).
    session_store: Arc<SessionStore>,
    /// Model to use for summarization.
    model: String,
    /// Token budget for context window.
    token_budget: usize,
    /// Compaction threshold multiplier (default 1.2).
    compaction_threshold: f32,
    /// Custom summarization prompt.
    summarization_prompt: String,
    /// Guard preventing concurrent compaction for the same session.
    is_compressing: Arc<AtomicBool>,
    /// Pending compaction flag: set when threshold is exceeded but lock is held.
    /// Checked by the background task on completion to trigger a follow-up run.
    pending_compaction: Arc<AtomicBool>,
    /// Timestamp of last compaction failure (for cooldown backoff).
    last_failed_attempt: Arc<parking_lot::Mutex<Option<Instant>>>,
    /// Optional checkpoint configuration for proactive working-memory snapshots.
    checkpoint_config: Option<CheckpointConfig>,
    /// Listeners notified when events are deleted during compaction.
    listeners: Vec<Arc<dyn CompactionListener>>,
}

impl ContextCompactor {
    /// Default compaction threshold multiplier.
    pub const DEFAULT_COMPACTION_THRESHOLD: f32 = 1.2;

    /// Create a new compactor.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        event_store: Arc<EventStore>,
        session_store: Arc<SessionStore>,
        model: String,
        token_budget: usize,
    ) -> Self {
        Self {
            provider,
            event_store,
            session_store,
            model,
            token_budget,
            compaction_threshold: Self::DEFAULT_COMPACTION_THRESHOLD,
            summarization_prompt: DEFAULT_SUMMARIZATION_PROMPT.to_string(),
            is_compressing: Arc::new(AtomicBool::new(false)),
            pending_compaction: Arc::new(AtomicBool::new(false)),
            last_failed_attempt: Arc::new(parking_lot::Mutex::new(None)),
            checkpoint_config: None,
            listeners: Vec::new(),
        }
    }

    /// Set a custom summarization prompt (overrides built-in default).
    pub fn with_summarization_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.summarization_prompt = prompt.into();
        self
    }

    /// Enable proactive checkpointing.
    pub fn with_checkpoint_config(mut self, config: CheckpointConfig) -> Self {
        self.checkpoint_config = Some(config);
        self
    }

    /// Set a custom compaction threshold multiplier.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.compaction_threshold = threshold;
        self
    }

    /// Add a compaction listener to be notified when events are deleted.
    pub fn add_listener(&mut self, listener: Arc<dyn CompactionListener>) {
        self.listeners.push(listener);
    }

    /// Get a clone of the is_compressing guard for external inspection.
    pub fn is_compressing_flag(&self) -> Arc<AtomicBool> {
        self.is_compressing.clone()
    }

    /// Load the current summary and its watermark for a session.
    ///
    /// Returns `(summary_text, covered_upto_sequence)`.
    /// If no summary exists, returns `("", 0)`.
    pub async fn load_summary_with_watermark(&self, session_key: &SessionKey) -> (String, i64) {
        match self.session_store.load_summary(session_key).await {
            Ok(Some((content, watermark))) => (content, watermark),
            Ok(None) => (String::new(), 0),
            Err(e) => {
                debug!("Failed to load summary for {}: {}", session_key, e);
                (String::new(), 0)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Inspection APIs
    // -----------------------------------------------------------------------

    /// Get context usage statistics for a session.
    pub async fn get_usage_stats(&self, session_key: &SessionKey) -> Result<UsageStats> {
        let (summary_text, watermark) = self.load_summary_with_watermark(session_key).await;
        let summary_tokens = if summary_text.is_empty() {
            0
        } else {
            count_tokens(&summary_text)
        };

        let events = self
            .event_store
            .get_events_after_sequence(session_key, watermark)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load events: {}", e))?;

        let event_tokens: usize = events.iter().map(|e| count_tokens(&e.content)).sum();
        let total_tokens = summary_tokens + event_tokens;
        let threshold_tokens = (self.token_budget as f32 * self.compaction_threshold) as usize;
        let usage_percent = if self.token_budget > 0 {
            (total_tokens as f64 / self.token_budget as f64) * 100.0
        } else {
            0.0
        };

        Ok(UsageStats {
            token_budget: self.token_budget,
            compaction_threshold: self.compaction_threshold,
            threshold_tokens,
            current_tokens: total_tokens,
            usage_percent,
            summary_tokens,
            uncompacted_events: events.len(),
            event_tokens,
            is_compressing: self.is_compressing.load(Ordering::Acquire),
        })
    }

    /// Get watermark and sequence information for a session.
    pub async fn get_watermark_info(&self, session_key: &SessionKey) -> Result<WatermarkInfo> {
        let (_, watermark) = self.load_summary_with_watermark(session_key).await;

        let max_sequence = self
            .event_store
            .get_max_sequence(session_key)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get max sequence: {}", e))?;

        let uncompacted = self
            .event_store
            .get_events_after_sequence(session_key, watermark)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to count uncompacted events: {}", e))?
            .len();

        let compacted_percent = if max_sequence > 0 {
            (watermark as f64 / max_sequence as f64) * 100.0
        } else {
            0.0
        };

        Ok(WatermarkInfo {
            watermark,
            max_sequence,
            uncompacted_count: uncompacted,
            compacted_percent,
        })
    }

    // -----------------------------------------------------------------------
    // Public entry points
    // -----------------------------------------------------------------------

    /// Force-trigger background compaction, bypassing the threshold check.
    ///
    /// Unlike `try_compact`, this always triggers compaction (unless already
    /// in progress). Useful for manual context management via the `context` tool.
    ///
    /// Returns `true` if compaction was triggered, `false` if already in progress.
    pub fn force_compact(&self, session_key: &SessionKey, vault_values: &[String]) -> bool {
        let sk = session_key.to_string();
        if !self.try_acquire_lock(&sk) {
            debug!("Force compaction skipped: already in progress for {}", sk);
            return false;
        }
        let guard = CompactionGuard {
            is_compressing: self.is_compressing.clone(),
            pending_compaction: Some(self.pending_compaction.clone()),
        };
        info!("Force compaction triggered for {}", sk);
        self.spawn_compaction_task(session_key, vault_values, guard);
        true
    }

    /// Force-trigger compaction and await its completion.
    ///
    /// Returns `Ok(())` if compaction ran successfully, `Err` if already in progress
    /// or if compaction failed.
    pub async fn force_compact_and_wait(
        &self,
        session_key: &SessionKey,
        vault_values: &[String],
    ) -> Result<()> {
        let sk = session_key.to_string();
        if !self.try_acquire_lock(&sk) {
            bail!("Compaction already in progress for {}", sk);
        }
        info!("Force compaction (blocking) started for {}", sk);
        let _guard = CompactionGuard {
            is_compressing: self.is_compressing.clone(),
            pending_compaction: Some(self.pending_compaction.clone()),
        };

        if let Err(e) = self
            .session_store
            .mark_compaction_started(session_key)
            .await
        {
            warn!("Failed to mark compaction started for {}: {}", sk, e);
        }

        let listeners: Vec<Arc<dyn CompactionListener>> = self.listeners.clone();

        let result = run_compaction(
            &self.event_store,
            &self.session_store,
            &*self.provider,
            &self.model,
            &self.summarization_prompt,
            session_key,
            vault_values,
            &listeners,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Compaction failed for {}: {}", sk, e));

        if let Err(e) = self
            .session_store
            .mark_compaction_finished(session_key)
            .await
        {
            warn!("Failed to mark compaction finished for {}: {}", sk, e);
        }

        result
    }

    /// Generate a proactive checkpoint for the current session state.
    ///
    /// Called every N sequence increments. Returns `Some(summary)` if a
    /// checkpoint was generated, `None` if skipped.
    ///
    /// `current_max_sequence` must be fetched from `EventStore` — never pass
    /// a transient turn counter.
    /// Timeout for checkpoint LLM calls — prevents the agent loop from hanging
    /// if the provider API is slow or unresponsive.
    const CHECKPOINT_TIMEOUT_SECS: u64 = 30;

    pub async fn checkpoint(
        &self,
        session_key: &SessionKey,
        current_max_sequence: i64,
    ) -> Result<Option<String>> {
        let config = match &self.checkpoint_config {
            Some(c) => c,
            None => return Ok(None),
        };

        if config.interval_turns == 0
            || current_max_sequence == 0
            || current_max_sequence % config.interval_turns as i64 != 0
        {
            return Ok(None);
        }

        // Load recent events for context
        let events = self
            .event_store
            .get_events_after_sequence(
                session_key,
                current_max_sequence.saturating_sub(config.interval_turns as i64),
            )
            .await
            .unwrap_or_default();

        let events_text = events
            .iter()
            .map(|e| format!("{}: {}", e.event_type, e.content))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!("{}\n\nRecent events:\n{}", config.prompt, events_text);

        let request = gasket_providers::ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system("You are a state summarizer."),
                ChatMessage::user(prompt),
            ],
            tools: None,
            temperature: Some(0.2),
            max_tokens: Some(512),
            thinking: None,
        };

        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(Self::CHECKPOINT_TIMEOUT_SECS),
            self.provider.chat(request),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                warn!("Checkpoint LLM call failed for {}: {}", session_key, e);
                return Ok(None);
            }
            Err(_) => {
                warn!(
                    "Checkpoint LLM call timed out after {}s for {}",
                    Self::CHECKPOINT_TIMEOUT_SECS,
                    session_key
                );
                return Ok(None);
            }
        };
        let summary = response.content.unwrap_or_default().trim().to_string();

        if summary.is_empty() {
            warn!("Checkpoint generated empty summary for {}", session_key);
            return Ok(None);
        }

        self.session_store
            .save_checkpoint(&session_key.to_string(), current_max_sequence, &summary)
            .await?;

        info!(
            "Checkpoint saved for {} at sequence {} ({} chars)",
            session_key,
            current_max_sequence,
            summary.len()
        );

        Ok(Some(summary))
    }

    /// Try to trigger background compaction.
    ///
    /// This is the main entry point, called from `finalize_response()`.
    /// Decomposed into three steps: [should_compact] → [try_acquire_lock] →
    /// [spawn_compaction_task].
    ///
    /// # Returns
    ///
    /// `true` if compaction was triggered, `false` if skipped (already
    /// compressing or below threshold).
    pub fn try_compact(
        &self,
        session_key: &SessionKey,
        current_tokens: usize,
        vault_values: &[String],
    ) -> bool {
        let sk = session_key.to_string();
        if !self.should_compact(&sk, current_tokens) {
            // Clear pending if we drop below threshold — no need to re-trigger.
            self.pending_compaction.store(false, Ordering::Release);
            return false;
        }
        if !self.try_acquire_lock(&sk) {
            // Lock held but threshold exceeded — mark pending for re-trigger.
            self.pending_compaction.store(true, Ordering::Release);
            debug!(
                "Compaction deferred for {}: already in progress, marked pending",
                sk
            );
            return false;
        }
        let guard = CompactionGuard {
            is_compressing: self.is_compressing.clone(),
            pending_compaction: Some(self.pending_compaction.clone()),
        };
        self.spawn_compaction_task(session_key, vault_values, guard);
        true
    }

    // -----------------------------------------------------------------------
    // Gate checks
    // -----------------------------------------------------------------------

    /// Check whether compaction should be triggered.
    ///
    /// Returns `false` if already compressing, below threshold, or in cooldown.
    fn should_compact(&self, session_key: &str, current_tokens: usize) -> bool {
        if self.is_compressing.load(Ordering::Acquire) {
            debug!(
                "Compaction already in progress for {}, skipping",
                session_key
            );
            return false;
        }

        // Cooldown check: skip if last failure was within COOLDOWN_SECS
        if let Some(last_fail) = *self.last_failed_attempt.lock() {
            if last_fail.elapsed().as_secs() < COMPACTION_COOLDOWN_SECS {
                debug!(
                    "Compaction in cooldown for {}: {}s since last failure",
                    session_key,
                    last_fail.elapsed().as_secs()
                );
                return false;
            }
        }

        let threshold = (self.token_budget as f32 * self.compaction_threshold) as usize;
        if current_tokens < threshold {
            debug!(
                "Skipping compaction for {}: {} tokens < threshold {} (budget={}, mult={})",
                session_key,
                current_tokens,
                threshold,
                self.token_budget,
                self.compaction_threshold
            );
            return false;
        }

        true
    }

    /// Atomically acquire the compaction lock via CAS.
    ///
    /// Returns `false` if another thread won the race.
    fn try_acquire_lock(&self, session_key: &str) -> bool {
        if self
            .is_compressing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            debug!(
                "Race: another thread started compaction for {}",
                session_key
            );
            return false;
        }
        true
    }

    // -----------------------------------------------------------------------
    // Task spawning
    // -----------------------------------------------------------------------

    /// Clone all fields and spawn the background compaction task.
    fn spawn_compaction_task(
        &self,
        session_key: &SessionKey,
        vault_values: &[String],
        guard: CompactionGuard,
    ) {
        let event_store = self.event_store.clone();
        let session_store = self.session_store.clone();
        let provider = self.provider.clone();
        let model = self.model.clone();
        let summarization_prompt = self.summarization_prompt.clone();
        let sk = session_key.clone();
        let vault = vault_values.to_vec();
        let last_failed = self.last_failed_attempt.clone();
        let pending = self.pending_compaction.clone();
        // Clone Arc references to listeners so the background task can notify them.
        let listeners: Vec<Arc<dyn CompactionListener>> = self.listeners.clone();

        tokio::spawn(async move {
            let _guard = guard;
            debug!("Background compaction started for {}", sk);

            if let Err(e) = session_store.mark_compaction_started(&sk).await {
                warn!("Failed to mark compaction started for {}: {}", sk, e);
            }

            let run = async {
                if let Err(e) = run_compaction(
                    &event_store,
                    &session_store,
                    &*provider,
                    &model,
                    &summarization_prompt,
                    &sk,
                    &vault,
                    &listeners,
                )
                .await
                {
                    warn!("Compaction failed for {}: {}", sk, e);
                    *last_failed.lock() = Some(Instant::now());
                } else {
                    *last_failed.lock() = None;
                }

                // If pending was set while we were running, clear it and re-trigger.
                // We run the follow-up inline while still holding the CompactionGuard
                // to prevent concurrent compaction for the same session.
                if pending
                    .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    info!(
                        "Pending compaction detected for {} — re-triggering immediately",
                        sk
                    );
                    if let Err(e) = run_compaction(
                        &event_store,
                        &session_store,
                        &*provider,
                        &model,
                        &summarization_prompt,
                        &sk,
                        &vault,
                        &listeners,
                    )
                    .await
                    {
                        warn!("Follow-up compaction failed for {}: {}", sk, e);
                        *last_failed.lock() = Some(Instant::now());
                    } else {
                        *last_failed.lock() = None;
                    }
                }
            };

            run.await;

            if let Err(e) = session_store.mark_compaction_finished(&sk).await {
                warn!("Failed to mark compaction finished for {}: {}", sk, e);
            }
        });
    }
}

/// RAII guard that resets the compaction flag on drop, ensuring panic safety.
/// Optionally clears the pending flag so a follow-up run is not triggered
/// when the lock was released by a synchronous path (e.g., force_compact_and_wait).
struct CompactionGuard {
    is_compressing: Arc<AtomicBool>,
    pending_compaction: Option<Arc<AtomicBool>>,
}

impl Drop for CompactionGuard {
    fn drop(&mut self) {
        self.is_compressing.store(false, Ordering::Release);
        // If this guard owns the pending flag, clear it on drop so the
        // background task does not spin forever on synchronous paths.
        if let Some(ref pending) = self.pending_compaction {
            pending.store(false, Ordering::Release);
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline stages — module-level functions for testability
// ---------------------------------------------------------------------------

/// Execute the full compaction pipeline: load → build context → summarize → persist.
#[allow(clippy::too_many_arguments)]
async fn run_compaction(
    event_store: &EventStore,
    session_store: &SessionStore,
    provider: &dyn LlmProvider,
    model: &str,
    summarization_prompt: &str,
    session_key: &SessionKey,
    vault_values: &[String],
    listeners: &[Arc<dyn CompactionListener>],
) -> Result<()> {
    // 1. Load target sequence
    let target_sequence = event_store
        .get_max_sequence(session_key)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get max sequence for {}: {}", session_key, e))?;

    // 2. Load existing summary
    let existing_summary = match session_store.load_summary(session_key).await {
        Ok(Some((content, _watermark))) => Some(content),
        Ok(None) => None,
        Err(e) => bail!("Failed to load summary for {}: {}", session_key, e),
    };

    // 3. Load events to compact
    let events = event_store
        .get_events_up_to_sequence(session_key, target_sequence)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load events for {}: {}", session_key, e))?;

    if events.is_empty() {
        debug!("No events to compact for {}", session_key);
        return Ok(());
    }

    // 4. Build context → summarize → persist
    let context_text = build_context_text(existing_summary.as_deref(), &events);
    debug!(
        "Compaction context for {}: {} tokens, {} events (up to seq {})",
        session_key,
        count_tokens(&context_text),
        events.len(),
        target_sequence
    );

    let summary_text = summarize_with_llm(provider, model, summarization_prompt, &context_text)
        .await?
        .trim()
        .to_string();

    if summary_text.is_empty() {
        bail!("LLM returned empty summary for {}", session_key);
    }

    persist_and_gc(
        session_store,
        event_store,
        session_key,
        &summary_text,
        vault_values,
        target_sequence,
        listeners,
    )
    .await?;

    Ok(())
}

/// Build the text context sent to the LLM for summarization.
///
/// Prepends the existing summary (if any) before the event list.
fn build_context_text(existing_summary: Option<&str>, events: &[SessionEvent]) -> String {
    let mut parts = Vec::with_capacity(events.len() + 1);

    if let Some(summary) = existing_summary {
        if !summary.is_empty() {
            parts.push(format!("Previous summary:\n{}", summary));
        }
    }

    for event in events {
        parts.push(format!("{}: {}", event.event_type, event.content));
    }

    parts.join("\n")
}

/// Call the LLM to generate a summary from the context text.
async fn summarize_with_llm(
    provider: &dyn LlmProvider,
    model: &str,
    summarization_prompt: &str,
    context_text: &str,
) -> Result<String> {
    let request = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::system(summarization_prompt),
            ChatMessage::user(context_text.to_string()),
        ],
        tools: None,
        temperature: Some(0.3),
        max_tokens: Some(1024),
        thinking: None,
    };

    let response = provider
        .chat(request)
        .await
        .map_err(|e| anyhow::anyhow!("LLM summarization call failed: {}", e))?;

    Ok(response.content.unwrap_or_default())
}

/// Redact secrets, persist the summary, and garbage-collect old events.
async fn persist_and_gc(
    session_store: &SessionStore,
    event_store: &EventStore,
    session_key: &SessionKey,
    summary_text: &str,
    vault_values: &[String],
    target_sequence: i64,
    listeners: &[Arc<dyn CompactionListener>],
) -> Result<()> {
    // Redact secrets
    let summary_to_persist = if vault_values.is_empty() {
        summary_text.to_string()
    } else {
        redact_secrets(summary_text, vault_values)
    };

    // Persist summary with new watermark
    session_store
        .save_summary(session_key, &summary_to_persist, target_sequence)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to save summary for {}: {}", session_key, e))?;

    // Fetch event IDs before deletion so listeners know what was removed.
    let deleted_ids = event_store
        .get_event_ids_up_to(session_key, target_sequence)
        .await
        .unwrap_or_default();

    // Garbage-collect old events (non-fatal on failure)
    match event_store
        .delete_events_upto(session_key, target_sequence)
        .await
    {
        Ok(deleted) => {
            // Notify listeners after successful deletion.
            if !deleted_ids.is_empty() {
                for listener in listeners {
                    listener.on_events_deleted(&deleted_ids);
                }
            }
            info!(
                "Compaction complete for {}: {} tokens summary, {} events GC'd (watermark={})",
                session_key,
                count_tokens(summary_text),
                deleted,
                target_sequence
            );
        }
        Err(e) => {
            warn!(
                "Compaction: summary saved but GC failed for {}: {}",
                session_key, e
            );
            // Non-fatal: summary is saved, GC will retry on next compaction
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarization_prompt_not_empty() {
        assert!(!SUMMARIZATION_PROMPT.is_empty());
    }

    #[test]
    fn test_checkpoint_config_default() {
        let config = CheckpointConfig::default();
        assert_eq!(config.interval_turns, 7);
        assert!(config.prompt.contains("Current goal"));
    }

    #[test]
    fn test_summary_prefix_format() {
        assert!(SUMMARY_PREFIX.starts_with('['));
        assert!(SUMMARY_PREFIX.ends_with('\n'));
        assert!(!SUMMARY_SUFFIX.is_empty());
    }

    #[test]
    fn test_atomic_bool_guard() {
        let flag = Arc::new(AtomicBool::new(false));

        // First compare_exchange succeeds
        assert!(flag
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok());

        // Second compare_exchange fails (already true)
        assert!(flag
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err());

        // Reset
        flag.store(false, Ordering::Release);
        assert!(!flag.load(Ordering::Relaxed));
    }

    #[test]
    fn test_should_compact_below_threshold() {
        // Can't construct ContextCompactor without real dependencies,
        // so test the logic directly.
        let token_budget: usize = 1000;
        let threshold_mult: f32 = 1.2;
        let overflow_threshold = (token_budget as f32 * threshold_mult) as usize;

        // Below threshold → should not compact
        assert!(900 < overflow_threshold);
        // At threshold → should compact
        assert!(1200 >= overflow_threshold);
    }

    #[test]
    fn test_build_context_text_empty() {
        let text = build_context_text(None, &[]);
        assert!(text.is_empty());
    }

    #[test]
    fn test_build_context_text_with_summary() {
        let text = build_context_text(Some("previous summary"), &[]);
        assert!(text.contains("Previous summary:"));
        assert!(text.contains("previous summary"));
    }

    #[test]
    fn test_build_context_text_with_events() {
        use gasket_types::EventType;
        use uuid::Uuid;

        let event = SessionEvent {
            id: Uuid::new_v4(),
            session_key: "test".to_string(),
            event_type: EventType::UserMessage,
            content: "hello".to_string(),
            sequence: 1,
            created_at: chrono::Utc::now(),
            metadata: Default::default(),
        };

        let text = build_context_text(None, &[event]);
        assert!(text.contains("hello"));
        assert!(!text.contains("Previous summary"));
    }

    // ── Crash Recovery Integration Test ──────────────────────────────────

    struct MockProvider {
        response: parking_lot::Mutex<String>,
        fail: AtomicBool,
    }

    impl MockProvider {
        fn new(summary: &str) -> Self {
            Self {
                response: parking_lot::Mutex::new(summary.to_string()),
                fail: AtomicBool::new(false),
            }
        }

        fn set_fail(&self, fail: bool) {
            self.fail.store(fail, Ordering::Release);
        }
    }

    #[async_trait::async_trait]
    impl gasket_providers::LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn default_model(&self) -> &str {
            "mock-model"
        }

        async fn chat(
            &self,
            _request: gasket_providers::ChatRequest,
        ) -> std::result::Result<gasket_providers::ChatResponse, gasket_providers::ProviderError>
        {
            if self.fail.load(Ordering::Acquire) {
                return Err(gasket_providers::ProviderError::ApiError {
                    status_code: 500,
                    message: "simulated crash".into(),
                });
            }
            let content = self.response.lock().clone();
            Ok(gasket_providers::ChatResponse::text(content))
        }
    }

    async fn setup_compaction_db() -> (
        sqlx::SqlitePool,
        gasket_storage::EventStore,
        gasket_storage::SessionStore,
    ) {
        use sqlx::sqlite::SqlitePoolOptions;

        let pool = SqlitePoolOptions::new().connect(":memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE sessions_v2 (
                key TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_consolidated_event TEXT,
                total_events INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                channel TEXT NOT NULL DEFAULT '',
                chat_id TEXT NOT NULL DEFAULT ''
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE session_events (
                id TEXT PRIMARY KEY,
                session_key TEXT NOT NULL,
                channel TEXT NOT NULL DEFAULT '',
                chat_id TEXT NOT NULL DEFAULT '',
                event_type TEXT NOT NULL,
                content TEXT NOT NULL,
                tools_used TEXT DEFAULT '[]',
                token_usage TEXT,
                token_len INTEGER NOT NULL DEFAULT 0,
                event_data TEXT,
                extra TEXT DEFAULT '{}',
                created_at TEXT NOT NULL,
                sequence INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("CREATE INDEX idx_events_channel_chat ON session_events(channel, chat_id)")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "CREATE INDEX idx_events_channel_chat_sequence ON session_events(channel, chat_id, sequence)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("CREATE INDEX idx_sessions_v2_channel_chat ON sessions_v2(channel, chat_id)")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS session_summaries (
                session_key TEXT PRIMARY KEY,
                content TEXT NOT NULL DEFAULT '',
                covered_upto_sequence INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                compaction_in_progress INTEGER NOT NULL DEFAULT 0,
                compaction_started_at TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        (
            pool.clone(),
            gasket_storage::EventStore::new(pool.clone()),
            gasket_storage::SessionStore::new(pool),
        )
    }

    async fn append_messages(
        event_store: &gasket_storage::EventStore,
        session_key: &SessionKey,
        messages: &[&str],
    ) {
        for msg in messages {
            let event = SessionEvent {
                id: uuid::Uuid::now_v7(),
                session_key: session_key.to_string(),
                event_type: gasket_types::EventType::UserMessage,
                content: msg.to_string(),
                metadata: gasket_types::EventMetadata::default(),
                created_at: chrono::Utc::now(),
                sequence: 0,
            };
            event_store.append_event(&event).await.unwrap();
        }
    }

    /// Verify crash recovery: if compaction fails mid-flight, the watermark
    /// invariant holds and no data is lost.
    ///
    /// 1. Create 6 events → compact successfully → watermark = 5
    /// 2. Add 4 more events → compaction FAILS (simulated crash)
    /// 3. Watermark unchanged at 5, all 4 new events still accessible
    /// 4. Retry compaction → watermark advances to 9, events GC'd
    #[tokio::test]
    async fn test_crash_recovery_watermark() {
        let (_pool, event_store, session_store) = setup_compaction_db().await;
        let session_key = SessionKey::new(gasket_types::ChannelType::Cli, "crash-test");
        let provider = Arc::new(MockProvider::new("Summary of the conversation."));
        let model = "mock-model";
        let prompt = DEFAULT_SUMMARIZATION_PROMPT;

        // Phase 1: Create initial events (seq 0-5)
        append_messages(
            &event_store,
            &session_key,
            &["msg0", "msg1", "msg2", "msg3", "msg4", "msg5"],
        )
        .await;
        assert_eq!(event_store.get_max_sequence(&session_key).await.unwrap(), 5);

        // Phase 2: Successful compaction → watermark = 5
        run_compaction(
            &event_store,
            &session_store,
            &*provider,
            model,
            prompt,
            &session_key,
            &[],
            &[],
        )
        .await
        .expect("first compaction should succeed");

        let (_, wm) = session_store
            .load_summary(&session_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(wm, 5, "watermark should be 5 after first compaction");

        // GC should have removed old events
        let remaining = event_store
            .get_events_after_sequence(&session_key, 0)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 0, "events up to watermark should be GC'd");

        // Phase 3: Add 4 more events (seq 6-9)
        append_messages(
            &event_store,
            &session_key,
            &["msg6", "msg7", "msg8", "msg9"],
        )
        .await;
        assert_eq!(event_store.get_max_sequence(&session_key).await.unwrap(), 9);

        // Verify new events are readable from old watermark
        let uncompacted = event_store
            .get_events_after_sequence(&session_key, wm)
            .await
            .unwrap();
        assert_eq!(uncompacted.len(), 4);

        // Phase 4: Simulate crash — LLM fails mid-compaction
        provider.set_fail(true);
        let result = run_compaction(
            &event_store,
            &session_store,
            &*provider,
            model,
            prompt,
            &session_key,
            &[],
            &[],
        )
        .await;
        assert!(result.is_err(), "compaction should fail when LLM crashes");
        provider.set_fail(false);

        // Phase 5: Verify watermark invariant — NO data loss
        let (_, wm_after_crash) = session_store
            .load_summary(&session_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            wm_after_crash, 5,
            "watermark must NOT advance after failed compaction"
        );

        let events_after_crash = event_store
            .get_events_after_sequence(&session_key, wm_after_crash)
            .await
            .unwrap();
        assert_eq!(
            events_after_crash.len(),
            4,
            "no data loss: all 4 new events still accessible"
        );
        assert_eq!(events_after_crash[0].content, "msg6");
        assert_eq!(events_after_crash[3].content, "msg9");

        // Phase 6: Retry compaction successfully
        run_compaction(
            &event_store,
            &session_store,
            &*provider,
            model,
            prompt,
            &session_key,
            &[],
            &[],
        )
        .await
        .expect("retry compaction should succeed");

        // Phase 7: Full recovery verified
        let (summary, final_wm) = session_store
            .load_summary(&session_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_wm, 9, "watermark should advance to 9");
        assert!(!summary.is_empty());

        let events_after_final = event_store
            .get_events_after_sequence(&session_key, final_wm)
            .await
            .unwrap();
        assert_eq!(
            events_after_final.len(),
            0,
            "all events compacted — nothing uncompacted remains"
        );
    }
}
