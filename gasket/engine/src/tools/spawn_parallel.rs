//! Parallel spawn tool for concurrent subagent execution with result aggregation

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, instrument};

use super::{format_subagent_response, Tool, ToolContext, ToolError, ToolResult};

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
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> ToolResult {
        // Cheap pre-check on the tasks array length before full deserialization.
        if let Some(tasks_arr) = args.get("tasks").and_then(|v| v.as_array()) {
            if tasks_arr.len() > 10 {
                return Err(ToolError::InvalidArguments(
                    "Maximum 10 parallel tasks allowed".to_string(),
                ));
            }
        }

        let args: SpawnParallelArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        // Get spawner from context (always present, may be NoopSpawner)
        let spawner = &ctx.spawner;

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

        // Spawn tasks with bounded concurrency to avoid API rate limits (429).
        // Max 5 concurrent LLM calls is a safe default across most providers.
        let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
        let mut handles = Vec::with_capacity(task_specs.len());
        for (idx, spec) in task_specs.into_iter().enumerate() {
            let spawner_clone = spawner.clone();
            let sem = semaphore.clone();
            let session_key = ctx.session_key.clone();
            let outbound_tx = ctx.outbound_tx.clone();
            let ws_summary_limit = ctx.ws_summary_limit;
            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                let (subagent_id, mut event_rx, result_rx) = spawner_clone
                    .spawn_with_stream(spec.task.clone(), spec.model_id)
                    .await
                    .map_err(|e| {
                        ToolError::ExecutionError(format!("Failed to spawn subagent: {}", e))
                    })?;

                // Notify frontend that subagent has started
                let _ = outbound_tx
                    .send(gasket_types::events::OutboundMessage::with_ws_message(
                        session_key.channel.clone(),
                        session_key.chat_id.clone(),
                        gasket_types::events::ChatEvent::subagent_started(
                            subagent_id.clone(),
                            spec.task.clone(),
                            idx as u32,
                        ),
                    ))
                    .await;

                // Forward subagent events to WebSocket in real-time
                let fwd_subagent_id = subagent_id.clone();
                let fwd_session_key = session_key.clone();
                let fwd_outbound_tx = outbound_tx.clone();
                let forward_handle = tokio::spawn(async move {
                    while let Some(event) = event_rx.recv().await {
                        use gasket_types::events::ChatEvent;
                        use gasket_types::StreamEventKind;

                        let chat_event = match &event.kind {
                            StreamEventKind::Thinking { content } => {
                                Some(ChatEvent::subagent_thinking(
                                    fwd_subagent_id.clone(),
                                    content.as_ref(),
                                ))
                            }
                            StreamEventKind::ToolStart { name, arguments } => {
                                Some(ChatEvent::subagent_tool_start(
                                    fwd_subagent_id.clone(),
                                    name.as_ref(),
                                    arguments.as_ref().map(|s| s.to_string()),
                                ))
                            }
                            StreamEventKind::ToolEnd { name, output } => {
                                Some(ChatEvent::subagent_tool_end(
                                    fwd_subagent_id.clone(),
                                    name.as_ref(),
                                    output.as_ref().map(|s| s.to_string()),
                                ))
                            }
                            StreamEventKind::Content { content } => {
                                Some(ChatEvent::subagent_content(
                                    fwd_subagent_id.clone(),
                                    content.as_ref(),
                                ))
                            }
                            _ => None,
                        };

                        if let Some(chat_event) = chat_event {
                            let msg = gasket_types::events::OutboundMessage::with_ws_message(
                                fwd_session_key.channel.clone(),
                                fwd_session_key.chat_id.clone(),
                                chat_event,
                            );
                            let _ = fwd_outbound_tx.send(msg).await;
                        }
                    }
                });

                let result = result_rx.await.map_err(|e| {
                    ToolError::ExecutionError(format!("Subagent result channel closed: {}", e))
                })?;

                // Notify frontend that subagent has completed
                let summary = if ws_summary_limit == 0 {
                    result.response.content.clone()
                } else {
                    result
                        .response
                        .content
                        .chars()
                        .take(ws_summary_limit)
                        .collect::<String>()
                };
                let _ = outbound_tx
                    .send(gasket_types::events::OutboundMessage::with_ws_message(
                        session_key.channel.clone(),
                        session_key.chat_id.clone(),
                        gasket_types::events::ChatEvent::subagent_completed(
                            subagent_id,
                            idx as u32,
                            summary,
                            result.response.tools_used.len() as u32,
                        ),
                    ))
                    .await;

                let _ = forward_handle.await;

                Ok::<_, ToolError>(result)
            });
            handles.push(handle);
        }

        // Collect all results
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            let result = handle
                .await
                .map_err(|e| ToolError::ExecutionError(format!("Task join error: {}", e)))??;
            results.push(result);
        }

        // Aggregate results
        let mut output = format!("Completed {} parallel tasks:\n\n", results.len());
        for (idx, result) in results.iter().enumerate() {
            output.push_str(&format!("## Task {}\n", idx + 1));
            output.push_str(&format_subagent_response(result));
            output.push_str("\n\n");
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
        assert!(result.unwrap_err().to_string().contains("not available"));
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
