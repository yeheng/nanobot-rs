//! Summarization service for conversation history.
//!
//! Provides LLM-based summarization for long conversations to manage context window.
//! Also handles embedding generation for evicted events (semantic history recall).

use std::sync::Arc;

use tracing::debug;

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
#[cfg(feature = "local-embedding")]
use gasket_storage::TextEmbedder;
use gasket_storage::{top_k_similar, EventStore, SqliteStore};
use gasket_types::{EventMetadata, EventType, SessionEvent, SummaryType};

use crate::agent::count_tokens;
use crate::vault::redact_secrets;

/// Fixed prompt for LLM summarization
pub const SUMMARIZATION_PROMPT: &str =
    "Summarize the following conversation briefly, keeping key facts.";

/// Prefix for injected summary assistant messages
pub const SUMMARY_PREFIX: &str = "[Conversation Summary]: ";

/// Prefix for recalled history injection
pub const RECALL_PREFIX: &str = "[回忆]";

/// Service for summarizing conversation history using LLM.
///
/// When conversations exceed token budgets, this service generates
/// summaries of older events to preserve context while reducing
/// token usage. Also stores embeddings for semantic history recall.
pub struct SummarizationService {
    provider: Arc<dyn LlmProvider>,
    store: Arc<SqliteStore>,
    /// Event store for persisting summaries as EventType::Summary events.
    event_store: Arc<EventStore>,
    model: String,
    /// Optional text embedder for semantic history recall
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
}

impl SummarizationService {
    /// Create a new summarization service.
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        store: Arc<SqliteStore>,
        event_store: Arc<EventStore>,
        model: String,
    ) -> Self {
        Self {
            provider,
            store,
            event_store,
            model,
            #[cfg(feature = "local-embedding")]
            embedder: None,
        }
    }

    /// Create a new summarization service with embedding support.
    #[cfg(feature = "local-embedding")]
    pub fn with_embedder(
        provider: Arc<dyn LlmProvider>,
        store: Arc<SqliteStore>,
        event_store: Arc<EventStore>,
        model: String,
        embedder: Arc<TextEmbedder>,
    ) -> Self {
        Self {
            provider,
            store,
            event_store,
            model,
            embedder: Some(embedder),
        }
    }

    /// Set the embedder for semantic history recall.
    #[cfg(feature = "local-embedding")]
    pub fn set_embedder(&mut self, embedder: Arc<TextEmbedder>) {
        self.embedder = Some(embedder);
    }

    /// Load an existing summary for a session.
    ///
    /// Queries the latest `EventType::Summary` event from the event stream
    /// instead of the deprecated `session_summaries` table.
    pub async fn load_summary(&self, session_key: &str) -> Option<String> {
        match self
            .event_store
            .get_latest_summary(session_key, "main")
            .await
        {
            Ok(Some(event)) => Some(event.content),
            Ok(None) => None,
            Err(e) => {
                debug!("Failed to load session summary from event store: {}", e);
                None
            }
        }
    }

    /// Generate and store embeddings for evicted events.
    ///
    /// This enables semantic recall of old conversations that were
    /// dropped from the context window. Events that already have
    /// embeddings are skipped to avoid redundant computation.
    #[cfg(feature = "local-embedding")]
    async fn save_evicted_embeddings(&self, session_key: &str, evicted_events: &[SessionEvent]) {
        let Some(ref embedder) = self.embedder else {
            debug!("No embedder configured, skipping embedding generation");
            return;
        };

        // Phase 1: filter out events that already have embeddings
        let mut to_embed: Vec<&SessionEvent> = Vec::new();
        for event in evicted_events {
            let event_id = event.id.to_string();
            match self.store.has_embedding(&event_id).await {
                Ok(true) => {
                    debug!("Embedding already exists for event {}, skipping", event_id);
                }
                Ok(false) => to_embed.push(event),
                Err(e) => {
                    debug!("Failed to check existing embedding for {}: {}", event_id, e);
                    to_embed.push(event); // try anyway
                }
            }
        }

        if to_embed.is_empty() {
            return;
        }

        // Phase 2: batch embed all new events in a single call
        let texts: Vec<String> = to_embed.iter().map(|e| e.content.clone()).collect();
        match embedder.embed_batch(&texts) {
            Ok(embeddings) => {
                for (event, embedding) in to_embed.into_iter().zip(embeddings) {
                    let event_id = event.id.to_string();
                    if let Err(e) = self
                        .store
                        .save_embedding(&event_id, session_key, &embedding)
                        .await
                    {
                        debug!(
                            "Failed to save embedding for evicted event {}: {}",
                            event_id, e
                        );
                    } else {
                        debug!(
                            "Saved embedding for evicted event {} in session {}",
                            event_id, session_key
                        );
                    }
                }
            }
            Err(e) => {
                debug!("Batch embedding failed for {} events: {}", texts.len(), e);
            }
        }
    }

    /// Run LLM summarization for evicted (old) events.
    ///
    /// Builds a summarization prompt from existing summary + evicted events
    /// (events that were dropped from context due to token budget).
    /// This preserves information from old events that would otherwise be lost.
    pub async fn summarize(
        &self,
        session_key: &str,
        evicted_events: &[SessionEvent],
        existing_summary: &Option<String>,
        vault_values: &[String],
    ) -> anyhow::Result<String> {
        // Build context for summarization: existing summary + evicted (old) events
        let mut context_parts = Vec::new();
        if let Some(existing) = existing_summary {
            if !existing.is_empty() {
                context_parts.push(format!("Previous summary:\n{}", existing));
            }
        }

        // Include evicted events (old events that were dropped from context)
        for event in evicted_events {
            context_parts.push(format!("{:?}: {}", event.event_type, event.content));
        }

        let context_text = context_parts.join("\n");

        // Count tokens of context to avoid sending too much
        let context_tokens = count_tokens(&context_text);
        debug!(
            "Summarization context: {} tokens, {} evicted events",
            context_tokens,
            evicted_events.len()
        );

        // Build the summarization request
        let summarization_messages = vec![
            ChatMessage::system(SUMMARIZATION_PROMPT),
            ChatMessage::user(context_text),
        ];

        let request = ChatRequest {
            model: self.model.clone(),
            messages: summarization_messages,
            tools: None,
            temperature: Some(0.3), // Low temperature for factual summarization
            max_tokens: Some(1024),
            thinking: None,
        };

        let response = self.provider.chat(request).await?;
        let summary_text = response.content.unwrap_or_default().trim().to_string();

        if summary_text.is_empty() {
            anyhow::bail!("Summarization returned empty content");
        }

        // Redact secrets before persisting to prevent leaked vault values
        let summary_to_persist = if !vault_values.is_empty() {
            redact_secrets(&summary_text, vault_values)
        } else {
            summary_text.clone()
        };

        // Persist the summary as an EventType::Summary event (single source of truth)
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
            "Generated and saved summary event for {}: {} tokens, covering {} events",
            session_key,
            count_tokens(&summary_text),
            evicted_events.len()
        );

        Ok(summary_text)
    }

    /// Context compression hook.
    ///
    /// Called after history truncation; generates summary and saves embeddings
    /// for evicted events. The returned summary is injected into the prompt.
    pub async fn compress(
        &self,
        session_key: &str,
        evicted_events: &[SessionEvent],
        vault_values: &[String],
    ) -> anyhow::Result<Option<String>> {
        if !evicted_events.is_empty() {
            // Save embeddings for evicted events (enables semantic recall)
            #[cfg(feature = "local-embedding")]
            self.save_evicted_embeddings(session_key, evicted_events)
                .await;

            let existing_summary = self.load_summary(session_key).await;
            match self
                .summarize(session_key, evicted_events, &existing_summary, vault_values)
                .await
            {
                Ok(new_summary) => Ok(Some(new_summary)),
                Err(e) => {
                    tracing::warn!(
                        "Summarization failed, using existing summary as fallback: {}",
                        e
                    );
                    Ok(existing_summary)
                }
            }
        } else {
            Ok(self.load_summary(session_key).await)
        }
    }

    /// Recall relevant historical events based on semantic similarity.
    ///
    /// Returns the top-K most relevant events from the embedding store
    /// for the given query embedding.
    pub async fn recall_history(
        &self,
        session_key: &str,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Vec<(String, f32)> {
        let Ok(embeddings) = self.store.load_session_embeddings(session_key).await else {
            debug!("Failed to load session embeddings for {}", session_key);
            return Vec::new();
        };

        if embeddings.is_empty() {
            return Vec::new();
        }

        // Build candidates list for top_k_similar
        // embeddings format: Vec<(event_id, content, embedding)>
        let candidates: Vec<(String, Vec<f32>)> = embeddings
            .iter()
            .map(|(event_id, _content, embedding)| (event_id.clone(), embedding.clone()))
            .collect();

        // Find top-K similar
        let top_results = top_k_similar(query_embedding, &candidates, top_k);

        // Map back to content using the original embeddings list
        let content_map: std::collections::HashMap<String, &str> = embeddings
            .iter()
            .map(|(event_id, content, _)| (event_id.clone(), content.as_str()))
            .collect();

        top_results
            .into_iter()
            .filter_map(|(event_id, score)| {
                content_map
                    .get(event_id)
                    .map(|content| (content.to_string(), score))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::const_is_empty)]
    fn test_summarization_prompt_not_empty() {
        assert!(
            !SUMMARIZATION_PROMPT.is_empty(),
            "SUMMARIZATION_PROMPT should not be empty"
        );
    }

    #[test]
    fn test_summary_prefix_format() {
        assert!(SUMMARY_PREFIX.starts_with('['));
        assert!(SUMMARY_PREFIX.ends_with(": "));
    }

    #[test]
    fn test_recall_prefix_format() {
        assert!(RECALL_PREFIX.starts_with('['));
        assert!(RECALL_PREFIX.ends_with(']'));
    }
}
