//! Spawn tool for background task execution (subagent)

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::instrument;

use super::base::{Tool, ToolError};
use crate::agent::subagent::{SubagentManager, SubagentTask};

/// Spawn tool for running background tasks via subagents.
///
/// When a `SubagentManager` is provided, tasks are actually dispatched to
/// independent agent loops. Without a manager the tool reports that spawning
/// is not available (used in CLI-only mode).
pub struct SpawnTool {
    manager: Option<Arc<SubagentManager>>,
}

impl SpawnTool {
    /// Create a new spawn tool (without a manager — spawn will be unavailable)
    pub fn new() -> Self {
        Self { manager: None }
    }

    /// Create a spawn tool backed by a SubagentManager
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
    action: Option<String>,
    task: Option<String>,
    timeout: Option<u64>,
    task_id: Option<String>,
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Manage background tasks. Actions: 'run' (spawn a task), 'status' (check task status), 'list' (list all tasks), 'cancel' (cancel a task)."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform: 'run', 'status', 'list', or 'cancel'",
                    "enum": ["run", "status", "list", "cancel"],
                    "default": "run"
                },
                "task": {
                    "type": "string",
                    "description": "Task description / prompt to execute in the background (required for 'run')"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (default: 300)"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID (required for 'status' and 'cancel')"
                }
            },
            "required": ["action"]
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

        let action = args.action.as_deref().unwrap_or("run");

        match action {
            "run" => {
                let prompt = args.task.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'task' is required for the 'run' action".to_string(),
                    )
                })?;

                if prompt.trim().is_empty() {
                    return Err(ToolError::InvalidArguments(
                        "Task description cannot be empty".to_string(),
                    ));
                }

                let mut task = SubagentTask::new(&prompt, "agent", "internal", "internal");

                if let Some(timeout) = args.timeout {
                    task = task.with_timeout(timeout);
                }

                let task_id = manager.submit(task).await.map_err(|e| {
                    ToolError::ExecutionError(format!("Failed to spawn task: {}", e))
                })?;

                Ok(format!(
                    "Background task spawned.\nTask ID: {}\nPrompt: {}\n\nUse spawn with action='status' and task_id='{}' to check progress.",
                    task_id,
                    &prompt[..prompt.len().min(100)],
                    task_id
                ))
            }
            "status" => {
                let task_id = args.task_id.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'task_id' is required for the 'status' action".to_string(),
                    )
                })?;

                match manager.get_task(&task_id).await {
                    Some(task) => {
                        let mut output = format!(
                            "Task: {}\nStatus: {:?}\nCreated: {}\n",
                            task.id, task.status, task.created_at
                        );
                        if let Some(started) = task.started_at {
                            output.push_str(&format!("Started: {}\n", started));
                        }
                        if let Some(completed) = task.completed_at {
                            output.push_str(&format!("Completed: {}\n", completed));
                        }
                        if let Some(result) = &task.result {
                            output.push_str(&format!("\nResult:\n{}\n", result));
                        }
                        if let Some(error) = &task.error {
                            output.push_str(&format!("\nError: {}\n", error));
                        }
                        Ok(output)
                    }
                    None => Ok(format!("Task not found: {}", task_id)),
                }
            }
            "list" => {
                let tasks = manager.get_all_tasks().await;
                if tasks.is_empty() {
                    return Ok("No background tasks.".to_string());
                }

                let mut output = format!("Background tasks ({}):\n\n", tasks.len());
                for task in tasks {
                    output.push_str(&format!(
                        "- {} [{:?}] {}\n",
                        &task.id[..8],
                        task.status,
                        &task.prompt[..task.prompt.len().min(60)]
                    ));
                }

                let stats = manager.stats().await;
                output.push_str(&format!(
                    "\nStats: {} running, {} completed, {} failed",
                    stats.running, stats.completed, stats.failed
                ));

                Ok(output)
            }
            "cancel" => {
                let task_id = args.task_id.ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "'task_id' is required for the 'cancel' action".to_string(),
                    )
                })?;

                if manager.cancel(&task_id).await {
                    Ok(format!("Task {} cancelled.", task_id))
                } else {
                    Ok(format!(
                        "Cannot cancel task {}. It may be already finished or not found.",
                        task_id
                    ))
                }
            }
            _ => Err(ToolError::InvalidArguments(format!(
                "Unknown action: '{}'. Use 'run', 'status', 'list', or 'cancel'.",
                action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_tool_creation() {
        let tool = SpawnTool::new();
        assert_eq!(tool.name(), "spawn");
        assert!(tool.description().contains("background"));
    }

    #[test]
    fn test_spawn_tool_parameters() {
        let tool = SpawnTool::new();
        let params = tool.parameters();
        assert!(params["properties"]["action"].is_object());
        assert!(params["properties"]["task"].is_object());
        assert!(params["properties"]["timeout"].is_object());
        assert!(params["properties"]["task_id"].is_object());
    }

    #[tokio::test]
    async fn test_spawn_without_manager() {
        let tool = SpawnTool::new();
        let result = tool
            .execute(serde_json::json!({
                "action": "run",
                "task": "do something"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not available"));
    }
}
