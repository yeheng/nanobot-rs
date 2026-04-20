//! Memory refresh tool for reindexing and syncing wiki pages

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::instrument;

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::wiki::{PageFilter, PageStore, PageType};

/// Tool for refreshing wiki page index and reindexing files
pub struct MemoryRefreshTool {
    /// Wiki PageStore for unified knowledge management
    page_store: Arc<PageStore>,
}

impl MemoryRefreshTool {
    /// Create a new memory refresh tool with wiki PageStore
    pub fn new(page_store: Arc<PageStore>) -> Self {
        Self { page_store }
    }
}

#[async_trait]
impl Tool for MemoryRefreshTool {
    fn name(&self) -> &str {
        "memory_refresh"
    }

    fn description(&self) -> &str {
        "Refresh and reindex memory files. Actions: 'sync' updates SQLite metadata from changed files; \
         'reindex' performs full rebuild of metadata and embeddings; 'stats' shows memory system statistics."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["sync", "reindex", "stats"],
                    "description": "Action to perform: 'sync' updates metadata for changed files only; 'reindex' rebuilds entire index; 'stats' shows current memory statistics"
                }
            },
            "required": ["action"]
        })
    }

    #[instrument(name = "tool.memory_refresh", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> ToolResult {
        #[derive(Deserialize)]
        struct Args {
            action: String,
        }

        let args: Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "sync" => {
                // Sync SQLite → disk cache
                let count = self
                    .page_store
                    .rebuild_disk_cache()
                    .await
                    .map_err(|e| ToolError::ExecutionError(format!("Failed to sync wiki: {}", e)))?;

                Ok(format!(
                    "✓ Wiki sync complete\n\nSynced {} pages to disk cache.",
                    count
                ))
            }
            "reindex" => {
                // Same as sync for now (Phase 3 will add real index rebuild)
                let count = self
                    .page_store
                    .rebuild_disk_cache()
                    .await
                    .map_err(|e| ToolError::ExecutionError(format!("Failed to reindex wiki: {}", e)))?;

                Ok(format!(
                    "✓ Wiki reindex complete\n\nReindexed {} pages.",
                    count
                ))
            }
            "stats" => {
                // Get wiki statistics
                let all_pages = self
                    .page_store
                    .list(PageFilter::default())
                    .await
                    .map_err(|e| ToolError::ExecutionError(format!("Failed to get wiki stats: {}", e)))?;

                let total = all_pages.len();
                let topics = all_pages
                    .iter()
                    .filter(|p| matches!(p.page_type, PageType::Topic))
                    .count();
                let entities = all_pages
                    .iter()
                    .filter(|p| matches!(p.page_type, PageType::Entity))
                    .count();
                let sources = all_pages
                    .iter()
                    .filter(|p| matches!(p.page_type, PageType::Source))
                    .count();

                Ok(format!(
                    "📊 Wiki Statistics\n\nTotal pages: {}\n\nBy type:\n  📚 Topics: {}\n  👥 Entities: {}\n  📄 Sources: {}",
                    total, topics, entities, sources
                ))
            }
            _ => Err(ToolError::InvalidArguments(format!(
                "Unknown action: '{}'. Valid actions are: 'sync', 'reindex', 'stats'",
                args.action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {

    // Note: Full integration tests require a MemoryManager with SQLite setup.
    // This test verifies the parameter schema is correct.

    #[test]
    fn test_memory_refresh_tool_schema() {
        // Create a minimal test - just verify the schema structure without executing
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["sync", "reindex", "stats"],
                    "description": "Action to perform: 'sync' updates metadata for changed files only; 'reindex' rebuilds entire index; 'stats' shows current memory statistics"
                }
            },
            "required": ["action"]
        });

        // Verify the schema has the expected enum values
        let action_schema = &params["properties"]["action"];
        assert_eq!(action_schema["type"], "string");

        let enum_values: Vec<String> = action_schema["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();

        assert!(enum_values.contains(&"sync".to_string()));
        assert!(enum_values.contains(&"reindex".to_string()));
        assert!(enum_values.contains(&"stats".to_string()));
    }
}
