//! Evolution hook — auto-learning from conversations via AfterResponse parallel hook.
//!
//! This hook batches conversation events and, once a threshold is reached,
//! triggers a background LLM call to extract persistent facts and skills,
//! writing them into long-term memory via `MemoryManager`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::AgentError;
use crate::hooks::{HookAction, HookPoint, PipelineHook, ReadonlyContext};
use crate::session::{EvolutionConfig, MemoryManager};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_storage::memory::{Frequency, Scenario};
use gasket_storage::{EventStore, SqliteStore};
use gasket_types::{EventType, SessionKey};

/// A single extracted memory item from the evolution LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvolutionMemory {
    title: String,
    #[serde(rename = "type")]
    memory_type: String,
    scenario: String,
    content: String,
    tags: Option<Vec<String>>,
}

/// Hook that performs self-evolution by extracting insights from conversations.
pub struct EvolutionHook {
    memory_manager: Arc<MemoryManager>,
    sqlite_store: Arc<SqliteStore>,
    event_store: Arc<EventStore>,
    provider: Arc<dyn LlmProvider>,
    model: String,
    config: EvolutionConfig,
}

impl EvolutionHook {
    /// Create a new `EvolutionHook` with all required dependencies.
    pub fn new(
        memory_manager: Arc<MemoryManager>,
        sqlite_store: Arc<SqliteStore>,
        event_store: Arc<EventStore>,
        provider: Arc<dyn LlmProvider>,
        model: String,
        config: EvolutionConfig,
    ) -> Self {
        Self {
            memory_manager,
            sqlite_store,
            event_store,
            provider,
            model,
            config,
        }
    }

    /// Build the watermark key for a given session.
    fn watermark_key(session_key: &str) -> String {
        format!("evolution_watermark_{}", session_key)
    }

    /// Extract JSON array from an LLM response.
    /// Handles markdown code blocks (```json ... ```) and trims surrounding whitespace/text.
    fn extract_json(text: &str) -> Result<Vec<EvolutionMemory>, serde_json::Error> {
        let trimmed = text.trim();

        // Try direct parse first.
        if let Ok(val) = serde_json::from_str::<Vec<EvolutionMemory>>(trimmed) {
            return Ok(val);
        }

        // Try stripping markdown code block fences.
        let without_fences = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        if let Ok(val) = serde_json::from_str::<Vec<EvolutionMemory>>(without_fences) {
            return Ok(val);
        }

        // Try finding the first '[' and last ']' to extract the JSON array.
        if let Some(start) = trimmed.find('[') {
            if let Some(end) = trimmed.rfind(']') {
                if end > start {
                    let slice = &trimmed[start..=end];
                    if let Ok(val) = serde_json::from_str::<Vec<EvolutionMemory>>(slice) {
                        return Ok(val);
                    }
                }
            }
        }

        // Final attempt: parse the stripped version for a clearer error message.
        serde_json::from_str::<Vec<EvolutionMemory>>(without_fences)
    }

    /// Format events into a conversation transcript for the LLM prompt.
    fn format_events(events: &[gasket_types::SessionEvent]) -> String {
        events
            .iter()
            .map(|e| {
                let role = match &e.event_type {
                    EventType::UserMessage => "User",
                    EventType::AssistantMessage => "Assistant",
                    EventType::ToolCall { tool_name, .. } => {
                        return format!("[Tool call: {}]", tool_name);
                    }
                    EventType::ToolResult {
                        tool_name,
                        is_error,
                        ..
                    } => {
                        return format!(
                            "[Tool result: {} — {}]",
                            tool_name,
                            if *is_error { "error" } else { "success" }
                        );
                    }
                    EventType::Summary { .. } => "[Summary]",
                };
                format!("{}: {}", role, e.content)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait]
impl PipelineHook for EvolutionHook {
    fn name(&self) -> &str {
        "evolution"
    }

    fn point(&self) -> HookPoint {
        HookPoint::AfterResponse
    }

    async fn run_parallel(&self, ctx: &ReadonlyContext<'_>) -> Result<HookAction, AgentError> {
        let session_key = SessionKey::parse(ctx.session_key)
            .unwrap_or_else(|| SessionKey::new(gasket_types::ChannelType::Cli, ctx.session_key));

        // 1. Get current max sequence for this session.
        let max_sequence = self
            .event_store
            .get_max_sequence(&session_key)
            .await
            .map_err(|e| {
                AgentError::Other(format!("Failed to get max sequence for evolution: {}", e))
            })?;

        if max_sequence == 0 {
            return Ok(HookAction::Continue);
        }

        // 2. Read the last evolution watermark.
        let watermark_key = Self::watermark_key(ctx.session_key);
        let watermark: i64 = self
            .sqlite_store
            .read_raw(&watermark_key)
            .await
            .map_err(|e| AgentError::Other(format!("Failed to read evolution watermark: {}", e)))?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        // 3. Threshold check (debounce).
        let delta = max_sequence.saturating_sub(watermark);
        if delta < self.config.batch_messages as i64 {
            debug!(
                "EvolutionHook: delta {} < threshold {}, skipping.",
                delta, self.config.batch_messages
            );
            return Ok(HookAction::Continue);
        }

        // 4. Fetch events since the last watermark.
        let events = self
            .event_store
            .get_events_after_sequence(&session_key, watermark)
            .await
            .map_err(|e| {
                AgentError::Other(format!("Failed to load events for evolution: {}", e))
            })?;

        if events.is_empty() {
            return Ok(HookAction::Continue);
        }

        // 5. Build the extraction prompt.
        let conversation = Self::format_events(&events);
        let system_prompt = "You are a structured data extraction engine. Your ONLY output must be a valid JSON array. Do not include markdown code blocks, explanations, or any text outside the JSON.";
        let user_prompt = format!(
            "Analyze the following conversation. Extract: \
             1. Persistent facts about the user (preferences, environment). \
             2. Reusable operational skills/procedures. \
             Output strict JSON array: [{{\"title\": string, \"type\": \"note\"|\"skill\", \"scenario\": \"profile\"|\"knowledge\", \"content\": string, \"tags\": [string]}}]. \
             If nothing valuable, return [].\n\n{}",
            conversation
        );

        // 6. Call the LLM (non-streaming, small temperature for deterministic extraction).
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(system_prompt),
                ChatMessage::user(user_prompt),
            ],
            tools: None,
            temperature: Some(0.3),
            max_tokens: Some(4096),
            thinking: None,
        };

        let response = self
            .provider
            .chat(request)
            .await
            .map_err(|e| AgentError::Other(format!("Evolution LLM call failed: {}", e)))?;

        let content = response.content.unwrap_or_default();

        // 7. Extract and parse JSON from LLM response.
        // LLMs sometimes wrap JSON in markdown code blocks or add extra text.
        let memories: Vec<EvolutionMemory> = match Self::extract_json(&content) {
            Ok(m) => m,
            Err(e) => {
                warn!(
                    "EvolutionHook: failed to parse LLM response as JSON: {}. Raw response (first 500 chars): {}",
                    e,
                    &content[..content.len().min(500)]
                );
                return Ok(HookAction::Continue);
            }
        };

        if memories.is_empty() {
            debug!("EvolutionHook: no valuable insights extracted.");
        } else {
            info!(
                "EvolutionHook: extracted {} memory item(s) for session {}",
                memories.len(),
                ctx.session_key
            );
        }

        // 8. Write each extracted item into long-term memory.
        for mem in memories {
            let scenario = match mem.scenario.as_str() {
                "profile" => Scenario::Profile,
                _ => Scenario::Knowledge,
            };

            let mut tags = mem.tags.unwrap_or_default();
            tags.push("auto_learned".to_string());

            if let Err(e) = self
                .memory_manager
                .create_memory(scenario, &mem.title, &tags, Frequency::Warm, &mem.content)
                .await
            {
                warn!(
                    "EvolutionHook: failed to create memory '{}': {}",
                    mem.title, e
                );
            } else {
                debug!("EvolutionHook: created memory '{}'", mem.title);
            }
        }

        // 9. Update the watermark so we don't re-process these events.
        self.sqlite_store
            .write_raw(&watermark_key, &max_sequence.to_string())
            .await
            .map_err(|e| {
                AgentError::Other(format!("Failed to write evolution watermark: {}", e))
            })?;

        Ok(HookAction::Continue)
    }
}
