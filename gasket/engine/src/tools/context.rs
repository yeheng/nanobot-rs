//! Context management tool for inspecting and controlling context window usage.
//!
//! Provides three actions via the `action` parameter:
//! - `usage`  — token usage stats (budget, threshold, current usage, status)
//! - `watermark` — compaction boundary (covered sequence, max sequence, progress)
//! - `compact` — force immediate context compression to free up tokens
//!
//! The tool delegates to [`ContextCompactor`] for all store access and
//! compaction logic, keeping the tool itself a thin adapter.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::instrument;

use super::{Tool, ToolContext, ToolError, ToolResult};
use crate::session::compactor::ContextCompactor;

/// Tool for managing conversation context window.
///
/// Wraps [`ContextCompactor`] to expose usage stats, watermark info, and
/// force-compaction as a tool callable by the agent.
pub struct ContextTool {
    compactor: Arc<ContextCompactor>,
}

impl ContextTool {
    /// Create a new context tool backed by the given compactor.
    pub fn new(compactor: Arc<ContextCompactor>) -> Self {
        Self { compactor }
    }
}

#[derive(Deserialize)]
struct ContextArgs {
    action: String,
}

#[async_trait]
impl Tool for ContextTool {
    fn name(&self) -> &str {
        "context"
    }

    fn description(&self) -> &str {
        "Inspect and manage conversation context window. \
         Actions: 'usage' — show token usage, budget, and compression status; \
         'watermark' — show compaction boundary (covered vs. max sequence); \
         'compact' — force immediate context compression to free up tokens."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["usage", "watermark", "compact"],
                    "description": "'usage' = token usage stats, 'watermark' = compaction boundary, 'compact' = force compression"
                }
            },
            "required": ["action"]
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[instrument(name = "tool.context", skip_all)]
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let args: ContextArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "usage" => self.exec_usage(&ctx.session_key).await,
            "watermark" => self.exec_watermark(&ctx.session_key).await,
            "compact" => self.exec_compact(&ctx.session_key).await,
            _ => Err(ToolError::InvalidArguments(format!(
                "Unknown action '{}'. Valid: usage, watermark, compact",
                args.action
            ))),
        }
    }
}

impl ContextTool {
    async fn exec_usage(&self, session_key: &gasket_types::SessionKey) -> ToolResult {
        let stats = self
            .compactor
            .get_usage_stats(session_key)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to get usage: {}", e)))?;

        Ok(format!(
            "Context Usage:\n  Token budget: {}\n  Auto-compact at: {} tokens ({:.0}%)\n  Current: {} tokens ({:.1}%)\n  Summary: {} tokens\n  Events: {} uncompacted ({} tokens)\n  Status: {}",
            stats.token_budget,
            stats.threshold_tokens,
            stats.compaction_threshold * 100.0,
            stats.current_tokens,
            stats.usage_percent,
            stats.summary_tokens,
            stats.uncompacted_events,
            stats.event_tokens,
            if stats.is_compressing { "compressing..." } else { "idle" }
        ).into())
    }

    async fn exec_watermark(&self, session_key: &gasket_types::SessionKey) -> ToolResult {
        let info = self
            .compactor
            .get_watermark_info(session_key)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to get watermark: {}", e)))?;

        Ok(format!(
            "Context Watermark:\n  Covered sequence: {}\n  Max sequence: {}\n  Uncompacted events: {}\n  Compacted: {:.1}% of history",
            info.watermark, info.max_sequence, info.uncompacted_count, info.compacted_percent
        ).into())
    }

    async fn exec_compact(&self, session_key: &gasket_types::SessionKey) -> ToolResult {
        let triggered = self.compactor.force_compact(session_key, &[]);
        if triggered {
            Ok("Compaction triggered. Running in background.".into())
        } else {
            Ok("Compaction skipped: already in progress.".into())
        }
    }
}
