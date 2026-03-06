//! Summarization service for conversation history.
//!
//! Provides LLM-based summarization for long conversations to manage context window.

use std::sync::Arc;

use tracing::debug;

use crate::memory::SqliteStore;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider};
use crate::session::SessionMessage;

use super::history_processor::count_tokens;

/// Fixed prompt for LLM summarization
pub const SUMMARIZATION_PROMPT: &str =
    "Summarize the following conversation briefly, keeping key facts.";

/// Prefix for injected summary assistant messages
pub const SUMMARY_PREFIX: &str = "[Conversation Summary]: ";

/// Service for summarizing conversation history using LLM.
///
/// When conversations exceed token budgets, this service generates
/// summaries of older messages to preserve context while reducing
/// token usage.
pub struct SummarizationService {
    provider: Arc<dyn LlmProvider>,
    store: Arc<SqliteStore>,
    model: String,
}

impl SummarizationService {
    /// Create a new summarization service.
    pub fn new(provider: Arc<dyn LlmProvider>, store: Arc<SqliteStore>, model: String) -> Self {
        Self {
            provider,
            store,
            model,
        }
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
}

// ── ContextCompressionHook ─────────────────────────────────

/// Hook interface for context compression.
///
/// Decouples the compression strategy (LLM summarization, vector retrieval,
/// entity extraction, etc.) from the agent loop.  The `AgentLoop` calls this
/// hook after history truncation; the returned summary is injected into the
/// prompt by `ContextBuilder`.
#[async_trait::async_trait]
pub trait ContextCompressionHook: Send + Sync {
    /// Compress evicted messages into an optional summary string.
    ///
    /// * If `evicted_messages` is non-empty, generate a new summary that
    ///   incorporates them (and any previously persisted summary).
    /// * If `evicted_messages` is empty, return the existing persisted
    ///   summary (if any).
    async fn compress(
        &self,
        session_key: &str,
        evicted_messages: &[SessionMessage],
    ) -> anyhow::Result<Option<String>>;
}

#[async_trait::async_trait]
impl ContextCompressionHook for SummarizationService {
    async fn compress(
        &self,
        session_key: &str,
        evicted_messages: &[SessionMessage],
    ) -> anyhow::Result<Option<String>> {
        if !evicted_messages.is_empty() {
            // Evicted messages exist — generate/update summary
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
            // No evictions — just load any existing summary
            Ok(self.load_summary(session_key).await)
        }
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
}
