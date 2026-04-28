//! Evolution maintenance tool — background auto-learning from conversations.
//!
//! Scans all sessions periodically, extracts persistent facts and skills
//! from unprocessed events, and writes them into the wiki knowledge system.
//! Designed to run as a cron job, completely decoupled from the hot path.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, instrument, warn};

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::wiki::{slugify, PageFilter, PageStore, PageType, WikiPage};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_storage::EventStore;
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

/// Default evolution user prompt template (fallback when not configured).
/// Must contain `{{conversation}}` which will be replaced with the transcript.
const DEFAULT_EVOLUTION_TEMPLATE: &str =
    "You are a memory extraction sub-system.\n\
     Analyze the following conversation transcript and extract ONLY NEW, PERSISTENT facts, preferences, or actionable skills.\n\n\
     CRITICAL RULES:\n\
     1. DO NOT extract transient context (e.g., 'User said hello', 'thanks', 'bye').\n\
     2. DO NOT extract generic world knowledge unless the user explicitly adopted it as their own preference or decision.\n\
     3. Focus on concrete, personal signals: server names, product preferences, budget habits, explicit choices, recurring topics.\n\
     4. 'User said it, it counts' — facts and preferences explicitly stated by the user are valuable even if no tool was executed. Tool results add confidence but are NOT required.\n\
     5. Classify each item:\n\
        - type: 'note' (factual) or 'skill' (procedural)\n\
        - scenario: 'profile' (user pref), 'knowledge' (env fact), 'procedure' (task skill)\n\
        - verified: true ONLY if backed by a successful tool result; false for user-stated facts\n\
        - confidence: 0.0-1.0 — 0.9+ for tool-confirmed, 0.6-0.8 for user explicitly stated, 0.3-0.5 for inferred\n\n\
     EXAMPLES OF GOOD EXTRACTIONS:\n\
     - User asked about 'tx-cloud-3' server and uses tmux → note, knowledge, content: 'User manages a server named tx-cloud-3 and prefers tmux for persistent sessions', verified: false, confidence: 0.75\n\
     - User compared Dyson V15 vs 追觅Z20 and preferred the latter for auto-dust-collection → note, profile, content: 'User values automatic dust-collection feature in vacuum cleaners; prefers 追觅Z20 over Dyson V15 for this reason', verified: false, confidence: 0.8\n\
     - User repeatedly asks about budget-friendly options before considering premium → note, profile, content: 'User typically evaluates budget/性价比 options before premium alternatives', verified: false, confidence: 0.65\n\n\
     EXAMPLES OF BAD EXTRACTIONS (do NOT include):\n\
     - 'User greeted the assistant'\n\
     - 'User asked about vacuum cleaners' (too vague; extract specific models or preferences instead)\n\
     - 'Dyson is a well-known brand' (generic knowledge, not user-specific)\n\n\
     If nothing NEW and VALUABLE is found, return an empty array [].\n\n\
     Output strict JSON array: [{\"title\": string, \"type\": \"note\"|\"skill\", \"scenario\": \"profile\"|\"knowledge\"|\"procedure\", \"content\": string, \"tags\": [string], \"verified\": bool, \"confidence\": float}].\n\n\
     {{conversation}}";

/// Tool for performing background evolution (auto-learning) on conversation sessions.
pub struct EvolutionTool {
    session_store: gasket_storage::SessionStore,
    maintenance_store: gasket_storage::MaintenanceStore,
    provider: Arc<dyn LlmProvider>,
    model: String,
    page_store: Option<PageStore>,
    default_threshold: usize,
    evolution_prompt: Option<String>,
}

impl EvolutionTool {
    /// Create a new `EvolutionTool` with all required dependencies.
    pub fn new(
        session_store: gasket_storage::SessionStore,
        maintenance_store: gasket_storage::MaintenanceStore,
        provider: Arc<dyn LlmProvider>,
        model: String,
        page_store: Option<PageStore>,
        default_threshold: usize,
        evolution_prompt: Option<String>,
    ) -> Self {
        Self {
            session_store,
            maintenance_store,
            provider,
            model,
            page_store,
            default_threshold,
            evolution_prompt,
        }
    }

    /// Scan all sessions and return those that need evolution.
    async fn scan_sessions(&self, threshold: usize) -> Result<Vec<(String, i64, i64)>, ToolError> {
        let rows = self
            .session_store
            .scan_active_sessions()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to scan sessions: {}", e)))?;

        let mut qualifying = Vec::new();
        for (session_key, total_events) in rows {
            let watermark = self
                .maintenance_store
                .read_watermark("evolution", &session_key)
                .await
                .map_err(|e| {
                    ToolError::ExecutionError(format!("Failed to read watermark: {}", e))
                })?;

            let delta = total_events.saturating_sub(watermark);
            if delta >= threshold as i64 {
                qualifying.push((session_key, total_events, watermark));
            } else {
                debug!(
                    "Evolution: session {} delta {} < threshold {}, skipping.",
                    session_key, delta, threshold
                );
            }
        }

        Ok(qualifying)
    }

    /// Process a single session: fetch events, extract memories, persist to wiki, update watermark.
    async fn process_session(&self, session_key: &str, watermark: i64) -> Result<usize, ToolError> {
        let session_key_parsed = SessionKey::parse(session_key)
            .unwrap_or_else(|| SessionKey::new(gasket_types::ChannelType::Cli, session_key));

        let event_store = EventStore::new(self.session_store.pool());

        // Fetch events since the last watermark.
        let events = event_store
            .get_events_after_sequence(&session_key_parsed, watermark)
            .await
            .map_err(|e| {
                ToolError::ExecutionError(format!("Failed to load events for evolution: {}", e))
            })?;

        if events.is_empty() {
            debug!("Evolution: no new events for session {}", session_key);
            return Ok(0);
        }

        // Build the extraction prompt.
        let conversation = Self::format_events(&events);
        let system_prompt = "You are a structured data extraction engine. Your ONLY output must be a valid JSON array. Do not include markdown code blocks, explanations, or any text outside the JSON.";
        let template = self
            .evolution_prompt
            .as_deref()
            .unwrap_or(DEFAULT_EVOLUTION_TEMPLATE);
        let user_prompt = template.replace("{{conversation}}", &conversation);

        // Call the LLM.
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage::system(system_prompt),
                ChatMessage::user(user_prompt.clone()),
            ],
            tools: None,
            temperature: Some(0.0),
            max_tokens: Some(4096),
            thinking: None,
        };
        info!(
            "Evolution: calling LLM for session {} with {:?} new events since watermark {}",
            session_key, request, watermark
        );

        let response =
            self.provider.chat(request).await.map_err(|e| {
                ToolError::ExecutionError(format!("Evolution LLM call failed: {}", e))
            })?;

        let content = response.content.unwrap_or_default();
        info!(
            "Evolution: LLM response for session {}: {}",
            session_key, content
        );

        // Parse JSON memories — retry once with a stronger re-prompt if parsing fails.
        let memories: Vec<EvolutionMemory> = match Self::extract_json(&content) {
            Ok(m) => m,
            Err(first_err) => {
                debug!(
                    "Evolution: first parse failed: {}. Retrying with stricter prompt.",
                    first_err
                );

                let retry_prompt = format!(
                    "Your previous response was NOT valid JSON. It started with: {:?}\n\n\
                     You MUST output ONLY a JSON array — no markdown, no explanation, no greeting.\n\
                     Output: []",
                    &content[..content.len().min(200)]
                );

                let retry_request = ChatRequest {
                    model: self.model.clone(),
                    messages: vec![
                        ChatMessage::system(system_prompt),
                        ChatMessage::user(user_prompt.clone()),
                        ChatMessage::assistant(&content),
                        ChatMessage::user(retry_prompt),
                    ],
                    tools: None,
                    temperature: Some(0.0),
                    max_tokens: Some(4096),
                    thinking: None,
                };

                match self.provider.chat(retry_request).await {
                    Ok(retry_response) => {
                        let retry_content = retry_response.content.unwrap_or_default();
                        match Self::extract_json(&retry_content) {
                            Ok(m) => m,
                            Err(e) => {
                                warn!(
                                    "Evolution: retry also failed to parse as JSON: {}. \
                                     First response (500 chars): {}",
                                    e,
                                    &content[..content.len().min(500)]
                                );
                                return Ok(0);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Evolution: retry LLM call failed: {}", e);
                        return Ok(0);
                    }
                }
            }
        };

        if memories.is_empty() {
            debug!(
                "Evolution: no valuable insights extracted for session {}.",
                session_key
            );
        } else {
            info!(
                "Evolution: extracted {} memory item(s) for session {}",
                memories.len(),
                session_key
            );
        }

        // Persist to wiki.
        let page_store = match &self.page_store {
            Some(ps) => ps,
            None => {
                warn!("Evolution: PageStore not configured, skipping memory extraction");
                return Ok(0);
            }
        };

        let mut existing_slugs: std::collections::HashSet<String> =
            match page_store.list(PageFilter::default()).await {
                Ok(pages) => pages.into_iter().map(|p| slugify(&p.title)).collect(),
                Err(e) => {
                    warn!("Evolution: failed to list pages for dedup: {}", e);
                    std::collections::HashSet::new()
                }
            };

        let mut persisted = 0;
        for mem in memories {
            match mem.memory_type.as_str() {
                "skill" => {
                    if self
                        .persist_as_sop(&mem, page_store, &mut existing_slugs)
                        .await
                        .is_ok()
                    {
                        persisted += 1;
                    }
                }
                _ => {
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

                    if existing_slugs.contains(&slug) {
                        continue;
                    }

                    let mut tags = mem.tags.clone().unwrap_or_default();
                    tags.push("auto_learned".to_string());

                    let page = WikiPage::new(page_path, mem.title, page_type, mem.content.clone());
                    let mut page = page;
                    page.tags = tags;

                    if let Err(e) = page_store.write(&page).await {
                        warn!("Evolution: failed to create wiki page: {}", e);
                    } else {
                        existing_slugs.insert(slug);
                        persisted += 1;
                    }
                }
            }
        }

        // Update watermark to max sequence.
        let max_sequence = event_store
            .get_max_sequence(&session_key_parsed)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to get max sequence: {}", e)))?;

        self.maintenance_store
            .write_watermark("evolution", session_key, max_sequence)
            .await
            .map_err(|e| {
                ToolError::ExecutionError(format!("Failed to write evolution watermark: {}", e))
            })?;

        Ok(persisted)
    }

    /// Extract JSON array from an LLM response.
    fn extract_json(text: &str) -> Result<Vec<EvolutionMemory>, serde_json::Error> {
        let trimmed = text.trim();

        // 1. Try direct parse first.
        if let Ok(val) = serde_json::from_str::<Vec<EvolutionMemory>>(trimmed) {
            return Ok(val);
        }

        // 2. Extract JSON from markdown code blocks using regex.
        static CODE_BLOCK_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let code_block_re = CODE_BLOCK_RE
            .get_or_init(|| Regex::new(r"(?s)```(?:json)?\s*(\[.*?\])\s*```").unwrap());
        if let Some(caps) = code_block_re.captures(trimmed) {
            let block = caps.get(1).map(|m| m.as_str()).unwrap_or(trimmed);
            if let Ok(val) = serde_json::from_str::<Vec<EvolutionMemory>>(block) {
                return Ok(val);
            }
        }

        // 3. Fallback: find the first '[' and last ']' to extract the JSON array.
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

        // 4. Final attempt: parse whatever is left for a clear error message.
        serde_json::from_str::<Vec<EvolutionMemory>>(trimmed)
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

    /// Persist a skill-type memory as an SOP wiki page.
    async fn persist_as_sop(
        &self,
        mem: &EvolutionMemory,
        page_store: &PageStore,
        existing_slugs: &mut std::collections::HashSet<String>,
    ) -> Result<(), ToolError> {
        let slug = slugify(&mem.title);

        if existing_slugs.contains(&slug) {
            debug!("Evolution: SOP '{}' already exists. Skipping.", mem.title);
            return Ok(());
        }

        let path = format!("sops/{}", slug);
        let mut page = WikiPage::new(
            path,
            mem.title.clone(),
            PageType::Sop,
            format_sop_content(mem),
        );

        let mut tags = mem.tags.clone().unwrap_or_default();
        tags.push("auto_learned".to_string());
        if mem.verified {
            tags.push("verified".to_string());
        }
        page.tags = tags;

        page_store
            .write(&page)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to write SOP page: {}", e)))?;

        existing_slugs.insert(slug);
        info!("Evolution: created SOP page '{}'", mem.title);
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
        mem.scenario,
        mem.content,
        mem.confidence * 100.0,
        mem.verified
    )
}

#[async_trait]
impl Tool for EvolutionTool {
    fn name(&self) -> &str {
        "evolution"
    }

    fn description(&self) -> &str {
        "Background evolution maintenance: scan conversation sessions, extract persistent facts \
         and skills via LLM, and persist them to the wiki knowledge system. \
         Runs as a cron job. Parameter 'threshold' controls how many new messages \
         must accumulate before a session is processed (default: 20)."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "threshold": {
                    "type": "integer",
                    "description": "Minimum number of new messages required to trigger evolution for a session",
                    "minimum": 1,
                    "default": 20
                }
            }
        })
    }

    #[instrument(name = "tool.evolution", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            #[serde(default)]
            threshold: Option<usize>,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let threshold = args.threshold.unwrap_or(self.default_threshold);
        if threshold == 0 {
            return Err(ToolError::InvalidArguments(
                "threshold must be >= 1".to_string(),
            ));
        }

        // Scan for qualifying sessions.
        let sessions = self.scan_sessions(threshold).await?;
        if sessions.is_empty() {
            return Ok("Evolution: no sessions need processing.".to_string());
        }

        info!(
            "Evolution: processing {} session(s) with threshold {}",
            sessions.len(),
            threshold
        );

        // Process sessions with bounded concurrency (3 at a time) to balance
        // throughput with API rate limits.
        let mut total_memories = 0;
        let mut processed = 0;
        let stream = futures_util::stream::iter(sessions)
            .map(|(session_key, _total_events, watermark)| {
                let this = &self;
                async move {
                    match this.process_session(&session_key, watermark).await {
                        Ok(count) => {
                            info!(
                                "Evolution: processed session {} ({} memories)",
                                session_key, count
                            );
                            Some((1, count))
                        }
                        Err(e) => {
                            warn!(
                                "Evolution: failed to process session {}: {}",
                                session_key, e
                            );
                            None
                        }
                    }
                }
            })
            .buffer_unordered(3);

        let results: Vec<_> = stream.collect().await;
        for (p, m) in results.into_iter().flatten() {
            processed += p;
            total_memories += m;
        }

        Ok(format!(
            "Evolution complete: {} session(s) processed, {} memory item(s) extracted.",
            processed, total_memories
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_with_verified() {
        let json = r#"[{"title":"Docker Build","type":"skill","scenario":"procedure","content":"Run docker build","tags":["docker"],"verified":true,"confidence":0.95}]"#;
        let mems = EvolutionTool::extract_json(json).unwrap();
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
        let json =
            r#"[{"title":"Test","type":"note","scenario":"knowledge","content":"fact","tags":[]}]"#;
        let mems = EvolutionTool::extract_json(json).unwrap();
        assert_eq!(mems.len(), 1);
        assert!(!mems[0].verified);
        assert_eq!(mems[0].confidence, 0.0);
    }

    #[test]
    fn test_extract_json_from_markdown_code_block() {
        let response = r#"Here is the result:
```json
[{"title":"Docker","type":"skill","scenario":"procedure","content":"Build","tags":[],"verified":true,"confidence":0.9}]
```
Hope that helps!"#;
        let mems = EvolutionTool::extract_json(response).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].title, "Docker");
    }

    #[test]
    fn test_extract_json_from_plain_code_block() {
        let response = r#"```
[{"title":"Test","type":"note","scenario":"knowledge","content":"fact","tags":[],"verified":false,"confidence":0.5}]
```"#;
        let mems = EvolutionTool::extract_json(response).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].title, "Test");
    }

    #[test]
    fn test_extract_json_with_noise_prefix() {
        let noisy = r#"Sure! Here's what I found:
[{"title":"X","type":"note","scenario":"knowledge","content":"y","tags":[]}]
Let me know if you need more."#;
        let mems = EvolutionTool::extract_json(noisy).unwrap();
        assert_eq!(mems.len(), 1);
        assert_eq!(mems[0].title, "X");
    }
}
