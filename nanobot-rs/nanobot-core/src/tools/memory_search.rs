//! Unified memory search tool — Tantivy-powered with filesystem fallback.
//!
//! Provides a single `memory_search` tool that:
//! - Uses Tantivy full-text search when available (fast, relevance-ranked)
//! - Falls back to simple file grep when Tantivy is unavailable
//!
//! This unifies the API: LLM only sees one tool, not two separate implementations.
//! The choice of search strategy is encapsulated internally.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::fs;
use tracing::{debug, info, warn};

use super::{simple_schema, Tool, ToolError, ToolResult};
use crate::search::tantivy::MemoryIndexReader;
use crate::search::{BooleanQuery, FuzzyQuery, SearchQuery, SortOrder};

// ── Unified Search Tool ────────────────────────────────────────

/// Unified memory search tool that automatically chooses the best available strategy.
///
/// - **Primary**: Tantivy full-text search (fast, relevance-ranked, supports advanced queries)
/// - **Fallback**: Simple file grep (always works, no dependencies)
///
/// The tool name is `memory_search` — LLM never sees implementation details.
pub struct MemorySearchTool {
    /// Memory directory containing `*.md` files
    memory_dir: PathBuf,
    /// Optional Tantivy index reader (None = filesystem-only mode)
    tantivy_reader: Option<Arc<MemoryIndexReader>>,
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
        Self {
            memory_dir,
            tantivy_reader: None,
        }
    }

    /// Create a memory search tool with a custom directory (for testing).
    pub fn with_dir(memory_dir: PathBuf) -> Self {
        Self {
            memory_dir,
            tantivy_reader: None,
        }
    }

    /// Attach a Tantivy index reader for advanced search capabilities.
    ///
    /// When a reader is attached, searches will prefer Tantivy and fall back
    /// to filesystem search on failure.
    pub fn with_tantivy_reader(mut self, reader: Arc<MemoryIndexReader>) -> Self {
        self.tantivy_reader = Some(reader);
        self
    }

    /// Try to create with Tantivy reader from default paths.
    pub fn with_defaults() -> Result<Self, ToolError> {
        let config_dir = crate::config::config_dir();
        let memory_dir = config_dir.join("memory");

        // Try to open Tantivy index. If it fails, we still work with filesystem-only mode.
        let index_path = config_dir.join("tantivy-index").join("memory");

        let tantivy_reader =
            match crate::search::tantivy::open_memory_index(&index_path, &memory_dir) {
                Ok((reader, _writer)) => {
                    debug!("Tantivy memory index opened successfully");
                    Some(Arc::new(reader))
                }
                Err(e) => {
                    warn!(
                        "Tantivy memory index unavailable, using filesystem fallback: {}",
                        e
                    );
                    None
                }
            };

        Ok(Self {
            memory_dir,
            tantivy_reader,
        })
    }
}

// ── Argument Parsing ───────────────────────────────────────────

#[derive(Deserialize)]
struct SearchArgs {
    /// Simple text query (keyword search)
    query: Option<String>,

    /// Boolean query with must/should/not clauses (Tantivy-only)
    boolean: Option<BooleanQueryInput>,

    /// Fuzzy query with typo tolerance (Tantivy-only)
    fuzzy: Option<FuzzyQueryInput>,

    /// Tag filters (AND semantics)
    #[serde(default)]
    tags: Vec<String>,

    /// Number of context lines for grep mode (filesystem fallback only)
    #[serde(default = "default_context_lines")]
    context_lines: usize,

    /// Maximum number of results
    #[serde(default = "default_limit")]
    limit: usize,

    /// Sort order: "relevance" or "date"
    #[serde(default)]
    sort: String,
}

#[derive(Deserialize)]
struct BooleanQueryInput {
    must: Option<Vec<String>>,
    should: Option<Vec<String>>,
    not: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct FuzzyQueryInput {
    text: String,
    distance: Option<u8>,
    prefix: Option<bool>,
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
         Supports keyword search. If Tantivy index is available, supports \
         advanced features like boolean queries and fuzzy matching."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "query",
                "string",
                false,
                "Search query text (keywords or phrases)",
            ),
            (
                "boolean",
                "object",
                false,
                "Boolean query with 'must' (AND), 'should' (OR), and 'not' (NOT) arrays of terms",
            ),
            (
                "fuzzy",
                "object",
                false,
                "Fuzzy query with 'text', optional 'distance' (1-2), and 'prefix' (boolean)",
            ),
            (
                "tags",
                "array",
                false,
                "Array of tags to filter by (AND semantics)",
            ),
            (
                "context_lines",
                "integer",
                false,
                "Number of context lines for matches (filesystem mode only, default: 2)",
            ),
            (
                "limit",
                "integer",
                false,
                "Maximum number of results (default: 10)",
            ),
            (
                "sort",
                "string",
                false,
                "Sort order: 'relevance' (default) or 'date'",
            ),
        ])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        let parsed: SearchArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        // Strategy selection: Try Tantivy first, fallback to filesystem
        if let Some(ref reader) = &self.tantivy_reader {
            // Check if this is a simple query that can be handled by filesystem
            let is_simple_query = parsed.boolean.is_none()
                && parsed.fuzzy.is_none()
                && parsed.tags.is_empty()
                && parsed.query.is_some();

            if is_simple_query {
                // For simple queries, try Tantivy first
                match self.search_with_tantivy(&parsed, reader.clone()) {
                    Ok(results) if !results.is_empty() => {
                        info!(
                            "memory_search: Tantivy found {} results for '{}'",
                            results.len(),
                            parsed.query.as_deref().unwrap_or("")
                        );
                        return Ok(self.format_tantivy_results(&parsed, results));
                    }
                    Ok(_) => {
                        // Empty results, fall through to filesystem
                    }
                    Err(e) => {
                        warn!("Tantivy search failed, falling back to filesystem: {}", e);
                        // Fall through to filesystem search
                    }
                }
            }

            // Advanced query or Tantivy failed - try Tantivy for all features
            match self.search_with_tantivy(&parsed, reader.clone()) {
                Ok(results) if !results.is_empty() => {
                    info!(
                        "memory_search: Tantivy found {} results (advanced query)",
                        results.len()
                    );
                    return Ok(self.format_tantivy_results(&parsed, results));
                }
                Ok(_) => {
                    // Empty results, fall through to filesystem
                }
                Err(e) => {
                    warn!("Tantivy search failed, falling back to filesystem: {}", e);
                    // Fall through to filesystem search
                }
            }
        }

        // Fallback: Filesystem search
        self.search_with_filesystem(&parsed).await
    }
}

impl MemorySearchTool {
    /// Execute search using Tantivy index, Returns the raw search results.
    fn search_with_tantivy(
        &self,
        parsed: &SearchArgs,
        reader: Arc<MemoryIndexReader>,
    ) -> Result<Vec<crate::search::SearchResult>, ToolError> {
        let search_query = SearchQuery {
            text: parsed.query.clone(),
            boolean: parsed.boolean.as_ref().map(|b| BooleanQuery {
                must: b.must.clone().unwrap_or_default(),
                should: b.should.clone().unwrap_or_default(),
                not: b.not.clone().unwrap_or_default(),
            }),
            fuzzy: parsed.fuzzy.as_ref().map(|f| FuzzyQuery {
                text: f.text.clone(),
                distance: f.distance.unwrap_or(2),
                prefix: f.prefix.unwrap_or(false),
            }),
            tags: parsed.tags.clone(),
            limit: parsed.limit,
            offset: 0,
            sort: match parsed.sort.as_str() {
                "date" => SortOrder::Date,
                _ => SortOrder::Relevance,
            },
            ..Default::default()
        };

        debug!("memory_search: executing Tantivy query {:?}", search_query);

        reader
            .search(&search_query)
            .map_err(|e| ToolError::ExecutionError(format!("Tantivy search failed: {}", e)))
    }

    /// Format Tantivy results into output string
    fn format_tantivy_results(
        &self,
        _parsed: &SearchArgs,
        results: Vec<crate::search::SearchResult>,
    ) -> String {
        if results.is_empty() {
            return "No matches found in memory files.".to_string();
        }

        // Format results
        let mut output = format!(
            "Found {} result{} in memory (Tantivy):\n\n",
            results.len(),
            if results.len() == 1 { "" } else { "s" }
        );

        for (i, result) in results.iter().enumerate() {
            output.push_str(&format!(
                "{}. **{}** (score: {:.2})\n",
                i + 1,
                result.id,
                result.score
            ));

            if !result.tags.is_empty() {
                output.push_str(&format!("   Tags: {}\n", result.tags.join(", ")));
            }

            if let Some(ref ts) = result.timestamp {
                output.push_str(&format!("   Updated: {}\n", ts));
            }

            // Show content preview
            let preview = if result.content.len() > 200 {
                format!("{}...", &result.content[..200])
            } else {
                result.content.clone()
            };
            output.push_str(&format!("   {}\n\n", preview.trim().replace('\n', " ")));
        }

        output
    }

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
