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
//! 1. is_compressing == false && token_budget exceeded → set flag, capture target_sequence
//! 2. tokio::spawn → load events up to target_sequence → call LLM → upsert summary → GC → clear flag
//! ```
//!
//! # Crash Safety
//!
//! If the process crashes during compaction, nothing is lost: the summary
//! watermark wasn't updated, so the next startup loads slightly more history
//! and triggers compaction again. This is **natural idempotency**.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing::{debug, info, warn};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_storage::{EventStore, SqliteStore};

use crate::agent::count_tokens;
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
    pub async fn load_summary_with_watermark(&self, session_key: &str) -> (String, i64) {
        match self.sqlite_store.load_session_summary(session_key).await {
            Ok(Some((content, watermark))) => (content, watermark),
            Ok(None) => (String::new(), 0),
            Err(e) => {
                debug!("Failed to load summary for {}: {}", session_key, e);
                (String::new(), 0)
            }
        }
    }

    /// Try to trigger background compaction.
    ///
    /// This is the main entry point, called from `finalize_response()`.
    /// If the `is_compressing` guard is already set, this is a no-op (compaction
    /// is already in progress for this session).
    ///
    /// # Arguments
    ///
    /// * `session_key` — session to compact
    /// * `current_tokens` — estimated token count of the current context
    /// * `vault_values` — secrets to redact from the persisted summary
    ///
    /// # Returns
    ///
    /// `true` if compaction was triggered, `false` if skipped (already compressing
    /// or below threshold).
    pub fn try_compact(
        &self,
        session_key: &str,
        current_tokens: usize,
        vault_values: &[String],
    ) -> bool {
        // Guard: already compressing?
        if self.is_compressing.load(Ordering::Relaxed) {
            debug!(
                "Compaction already in progress for {}, skipping",
                session_key
            );
            return false;
        }

        // Threshold check: only compact when tokens exceed budget * threshold
        let overflow_threshold = (self.token_budget as f32 * self.compaction_threshold) as usize;
        if current_tokens < overflow_threshold {
            debug!(
                "Skipping compaction for {}: {} tokens < threshold {} (budget={}, threshold={})",
                session_key,
                current_tokens,
                overflow_threshold,
                self.token_budget,
                self.compaction_threshold
            );
            return false;
        }

        // Set the guard — compare_exchange ensures no race with another caller
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

        // Capture current max sequence as the compaction target
        let event_store = self.event_store.clone();
        let sqlite_store = self.sqlite_store.clone();
        let provider = self.provider.clone();
        let model = self.model.clone();
        let summarization_prompt = self.summarization_prompt.clone();
        let sk = session_key.to_string();
        let vault = vault_values.to_vec();
        let flag = self.is_compressing.clone();

        tokio::spawn(async move {
            debug!("Background compaction started for {}", sk);

            // 1. Get the current max sequence as compaction target
            let target_sequence = match event_store.get_max_sequence(&sk).await {
                Ok(seq) => seq,
                Err(e) => {
                    warn!("Compaction: failed to get max sequence for {}: {}", sk, e);
                    flag.store(false, Ordering::SeqCst);
                    return;
                }
            };

            // 2. Load existing summary
            let (existing_summary, _old_watermark) =
                match sqlite_store.load_session_summary(&sk).await {
                    Ok(Some((content, watermark))) => (Some(content), watermark),
                    Ok(None) => (None, 0),
                    Err(e) => {
                        warn!("Compaction: failed to load summary for {}: {}", sk, e);
                        flag.store(false, Ordering::SeqCst);
                        return;
                    }
                };

            // 3. Load events to compact (up to target, excluding summaries)
            let events = match event_store
                .get_events_up_to_sequence(&sk, target_sequence)
                .await
            {
                Ok(events) => events,
                Err(e) => {
                    warn!("Compaction: failed to load events for {}: {}", sk, e);
                    flag.store(false, Ordering::SeqCst);
                    return;
                }
            };

            if events.is_empty() {
                debug!("No events to compact for {}", sk);
                flag.store(false, Ordering::SeqCst);
                return;
            }

            // 4. Build context for LLM: existing summary + events
            let mut context_parts = Vec::new();
            if let Some(ref existing) = existing_summary {
                if !existing.is_empty() {
                    context_parts.push(format!("Previous summary:\n{}", existing));
                }
            }

            for event in &events {
                context_parts.push(format!("{:?}: {}", event.event_type, event.content));
            }

            let context_text = context_parts.join("\n");
            let context_tokens = count_tokens(&context_text);
            debug!(
                "Compaction context for {}: {} tokens, {} events (up to seq {})",
                sk,
                context_tokens,
                events.len(),
                target_sequence
            );

            // 5. Call LLM for summarization
            let request = ChatRequest {
                model: model.clone(),
                messages: vec![
                    ChatMessage::system(&summarization_prompt),
                    ChatMessage::user(context_text),
                ],
                tools: None,
                temperature: Some(0.3),
                max_tokens: Some(1024),
                thinking: None,
            };

            match provider.chat(request).await {
                Ok(response) => {
                    let summary_text = response.content.unwrap_or_default().trim().to_string();

                    if summary_text.is_empty() {
                        warn!("Compaction for {}: LLM returned empty summary", sk);
                        flag.store(false, Ordering::SeqCst);
                        return;
                    }

                    // 6. Redact secrets
                    let summary_to_persist = if !vault.is_empty() {
                        redact_secrets(&summary_text, &vault)
                    } else {
                        summary_text.clone()
                    };

                    // 7. Upsert summary with new watermark
                    if let Err(e) = sqlite_store
                        .save_session_summary(&sk, &summary_to_persist, target_sequence)
                        .await
                    {
                        warn!("Compaction: failed to save summary for {}: {}", sk, e);
                        flag.store(false, Ordering::SeqCst);
                        return;
                    }

                    // 8. Garbage-collect old events
                    match event_store.delete_events_upto(&sk, target_sequence).await {
                        Ok(deleted) => {
                            info!(
                                "Compaction complete for {}: {} tokens, {} events compacted, {} events GC'd (watermark={})",
                                sk,
                                count_tokens(&summary_text),
                                events.len(),
                                deleted,
                                target_sequence
                            );
                        }
                        Err(e) => {
                            warn!("Compaction: summary saved but GC failed for {}: {}", sk, e);
                            // Non-fatal: summary is saved, GC will retry on next compaction
                        }
                    }
                }
                Err(e) => {
                    warn!("Compaction: LLM call failed for {}: {}", sk, e);
                }
            }

            // 9. Always clear the flag, even on failure
            flag.store(false, Ordering::SeqCst);
        });

        true
    }
}

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
}
