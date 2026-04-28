//! Wiki decay tool — automated frequency degradation for wiki pages.
//!
//! Wraps `FrequencyManager::run_decay_batch` as a Tool so it can be
//! invoked directly by system cron jobs (zero LLM token cost).

use async_trait::async_trait;
use serde_json::Value;
use tracing::instrument;

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::wiki::{lifecycle::FrequencyManager, PageStore};

/// Tool for running wiki frequency decay.
pub struct WikiDecayTool {
    page_store: PageStore,
}

impl WikiDecayTool {
    pub fn new(page_store: PageStore) -> Self {
        Self { page_store }
    }
}

#[async_trait]
impl Tool for WikiDecayTool {
    fn name(&self) -> &str {
        "wiki_decay"
    }

    fn description(&self) -> &str {
        "Run wiki frequency decay to downgrade stale pages. \
         Hot pages become Warm after 7 days, Warm becomes Cold after 30 days, \
         Cold becomes Archived after 90 days. Profile/Decision/SOP/Source pages are exempt."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    #[instrument(name = "tool.wiki_decay", skip_all)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext) -> ToolResult {
        let report = FrequencyManager::run_decay_batch(self.page_store.db())
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Wiki decay failed: {}", e)))?;

        if report.decayed == 0 {
            Ok(format!(
                "No stale wiki pages found. {} candidates scanned.",
                report.total_scanned
            ))
        } else {
            Ok(format!(
                "Wiki decay complete: {} scanned, {} decayed{}",
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
