//! Memory search tool using filesystem-based search.
//!
//! Provides a `memory_search` tool that searches memory files using filesystem grep.
//! For advanced Tantivy-based full-text search, use the standalone `tantivy-mcp` server.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;
use tracing::debug;

use super::{simple_schema, Tool, ToolError, ToolResult};

// ── Unified Search Tool ────────────────────────────────────────

/// Memory search tool using filesystem-based search.
///
/// Searches memory files using filesystem grep. For advanced features like
/// boolean queries and fuzzy matching, use the standalone `tantivy-mcp` server.
pub struct MemorySearchTool {
    /// Memory directory containing `*.md` files
    memory_dir: PathBuf,
}

impl Default for MemorySearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl MemorySearchTool {
    /// Create a new memory search tool with the default memory directory.
    pub fn new() -> Self {
        let memory_dir = crate::config::config_dir().join("memory");
        Self { memory_dir }
    }

    /// Create a memory search tool with a custom directory (for testing).
    pub fn with_dir(memory_dir: PathBuf) -> Self {
        Self { memory_dir }
    }

    /// Create with default paths.
    pub fn with_defaults() -> Result<Self, ToolError> {
        let config_dir = crate::config::config_dir();
        let memory_dir = config_dir.join("memory");

        Ok(Self { memory_dir })
    }
}

// ── Argument Parsing ───────────────────────────────────────────

#[derive(Deserialize)]
struct SearchArgs {
    /// Simple text query (keyword search)
    query: Option<String>,

    /// Number of context lines for grep mode
    #[serde(default = "default_context_lines")]
    context_lines: usize,

    /// Maximum number of results
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_context_lines() -> usize {
    2
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
         Uses filesystem-based keyword search."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "query",
                "string",
                true,
                "Search query text (keywords or phrases)",
            ),
            (
                "context_lines",
                "integer",
                false,
                "Number of context lines for matches (default: 2)",
            ),
            (
                "limit",
                "integer",
                false,
                "Maximum number of results (default: 10)",
            ),
        ])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        let parsed: SearchArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        self.search_with_filesystem(&parsed).await
    }
}

impl MemorySearchTool {
    /// Execute search using filesystem grep.
    async fn search_with_filesystem(&self, parsed: &SearchArgs) -> ToolResult {
        let query_text = match &parsed.query {
            Some(q) => q.clone(),
            None => {
                return Err(ToolError::InvalidArguments(
                    "Filesystem search requires 'query' parameter".to_string(),
                ))
            }
        };

        let query_lower = query_text.to_lowercase();

        // Ensure memory directory exists
        if !self.memory_dir.exists() {
            return Ok(format!(
                "Memory directory does not exist: {}. No memories to search.",
                self.memory_dir.display()
            ));
        }

        // Collect all .md files in the memory directory
        let mut md_files = Vec::new();
        let mut read_dir = fs::read_dir(&self.memory_dir).await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to read memory directory: {}", e))
        })?;

        while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to read directory entry: {}", e))
        })? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                md_files.push(path);
            }
        }

        md_files.sort(); // deterministic order

        debug!(
            "memory_search: searching {} files for '{}' (filesystem)",
            md_files.len(),
            query_text
        );

        // Search each file
        let mut matches: Vec<MatchResult> = Vec::new();

        for file_path in &md_files {
            let content = match fs::read_to_string(file_path).await {
                Ok(c) => c,
                Err(e) => {
                    debug!("Skipping unreadable file {:?}: {}", file_path, e);
                    continue;
                }
            };

            let lines: Vec<&str> = content.lines().collect();
            let file_name = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            for (i, line) in lines.iter().enumerate() {
                if line.to_lowercase().contains(&query_lower) {
                    // Build context window
                    let start = i.saturating_sub(parsed.context_lines);
                    let end = (i + parsed.context_lines + 1).min(lines.len());

                    let mut context_parts = Vec::new();
                    for (j, line_content) in lines.iter().enumerate().skip(start).take(end - start)
                    {
                        let marker = if j == i { ">>>" } else { "   " };
                        context_parts.push(format!("{} {:>4} | {}", marker, j + 1, line_content));
                    }

                    matches.push(MatchResult {
                        file_name: file_name.clone(),
                        line_number: i + 1,
                        context: context_parts.join("\n"),
                    });

                    if matches.len() >= parsed.limit {
                        break;
                    }
                }
            }

            if matches.len() >= parsed.limit {
                break;
            }
        }

        if matches.is_empty() {
            return Ok(format!(
                "No matches found for '{}' in {} memory file(s).",
                query_text,
                md_files.len()
            ));
        }

        // Format output grouped by file
        let mut output = format!(
            "Found {} match{} for '{}' across {} file(s) (filesystem):\n\n",
            matches.len(),
            if matches.len() == 1 { "" } else { "es" },
            query_text,
            md_files.len()
        );

        let mut current_file = String::new();
        for m in &matches {
            if m.file_name != current_file {
                current_file = m.file_name.clone();
                output.push_str(&format!("━━━ {} ━━━\n", current_file));
            }
            output.push_str(&format!("  [line {}]\n{}\n\n", m.line_number, m.context));
        }

        Ok(output)
    }
}

// ── Filesystem Search Types ──────────────────────────────────────

/// A single match result: the file, line number, and context window.
struct MatchResult {
    file_name: String,
    line_number: usize,
    context: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_dir() -> (tempfile::TempDir, MemorySearchTool) {
        let tmp = tempfile::tempdir().unwrap();
        let tool = MemorySearchTool::with_dir(tmp.path().to_path_buf());

        // Create test files
        std::fs::write(
            tmp.path().join("project_alpha.md"),
            "# Project Alpha\n\nWe decided to use PostgreSQL for the database.\nThe API uses REST with JSON responses.\nDeployment is on AWS ECS.\n",
        ).unwrap();

        std::fs::write(
            tmp.path().join("preferences.md"),
            "# User Preferences\n\n- Prefers Rust over Python\n- Uses dark mode everywhere\n- Likes concise responses\n",
        ).unwrap();

        (tmp, tool)
    }

    #[tokio::test]
    async fn test_search_finds_keyword() {
        let (_tmp, tool) = setup_test_dir();
        let args = serde_json::json!({"query": "PostgreSQL"});
        let result = tool.execute(args).await.unwrap();
        assert!(result.contains("PostgreSQL"));
        assert!(result.contains("project_alpha.md"));
    }

    #[tokio::test]
    async fn test_search_case_insensitive() {
        let (_tmp, tool) = setup_test_dir();
        let args = serde_json::json!({"query": "postgresql"});
        let result = tool.execute(args).await.unwrap();
        assert!(result.contains("PostgreSQL"));
    }

    #[tokio::test]
    async fn test_search_no_results() {
        let (_tmp, tool) = setup_test_dir();
        let args = serde_json::json!({"query": "nonexistent_keyword_xyz"});
        let result = tool.execute(args).await.unwrap();
        assert!(result.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_search_multiple_files() {
        let (_tmp, tool) = setup_test_dir();
        let args = serde_json::json!({"query": "Rust"});
        let result = tool.execute(args).await.unwrap();
        assert!(result.contains("preferences.md"));
    }

    #[tokio::test]
    async fn test_search_with_context() {
        let (_tmp, tool) = setup_test_dir();
        let args = serde_json::json!({"query": "PostgreSQL", "context_lines": 1});
        let result = tool.execute(args).await.unwrap();
        assert!(result.contains(">>>"));
    }

    #[tokio::test]
    async fn test_search_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = MemorySearchTool::with_dir(tmp.path().to_path_buf());
        let args = serde_json::json!({"query": "anything"});
        let result = tool.execute(args).await.unwrap();
        assert!(result.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_search_nonexistent_directory() {
        let tool = MemorySearchTool::with_dir(PathBuf::from("/tmp/nonexistent_nanobot_test"));
        let args = serde_json::json!({"query": "anything"});
        let result = tool.execute(args).await.unwrap();
        assert!(result.contains("does not exist"));
    }
}
