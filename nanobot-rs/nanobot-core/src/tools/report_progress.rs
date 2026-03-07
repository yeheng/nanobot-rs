//! Progress reporting tool for executing agents.
//!
//! Ministry agents call this tool to report progress, update their
//! heartbeat timestamp, and optionally notify the orchestrator.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::instrument;

use super::base::{Tool, ToolError, ToolResult};
use crate::pipeline::orchestrator::PipelineEvent;
use crate::pipeline::store::PipelineStore;

pub struct ReportProgressTool {
    store: PipelineStore,
    event_tx: mpsc::Sender<PipelineEvent>,
}

impl ReportProgressTool {
    pub fn new(store: PipelineStore, event_tx: mpsc::Sender<PipelineEvent>) -> Self {
        Self { store, event_tx }
    }
}

#[derive(Deserialize)]
struct ProgressArgs {
    task_id: String,
    agent_role: String,
    content: String,
    percentage: Option<f32>,
}

#[async_trait]
impl Tool for ReportProgressTool {
    fn name(&self) -> &str {
        "report_progress"
    }

    fn description(&self) -> &str {
        "Report execution progress for a pipeline task. Updates the heartbeat \
         and appends a progress log entry."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The pipeline task ID"
                },
                "agent_role": {
                    "type": "string",
                    "description": "Your role name (e.g. gong, hu)"
                },
                "content": {
                    "type": "string",
                    "description": "Progress description"
                },
                "percentage": {
                    "type": "number",
                    "description": "Optional completion percentage (0-100)"
                }
            },
            "required": ["task_id", "agent_role", "content"]
        })
    }

    #[instrument(name = "tool.report_progress", skip_all)]
    async fn execute(&self, args: Value) -> ToolResult {
        let args: ProgressArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        // Validate percentage range
        if let Some(pct) = args.percentage {
            if !(0.0..=100.0).contains(&pct) {
                return Err(ToolError::InvalidArguments(
                    "percentage must be between 0 and 100".into(),
                ));
            }
        }

        // Persist progress + update heartbeat
        self.store
            .append_progress(
                &args.task_id,
                &args.agent_role,
                &args.content,
                args.percentage,
            )
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        // Notify orchestrator
        let _ = self
            .event_tx
            .send(PipelineEvent::ProgressReported {
                task_id: args.task_id.clone(),
                agent_role: args.agent_role.clone(),
            })
            .await;

        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "recorded",
            "task_id": args.task_id,
            "percentage": args.percentage,
        }))
        .unwrap())
    }
}
