//! Tantivy-powered memory search tool.
//!
//! Provides advanced full-text search over `~/.nanobot/memory/*.md` files using
//! the Tantivy search engine with support for:
//! - Boolean queries (AND/OR/NOT)
//! - Fuzzy matching with typo tolerance
//! - Tag filtering
//! - Relevance scoring (BM25)

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::info;

use super::{simple_schema, Tool, ToolError, ToolResult};
use crate::search::tantivy::{open_memory_index, MemoryIndexReader};
use crate::search::{BooleanQuery, FuzzyQuery, SearchQuery, SortOrder};

/// Tool that searches memory files using Tantivy full-text search.
pub struct MemoryTantivySearchTool {
    reader: Arc<MemoryIndexReader>,
}

impl MemoryTantivySearchTool {
    /// Create a new memory tantivy search tool from a shared reader.
    pub fn new(reader: Arc<MemoryIndexReader>) -> Self {
        Self { reader }
    }

    /// Create with default paths.
    pub fn with_defaults() -> Result<Self, ToolError> {
        let config_dir = crate::config::config_dir();
        let index_path = config_dir.join("tantivy-index").join("memory");
        let memory_dir = config_dir.join("memory");

        let (reader, _writer) = open_memory_index(&index_path, &memory_dir).map_err(|e| {
            ToolError::ExecutionError(format!("Failed to open memory index: {}", e))
        })?;

        Ok(Self {
            reader: Arc::new(reader),
        })
    }
}

#[derive(Debug, Deserialize)]
struct SearchArgs {
    /// Full-text search query
    query: Option<String>,

    /// Boolean query with must/should/not clauses
    boolean: Option<BooleanQueryInput>,

    /// Fuzzy query with typo tolerance
    fuzzy: Option<FuzzyQueryInput>,

    /// Tag filters (AND semantics)
    #[serde(default)]
    tags: Vec<String>,

    /// Maximum number of results
    #[serde(default = "default_limit")]
    limit: usize,

    /// Sort order: "relevance" or "date"
    #[serde(default)]
    sort: String,
}

#[derive(Debug, Deserialize)]
struct BooleanQueryInput {
    must: Option<Vec<String>>,
    should: Option<Vec<String>>,
    not: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct FuzzyQueryInput {
    text: String,
    distance: Option<u8>,
    prefix: Option<bool>,
}

fn default_limit() -> usize {
    10
}

#[async_trait]
impl Tool for MemoryTantivySearchTool {
    fn name(&self) -> &str {
        "memory_tantivy_search"
    }

    fn description(&self) -> &str {
        "Advanced full-text search over long-term memory files using Tantivy. \
         Supports boolean queries (AND/OR/NOT), fuzzy matching for typos, \
         tag filtering, and relevance scoring. Use this for precise memory \
         retrieval when simple keyword search isn't enough."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            (
                "query",
                "string",
                false,
                "Full-text search query (keywords or phrases)",
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
                "Array of tags to filter by (AND semantics - all tags must match)",
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

        // Build search query
        let search_query = SearchQuery {
            text: parsed.query,
            boolean: parsed.boolean.map(|b| BooleanQuery {
                must: b.must.unwrap_or_default(),
                should: b.should.unwrap_or_default(),
                not: b.not.unwrap_or_default(),
            }),
            fuzzy: parsed.fuzzy.map(|f| FuzzyQuery {
                text: f.text,
                distance: f.distance.unwrap_or(2),
                prefix: f.prefix.unwrap_or(false),
            }),
            tags: parsed.tags,
            limit: parsed.limit,
            offset: 0,
            sort: match parsed.sort.as_str() {
                "date" => SortOrder::Date,
                _ => SortOrder::Relevance,
            },
            ..Default::default()
        };

        info!("memory_tantivy_search: executing query {:?}", search_query);

        // Execute search — direct call, no lock needed
        let results = self
            .reader
            .search(&search_query)
            .map_err(|e| ToolError::ExecutionError(format!("Search failed: {}", e)))?;

        if results.is_empty() {
            return Ok("No matches found in memory files.".to_string());
        }

        // Format results
        let mut output = format!(
            "Found {} result{} in memory:\n\n",
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

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_args_parsing() {
        let args = serde_json::json!({
            "query": "test query",
            "tags": ["rust", "project"],
            "limit": 5,
            "sort": "date"
        });

        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.query, Some("test query".to_string()));
        assert_eq!(parsed.tags, vec!["rust", "project"]);
        assert_eq!(parsed.limit, 5);
        assert_eq!(parsed.sort, "date");
    }

    #[test]
    fn test_boolean_query_parsing() {
        let args = serde_json::json!({
            "boolean": {
                "must": ["important"],
                "not": ["draft"]
            }
        });

        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        let bq = parsed.boolean.unwrap();
        assert_eq!(bq.must, Some(vec!["important".to_string()]));
        assert_eq!(bq.not, Some(vec!["draft".to_string()]));
    }

    #[test]
    fn test_fuzzy_query_parsing() {
        let args = serde_json::json!({
            "fuzzy": {
                "text": "projct",
                "distance": 1
            }
        });

        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        let fq = parsed.fuzzy.unwrap();
        assert_eq!(fq.text, "projct");
        assert_eq!(fq.distance, Some(1));
    }
}
