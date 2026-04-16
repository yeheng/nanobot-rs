//! Spawn tool for subagent execution with synchronous blocking and streaming output
//!
//! This tool spawns a subagent and blocks until completion, streaming events
//! to the WebSocket/channel in real-time. This ensures the main agent waits
//! for results instead of using fire-and-forget semantics.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument};

use super::base::{Tool, ToolContext, ToolError, ToolResult};

pub struct SpawnTool;

impl SpawnTool {
    pub fn new() -> Self {
        Self
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
    model_id: Option<String>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to execute a task synchronously with real-time streaming output. \
         The main agent blocks until the subagent completes and returns the result. \
         Use this for tasks that need immediate results with progress feedback."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Task description / prompt to execute"
                },
                "model_id": {
                    "type": "string",
                    "description": "Optional model profile ID to use for this subagent. If not specified, uses the default model."
                }
            },
            "required": ["task"]
        })
    }

    #[instrument(name = "tool.spawn", skip_all)]
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let args: SpawnArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        if args.task.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "Task description cannot be empty".to_string(),
            ));
        }

        // Get spawner from context (always present, may be NoopSpawner)
        let spawner = &ctx.spawner;

        info!("[Spawn] Starting subagent for task: {}", args.task);

        // Spawn subagent via the trait
        let result = spawner
            .spawn(args.task.clone(), args.model_id.clone())
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn subagent: {}", e)))?;

        // Format output
        let mut output = String::new();

        // Include thinking content if available
        if let Some(ref reasoning) = result.response.reasoning_content {
            if !reasoning.is_empty() {
                output.push_str(&format!("**Thinking:**\n{}\n\n", reasoning));
            }
        }

        output.push_str(&format!(
            "**Model:** {}\n**Task:** {}\n\n**Response:**\n{}",
            result.model.as_deref().unwrap_or("unknown"),
            result.task,
            result.response.content
        ));

        Ok(output)
    }
}
