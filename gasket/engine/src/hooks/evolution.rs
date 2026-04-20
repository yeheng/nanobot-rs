//! Evolution hook — auto-learning from conversations via AfterResponse parallel hook.
//!
//! This hook batches conversation events and, once a threshold is reached,
//! triggers a background LLM call to extract persistent facts and skills,
//! writing them into the wiki knowledge system.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::AgentError;
use crate::hooks::{HookAction, HookPoint, PipelineHook, ReadonlyContext};
use crate::session::config::EvolutionConfig;
use crate::wiki::{slugify, PageFilter, PageStore, PageType, WikiPage};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
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
    #[serde(default)]
    verified: bool,
    #[serde(default)]
    confidence: f32,
}

/// Hook that performs self-evolution by extracting insights from conversations.
pub struct EvolutionHook {
    sqlite_store: Arc<SqliteStore>,
    event_store: Arc<EventStore>,
    provider: Arc<dyn LlmProvider>,
    model: String,
    config: EvolutionConfig,
    page_store: Option<Arc<PageStore>>,
}

impl EvolutionHook {
    /// Create a new `EvolutionHook` with all required dependencies.
    pub fn new(
        sqlite_store: Arc<SqliteStore>,
        event_store: Arc<EventStore>,
        provider: Arc<dyn LlmProvider>,
        model: String,
        config: EvolutionConfig,
    ) -> Self {
        Self {
            sqlite_store,
            event_store,
            provider,
            model,
            config,
            page_store: None,
        }
    }

    /// Builder method to set the page store for wiki-based memory storage.
    pub fn with_page_store(mut self, page_store: Arc<PageStore>) -> Self {
        self.page_store = Some(page_store);
        self
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
            "You are a memory extraction sub-system.\n\
             Analyze the following conversation transcript and extract ONLY NEW, PERSISTENT facts, preferences, or actionable skills.\n\n\
             CRITICAL RULES:\n\
             1. DO NOT extract transient context (e.g., 'User said hello').\n\
             2. DO NOT extract information that is likely already known.\n\
             3. Focus on concrete nouns: names, explicit architectural choices, strict preferences.\n\
             4. 'No Execution, No Memory' — only include facts/skills confirmed by successful tool calls.\n\
             5. Classify each item:\n\
                - type: 'note' (factual) or 'skill' (procedural)\n\
                - scenario: 'profile' (user pref), 'knowledge' (env fact), 'procedure' (task skill)\n\
                - verified: true if backed by successful tool result\n\
                - confidence: 0.0-1.0 based on verification strength\n\
             If nothing NEW and VALUABLE is found, return an empty array [].\n\n\
             Output strict JSON array: [{{\"title\": string, \"type\": \"note\"|\"skill\", \"scenario\": \"profile\"|\"knowledge\"|\"procedure\", \"content\": string, \"tags\": [string], \"verified\": bool, \"confidence\": float}}].\n\n{}",
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

        // 8. Write each extracted item into long-term memory (wiki-only).
        for mem in memories {
            // PageStore is required for evolution hook
            let page_store = match &self.page_store {
                Some(ps) => ps,
                None => {
                    warn!("EvolutionHook: PageStore not configured, skipping memory extraction");
                    continue;
                }
            };

            match mem.memory_type.as_str() {
                "skill" => {
                    if let Err(e) = self.persist_as_sop(&mem, page_store).await {
                        warn!("EvolutionHook: failed to persist SOP '{}': {}", mem.title, e);
                    }
                }
                _ => {
                    // Existing note/topic path
                    let path_prefix = match mem.scenario.as_str() {
                        "profile" => "entities/people",
                        _ => "topics",
                    };
                    let page_type = match mem.scenario.as_str() {
                        "profile" => PageType::Entity,
                        _ => PageType::Topic,
                    };

                    let slug = slugify(&mem.title);
                    let page_path = format!("{}/{}", path_prefix, slug);

                    // Deduplication
                    let existing = match page_store.list(PageFilter::default()).await {
                        Ok(pages) => pages,
                        Err(e) => {
                            warn!("EvolutionHook: failed to list pages for dedup: {}", e);
                            continue;
                        }
                    };
                    let is_dup = existing
                        .iter()
                        .any(|p| slugify(&p.title) == slug || p.path.contains(&slug));
                    if is_dup {
                        continue;
                    }

                    let mut tags = mem.tags.clone().unwrap_or_default();
                    tags.push("auto_learned".to_string());

                    let page = WikiPage::new(page_path, mem.title, page_type, mem.content.clone());
                    let mut page = page;
                    page.tags = tags;

                    if let Err(e) = page_store.write(&page).await {
                        warn!("EvolutionHook: failed to create wiki page: {}", e);
                    }
                }
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

impl EvolutionHook {
    /// Persist a skill-type memory as an SOP wiki page.
    async fn persist_as_sop(
        &self,
        mem: &EvolutionMemory,
        page_store: &PageStore,
    ) -> Result<(), AgentError> {
        let slug = slugify(&mem.title);

        // Deduplication
        let existing = page_store.list(PageFilter::default()).await.map_err(|e| {
            AgentError::Other(format!("EvolutionHook: failed to list pages for dedup: {}", e))
        })?;
        let is_duplicate = existing
            .iter()
            .any(|p| slugify(&p.title) == slug || p.path.contains(&slug));

        if is_duplicate {
            debug!("EvolutionHook: SOP '{}' already exists. Skipping.", mem.title);
            return Ok(());
        }

        let path = format!("sops/{}", slug);
        let mut page = WikiPage::new(path, mem.title.clone(), PageType::Sop, format_sop_content(mem));

        let mut tags = mem.tags.clone().unwrap_or_default();
        tags.push("auto_learned".to_string());
        if mem.verified {
            tags.push("verified".to_string());
        }
        page.tags = tags;

        page_store.write(&page).await.map_err(|e| {
            AgentError::Other(format!("EvolutionHook: failed to write SOP page: {}", e))
        })?;

        info!("EvolutionHook: created SOP page '{}'", mem.title);
        Ok(())
    }
}

/// Format an EvolutionMemory as SOP Markdown content.
fn format_sop_content(mem: &EvolutionMemory) -> String {
    format!(
        "## Trigger Scenario\n\
         - {}\n\n\
         ## Preconditions\n\
         - (observed during execution)\n\n\
         ## Key Steps\n\
         {}\n\n\
         ## Pitfalls\n\
         - Review before reuse in different environments.\n\n\
         ## Confidence\n\
         - {:.1}% (verified: {})",
        mem.scenario, mem.content, mem.confidence * 100.0, mem.verified
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_with_verified() {
        let json = r#"[{"title":"Docker Build","type":"skill","scenario":"procedure","content":"Run docker build","tags":["docker"],"verified":true,"confidence":0.95}]"#;
        let mems = EvolutionHook::extract_json(json).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].memory_type, "skill");
        assert!(mems[0].verified);
        assert!((mems[0].confidence - 0.95).abs() < 0.01);
    }

    #[test]
    fn test_format_sop_content() {
        let mem = EvolutionMemory {
            title: "Test".to_string(),
            memory_type: "skill".to_string(),
            scenario: "procedure".to_string(),
            content: "1. Step one\n2. Step two".to_string(),
            tags: Some(vec!["docker".to_string()]),
            verified: true,
            confidence: 0.9,
        };
        let content = format_sop_content(&mem);
        assert!(content.contains("Trigger Scenario"));
        assert!(content.contains("Preconditions"));
        assert!(content.contains("Key Steps"));
        assert!(content.contains("Pitfalls"));
        assert!(content.contains("Step one"));
        assert!(content.contains("90.0%"));
        assert!(content.contains("verified: true"));
    }

    #[test]
    fn test_extract_json_backward_compat() {
        // Old format without verified/confidence should still parse (defaults)
        let json = r#"[{"title":"Test","type":"note","scenario":"knowledge","content":"fact","tags":[]}]"#;
        let mems = EvolutionHook::extract_json(json).unwrap();
        assert_eq!(mems.len(), 1);
        assert!(!mems[0].verified);
        assert_eq!(mems[0].confidence, 0.0);
    }
}
