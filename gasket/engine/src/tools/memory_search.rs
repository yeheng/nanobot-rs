//! Memory search tool using SQLite MetadataStore.
//!
//! Provides a `memory_search` tool that searches memory files using the
//! SQLite-backed `MetadataStore` for tag-based queries. Falls back to a
//! lightweight filesystem scan when MetadataStore is unavailable.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;
use tracing::debug;

use super::{simple_schema, Tool, ToolContext, ToolError, ToolResult};
use gasket_storage::memory::{memory_base_dir, MetadataStore, Scenario};

// ── Memory Search Tool ─────────────────────────────────────────

/// Memory search tool backed by SQLite MetadataStore.
///
/// Searches memory metadata via `MetadataStore::query_by_tags()` for
/// structured retrieval. Reads content snippets from disk for context.
/// When no `MetadataStore` is available, falls back to a lightweight
/// filesystem keyword scan.
pub struct MemorySearchTool {
    /// Optional SQLite-backed metadata store for fast queries.
    metadata_store: Option<MetadataStore>,
    /// Memory base directory (e.g. ~/.gasket/memory/).
    memory_dir: PathBuf,
}

impl Default for MemorySearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MemorySearchTool {
    /// Create a new memory search tool with default paths and no MetadataStore.
    pub fn new() -> Self {
        let memory_dir = memory_base_dir();
        Self {
            metadata_store: None,
            memory_dir,
        }
    }

    /// Create with a MetadataStore for SQLite-backed search.
    pub fn with_store(metadata_store: MetadataStore) -> Self {
        let memory_dir = memory_base_dir();
        Self {
            metadata_store: Some(metadata_store),
            memory_dir,
        }
    }

    /// Create with custom directory (for testing).
    pub fn with_dir(memory_dir: PathBuf) -> Self {
        Self {
            metadata_store: None,
            memory_dir,
        }
    }

    /// Create with both custom directory and MetadataStore.
    pub fn with_dir_and_store(memory_dir: PathBuf, metadata_store: MetadataStore) -> Self {
        Self {
            metadata_store: Some(metadata_store),
            memory_dir,
        }
    }
}

// ── Argument Parsing ───────────────────────────────────────────

#[derive(Deserialize)]
struct SearchArgs {
    /// Text query — keywords split into tag candidates.
    query: Option<String>,

    /// Explicit tags to match (any-tag OR semantics).
    #[serde(default)]
    tags: Vec<String>,

    /// Maximum number of results.
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    10
}

// ── Tool Implementation ────────────────────────────────────────

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search long-term memory files for past decisions, preferences, and project context. \
         Uses SQLite-backed tag search for fast, structured retrieval. \
         Supports both text queries and explicit tag filters."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "query",
                "string",
                false,
                "Search query text (split into keyword tags for matching)",
            ),
            (
                "tags",
                "array",
                false,
                "Explicit tags to filter by (e.g. [\"rust\", \"async\"])",
            ),
            (
                "limit",
                "integer",
                false,
                "Maximum number of results (default: 10)",
            ),
        ])
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: SearchArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        // Collect search tags: explicit tags + query words
        let mut search_tags = parsed.tags.clone();
        if let Some(ref q) = parsed.query {
            // Split query into lowercase keywords (skip short words)
            for word in q.split_whitespace() {
                let w = word.to_lowercase();
                if w.len() >= 2 {
                    search_tags.push(w);
                }
            }
        }

        if search_tags.is_empty() {
            return Err(ToolError::InvalidArguments(
                "Provide at least 'query' or 'tags'".to_string(),
            ));
        }

        debug!(
            "memory_search: searching with {} tag(s) (limit {})",
            search_tags.len(),
            parsed.limit
        );

        // Try SQLite-backed search first
        if let Some(ref store) = self.metadata_store {
            return self.search_with_store(store, &search_tags, &parsed).await;
        }

        // Fallback: filesystem keyword scan
        self.search_with_filesystem(&search_tags, &parsed).await
    }
}

impl MemorySearchTool {
    /// SQLite-backed search via MetadataStore.
    async fn search_with_store(
        &self,
        store: &MetadataStore,
        search_tags: &[String],
        parsed: &SearchArgs,
    ) -> ToolResult {
        let entries = store
            .query_by_tags(search_tags, None, parsed.limit)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("MetadataStore query failed: {}", e)))?;

        if entries.is_empty() {
            return Ok(format!(
                "No memories found matching tags: {}",
                search_tags.join(", ")
            ));
        }

        let mut output = format!(
            "Found {} memor{} matching tags [{}]:\n\n",
            entries.len(),
            if entries.len() == 1 { "y" } else { "ies" },
            search_tags.join(", ")
        );

        for entry in &entries {
            let scenario_dir = self.memory_dir.join(entry.scenario.dir_name());
            let file_path = scenario_dir.join(&entry.filename);

            let snippet = self.read_snippet(&file_path, 6).await;

            let tags_display = if entry.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", entry.tags.join(", "))
            };

            output.push_str(&format!(
                "━━━ {} ━━━\n  Scenario: {} | Tags:{} | Tokens: {}\n  {}\n\n",
                entry.title, entry.scenario, tags_display, entry.tokens, snippet,
            ));
        }

        Ok(output)
    }

    /// Read the first N lines of a file as a content snippet.
    async fn read_snippet(&self, path: &std::path::Path, max_lines: usize) -> String {
        match fs::read_to_string(path).await {
            Ok(content) => {
                // Skip frontmatter (between --- delimiters)
                let body = if content.trim_start().starts_with("---") {
                    if let Some(end) = content[3..].find("\n---") {
                        content[(end + 7)..].trim()
                    } else {
                        &content
                    }
                } else {
                    &content
                };

                let lines: Vec<&str> = body.lines().take(max_lines).collect();
                if body.lines().count() > max_lines {
                    format!("{}\n  ...", lines.join("\n  "))
                } else {
                    lines.join("\n  ")
                }
            }
            Err(_) => "(content unavailable)".to_string(),
        }
    }

    /// Lightweight filesystem fallback when MetadataStore is unavailable.
    ///
    /// Scans .md files, parses frontmatter titles and tags, and matches
    /// against search keywords. Much slower than SQLite but works without a DB.
    async fn search_with_filesystem(
        &self,
        search_tags: &[String],
        parsed: &SearchArgs,
    ) -> ToolResult {
        if !self.memory_dir.exists() {
            return Ok(format!(
                "Memory directory does not exist: {}. No memories to search.",
                self.memory_dir.display()
            ));
        }

        let mut results: Vec<(String, Scenario, Vec<String>, String, String)> = Vec::new();

        // Scan each scenario directory
        for scenario in Scenario::all() {
            let dir = self.memory_dir.join(scenario.dir_name());
            if !dir.exists() {
                continue;
            }

            let mut read_dir = fs::read_dir(&dir).await.map_err(|e| {
                ToolError::ExecutionError(format!("Failed to read directory: {}", e))
            })?;

            while let Some(entry) = read_dir
                .next_entry()
                .await
                .map_err(|e| ToolError::ExecutionError(format!("Failed to read entry: {}", e)))?
            {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "md") {
                    continue;
                }

                let content = match fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                // Quick tag match in content
                let lower = content.to_lowercase();
                let matched = search_tags
                    .iter()
                    .any(|tag| lower.contains(&tag.to_lowercase()));

                if !matched {
                    continue;
                }

                let filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let snippet = {
                    let body = if content.trim_start().starts_with("---") {
                        if let Some(end) = content[3..].find("\n---") {
                            content[(end + 7)..].trim()
                        } else {
                            &content
                        }
                    } else {
                        &content
                    };
                    body.lines().take(4).collect::<Vec<_>>().join("\n  ")
                };

                // Try to extract title from frontmatter
                let title = if content.trim_start().starts_with("---") {
                    content["---".len()..]
                        .lines()
                        .find(|l| l.trim().starts_with("title:"))
                        .map(|l| l.trim().trim_start_matches("title:").trim().to_string())
                        .unwrap_or_else(|| filename.clone())
                } else {
                    filename.clone()
                };

                results.push((title, *scenario, vec![], filename, snippet));

                if results.len() >= parsed.limit {
                    break;
                }
            }
            if results.len() >= parsed.limit {
                break;
            }
        }

        if results.is_empty() {
            return Ok(format!(
                "No memories found matching keywords: {}",
                search_tags.join(", ")
            ));
        }

        let mut output = format!(
            "Found {} memor{} matching [{}]:\n\n",
            results.len(),
            if results.len() == 1 { "y" } else { "ies" },
            search_tags.join(", ")
        );

        for (title, scenario, _tags, filename, snippet) in &results {
            output.push_str(&format!(
                "━━━ {} ━━━\n  Scenario: {} | File: {}\n  {}\n\n",
                title, scenario, filename, snippet,
            ));
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_parsing() {
        let args = serde_json::json!({
            "query": "database design",
            "tags": ["architecture"],
            "limit": 5
        });
        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.query, Some("database design".to_string()));
        assert_eq!(parsed.tags, vec!["architecture"]);
        assert_eq!(parsed.limit, 5);
    }

    #[test]
    fn test_search_args_defaults() {
        let args = serde_json::json!({});
        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        assert!(parsed.query.is_none());
        assert!(parsed.tags.is_empty());
        assert_eq!(parsed.limit, 10);
    }

    #[tokio::test]
    async fn test_search_filesystem_finds_keyword() {
        let tmp = tempfile::tempdir().unwrap();
        let knowledge_dir = tmp.path().join("knowledge");
        std::fs::create_dir_all(&knowledge_dir).unwrap();
        std::fs::write(
            knowledge_dir.join("project_alpha.md"),
            "# Project Alpha\n\nWe decided to use PostgreSQL for the database.\nThe API uses REST with JSON responses.\n",
        ).unwrap();

        let tool = MemorySearchTool::with_dir(tmp.path().to_path_buf());
        let args = serde_json::json!({"query": "PostgreSQL"});
        let result = tool.execute(args, &ToolContext::default()).await.unwrap();
        assert!(result.contains("PostgreSQL"));
    }

    #[tokio::test]
    async fn test_search_no_results() {
        let tmp = tempfile::tempdir().unwrap();
        let knowledge_dir = tmp.path().join("knowledge");
        std::fs::create_dir_all(&knowledge_dir).unwrap();

        let tool = MemorySearchTool::with_dir(tmp.path().to_path_buf());
        let args = serde_json::json!({"query": "nonexistent_keyword_xyz"});
        let result = tool.execute(args, &ToolContext::default()).await.unwrap();
        assert!(result.contains("No memories found"));
    }

    #[tokio::test]
    async fn test_search_requires_query_or_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = MemorySearchTool::with_dir(tmp.path().to_path_buf());
        let args = serde_json::json!({});
        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Provide at least"));
    }

    #[tokio::test]
    async fn test_search_nonexistent_directory() {
        let tool = MemorySearchTool::with_dir(PathBuf::from("/tmp/nonexistent_gasket_test"));
        let args = serde_json::json!({"query": "anything"});
        let result = tool.execute(args, &ToolContext::default()).await.unwrap();
        assert!(result.contains("does not exist"));
    }

    #[tokio::test]
    async fn test_search_with_tags_only() {
        let tmp = tempfile::tempdir().unwrap();
        let knowledge_dir = tmp.path().join("knowledge");
        std::fs::create_dir_all(&knowledge_dir).unwrap();
        std::fs::write(
            knowledge_dir.join("rust_note.md"),
            "---\ntitle: Rust Notes\nscenario: knowledge\ntags:\n  - rust\n  - programming\n---\n\nRust is great for systems programming.\n",
        ).unwrap();

        let tool = MemorySearchTool::with_dir(tmp.path().to_path_buf());
        let args = serde_json::json!({"tags": ["rust"]});
        let result = tool.execute(args, &ToolContext::default()).await.unwrap();
        assert!(result.contains("Rust Notes"));
    }

    #[tokio::test]
    async fn test_search_skips_short_query_words() {
        // Words < 2 chars should be skipped when building tag candidates
        let tmp = tempfile::tempdir().unwrap();
        let knowledge_dir = tmp.path().join("knowledge");
        std::fs::create_dir_all(&knowledge_dir).unwrap();

        let tool = MemorySearchTool::with_dir(tmp.path().to_path_buf());
        let args = serde_json::json!({"query": "a I"});
        let result = tool.execute(args, &ToolContext::default()).await;
        // All words are < 2 chars, so search_tags will be empty
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Provide at least"));
    }
}
