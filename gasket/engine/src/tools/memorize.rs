//! Memorize tool for writing structured long-term memories to wiki.
//!
//! Provides a `memorize` tool that persists knowledge as wiki pages with
//! proper tagging and categorization.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use super::{simple_schema, Tool, ToolContext, ToolError, ToolResult};
use crate::wiki::{PageStore, PageType, WikiPage, slugify};

// ── Memorize Tool ───────────────────────────────────────────────

/// Tool for writing structured long-term memories to wiki.
///
/// Creates wiki pages with proper tagging and categorization in the
/// unified knowledge management system.
pub struct MemorizeTool {
    /// Wiki PageStore for unified knowledge management.
    page_store: Arc<PageStore>,
}

impl MemorizeTool {
    /// Create a new memorize tool with wiki PageStore.
    pub fn new(page_store: Arc<PageStore>) -> Self {
        Self { page_store }
    }
}

// ── Argument Parsing ────────────────────────────────────────────

#[derive(Deserialize)]
struct MemorizeArgs {
    /// Short title for the memory (used in frontmatter and search results).
    title: String,

    /// Body content of the memory (Markdown).
    content: String,

    /// Scenario bucket: one of profile, active, knowledge, decisions,
    /// episodes, reference. Defaults to "knowledge" when absent.
    #[serde(default = "default_scenario")]
    scenario: String,

    /// Memory type: "note" (default) for facts, "skill" for reusable procedures.
    /// Use "skill" when the content describes a workflow with steps and pitfalls.
    #[serde(default = "default_memory_type")]
    memory_type: String,

    /// Optional tags for categorization and retrieval.
    #[serde(default)]
    tags: Vec<String>,
}

fn default_scenario() -> String {
    "knowledge".to_string()
}

fn default_memory_type() -> String {
    "note".to_string()
}

// ── Tool Implementation ─────────────────────────────────────────

#[async_trait]
impl Tool for MemorizeTool {
    fn name(&self) -> &str {
        "memorize"
    }

    fn description(&self) -> &str {
        "Save important information to long-term memory for future reference. \
         Use this to persist decisions, learned facts, project context, or any \
         knowledge worth remembering across conversations."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "title",
                "string",
                true,
                "Short descriptive title for this memory",
            ),
            (
                "content",
                "string",
                true,
                "Markdown content to store in the memory file",
            ),
            (
                "scenario",
                "string",
                false,
                "Memory category: profile, active, knowledge (default), decisions, episodes, reference",
            ),
            (
                "memory_type",
                "string",
                false,
                "Memory type: 'note' (default) for facts, 'skill' for reusable procedures with steps and pitfalls",
            ),
            (
                "tags",
                "array",
                false,
                "Optional tags for categorization and retrieval (e.g. [\"rust\", \"async\"])",
            ),
        ])
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: MemorizeArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        let title = parsed.title.trim();
        let content = parsed.content.trim();

        if title.is_empty() {
            return Err(ToolError::InvalidArguments(
                "title must not be empty".to_string(),
            ));
        }
        if content.is_empty() {
            return Err(ToolError::InvalidArguments(
                "content must not be empty".to_string(),
            ));
        }

        let tags_display = if parsed.tags.is_empty() {
            "(none)".to_string()
        } else {
            parsed.tags.join(", ")
        };

        let scenario = parsed.scenario.trim();
        let memory_type = parsed.memory_type.trim();

        // Map scenario to page path prefix
        let prefix = match scenario {
            "profile" => "entities/people",
            "decisions" => "topics",
            "knowledge" => "topics",
            "active" => "topics",
            "episodes" => "topics",
            "reference" => "sources",
            _ => "topics",
        };

        // Map memory_type to PageType (everything is Topic for now)
        let page_type = match memory_type {
            "skill" => PageType::Topic,
            _ => PageType::Topic,
        };

        // Build path: prefix/slugified-title
        let slug = slugify(title);
        let path = format!("{}/{}", prefix, slug);

        debug!(
            "memorize: saving '{}' to wiki path '{}' with {} tag(s)",
            title,
            path,
            parsed.tags.len()
        );

        let mut page = WikiPage::new(path.clone(), title.to_string(), page_type, content.to_string());
        page.tags = parsed.tags;

        self.page_store
            .write(&page)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to write wiki page: {}", e)))?;

        Ok(format!(
            "Memory saved: {} [{}] in {} (type: {}) — {}",
            title, tags_display, prefix, memory_type, path
        ))
    }
}
