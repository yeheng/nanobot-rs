//! Tantivy-powered history search tool.
//!
//! Provides advanced full-text search over session history (stored in SQLite)
//! using the Tantivy search engine with support for:
//! - Boolean queries (AND/OR/NOT)
//! - Fuzzy matching with typo tolerance
//! - Role filtering (user/assistant/system)
//! - Session filtering
//! - Date range filtering

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::info;

use super::{simple_schema, Tool, ToolError, ToolResult};
use crate::search::tantivy::{open_history_index, HistoryIndexReader};
use crate::search::{BooleanQuery, DateRange, FuzzyQuery, SearchQuery, SortOrder};

/// Tool that searches session history using Tantivy full-text search.
pub struct HistoryTantivySearchTool {
    reader: Arc<HistoryIndexReader>,
}

impl HistoryTantivySearchTool {
    /// Create a new history tantivy search tool from a shared reader.
    pub fn new(reader: Arc<HistoryIndexReader>) -> Self {
        Self { reader }
    }

    /// Create with default paths.
    pub fn with_defaults() -> Result<Self, ToolError> {
        let config_dir = crate::config::config_dir();
        let index_path = config_dir.join("tantivy-index").join("history");

        let (reader, _writer) = open_history_index(&index_path).map_err(|e| {
            ToolError::ExecutionError(format!("Failed to open history index: {}", e))
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

    /// Filter by role (user/assistant/system/tool)
    role: Option<String>,

    /// Filter by session key
    session_key: Option<String>,

    /// Date range filter
    date_range: Option<DateRangeInput>,

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

#[derive(Debug, Deserialize)]
struct DateRangeInput {
    from: Option<String>,
    to: Option<String>,
}

fn default_limit() -> usize {
    10
}

#[async_trait]
impl Tool for HistoryTantivySearchTool {
    fn name(&self) -> &str {
        "history_tantivy_search"
    }

    fn description(&self) -> &str {
        "Advanced full-text search over conversation history using Tantivy. \
         Supports boolean queries, fuzzy matching, role filtering (user/assistant/system), \
         session filtering, and date range queries. Use this to find past conversations \
         or specific discussions."
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
                "role",
                "string",
                false,
                "Filter by role: 'user', 'assistant', 'system', or 'tool'",
            ),
            (
                "session_key",
                "string",
                false,
                "Filter by session key (e.g., 'telegram:123456')",
            ),
            (
                "date_range",
                "object",
                false,
                "Date range filter with 'from' and/or 'to' (ISO 8601 format)",
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
            role: parsed.role,
            session_key: parsed.session_key,
            date_range: parsed.date_range.map(|dr| DateRange {
                from: dr.from,
                to: dr.to,
            }),
            tags: Vec::new(),
            limit: parsed.limit,
            offset: 0,
            sort: match parsed.sort.as_str() {
                "date" => SortOrder::Date,
                _ => SortOrder::Relevance,
            },
        };

        info!("history_tantivy_search: executing query {:?}", search_query);

        // Execute search — direct call, no lock needed
        let results = self
            .reader
            .search(&search_query)
            .map_err(|e| ToolError::ExecutionError(format!("Search failed: {}", e)))?;

        if results.is_empty() {
            return Ok("No matches found in conversation history.".to_string());
        }

        // Format results
        let mut output = format!(
            "Found {} result{} in history:\n\n",
            results.len(),
            if results.len() == 1 { "" } else { "s" }
        );

        for (i, result) in results.iter().enumerate() {
            let role = result.source.as_deref().unwrap_or("unknown");
            output.push_str(&format!(
                "{}. [{}] **{}** (score: {:.2})\n",
                i + 1,
                role,
                result.id,
                result.score
            ));

            if let Some(ref ts) = result.timestamp {
                output.push_str(&format!("   Time: {}\n", ts));
            }

            if let Some(session_key) = result.metadata.get("session_key") {
                output.push_str(&format!("   Session: {}\n", session_key));
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
            "query": "API design",
            "role": "user",
            "limit": 20,
            "sort": "date"
        });

        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.query, Some("API design".to_string()));
        assert_eq!(parsed.role, Some("user".to_string()));
        assert_eq!(parsed.limit, 20);
        assert_eq!(parsed.sort, "date");
    }

    #[test]
    fn test_date_range_parsing() {
        let args = serde_json::json!({
            "date_range": {
                "from": "2024-01-01T00:00:00Z",
                "to": "2024-12-31T23:59:59Z"
            }
        });

        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        let dr = parsed.date_range.unwrap();
        assert_eq!(dr.from, Some("2024-01-01T00:00:00Z".to_string()));
        assert_eq!(dr.to, Some("2024-12-31T23:59:59Z".to_string()));
    }

    #[test]
    fn test_session_key_filter() {
        let args = serde_json::json!({
            "session_key": "telegram:123456",
            "query": "test"
        });

        let parsed: SearchArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.session_key, Some("telegram:123456".to_string()));
    }
}
