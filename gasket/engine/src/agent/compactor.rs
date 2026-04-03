//! Synchronous context compactor — replaces async background summarization.
//!
//! # Design Philosophy
//!
//! Compaction is a **post-response lifecycle step**, not a background task.
//! After the agent responds and the assistant event is persisted, the compactor
//! runs synchronously to ensure the next request always sees the latest summary.
//!
//! This eliminates the race condition where `tokio::spawn` background compression
//! might not complete before the next request arrives, causing stale context.
//!
//! # LSM-Tree Analogy
//!
//! Like an LSM-Tree's compaction:
//! - L0 (active context): recent events in the token budget
//! - L1 (compacted): summary checkpoint
//!   When L0 overflows, we flush to L1.

use std::sync::Arc;

use tracing::debug;

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_storage::EventStore;
use gasket_types::{EventMetadata, EventType, SessionEvent, SummaryType};

use crate::agent::count_tokens;
use crate::vault::redact_secrets;

/// Fixed prompt for LLM summarization.
pub const SUMMARIZATION_PROMPT: &str =
    "Summarize the following conversation briefly, keeping key facts, decisions, and outcomes.";

/// Prefix for injected summary assistant messages.
pub const SUMMARY_PREFIX: &str = "[Conversation Summary]: ";

/// Prefix for recalled history injection.
pub const RECALL_PREFIX: &str = "[回忆]";

/// Synchronous context compactor.
///
/// Called directly (not via `tokio::spawn`) after each agent response.
/// If no events were evicted, this is a no-op.
///
/// # Lifecycle
///
/// ```text
/// AgentLoop::process_direct()
///   → prepare_pipeline()     // history + prompt assembly
///   → run_agent_loop()       // LLM iteration
///   → finalize_response()    // save event + compact + return
/// ```
///
/// Compaction happens at the end of `finalize_response()`, ensuring:
/// 1. The user already received their response (no added latency)
/// 2. The next request will see the updated summary (no stale data)
pub struct ContextCompactor {
    /// LLM provider for generating summaries.
    provider: Arc<dyn LlmProvider>,
    /// Event store for persisting summary events.
    event_store: Arc<EventStore>,
    /// Model to use for summarization.
    model: String,
}

impl ContextCompactor {
    /// Create a new compactor.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        event_store: Arc<EventStore>,
        model: String,
    ) -> Self {
        Self {
            provider,
            event_store,
            model,
        }
    }

    /// Run compaction on evicted events.
    ///
    /// This is the main entry point, called from `finalize_response()`.
    /// Returns the new summary text if compaction occurred, or the existing
    /// summary if no events were evicted.
    ///
    /// # Arguments
    ///
    /// * `session_key` — session to compact
    /// * `evicted_events` — events that exceeded the token budget
    /// * `vault_values` — secrets to redact from the persisted summary
    ///
    /// # Errors
    ///
    /// If summarization fails, returns the existing summary as a fallback.
    /// Errors are logged but do not propagate — a failed compaction must not
    /// block the response pipeline.
    pub async fn compact(
        &self,
        session_key: &str,
        evicted_events: &[SessionEvent],
        vault_values: &[String],
    ) -> anyhow::Result<Option<String>> {
        // Load existing summary checkpoint (L1 layer)
        let existing_summary = self.load_summary(session_key).await?;

        if evicted_events.is_empty() {
            // No evicted events — just return existing summary (fast path)
            return Ok(existing_summary);
        }

        // Generate new summary from: existing + evicted events
        match self
            .summarize(session_key, evicted_events, &existing_summary, vault_values)
            .await
        {
            Ok(new_summary) => Ok(Some(new_summary)),
            Err(e) => {
                tracing::warn!(
                    "Compaction failed, keeping existing summary as fallback: {}",
                    e
                );
                Ok(existing_summary)
            }
        }
    }

    /// Load the latest summary checkpoint for a session.
    ///
    /// Queries the event store for the most recent `EventType::Summary` event.
    async fn load_summary(&self, session_key: &str) -> anyhow::Result<Option<String>> {
        match self
            .event_store
            .get_latest_summary(session_key, "main")
            .await
        {
            Ok(Some(event)) => Ok(Some(event.content)),
            Ok(None) => Ok(None),
            Err(e) => {
                debug!("Failed to load summary for {}: {}", session_key, e);
                Ok(None)
            }
        }
    }

    /// Generate a summary from evicted events using LLM.
    ///
    /// Builds a summarization prompt combining the existing summary (if any)
    /// with the evicted events, then persists the result as an `EventType::Summary`
    /// event in the event store.
    async fn summarize(
        &self,
        session_key: &str,
        evicted_events: &[SessionEvent],
        existing_summary: &Option<String>,
        vault_values: &[String],
    ) -> anyhow::Result<String> {
        // Build context: existing summary (L1) + evicted events (overflow from L0)
        let mut context_parts = Vec::new();
        if let Some(existing) = existing_summary {
            if !existing.is_empty() {
                context_parts.push(format!("Previous summary:\n{}", existing));
            }
        }

        for event in evicted_events {
            context_parts.push(format!("{:?}: {}", event.event_type, event.content));
        }

        let context_text = context_parts.join("\n");
        let context_tokens = count_tokens(&context_text);
        debug!(
            "Compaction context: {} tokens, {} evicted events",
            context_tokens,
            evicted_events.len()
        );

        // LLM summarization request
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(SUMMARIZATION_PROMPT),
                ChatMessage::user(context_text),
            ],
            tools: None,
            temperature: Some(0.3),
            max_tokens: Some(1024),
            thinking: None,
        };

        let response = self.provider.chat(request).await?;
        let summary_text = response.content.unwrap_or_default().trim().to_string();

        if summary_text.is_empty() {
            anyhow::bail!("Summarization returned empty content");
        }

        // Redact secrets before persisting
        let summary_to_persist = if !vault_values.is_empty() {
            redact_secrets(&summary_text, vault_values)
        } else {
            summary_text.clone()
        };

        // Persist as EventType::Summary (single source of truth)
        let covered_ids: Vec<uuid::Uuid> = evicted_events.iter().map(|e| e.id).collect();
        let summary_event = SessionEvent {
            id: uuid::Uuid::now_v7(),
            session_key: session_key.to_string(),
            event_type: EventType::Summary {
                summary_type: SummaryType::Compression { token_budget: 8000 },
                covered_event_ids: covered_ids,
            },
            content: summary_to_persist,
            embedding: None,
            metadata: EventMetadata::default(),
            created_at: chrono::Utc::now(),
        };
        self.event_store.append_event(&summary_event).await?;

        debug!(
            "Compaction complete for {}: {} tokens, covering {} events",
            session_key,
            count_tokens(&summary_text),
            evicted_events.len()
        );

        Ok(summary_text)
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
        assert!(SUMMARY_PREFIX.ends_with(": "));
    }
}
