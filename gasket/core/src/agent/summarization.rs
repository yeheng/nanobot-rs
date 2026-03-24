//! Summarization service for conversation history.
//!
//! Provides LLM-based summarization for long conversations to manage context window.
//! Also handles embedding generation for evicted messages (semantic history recall).

use std::sync::Arc;

use tracing::debug;

use crate::memory::SqliteStore;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider};
use crate::search::{top_k_similar, TextEmbedder};
use crate::session::SessionMessage;

use super::history_processor::count_tokens;

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
/// summaries of older messages to preserve context while reducing
/// token usage. Also stores embeddings for semantic history recall.
pub struct SummarizationService {
    provider: Arc<dyn LlmProvider>,
    store: Arc<SqliteStore>,
    model: String,
    /// Optional text embedder for semantic history recall
    embedder: Option<Arc<TextEmbedder>>,
}

impl SummarizationService {
    /// Create a new summarization service.
    pub fn new(provider: Arc<dyn LlmProvider>, store: Arc<SqliteStore>, model: String) -> Self {
        Self {
            provider,
            store,
            model,
            embedder: None,
        }
    }

    /// Create a new summarization service with embedding support.
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

    /// Generate and store embeddings for evicted messages.
    ///
    /// This enables semantic recall of old conversations that were
    /// dropped from the context window.
    async fn save_evicted_embeddings(
        &self,
        session_key: &str,
        evicted_messages: &[SessionMessage],
    ) {
        let Some(ref embedder) = self.embedder else {
            debug!("No embedder configured, skipping embedding generation");
            return;
        };

        for (idx, msg) in evicted_messages.iter().enumerate() {
            // Generate a unique message ID based on session key and index
            let message_id = format!("{}:evicted:{}", session_key, idx);

            match embedder.embed(&msg.content) {
                Ok(embedding) => {
                    if let Err(e) = self
                        .store
                        .save_embedding(&message_id, session_key, &embedding)
                        .await
                    {
                        debug!(
                            "Failed to save embedding for evicted message {}: {}",
                            idx, e
                        );
                    } else {
                        debug!(
                            "Saved embedding for evicted message {} in session {}",
                            idx, session_key
                        );
                    }
                }
                Err(e) => {
                    debug!("Failed to embed evicted message {}: {}", idx, e);
                }
            }
        }
    }

    /// Run LLM summarization for evicted (old) messages.
    ///
    /// Builds a summarization prompt from existing summary + evicted messages
    /// (messages that were dropped from context due to token budget).
    /// This preserves information from old messages that would otherwise be lost.
    pub async fn summarize(
        &self,
        session_key: &str,
        evicted_messages: &[SessionMessage],
        existing_summary: &Option<String>,
    ) -> anyhow::Result<String> {
        // Build context for summarization: existing summary + evicted (old) messages
        let mut context_parts = Vec::new();
        if let Some(existing) = existing_summary {
            if !existing.is_empty() {
                context_parts.push(format!("Previous summary:\n{}", existing));
            }
        }

        // Include evicted messages (old messages that were dropped from context)
        for msg in evicted_messages {
            context_parts.push(format!("{}: {}", msg.role, msg.content));
        }

        let context_text = context_parts.join("\n");

        // Count tokens of context to avoid sending too much
        let context_tokens = count_tokens(&context_text);
        debug!(
            "Summarization context: {} tokens, {} evicted messages",
            context_tokens,
            evicted_messages.len()
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
    /// for evicted messages. The returned summary is injected into the prompt.
    pub async fn compress(
        &self,
        session_key: &str,
        evicted_messages: &[SessionMessage],
    ) -> anyhow::Result<Option<String>> {
        if !evicted_messages.is_empty() {
            // Save embeddings for evicted messages (enables semantic recall)
            self.save_evicted_embeddings(session_key, evicted_messages)
                .await;

            let existing_summary = self.load_summary(session_key).await;
            match self
                .summarize(session_key, evicted_messages, &existing_summary)
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

    /// Recall relevant historical messages based on semantic similarity.
    ///
    /// Returns the top-K most relevant messages from the embedding store
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
        // embeddings format: Vec<(message_id, content, embedding)>
        let candidates: Vec<(String, Vec<f32>)> = embeddings
            .iter()
            .map(|(msg_id, _content, embedding)| (msg_id.clone(), embedding.clone()))
            .collect();

        // Find top-K similar
        let top_results = top_k_similar(query_embedding, &candidates, top_k);

        // Map back to content using the original embeddings list
        let content_map: std::collections::HashMap<String, &str> = embeddings
            .iter()
            .map(|(msg_id, content, _)| (msg_id.clone(), content.as_str()))
            .collect();

        top_results
            .into_iter()
            .filter_map(|(msg_id, score)| {
                content_map
                    .get(msg_id)
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
