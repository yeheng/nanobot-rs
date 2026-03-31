//! Summarization service for conversation history.
//!
//! Provides LLM-based summarization for long conversations to manage context window.
//! Also handles embedding generation for evicted events (semantic history recall).

use std::sync::Arc;

use tracing::debug;

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_storage::top_k_similar;
use gasket_storage::SqliteStore;
#[cfg(feature = "local-embedding")]
use gasket_storage::TextEmbedder;
use gasket_types::SessionEvent;

use crate::agent::count_tokens;

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
    model: String,
    /// Optional text embedder for semantic history recall
    #[cfg(feature = "local-embedding")]
    embedder: Option<Arc<TextEmbedder>>,
}

impl SummarizationService {
    /// Create a new summarization service.
    pub fn new(provider: Arc<dyn LlmProvider>, store: Arc<SqliteStore>, model: String) -> Self {
        Self {
            provider,
            store,
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
        model: String,
        embedder: Arc<TextEmbedder>,
    ) -> Self {
        Self {
            provider,
            store,
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
    pub async fn load_summary(&self, session_key: &str) -> Option<String> {
        match self.store.load_session_summary(session_key).await {
            Ok(s) => s,
            Err(e) => {
                debug!("Failed to load session summary: {}", e);
                None
            }
        }
    }

    /// Generate and store embeddings for evicted events.
    ///
    /// This enables semantic recall of old conversations that were
    /// dropped from the context window.
    #[cfg(feature = "local-embedding")]
    async fn save_evicted_embeddings(&self, session_key: &str, evicted_events: &[SessionEvent]) {
        let Some(ref embedder) = self.embedder else {
            debug!("No embedder configured, skipping embedding generation");
            return;
        };

        for (idx, event) in evicted_events.iter().enumerate() {
            // Generate a unique event ID based on session key and index
            let event_id = format!("{}:evicted:{}", session_key, idx);

            match embedder.embed(&event.content) {
                Ok(embedding) => {
                    if let Err(e) = self
                        .store
                        .save_embedding(&event_id, session_key, &embedding)
                        .await
                    {
                        debug!("Failed to save embedding for evicted event {}: {}", idx, e);
                    } else {
                        debug!(
                            "Saved embedding for evicted event {} in session {}",
                            idx, session_key
                        );
                    }
                }
                Err(e) => {
                    debug!("Failed to embed evicted event {}: {}", idx, e);
                }
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

        // Persist the summary
        self.store
            .save_session_summary(session_key, &summary_text)
            .await?;

        debug!(
            "Generated and saved session summary for {}: {} tokens",
            session_key,
            count_tokens(&summary_text)
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
    ) -> anyhow::Result<Option<String>> {
        if !evicted_events.is_empty() {
            // Save embeddings for evicted events (enables semantic recall)
            #[cfg(feature = "local-embedding")]
            self.save_evicted_embeddings(session_key, evicted_events)
                .await;

            let existing_summary = self.load_summary(session_key).await;
            match self
                .summarize(session_key, evicted_events, &existing_summary)
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
