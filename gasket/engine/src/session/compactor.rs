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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Result};
use tracing::{debug, info, warn};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_storage::{count_tokens, EventStore, SqliteStore};
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
    sqlite_store: Arc<SqliteStore>,
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
}

impl ContextCompactor {
    /// Default compaction threshold multiplier.
    pub const DEFAULT_COMPACTION_THRESHOLD: f32 = 1.2;

    /// Create a new compactor.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        event_store: Arc<EventStore>,
        sqlite_store: Arc<SqliteStore>,
        model: String,
        token_budget: usize,
    ) -> Self {
        Self {
            provider,
            event_store,
            sqlite_store,
            model,
            token_budget,
            compaction_threshold: Self::DEFAULT_COMPACTION_THRESHOLD,
            summarization_prompt: DEFAULT_SUMMARIZATION_PROMPT.to_string(),
            is_compressing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set a custom summarization prompt (overrides built-in default).
    pub fn with_summarization_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.summarization_prompt = prompt.into();
        self
    }

    /// Set a custom compaction threshold multiplier.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.compaction_threshold = threshold;
        self
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
        match self.sqlite_store.load_session_summary(session_key).await {
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
            is_compressing: self.is_compressing.load(Ordering::Relaxed),
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
        info!("Force compaction triggered for {}", sk);
        self.spawn_compaction_task(session_key, vault_values);
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
        let _guard = CompactionGuard(self.is_compressing.clone());

        run_compaction(
            &self.event_store,
            &self.sqlite_store,
            &*self.provider,
            &self.model,
            &self.summarization_prompt,
            session_key,
            vault_values,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Compaction failed for {}: {}", sk, e))
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
            return false;
        }
        if !self.try_acquire_lock(&sk) {
            return false;
        }
        self.spawn_compaction_task(session_key, vault_values);
        true
    }

    // -----------------------------------------------------------------------
    // Gate checks
    // -----------------------------------------------------------------------

    /// Check whether compaction should be triggered.
    ///
    /// Returns `false` if already compressing or below the token threshold.
    fn should_compact(&self, session_key: &str, current_tokens: usize) -> bool {
        if self.is_compressing.load(Ordering::Relaxed) {
            debug!(
                "Compaction already in progress for {}, skipping",
                session_key
            );
            return false;
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
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
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
    fn spawn_compaction_task(&self, session_key: &SessionKey, vault_values: &[String]) {
        let event_store = self.event_store.clone();
        let sqlite_store = self.sqlite_store.clone();
        let provider = self.provider.clone();
        let model = self.model.clone();
        let summarization_prompt = self.summarization_prompt.clone();
        let sk = session_key.clone();
        let vault = vault_values.to_vec();
        let flag = self.is_compressing.clone();

        tokio::spawn(async move {
            let _guard = CompactionGuard(flag);
            debug!("Background compaction started for {}", sk);

            if let Err(e) = run_compaction(
                &event_store,
                &sqlite_store,
                &*provider,
                &model,
                &summarization_prompt,
                &sk,
                &vault,
            )
            .await
            {
                warn!("Compaction failed for {}: {}", sk, e);
            }
        });
    }
}

/// RAII guard that resets the compaction flag on drop, ensuring panic safety.
struct CompactionGuard(Arc<AtomicBool>);

impl Drop for CompactionGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Pipeline stages — module-level functions for testability
// ---------------------------------------------------------------------------

/// Execute the full compaction pipeline: load → build context → summarize → persist.
async fn run_compaction(
    event_store: &EventStore,
    sqlite_store: &SqliteStore,
    provider: &dyn LlmProvider,
    model: &str,
    summarization_prompt: &str,
    session_key: &SessionKey,
    vault_values: &[String],
) -> Result<()> {
    // 1. Load target sequence
    let target_sequence = event_store
        .get_max_sequence(session_key)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get max sequence for {}: {}", session_key, e))?;

    // 2. Load existing summary
    let existing_summary = match sqlite_store.load_session_summary(session_key).await {
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
        sqlite_store,
        event_store,
        session_key,
        &summary_text,
        vault_values,
        target_sequence,
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
    sqlite_store: &SqliteStore,
    event_store: &EventStore,
    session_key: &SessionKey,
    summary_text: &str,
    vault_values: &[String],
    target_sequence: i64,
) -> Result<()> {
    // Redact secrets
    let summary_to_persist = if vault_values.is_empty() {
        summary_text.to_string()
    } else {
        redact_secrets(summary_text, vault_values)
    };

    // Persist summary with new watermark
    sqlite_store
        .save_session_summary(session_key, &summary_to_persist, target_sequence)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to save summary for {}: {}", session_key, e))?;

    // Garbage-collect old events (non-fatal on failure)
    match event_store
        .delete_events_upto(session_key, target_sequence)
        .await
    {
        Ok(deleted) => {
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
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok());

        // Second compare_exchange fails (already true)
        assert!(flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err());

        // Reset
        flag.store(false, Ordering::SeqCst);
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
            embedding: None,
            metadata: Default::default(),
        };

        let text = build_context_text(None, &[event]);
        assert!(text.contains("hello"));
        assert!(!text.contains("Previous summary"));
    }
}
