//! Evolution maintenance tool — background auto-learning from conversations.
//!
//! Scans all sessions periodically, extracts persistent facts and skills
//! from unprocessed events, and writes them into the wiki knowledge system.
//! Designed to run as a cron job, completely decoupled from the hot path.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, instrument, warn};

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::wiki::{slugify, PageStore, PageType, WikiPage};

use gasket_providers::{ChatMessage, ChatRequest, LlmProvider};
use gasket_types::{EventType, SessionKey};

/// A single extracted memory item from the evolution LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvolutionMemory {
    title: String,
    #[serde(rename = "type")]
    memory_type: String,
    scenario: String,
    content: String,
    summary: Option<String>,
    tags: Option<Vec<String>>,
    #[serde(default)]
    verified: bool,
    #[serde(default)]
    confidence: f32,
}

/// Result of the distill meta-analysis step.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DistillResult {
    /// Transferable skill patterns discovered across memories.
    #[serde(default)]
    skill_patterns: Vec<DistillItem>,
    /// Common mistakes or anti-patterns to avoid.
    #[serde(default)]
    anti_patterns: Vec<DistillItem>,
    /// Meta-observations about capability gaps or recurring themes.
    #[serde(default)]
    meta_observations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DistillItem {
    title: String,
    content: String,
    #[serde(default)]
    tags: Vec<String>,
}

/// Prompt for the distill meta-analysis step.
/// Takes extracted memories as input and produces higher-order learnings.
const DISTILL_PROMPT_TEMPLATE: &str =
    "You are a knowledge distillation engine. Given a list of extracted memories from conversation sessions, \
     perform meta-analysis to discover PATTERNS, SKILLS, and ANTI-PATTERNS that are NOT visible in individual items.\n\n\
     Input memories (JSON array):\n{{memories}}\n\n\
     Produce:\n\n\
     1. **Skill Patterns**: Identify transferable procedural patterns that span multiple memories.\n\
        - title: short name for the pattern\n\
        - content: step-by-step procedure or reusable technique\n\
        - tags: relevant categories\n\n\
     2. **Anti-Patterns**: Identify common mistakes, misconceptions, or inefficient approaches observed.\n\
        - title: name of the anti-pattern\n\
        - content: how to detect it and what to do instead\n\
        - tags: relevant categories\n\n\
     3. **Meta-Observations**: One-sentence insights about the user's overall workflow, preferences, or gaps.\n\n\
     RULES:\n\
     - Only produce items that synthesize insights from MULTIPLE memories.\n\
     - If no genuine patterns exist, return empty arrays.\n\
     - Do NOT simply restate individual memories.\n\n\
     Output strict JSON: {\"skill_patterns\": [...], \"anti_patterns\": [...], \"meta_observations\": [...]}";

/// Default evolution user prompt template (fallback when not configured).
/// Must contain `{{conversation}}` which will be replaced with the transcript.
const DEFAULT_EVOLUTION_TEMPLATE: &str =
    "You are a memory extraction sub-system.\n\
     Analyze the following conversation transcript and extract ONLY NEW, PERSISTENT facts, preferences, or actionable skills.\n\n\
     CRITICAL RULES:\n\
     1. DO NOT extract transient context (e.g., 'User said hello', 'thanks', 'bye').\n\
     2. DO NOT extract generic world knowledge unless the user explicitly adopted it as their own preference or decision.\n\
     3. DO NOT extract casual conversation, temporary tasks, or trivial facts.\n\
     4. ONLY extract highly reusable skills, permanent user preferences, or verified factual knowledge.\n\
     5. If nothing meets this high bar, return an empty array [].\n\
     6. Focus on concrete, personal signals: server names, product preferences, budget habits, explicit choices, recurring topics.\n\
     7. 'User said it, it counts' — facts and preferences explicitly stated by the user are valuable even if no tool was executed. Tool results add confidence but are NOT required.\n\
     8. Classify each item:\n\
        - type: 'note' (factual) or 'skill' (procedural)\n\
        - scenario: 'profile' (user pref), 'knowledge' (env fact), 'procedure' (task skill)\n\
        - verified: true ONLY if backed by a successful tool result; false for user-stated facts\n\
        - confidence: 0.0-1.0 — 0.9+ for tool-confirmed, 0.6-0.8 for user explicitly stated, 0.3-0.5 for inferred\n\
     9. ENTITY LINKING: In the `content` field, wrap important entities (people, projects, technologies, products) with double-bracket wiki links using the format [[entities/category/name]] or [[topics/name]]. Examples:\n\
        - 'User manages [[entities/servers/tx-cloud-3]] and prefers tmux'\n\
        - 'User prefers [[topics/rust]] over [[topics/python]] for systems programming'\n\
        - 'User works on [[entities/projects/gasket]]'\n\n\
     EXAMPLES OF GOOD EXTRACTIONS:\n\
     - User asked about 'tx-cloud-3' server and uses tmux → note, knowledge, content: 'User manages [[entities/servers/tx-cloud-3]] and prefers tmux for persistent sessions', verified: false, confidence: 0.75\n\
     - User compared Dyson V15 vs 追觅Z20 and preferred the latter for auto-dust-collection → note, profile, content: 'User values automatic dust-collection feature in vacuum cleaners; prefers [[topics/dreame-z20]] over [[topics/dyson-v15]]', verified: false, confidence: 0.8\n\
     - User repeatedly asks about budget-friendly options before considering premium → note, profile, content: 'User typically evaluates budget/性价比 options before premium alternatives', verified: false, confidence: 0.65\n\n\
     EXAMPLES OF BAD EXTRACTIONS (do NOT include):\n\
     - 'User greeted the assistant'\n\
     - 'User asked about vacuum cleaners' (too vague; extract specific models or preferences instead)\n\
     - 'Dyson is a well-known brand' (generic knowledge, not user-specific)\n\
     - 'User wants to fix a bug today' (temporary task, not persistent knowledge)\n\n\
     If nothing NEW and VALUABLE is found, return an empty array [].\n\n\
     Also include a brief `summary` field for each item: one sentence (max 50 chars) capturing the essence.\n\
     Output strict JSON array: [{\"title\": string, \"type\": \"note\"|\"skill\", \"scenario\": \"profile\"|\"knowledge\"|\"procedure\", \"content\": string, \"summary\": string, \"tags\": [string], \"verified\": bool, \"confidence\": float}].\n\n\
     {{conversation}}";

/// Configuration for building an [`EvolutionTool`].
pub struct EvolutionConfig {
    pub session_store: gasket_storage::SessionStore,
    pub maintenance_store: gasket_storage::MaintenanceStore,
    pub provider: Arc<dyn LlmProvider>,
    pub model: String,
    pub page_store: Option<PageStore>,
    pub event_store: gasket_storage::EventStore,
    pub default_threshold: usize,
    pub evolution_prompt: Option<String>,
    pub distill_prompt: Option<String>,
    /// Maximum number of concurrent evolution tasks (default: 3).
    pub concurrency: usize,
}

/// Tool for performing background evolution (auto-learning) on conversation sessions.
pub struct EvolutionTool {
    session_store: gasket_storage::SessionStore,
    maintenance_store: gasket_storage::MaintenanceStore,
    provider: Arc<dyn LlmProvider>,
    model: String,
    page_store: Option<PageStore>,
    event_store: gasket_storage::EventStore,
    default_threshold: usize,
    evolution_prompt: Option<String>,
    distill_prompt: Option<String>,
    concurrency: usize,
}

impl EvolutionTool {
    /// Create a new `EvolutionTool` with all required dependencies.
    pub fn new(config: EvolutionConfig) -> Self {
        Self {
            session_store: config.session_store,
            maintenance_store: config.maintenance_store,
            provider: config.provider,
            model: config.model,
            page_store: config.page_store,
            event_store: config.event_store,
            default_threshold: config.default_threshold,
            evolution_prompt: config.evolution_prompt,
            distill_prompt: config.distill_prompt,
            concurrency: config.concurrency,
        }
    }

    /// Scan all sessions and return those that need evolution.
    async fn scan_sessions(&self, threshold: usize) -> Result<Vec<(String, i64, i64)>, ToolError> {
        let qualifying = self
            .session_store
            .get_sessions_needing_evolution("evolution", threshold as i64)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to scan sessions: {}", e)))?;

        for (session_key, total_events, watermark) in &qualifying {
            debug!(
                "Evolution: session {} delta {} >= threshold {}, will process.",
                session_key,
                total_events.saturating_sub(*watermark),
                threshold
            );
        }

        Ok(qualifying)
    }

    /// Process a single session: fetch events, extract memories, persist to wiki, update watermark.
    async fn process_session(&self, session_key: &str, watermark: i64) -> Result<usize, ToolError> {
        let session_key_parsed = SessionKey::parse(session_key)
            .unwrap_or_else(|| SessionKey::new(gasket_types::ChannelType::Cli, session_key));

        // Fetch events since the last watermark.
        let events = self
            .event_store
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

                let preview_len = content.chars().count().min(200);
                let preview_end = content
                    .char_indices()
                    .nth(preview_len)
                    .map(|(i, _)| i)
                    .unwrap_or(content.len());
                let retry_prompt = format!(
                    "Your previous response was NOT valid JSON. It started with: {:?}\n\n\
                     You MUST output ONLY a JSON array — no markdown, no explanation, no greeting.\n\
                     Output: []",
                    &content[..preview_end]
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
                                let preview_len = content.chars().count().min(500);
                                let preview_end = content
                                    .char_indices()
                                    .nth(preview_len)
                                    .map(|(i, _)| i)
                                    .unwrap_or(content.len());
                                warn!(
                                    "Evolution: retry also failed to parse as JSON: {}. \
                                     First response (500 chars): {}",
                                    e,
                                    &content[..preview_end]
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

        // Persist to wiki using ADD-only strategy:
        // - If target page already exists, append new memory as a dated update block.
        // - If not, create a new page.
        // This prevents silent data loss when user preferences evolve over time.
        let page_store = match &self.page_store {
            Some(ps) => ps,
            None => {
                warn!("Evolution: PageStore not configured, skipping memory extraction");
                return Ok(0);
            }
        };

        let mut persisted = 0;
        for mem in &memories {
            match mem.memory_type.as_str() {
                "skill" => {
                    if self.persist_as_sop(mem, page_store).await.is_ok() {
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

                    let mut tags = mem.tags.clone().unwrap_or_default();
                    tags.push("auto_learned".to_string());

                    if page_store.exists(&page_path).await.unwrap_or(false) {
                        // Page exists — append new memory as a dated update block.
                        match page_store.read(&page_path).await {
                            Ok(mut existing_page) => {
                                let today = Utc::now().format("%Y-%m-%d").to_string();
                                existing_page.content.push_str(&format!(
                                    "\n\n### {} Update\n\n{}",
                                    today, mem.content
                                ));
                                merge_tags(&mut existing_page.tags, &tags);
                                if let Some(ref summary) = mem.summary {
                                    existing_page.summary = Some(summary.clone());
                                }
                                if let Err(e) = page_store.write(&existing_page).await {
                                    warn!(
                                        "Evolution: failed to append to wiki page '{}': {}",
                                        page_path, e
                                    );
                                } else {
                                    persisted += 1;
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Evolution: failed to read existing page '{}': {}",
                                    page_path, e
                                );
                            }
                        }
                    } else {
                        // New page — create fresh.
                        let mut page = WikiPage::new(
                            page_path,
                            mem.title.clone(),
                            page_type,
                            mem.content.clone(),
                        );
                        page.summary = mem.summary.clone();
                        page.tags = tags;

                        if let Err(e) = page_store.write(&page).await {
                            warn!("Evolution: failed to create wiki page: {}", e);
                        } else {
                            persisted += 1;
                        }
                    }
                }
            }
        }

        // Distill meta-analysis: produce higher-order learnings from extracted memories.
        if let Some(ps) = &self.page_store {
            match self.distill_memories(&memories, ps).await {
                Ok(distill_count) => {
                    if distill_count > 0 {
                        info!(
                            "Evolution: distilled {} meta-item(s) for session {}",
                            distill_count, session_key
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Evolution: distill failed for session {}: {}",
                        session_key, e
                    );
                }
            }
        }

        // Update watermark to max sequence.
        let max_sequence = self
            .event_store
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
        super::extract_json_array(text)
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

    /// Run the distill meta-analysis on extracted memories.
    ///
    /// Produces higher-order learnings: skill patterns, anti-patterns,
    /// and meta-observations. Only runs when ≥3 memories were extracted.
    async fn distill_memories(
        &self,
        memories: &[EvolutionMemory],
        page_store: &PageStore,
    ) -> Result<usize, ToolError> {
        if memories.len() < 3 {
            debug!(
                "Evolution: skipping distill — only {} memories (< 3)",
                memories.len()
            );
            return Ok(0);
        }

        let memories_json = serde_json::to_string(memories).map_err(|e| {
            ToolError::ExecutionError(format!("Failed to serialize memories for distill: {}", e))
        })?;

        let template = self
            .distill_prompt
            .as_deref()
            .unwrap_or(DISTILL_PROMPT_TEMPLATE);
        let user_prompt = template.replace("{{memories}}", &memories_json);

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage::user(user_prompt)],
            tools: None,
            temperature: Some(0.0),
            max_tokens: Some(4096),
            thinking: None,
        };

        let response =
            self.provider.chat(request).await.map_err(|e| {
                ToolError::ExecutionError(format!("Distill LLM call failed: {}", e))
            })?;

        let content = response.content.unwrap_or_default();
        let distill: DistillResult = match extract_distill_json(&content) {
            Ok(d) => d,
            Err(e) => {
                debug!("Evolution: distill parse failed: {}. Skipping.", e);
                return Ok(0);
            }
        };

        let mut persisted = 0;

        // Persist skill patterns as SOP pages.
        for skill in &distill.skill_patterns {
            if self
                .persist_distill_item(skill, "sops", "auto_distilled", PageType::Sop, page_store)
                .await
                .is_ok()
            {
                persisted += 1;
            }
        }

        // Persist anti-patterns as topic pages.
        for anti in &distill.anti_patterns {
            if self
                .persist_distill_item(
                    anti,
                    "topics/anti-patterns",
                    "auto_distilled",
                    PageType::Topic,
                    page_store,
                )
                .await
                .is_ok()
            {
                persisted += 1;
            }
        }

        // Persist meta-observations as a single page per session batch.
        if !distill.meta_observations.is_empty() {
            let path = format!("topics/meta-observations/{}", Utc::now().format("%Y-%m-%d"));
            let content = distill
                .meta_observations
                .iter()
                .enumerate()
                .map(|(i, obs)| format!("{}. {}", i + 1, obs))
                .collect::<Vec<_>>()
                .join("\n\n");

            let mut page = WikiPage::new(
                path,
                format!("Meta Observations ({})", Utc::now().format("%Y-%m-%d")),
                PageType::Topic,
                content,
            );
            page.tags = vec!["auto_distilled".to_string(), "meta".to_string()];

            if page_store.write(&page).await.is_ok() {
                persisted += 1;
            }
        }

        if persisted > 0 {
            info!(
                "Evolution: distill produced {} persisted item(s)",
                persisted
            );
        }

        Ok(persisted)
    }

    /// Persist a DistillItem as a wiki page.
    async fn persist_distill_item(
        &self,
        item: &DistillItem,
        prefix: &str,
        tag: &str,
        page_type: PageType,
        page_store: &PageStore,
    ) -> Result<(), ToolError> {
        let slug = slugify(&item.title);
        let path = format!("{}/{}", prefix, slug);
        let mut tags = item.tags.clone();
        tags.push(tag.to_string());

        if page_store.exists(&path).await.unwrap_or(false) {
            match page_store.read(&path).await {
                Ok(mut existing) => {
                    let today = Utc::now().format("%Y-%m-%d").to_string();
                    existing
                        .content
                        .push_str(&format!("\n\n### {} Update\n\n{}", today, item.content));
                    merge_tags(&mut existing.tags, &tags);
                    page_store.write(&existing).await.map_err(|e| {
                        ToolError::ExecutionError(format!("Failed to append distilled page: {}", e))
                    })?;
                }
                Err(e) => {
                    warn!(
                        "Evolution: failed to read existing distilled page '{}': {}",
                        path, e
                    );
                }
            }
        } else {
            let mut page = WikiPage::new(path, item.title.clone(), page_type, item.content.clone());
            page.tags = tags;
            page_store.write(&page).await.map_err(|e| {
                ToolError::ExecutionError(format!("Failed to write distilled page: {}", e))
            })?;
        }
        Ok(())
    }

    /// Persist a skill-type memory as an SOP wiki page.
    /// If the SOP already exists, appends new observations as a dated update block.
    async fn persist_as_sop(
        &self,
        mem: &EvolutionMemory,
        page_store: &PageStore,
    ) -> Result<(), ToolError> {
        let slug = slugify(&mem.title);
        let path = format!("sops/{}", slug);

        let mut tags = mem.tags.clone().unwrap_or_default();
        tags.push("auto_learned".to_string());
        if mem.verified {
            tags.push("verified".to_string());
        }

        if page_store.exists(&path).await.unwrap_or(false) {
            // SOP exists — append new observations.
            match page_store.read(&path).await {
                Ok(mut existing_page) => {
                    let today = Utc::now().format("%Y-%m-%d").to_string();
                    existing_page
                        .content
                        .push_str(&format!("\n\n### {} Update\n\n{}", today, mem.content));
                    merge_tags(&mut existing_page.tags, &tags);
                    if let Some(ref summary) = mem.summary {
                        existing_page.summary = Some(summary.clone());
                    }
                    page_store.write(&existing_page).await.map_err(|e| {
                        ToolError::ExecutionError(format!("Failed to append to SOP page: {}", e))
                    })?;
                    info!("Evolution: appended to existing SOP page '{}'", mem.title);
                    Ok(())
                }
                Err(e) => {
                    warn!("Evolution: failed to read existing SOP '{}': {}", path, e);
                    Ok(())
                }
            }
        } else {
            // New SOP — create fresh.
            let mut page = WikiPage::new(
                path,
                mem.title.clone(),
                PageType::Sop,
                format_sop_content(mem),
            );
            page.summary = mem.summary.clone();
            page.tags = tags;

            page_store.write(&page).await.map_err(|e| {
                ToolError::ExecutionError(format!("Failed to write SOP page: {}", e))
            })?;
            info!("Evolution: created SOP page '{}'", mem.title);
            Ok(())
        }
    }
}

/// Extract `DistillResult` JSON from an LLM response.
fn extract_distill_json(text: &str) -> Result<DistillResult, serde_json::Error> {
    super::extract_json_object(text)
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

/// Merge new tags into existing tags (union, preserving order).
fn merge_tags(existing: &mut Vec<String>, new_tags: &[String]) {
    for tag in new_tags {
        if !existing.contains(tag) {
            existing.push(tag.clone());
        }
    }
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
            .buffer_unordered(self.concurrency);

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
            summary: Some("Test SOP summary".to_string()),
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
