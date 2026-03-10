//! Spawn tool for background task execution (subagent)

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::instrument;

use super::base::{Tool, ToolError};
use crate::agent::subagent::SubagentManager;

pub struct SpawnTool {
    manager: Option<Arc<SubagentManager>>,
}

impl SpawnTool {
    pub fn new() -> Self {
        Self { manager: None }
    }

    pub fn with_manager(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager: Some(manager),
        }
    }
}

impl Default for SpawnTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct SpawnArgs {
    task: String,
    channel: Option<String>,
    chat_id: Option<String>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a background task to execute a prompt asynchronously."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description / prompt to execute in the background"
                },
                "channel": {
                    "type": "string",
                    "description": "Target channel to reply to (e.g. telegram, discord). Default is cli.",
                    "default": "cli"
                },
                "chat_id": {
                    "type": "string",
                    "description": "Target chat ID to reply to",
                    "default": "internal"
                }
            },
            "required": ["task"]
        })
    }

    #[instrument(name = "tool.spawn", skip_all)]
    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let args: SpawnArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let manager = match &self.manager {
            Some(m) => m,
            None => {
                return Err(ToolError::ExecutionError(
                    "Background task spawning is not available in this mode.".to_string(),
                ))
            }
        };

        if args.task.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "Task description cannot be empty".to_string(),
            ));
        }

        let channel = args.channel.unwrap_or_else(|| "cli".to_string());
        let chat_id = args.chat_id.unwrap_or_else(|| "internal".to_string());

        manager
            .submit(&args.task, &channel, &chat_id)
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn task: {}", e)))?;

        Ok(format!("Background task started: {}", &args.task))
    }
}
