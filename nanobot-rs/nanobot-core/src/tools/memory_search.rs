//! Memory search tool — exposes SQLite FTS5 full-text search to the agent.
//!
//! Wraps `MemoryStore::search` so the agent can query structured memories
//! stored in the L3 archive tier (SQLite `memories` table with FTS5 index).

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{simple_schema, Tool, ToolResult};
use crate::agent::memory::MemoryStore;
use crate::memory::MemoryQuery;

/// Tool that allows the agent to search the SQLite FTS5 memory archive.
pub struct MemorySearchTool {
    store: Arc<MemoryStore>,
}

impl MemorySearchTool {
    /// Create a new memory search tool backed by the given store.
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search long-term memory archive using full-text search. \
         Returns matching memory entries from the SQLite FTS5 index. \
         Use this to recall past decisions, lessons learned, or archived knowledge."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[
            ("query", "string", true, "Search query text (keywords or phrases)"),
            (
                "tags",
                "array",
                false,
                "Optional tag filters (AND semantics — entry must have all listed tags)",
            ),
            (
                "limit",
                "integer",
                false,
                "Maximum number of results to return (default: 10)",
            ),
        ])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        let parsed: SearchArgs = serde_json::from_value(args).map_err(|e| {
            super::ToolError::InvalidArguments(format!("Invalid arguments: {}", e))
        })?;

        let query = MemoryQuery {
            text: Some(parsed.query.clone()),
            tags: parsed.tags,
            limit: Some(parsed.limit.unwrap_or(10)),
            ..Default::default()
        };

        let results = self
            .store
            .search(&query)
            .await
            .map_err(|e| super::ToolError::ExecutionError(format!("Search failed: {}", e)))?;

        if results.is_empty() {
            return Ok(format!(
                "No memories found matching '{}'.",
                parsed.query
            ));
        }

        let mut output = format!("Found {} memor{}:\n\n", results.len(), if results.len() == 1 { "y" } else { "ies" });
        for (i, entry) in results.iter().enumerate() {
            output.push_str(&format!(
                "--- Memory {} [{}] ---\n{}\n(tags: {:?}, updated: {})\n\n",
                i + 1,
                entry.id,
                entry.content,
                entry.metadata.tags,
                entry.updated_at.format("%Y-%m-%d %H:%M"),
            ));
        }

        Ok(output)
    }
}
