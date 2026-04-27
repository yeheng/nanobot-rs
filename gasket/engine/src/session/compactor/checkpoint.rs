//! Proactive checkpoint configuration and generation.

use anyhow::Result;
use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_storage::SessionStore;
use gasket_types::SessionKey;
use tracing::{info, warn};

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

/// Timeout for checkpoint LLM calls — prevents the agent loop from hanging
/// if the provider API is slow or unresponsive.
const CHECKPOINT_TIMEOUT_SECS: u64 = 30;

/// Generate a proactive checkpoint for the current session state.
///
/// Called every N sequence increments. Returns `Some(summary)` if a
/// checkpoint was generated, `None` if skipped.
pub async fn checkpoint(
    provider: &dyn LlmProvider,
    model: &str,
    config: &CheckpointConfig,
    event_store: &gasket_storage::EventStore,
    session_store: &SessionStore,
    session_key: &SessionKey,
    current_max_sequence: i64,
) -> Result<Option<String>> {
    if config.interval_turns == 0
        || current_max_sequence == 0
        || current_max_sequence % config.interval_turns as i64 != 0
    {
        return Ok(None);
    }

    // Load recent events for context
    let events = event_store
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

    let request = ChatRequest {
        model: model.to_string(),
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
        std::time::Duration::from_secs(CHECKPOINT_TIMEOUT_SECS),
        provider.chat(request),
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
                CHECKPOINT_TIMEOUT_SECS, session_key
            );
            return Ok(None);
        }
    };
    let summary = response.content.unwrap_or_default().trim().to_string();

    if summary.is_empty() {
        warn!("Checkpoint generated empty summary for {}", session_key);
        return Ok(None);
    }

    session_store
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
