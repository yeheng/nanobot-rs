//! Tantivy-powered history index management tool.
//!
//! Provides index management operations for session history:
//! - `rebuild`: Clear and rebuild the entire index from SQLite database
//! - `update`: Incremental update - only sync new/deleted messages

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::info;

use super::{simple_schema, Tool, ToolError, ToolResult};
use crate::memory::SqliteStore;
use crate::search::tantivy::{open_history_index, HistoryIndexWriter};

/// Tool that manages the history Tantivy index.
pub struct HistoryTantivyIndexTool {
    writer: Arc<Mutex<HistoryIndexWriter>>,
    db: SqliteStore,
}

impl HistoryTantivyIndexTool {
    /// Create a new history tantivy index tool from a shared writer.
    pub fn new(writer: Arc<Mutex<HistoryIndexWriter>>, db: SqliteStore) -> Self {
        Self { writer, db }
    }

    /// Create with default paths.
    pub async fn with_defaults() -> Result<Self, ToolError> {
        let config_dir = crate::config::config_dir();
        let index_path = config_dir.join("tantivy-index").join("history");

        let (_reader, writer) = open_history_index(&index_path).map_err(|e| {
            ToolError::ExecutionError(format!("Failed to open history index: {}", e))
        })?;

        let db = SqliteStore::new()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to open database: {}", e)))?;

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            db,
        })
    }
}

#[derive(Debug, Deserialize)]
struct IndexArgs {
    /// Action to perform: "rebuild" or "update"
    #[serde(default = "default_action")]
    action: String,
}

fn default_action() -> String {
    "update".to_string()
}

#[async_trait]
impl Tool for HistoryTantivyIndexTool {
    fn name(&self) -> &str {
        "history_tantivy_index"
    }

    fn description(&self) -> &str {
        "Manage the Tantivy full-text index for conversation history. \
         Use 'rebuild' to completely rebuild the index from the database (slow but thorough), \
         or 'update' for incremental sync (fast, only processes changes)."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[(
            "action",
            "string",
            false,
            "Action: 'rebuild' (full rebuild) or 'update' (incremental sync, default)",
        )])
    }

    async fn execute(&self, args: Value) -> ToolResult {
        let parsed: IndexArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("Invalid arguments: {}", e)))?;

        info!("history_tantivy_index: executing {}", parsed.action);

        match parsed.action.as_str() {
            "rebuild" => self.execute_rebuild().await,
            "update" => self.execute_update().await,
            _ => Err(ToolError::InvalidArguments(format!(
                "Unknown action: {}. Use 'rebuild' or 'update'.",
                parsed.action
            ))),
        }
    }
}

impl HistoryTantivyIndexTool {
    async fn execute_rebuild(&self) -> ToolResult {
        let mut w = self.writer.lock().await;
        let count = w.rebuild_from_db(&self.db).await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to rebuild history index: {}", e))
        })?;
        Ok(format!(
            "History index rebuilt successfully. {} messages indexed.",
            count
        ))
    }

    async fn execute_update(&self) -> ToolResult {
        let mut w = self.writer.lock().await;
        let stats = w.incremental_update(&self.db).await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to update history index: {}", e))
        })?;
        Ok(format!(
            "History index updated. Added: {}, Removed: {}",
            stats.added, stats.removed
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_args_parsing() {
        let args = serde_json::json!({
            "action": "rebuild"
        });

        let parsed: IndexArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.action, "rebuild");
    }

    #[test]
    fn test_index_args_default() {
        let args = serde_json::json!({});
        let parsed: IndexArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.action, "update");
    }
}
