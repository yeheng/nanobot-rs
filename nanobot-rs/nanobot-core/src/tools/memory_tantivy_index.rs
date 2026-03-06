//! Tantivy-powered memory index management tool.
//!
//! Provides index management operations for memory files:
//! - `rebuild`: Clear and rebuild the entire index from memory directory
//! - `update`: Incremental update - only index new/modified files

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::info;

use super::{simple_schema, Tool, ToolError, ToolResult};
use crate::search::tantivy::{open_memory_index, MemoryIndexWriter};

/// Tool that manages the memory Tantivy index.
pub struct MemoryTantivyIndexTool {
    writer: Arc<Mutex<MemoryIndexWriter>>,
}

impl MemoryTantivyIndexTool {
    /// Create a new memory tantivy index tool from a shared writer.
    pub fn new(writer: Arc<Mutex<MemoryIndexWriter>>) -> Self {
        Self { writer }
    }

    /// Create with default paths.
    pub fn with_defaults() -> Result<Self, ToolError> {
        let config_dir = crate::config::config_dir();
        let index_path = config_dir.join("tantivy-index").join("memory");
        let memory_dir = config_dir.join("memory");

        let (_reader, writer) = open_memory_index(&index_path, &memory_dir).map_err(|e| {
            ToolError::ExecutionError(format!("Failed to open memory index: {}", e))
        })?;

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
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
impl Tool for MemoryTantivyIndexTool {
    fn name(&self) -> &str {
        "memory_tantivy_index"
    }

    fn description(&self) -> &str {
        "Manage the Tantivy full-text index for memory files. \
         Use 'rebuild' to completely rebuild the index (slow but thorough), \
         or 'update' for incremental updates (fast, only processes changes)."
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

        info!("memory_tantivy_index: executing {}", parsed.action);

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

impl MemoryTantivyIndexTool {
    async fn execute_rebuild(&self) -> ToolResult {
        let mut w = self.writer.lock().await;
        let count = w.rebuild().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to rebuild memory index: {}", e))
        })?;
        Ok(format!(
            "Memory index rebuilt successfully. {} documents indexed.",
            count
        ))
    }

    async fn execute_update(&self) -> ToolResult {
        let mut w = self.writer.lock().await;
        let stats = w.incremental_update().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to update memory index: {}", e))
        })?;
        Ok(format!(
            "Memory index updated. Added: {}, Updated: {}, Removed: {}",
            stats.added, stats.updated, stats.removed
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
