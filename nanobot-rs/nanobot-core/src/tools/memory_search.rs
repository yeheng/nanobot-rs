//! Memory search tool — file-system grep over `~/.nanobot/memory/*.md`.
//!
//! Provides a `memory_search` tool that performs case-insensitive text search
//! across all Markdown files in the memory directory. Returns matching lines
//! with surrounding context (±2 lines), grouped by file.
//!
//! This replaces the old SQLite FTS5 implementation, establishing the file
//! system as the Single Source of Truth (SSOT) for long-term memory.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;
use tracing::debug;

use super::{simple_schema, Tool, ToolResult};

/// Tool that searches `~/.nanobot/memory/*.md` files using simple text matching.
///
/// The memory directory defaults to `~/.nanobot/memory/` but can be overridden
/// for testing purposes.
pub struct MemorySearchTool {
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
    #[allow(dead_code)]
    pub fn with_dir(memory_dir: PathBuf) -> Self {
        Self { memory_dir }
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_context_lines")]
    context_lines: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_context_lines() -> usize {
    2
}

fn default_limit() -> usize {
    20
}

/// A single match result: the file, line number, and context window.
struct MatchResult {
    file_name: String,
    line_number: usize,
    context: String, // The matching line + surrounding context lines
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search long-term memory files (memory/*.md) using keyword matching. \
         Returns matching lines with surrounding context, grouped by file. \
         Use this to recall past decisions, preferences, project context, or archived knowledge."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "query",
                "string",
                true,
                "Search query text (keywords or phrases, case-insensitive)",
            ),
            (
                "context_lines",
                "integer",
                false,
                "Number of context lines before and after each match (default: 2)",
            ),
            (
                "limit",
                "integer",
                false,
                "Maximum number of match results to return (default: 20)",
            ),
        ])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        let parsed: SearchArgs = serde_json::from_value(args)
            .map_err(|e| super::ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        let query_lower = parsed.query.to_lowercase();

        // Ensure memory directory exists
        if !self.memory_dir.exists() {
            return Ok(format!(
                "Memory directory does not exist: {}. No memories to search.",
                self.memory_dir.display()
            ));
        }

        // Collect all .md files in the memory directory (non-recursive for now)
        let mut md_files = Vec::new();
        let mut read_dir = fs::read_dir(&self.memory_dir).await.map_err(|e| {
            super::ToolError::ExecutionError(format!("Failed to read memory directory: {}", e))
        })?;

        while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
            super::ToolError::ExecutionError(format!("Failed to read directory entry: {}", e))
        })? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                md_files.push(path);
            }
        }

        md_files.sort(); // deterministic order

        debug!(
            "memory_search: searching {} files for '{}'",
            md_files.len(),
            parsed.query
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
                parsed.query,
                md_files.len()
            ));
        }

        // Format output grouped by file
        let mut output = format!(
            "Found {} match{} for '{}' across {} file(s):\n\n",
            matches.len(),
            if matches.len() == 1 { "" } else { "es" },
            parsed.query,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_test_dir() -> (tempfile::TempDir, MemorySearchTool) {
        let tmp = tempfile::tempdir().unwrap();
        let tool = MemorySearchTool::with_dir(tmp.path().to_path_buf());

        // Create test files
        fs::write(
            tmp.path().join("project_alpha.md"),
            "# Project Alpha\n\nWe decided to use PostgreSQL for the database.\nThe API uses REST with JSON responses.\nDeployment is on AWS ECS.\n",
        ).unwrap();

        fs::write(
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
        // Should contain surrounding lines
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
