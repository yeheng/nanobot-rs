//! Memory decay tool for automated frequency degradation
//!
//! Wraps `FrequencyManager::run_decay_batch` as a Tool so it can be
//! invoked directly by system cron jobs (zero LLM token cost).

use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::instrument;

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::memory::{FileMemoryStore, FrequencyManager, MetadataStore, SqliteStore};

/// Tool for running memory frequency decay
pub struct MemoryDecayTool {
    workspace: PathBuf,
    db: Arc<SqliteStore>,
}

impl MemoryDecayTool {
    /// Create a new memory decay tool
    pub fn new(workspace: PathBuf, db: Arc<SqliteStore>) -> Self {
        Self { workspace, db }
    }
}

#[async_trait]
impl Tool for MemoryDecayTool {
    fn name(&self) -> &str {
        "memory_decay"
    }

    fn description(&self) -> &str {
        "Run memory frequency decay to downgrade stale memories. \
         Hot memories become Warm after 7 days, Warm becomes Cold after 30 days, \
         Cold becomes Archived after 90 days. Profile/Decision/Reference memories are exempt."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    #[instrument(name = "tool.memory_decay", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext) -> ToolResult {
        let memory_dir = self.workspace.join("memory");
        if !memory_dir.exists() {
            return Ok("Memory directory does not exist. Nothing to decay.".to_string());
        }

        let store = FileMemoryStore::new(memory_dir);
        let metadata_store = MetadataStore::new(self.db.pool().clone());

        let report = FrequencyManager::run_decay_batch(&store, &metadata_store)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Memory decay failed: {}", e)))?;

        if report.decayed == 0 {
            Ok(format!(
                "No stale memories found. {} candidates scanned.",
                report.total_scanned
            ))
        } else {
            Ok(format!(
                "Memory decay complete: {} scanned, {} decayed{}",
                report.total_scanned,
                report.decayed,
                if report.errors > 0 {
                    format!(", {} errors", report.errors)
                } else {
                    String::new()
                }
            ))
        }
    }
}
