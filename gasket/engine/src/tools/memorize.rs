//! Memorize tool for writing structured long-term memories.
//!
//! Provides a `memorize` tool that persists knowledge as Markdown files with
//! YAML frontmatter into `~/.gasket/memory/<scenario>/`. Each memory gets a
//! UUID-based filename, proper metadata, and is written atomically to disk.
//!
//! Uses `FileMemoryStore::create()` for the actual write, which generates the
//! YAML frontmatter (id, title, scenario, tags, timestamps, tokens, etc.).

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use super::{simple_schema, Tool, ToolContext, ToolError, ToolResult};
use gasket_storage::memory::{FileMemoryStore, Scenario};

// ── Memorize Tool ───────────────────────────────────────────────

/// Tool for writing structured long-term memories to disk.
///
/// Creates a Markdown file with YAML frontmatter in the appropriate scenario
/// directory under `~/.gasket/memory/`. The frontmatter is generated
/// automatically by `FileMemoryStore::create()`.
pub struct MemorizeTool {
    /// Memory base directory (usually ~/.gasket/memory/).
    memory_dir: PathBuf,
}

impl Default for MemorizeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MemorizeTool {
    /// Create a new memorize tool with the default memory directory.
    pub fn new() -> Self {
        let memory_dir = crate::config::config_dir().join("memory");
        Self { memory_dir }
    }

    /// Create a memorize tool with a custom directory (for testing).
    pub fn with_dir(memory_dir: PathBuf) -> Self {
        Self { memory_dir }
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

    /// Optional tags for categorization and retrieval.
    #[serde(default)]
    tags: Vec<String>,
}

fn default_scenario() -> String {
    "knowledge".to_string()
}

/// Parse a scenario string, falling back to Knowledge for invalid values.
fn parse_scenario(s: &str) -> Scenario {
    Scenario::from_dir_name(s).unwrap_or(Scenario::Knowledge)
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
                "tags",
                "array",
                false,
                "Optional tags for categorization and retrieval (e.g. [\"rust\", \"async\"])",
            ),
        ])
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

        let scenario = parse_scenario(parsed.scenario.trim());

        debug!(
            "memorize: saving '{}' to {:?} with {} tag(s)",
            title,
            scenario,
            parsed.tags.len()
        );

        let store = FileMemoryStore::new(self.memory_dir.clone());
        store.init().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to initialize memory store: {}", e))
        })?;

        let filename = store
            .create(scenario, title, "note", &parsed.tags, content)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to create memory: {}", e)))?;

        let tags_display = if parsed.tags.is_empty() {
            "(none)".to_string()
        } else {
            parsed.tags.join(", ")
        };

        Ok(format!(
            "Memory saved: {} [{}] in {} — {}",
            title, tags_display, scenario, filename
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        // Pre-create the knowledge dir so FileMemoryStore::init() has fewer dirs
        std::fs::create_dir_all(tmp.path().join("knowledge")).unwrap();
        tmp
    }

    #[tokio::test]
    async fn test_memorize_default_scenario() {
        let tmp = setup_test_dir();
        let tool = MemorizeTool::with_dir(tmp.path().to_path_buf());

        let args = serde_json::json!({
            "title": "Some Fact",
            "content": "The sky is blue."
        });

        let result = tool.execute(args, &ToolContext::default()).await.unwrap();

        assert!(result.contains("knowledge"));

        // File should be in knowledge dir
        let files: Vec<_> = std::fs::read_dir(tmp.path().join("knowledge"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(1, files.len());
    }

    #[tokio::test]
    async fn test_memorize_invalid_scenario_falls_back() {
        let tmp = setup_test_dir();
        let tool = MemorizeTool::with_dir(tmp.path().to_path_buf());

        let args = serde_json::json!({
            "title": "Fallback",
            "content": "Some content",
            "scenario": "nonexistent"
        });

        let result = tool.execute(args, &ToolContext::default()).await.unwrap();

        assert!(result.contains("knowledge"));
    }

    #[tokio::test]
    async fn test_memorize_empty_title_rejected() {
        let tmp = setup_test_dir();
        let tool = MemorizeTool::with_dir(tmp.path().to_path_buf());

        let args = serde_json::json!({
            "title": "  ",
            "content": "Some content"
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("title must not be empty"));
    }

    #[tokio::test]
    async fn test_memorize_empty_content_rejected() {
        let tmp = setup_test_dir();
        let tool = MemorizeTool::with_dir(tmp.path().to_path_buf());

        let args = serde_json::json!({
            "title": "Title",
            "content": ""
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("content must not be empty"));
    }

    #[tokio::test]
    async fn test_memorize_without_tags() {
        let tmp = setup_test_dir();
        let tool = MemorizeTool::with_dir(tmp.path().to_path_buf());

        let args = serde_json::json!({
            "title": "No Tags",
            "content": "Just content"
        });

        let result = tool.execute(args, &ToolContext::default()).await.unwrap();

        assert!(result.contains("(none)"));
    }

    #[tokio::test]
    async fn test_memorize_with_tags() {
        let tmp = setup_test_dir();
        let tool = MemorizeTool::with_dir(tmp.path().to_path_buf());

        let args = serde_json::json!({
            "title": "Tagged Memory",
            "content": "Content with tags",
            "tags": ["rust", "async"]
        });

        let result = tool.execute(args, &ToolContext::default()).await.unwrap();

        assert!(result.contains("rust, async"));
    }

    #[tokio::test]
    async fn test_memorize_explicit_scenario() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("decisions")).unwrap();
        let tool = MemorizeTool::with_dir(tmp.path().to_path_buf());

        let args = serde_json::json!({
            "title": "Architecture Decision",
            "content": "We chose SQLite for metadata storage.",
            "scenario": "decisions"
        });

        let result = tool.execute(args, &ToolContext::default()).await.unwrap();

        assert!(result.contains("decisions"));

        let files: Vec<_> = std::fs::read_dir(tmp.path().join("decisions"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(1, files.len());
    }

    #[test]
    fn test_parse_scenario_known() {
        assert_eq!(Scenario::Profile, parse_scenario("profile"));
        assert_eq!(Scenario::Active, parse_scenario("active"));
        assert_eq!(Scenario::Knowledge, parse_scenario("knowledge"));
        assert_eq!(Scenario::Decisions, parse_scenario("decisions"));
        assert_eq!(Scenario::Episodes, parse_scenario("episodes"));
        assert_eq!(Scenario::Reference, parse_scenario("reference"));
    }

    #[test]
    fn test_parse_scenario_unknown_falls_back() {
        assert_eq!(Scenario::Knowledge, parse_scenario("unknown"));
        assert_eq!(Scenario::Knowledge, parse_scenario(""));
    }
}
