//! Memory search tool using wiki PageStore/PageIndex.
//!
//! Provides a `memory_search` tool that searches wiki pages via the
/// PageStore for tag-based queries and PageIndex for semantic search.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use super::{simple_schema, Tool, ToolContext, ToolError, ToolResult};
use crate::wiki::{PageFilter, PageIndex, PageStore};

// ── Memory Search Tool ─────────────────────────────────────────

/// Memory search tool backed by wiki PageStore/PageIndex.
///
/// Searches wiki pages via PageStore list for tag-based queries and
/// PageIndex for semantic search (Phase 3).
pub struct MemorySearchTool {
    /// Wiki PageStore for unified knowledge search.
    page_store: Arc<PageStore>,
    /// Optional wiki PageIndex for semantic search.
    page_index: Option<Arc<PageIndex>>,
}

impl MemorySearchTool {
    /// Create with wiki PageStore and PageIndex for unified knowledge search.
    pub fn new(page_store: Arc<PageStore>, page_index: Option<Arc<PageIndex>>) -> Self {
        Self {
            page_store,
            page_index,
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

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: SearchArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        // Wildcard query: list all memories (bypass tag filtering)
        let is_wildcard = parsed.query.as_deref() == Some("*");

        // Collect search tags: explicit tags + query words
        let mut search_tags = parsed.tags.clone();
        if let Some(ref q) = parsed.query {
            if !is_wildcard {
                // Split query into lowercase keywords (skip short words)
                for word in q.split_whitespace() {
                    let w = word.to_lowercase();
                    if w.len() >= 2 {
                        search_tags.push(w);
                    }
                }
            }
        }

        if search_tags.is_empty() && !is_wildcard {
            if parsed.query.is_some() {
                return Err(ToolError::InvalidArguments(
                    "All query terms were too short (minimum 2 characters each). \
                     Use longer keywords, explicit 'tags', or query '*' to list all."
                        .to_string(),
                ));
            }
            return Err(ToolError::InvalidArguments(
                "Provide at least 'query' or 'tags'".to_string(),
            ));
        }

        debug!(
            "memory_search: searching with {} tag(s), wildcard={} (limit {})",
            search_tags.len(),
            is_wildcard,
            parsed.limit
        );

        // Try wiki PageIndex first if available
        if let Some(ref index) = self.page_index {
            return self
                .search_with_wiki(index, &self.page_store, &search_tags, &parsed, is_wildcard)
                .await;
        }

        // Fallback to PageStore list + in-memory tag filtering
        self.search_with_page_store(&self.page_store, &search_tags, &parsed, is_wildcard)
            .await
    }
}

impl MemorySearchTool {
    /// Wiki-backed search via PageIndex (semantic) + PageStore (fallback).
    async fn search_with_wiki(
        &self,
        index: &PageIndex,
        store: &PageStore,
        search_tags: &[String],
        parsed: &SearchArgs,
        is_wildcard: bool,
    ) -> ToolResult {
        // Try PageIndex search first (currently stub, returns empty)
        let query = if is_wildcard { "" } else { &search_tags.join(" ") };
        let index_results = index.search(&query, parsed.limit).await.unwrap_or_default();

        if !index_results.is_empty() {
            // TODO: Phase 3 - use semantic search results
        }

        // Fallback to PageStore list + in-memory tag filtering
        self.search_with_page_store(store, search_tags, parsed, is_wildcard)
            .await
    }

    /// PageStore-backed search via list + in-memory filtering.
    async fn search_with_page_store(
        &self,
        store: &PageStore,
        search_tags: &[String],
        parsed: &SearchArgs,
        is_wildcard: bool,
    ) -> ToolResult {
        let filter = PageFilter::default();
        let pages = store
            .list(filter)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("PageStore list failed: {}", e)))?;

        let mut results = Vec::new();

        for summary in pages {
            if is_wildcard {
                results.push(summary);
                if results.len() >= parsed.limit {
                    break;
                }
                continue;
            }

            // Check if any search tag matches title, tags, or category
            let matches = search_tags.iter().any(|tag| {
                let tag_lower = tag.to_lowercase();
                summary.title.to_lowercase().contains(&tag_lower)
                    || summary.tags.iter().any(|t| t.to_lowercase().contains(&tag_lower))
                    || summary
                        .category
                        .as_ref()
                        .map(|c| c.to_lowercase().contains(&tag_lower))
                        .unwrap_or(false)
            });

            if matches {
                results.push(summary);
                if results.len() >= parsed.limit {
                    break;
                }
            }
        }

        if results.is_empty() {
            return Ok(format!(
                "No memories found matching tags: {}",
                search_tags.join(", ")
            ));
        }

        let mut output = format!(
            "Found {} memor{} matching tags [{}]:\n\n",
            results.len(),
            if results.len() == 1 { "y" } else { "ies" },
            search_tags.join(", ")
        );

        for summary in &results {
            let tags_display = if summary.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", summary.tags.join(", "))
            };

            let category_display = summary
                .category
                .as_ref()
                .map(|c| format!(" | Category: {}", c))
                .unwrap_or_default();

            output.push_str(&format!(
                "━━━ {} ━━━\n  Type: {}{} | Tags:{} | Confidence: {:.1}\n  Path: {}\n\n",
                summary.title,
                summary.page_type.as_str(),
                category_display,
                tags_display,
                summary.confidence,
                summary.path
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

    // TODO: Add integration tests with mock PageStore
}
