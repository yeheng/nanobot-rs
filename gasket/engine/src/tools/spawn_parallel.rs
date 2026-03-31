//! Parallel spawn tool for concurrent subagent execution with result aggregation

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument};

use super::base::{Tool, ToolContext, ToolError, ToolResult};

pub struct SpawnParallelTool;

impl Default for SpawnParallelTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SpawnParallelTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct TaskSpec {
    task: String,
    model_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TaskInput {
    Simple(Vec<String>),
    WithModels(Vec<TaskSpec>),
    /// Handle LLM passing JSON as a string
    JsonString(String),
}

#[derive(Deserialize)]
struct SpawnParallelArgs {
    tasks: TaskInput,
}

#[async_trait]
impl Tool for SpawnParallelTool {
    fn name(&self) -> &str {
        "spawn_parallel"
    }

    fn description(&self) -> &str {
        "Execute multiple tasks in parallel using subagents with optional per-task model selection. Returns aggregated responses from all subagents. Useful for parallel research, data gathering, or independent computations with different models."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "description": "List of tasks to execute in parallel. Can be simple strings or objects with task and model_id",
                    "oneOf": [
                        {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "minItems": 1,
                            "maxItems": 10
                        },
                        {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "task": {
                                        "type": "string",
                                        "description": "Task description"
                                    },
                                    "model_id": {
                                        "type": "string",
                                        "description": "Optional model profile ID for this specific task"
                                    }
                                },
                                "required": ["task"]
                            },
                            "minItems": 1,
                            "maxItems": 10
                        }
                    ]
                }
            },
            "required": ["tasks"]
        })
    }

    #[instrument(name = "tool.spawn_parallel", skip_all)]
    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        let args: SpawnParallelArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        // Get spawner from context
        let spawner = ctx.spawner.as_ref().ok_or_else(|| {
            ToolError::ExecutionError("No spawner available in ToolContext".to_string())
        })?;

        // Normalize tasks to TaskSpec format
        let task_specs: Vec<TaskSpec> = match args.tasks {
            TaskInput::Simple(tasks) => tasks
                .into_iter()
                .map(|task| TaskSpec {
                    task,
                    model_id: None,
                })
                .collect(),
            TaskInput::WithModels(specs) => specs,
            TaskInput::JsonString(json_str) => {
                // Try to parse the JSON string
                // First try as Vec<TaskSpec> (with models)
                if let Ok(specs) = serde_json::from_str::<Vec<TaskSpec>>(&json_str) {
                    specs
                } else if let Ok(tasks) = serde_json::from_str::<Vec<String>>(&json_str) {
                    tasks
                        .into_iter()
                        .map(|task| TaskSpec {
                            task,
                            model_id: None,
                        })
                        .collect()
                } else {
                    return Err(ToolError::InvalidArguments(
                        "Failed to parse tasks JSON string. Expected array of strings or objects with 'task' field.".to_string()
                    ));
                }
            }
        };

        if task_specs.is_empty() {
            return Err(ToolError::InvalidArguments(
                "At least one task is required".to_string(),
            ));
        }

        if task_specs.len() > 10 {
            return Err(ToolError::InvalidArguments(
                "Maximum 10 parallel tasks allowed".to_string(),
            ));
        }

        info!("Spawning {} parallel subagent tasks", task_specs.len());

        // Spawn all tasks concurrently
        let mut handles = Vec::with_capacity(task_specs.len());
        for spec in task_specs {
            let spawner_clone = spawner.clone();
            let handle =
                tokio::spawn(async move { spawner_clone.spawn(spec.task, spec.model_id).await });
            handles.push(handle);
        }

        // Collect all results
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            let result = handle
                .await
                .map_err(|e| ToolError::ExecutionError(format!("Task join error: {}", e)))?
                .map_err(|e| ToolError::ExecutionError(format!("Spawn error: {}", e)))?;
            results.push(result);
        }

        // Aggregate results
        let mut output = format!("Completed {} parallel tasks:\n\n", results.len());
        for (idx, result) in results.iter().enumerate() {
            // Include thinking content if available
            if let Some(ref reasoning) = result.response.reasoning_content {
                if !reasoning.is_empty() {
                    output.push_str(&format!("**Thinking:**\n{}\n\n", reasoning));
                }
            }

            output.push_str(&format!(
                "## Task {}\n**Model:** {}\n**Prompt:** {}\n**Response:**\n{}\n\n",
                idx + 1,
                result.model.as_deref().unwrap_or("unknown"),
                result.task,
                result.response.content
            ));
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_name() {
        let tool = SpawnParallelTool::new();
        assert_eq!(tool.name(), "spawn_parallel");
    }

    #[test]
    fn test_tool_description() {
        let tool = SpawnParallelTool::new();
        assert!(tool.description().contains("parallel"));
        assert!(tool.description().contains("subagents"));
    }

    #[test]
    fn test_parameters_schema() {
        let tool = SpawnParallelTool::new();
        let params = tool.parameters();

        assert_eq!(params["type"], "object");
        assert!(params["properties"]["tasks"].is_object());
        assert_eq!(params["required"][0], "tasks");
    }

    #[tokio::test]
    async fn test_empty_tasks_validation() {
        let tool = SpawnParallelTool::new();
        let args = serde_json::json!({
            "tasks": []
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_too_many_tasks_validation() {
        let tool = SpawnParallelTool::new();
        let tasks: Vec<String> = (0..15).map(|i| format!("Task {}", i)).collect();
        let args = serde_json::json!({
            "tasks": tasks
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_no_spawner_error() {
        let tool = SpawnParallelTool::new();
        let args = serde_json::json!({
            "tasks": ["Task 1"]
        });

        let result = tool.execute(args, &ToolContext::default()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No spawner"));
    }

    #[test]
    fn test_json_string_parsing_with_models() {
        // Simulate LLM passing tasks as a JSON string
        let json_str = r#"[{"task": "Task 1", "model_id": "gpt-4"}, {"task": "Task 2"}]"#;
        let args = serde_json::json!({
            "tasks": json_str
        });
        let parsed: SpawnParallelArgs = serde_json::from_value(args).unwrap();
        match parsed.tasks {
            TaskInput::JsonString(s) => {
                assert_eq!(s, json_str);
            }
            _ => panic!("Expected JsonString variant"),
        }
    }
}
