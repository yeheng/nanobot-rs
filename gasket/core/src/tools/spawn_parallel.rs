//! Parallel spawn tool for concurrent subagent execution with result aggregation

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::join_all;
use serde::Deserialize;
use serde_json::Value;
use tokio::time::timeout;
use tracing::{info, instrument, trace, warn};

use super::base::{Tool, ToolError};
use crate::agent::stream_buffer::BufferedEvents;
use crate::agent::subagent::SubagentManager;
use crate::agent::subagent_tracker::{SubagentEvent, SubagentTracker};
use crate::bus::events::{OutboundMessage, WebSocketMessage};
use crate::config::ModelRegistry;
use crate::providers::ProviderRegistry;

pub struct SpawnParallelTool {
    manager: Option<Arc<SubagentManager>>,
    model_registry: Option<Arc<ModelRegistry>>,
    provider_registry: Option<Arc<ProviderRegistry>>,
}

impl Default for SpawnParallelTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SpawnParallelTool {
    pub fn new() -> Self {
        Self {
            manager: None,
            model_registry: None,
            provider_registry: None,
        }
    }

    pub fn with_manager(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager: Some(manager),
            model_registry: None,
            provider_registry: None,
        }
    }

    pub fn with_registries(
        manager: Arc<SubagentManager>,
        model_registry: Arc<ModelRegistry>,
        provider_registry: Arc<ProviderRegistry>,
    ) -> Self {
        Self {
            manager: Some(manager),
            model_registry: Some(model_registry),
            provider_registry: Some(provider_registry),
        }
    }

    /// Select model based on model_id with fallback and optional smart selection.
    ///
    /// Selection logic:
    /// 1. If model_id is provided, try exact match first
    /// 2. If not found, fallback to default model
    /// 3. If no model_id and smart-model-selection feature is enabled,
    ///    analyze task content and select by capability
    /// 4. Otherwise use default model
    /// 5. Return None if no model profile matches (use manager default)
    fn select_model<'a>(
        &'a self,
        model_id: &'a Option<String>,
        _task: &str,
    ) -> Option<(&'a str, &'a crate::config::ModelProfile)> {
        let model_registry = self.model_registry.as_ref()?;

        match model_id {
            Some(id) => {
                // Try exact match with fallback to default
                model_registry.get_profile_with_fallback(Some(id))
            }
            None => {
                // No model_id specified
                #[cfg(feature = "smart-model-selection")]
                {
                    // Smart selection based on task content
                    model_registry.select_by_capability(_task)
                }
                #[cfg(not(feature = "smart-model-selection"))]
                {
                    // Use default model
                    model_registry.get_profile_with_fallback(None)
                }
            }
        }
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
    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let args: SpawnParallelArgs =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let manager = match &self.manager {
            Some(m) => m,
            None => {
                return Err(ToolError::ExecutionError(
                    "Parallel task spawning is not available in this mode.".to_string(),
                ))
            }
        };

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
                    // Try as Vec<String> (simple)
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

        let mut tracker = SubagentTracker::new();
        let result_tx = tracker.result_sender();
        let event_tx = tracker.event_sender();
        let cancellation_token = tracker.cancellation_token();
        let task_count = task_specs.len();

        info!(
            "Preparing {} parallel subagent tasks with streaming support",
            task_count
        );

        // Prepare spawn configurations for all tasks first (sequential but fast)
        // Use Box<dyn Future> to unify different async block types
        let mut spawn_futures: Vec<
            std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>>,
        > = Vec::with_capacity(task_count);

        // Track subagent_id -> task_index mapping for [Task N] labeling
        let mut task_id_map: HashMap<String, usize> = HashMap::with_capacity(task_count);

        for (idx, spec) in task_specs.into_iter().enumerate() {
            let subagent_id = SubagentTracker::generate_id();
            // Record mapping for [Task N] labeling in WebSocket messages
            task_id_map.insert(subagent_id.clone(), idx + 1); // 1-indexed for user display
            let task = spec.task.clone();
            // Clone sender inside the loop - each task gets its own sender
            // The original sender will be dropped after all futures are created
            let result_tx = result_tx.clone();
            let event_tx_clone = event_tx.clone();
            let cancellation_token_clone = cancellation_token.clone();

            // Model selection with fallback and optional smart selection
            let selected_model = self.select_model(&spec.model_id, &task);

            if let Some((profile_id, profile)) = selected_model {
                // Model profile found
                let provider_registry = self.provider_registry.as_ref().ok_or_else(|| {
                    ToolError::ExecutionError("Provider registry not available".to_string())
                })?;

                let provider = provider_registry
                    .get_or_create(&profile.provider)
                    .map_err(|e| {
                        ToolError::ExecutionError(format!("Failed to create provider: {}", e))
                    })?;

                let agent_config = crate::agent::loop_::AgentConfig {
                    model: profile.model.clone(),
                    temperature: profile.temperature.unwrap_or(0.7),
                    thinking_enabled: profile.thinking_enabled.unwrap_or(false),
                    max_tokens: profile.max_tokens.unwrap_or(4096),
                    ..Default::default()
                };

                info!(
                    "[SpawnParallel] Task {}: Using model profile '{}' (model: {})",
                    idx + 1,
                    profile_id,
                    profile.model
                );

                // Create boxed future using Builder pattern with cancellation
                let manager = manager.clone();
                spawn_futures.push(Box::pin(async move {
                    manager
                        .task(subagent_id, task)
                        .with_provider(provider)
                        .with_config(agent_config)
                        .with_streaming(event_tx_clone)
                        .with_cancellation_token(cancellation_token_clone)
                        .spawn(result_tx)
                        .await
                }));
            } else {
                // No model profile matched, use default provider
                info!(
                    "[SpawnParallel] Task {}: Using default provider (no model profile matched)",
                    idx + 1
                );

                let manager = manager.clone();
                spawn_futures.push(Box::pin(async move {
                    manager
                        .task(subagent_id, task)
                        .with_streaming(event_tx_clone)
                        .with_cancellation_token(cancellation_token_clone)
                        .spawn(result_tx)
                        .await
                }));
            }
        }

        // CRITICAL: Drop the original senders now that all futures have been created.
        // Each future owns its own cloned sender. Once all subagents complete,
        // these cloned senders will be dropped, and the channel will close naturally,
        // allowing event_rx.recv() to return None.
        // Without this drop, the channel would never close because the original
        // sender would keep it alive indefinitely.
        drop(result_tx);
        drop(event_tx);

        // Spawn all subagents in parallel - this is the key change!
        info!(
            "Spawning {} subagents in parallel with streaming",
            spawn_futures.len()
        );

        // Take event receiver - we own it now, no Arc<Mutex> needed
        let mut event_rx = tracker.take_event_receiver().map_err(|e| {
            ToolError::ExecutionError(format!("Failed to take event receiver: {}", e))
        })?;

        // Get outbound channel and session key from manager for WebSocket streaming
        let outbound_tx = manager.outbound_sender();
        let session_key = manager.get_session_key();

        // Spawn a background task to collect events and forward to WebSocket/channel
        // Move task_id_map into the task for [Task N] labeling
        // Track whether each subagent is at the start of a new line (for prefix insertion)
        let mut subagent_at_line_start: HashMap<String, bool> = HashMap::new();

        // Buffer events per subagent - key is subagent ID
        let mut subagent_buffers: HashMap<String, BufferedEvents> = HashMap::new();

        tokio::spawn(async move {
            // Direct ownership - no lock needed
            while let Some(event) = event_rx.recv().await {
                // Extract subagent ID for task index lookup
                let subagent_id = match &event {
                    SubagentEvent::Started { id, .. } => id,
                    SubagentEvent::Thinking { id, .. } => id,
                    SubagentEvent::Content { id, .. } => id,
                    SubagentEvent::Iteration { id, .. } => id,
                    SubagentEvent::ToolStart { id, .. } => id,
                    SubagentEvent::ToolEnd { id, .. } => id,
                    SubagentEvent::Completed { id, .. } => id,
                    SubagentEvent::Error { id, .. } => id,
                };

                // Get task index for [Task N] labeling (1-indexed for display)
                let task_label = task_id_map
                    .get(subagent_id)
                    .map(|idx| format!("[Task {}]", idx))
                    .unwrap_or_else(|| "[Subagent]".to_string());

                // Log the event with task label
                match &event {
                    SubagentEvent::Started { id, task } => {
                        info!("{} Started: {} (ID: {})", task_label, task, id);
                        // Initialize line start state for new subagent
                        subagent_at_line_start.insert(id.clone(), true);
                        // Initialize buffer for this subagent
                        subagent_buffers.insert(id.clone(), BufferedEvents::new());
                    }
                    SubagentEvent::Thinking { id, content } => {
                        trace!("{} Thinking: {} (ID: {})", task_label, content, id);
                    }
                    SubagentEvent::Content { id, content } => {
                        trace!(
                            "{} Content: {} bytes (ID: {})",
                            task_label,
                            content.len(),
                            id
                        );
                    }
                    SubagentEvent::Iteration { id, iteration } => {
                        info!(
                            "{} Iteration {} completed (ID: {})",
                            task_label, iteration, id
                        );
                        // After iteration, we're at line start
                        subagent_at_line_start.insert(id.clone(), true);
                    }
                    SubagentEvent::ToolStart { id, tool_name, .. } => {
                        trace!("{} Tool: {} started (ID: {})", task_label, tool_name, id);
                    }
                    SubagentEvent::ToolEnd { id, tool_name, .. } => {
                        trace!("{} Tool: {} done (ID: {})", task_label, tool_name, id);
                        // After tool end, we're at line start
                        subagent_at_line_start.insert(id.clone(), true);
                    }
                    SubagentEvent::Completed { id, result } => {
                        info!(
                            "{} Completed, model={} (ID: {})",
                            task_label,
                            result.model.as_deref().unwrap_or("unknown"),
                            id
                        );
                        // Mark this subagent as completed
                        if let Some(buf) = subagent_buffers.get_mut(id) {
                            buf.completed = true;
                        }
                    }
                    SubagentEvent::Error { id, error } => {
                        warn!("{} Error: {} (ID: {})", task_label, error, id);
                        // Mark this subagent as completed (with error)
                        if let Some(buf) = subagent_buffers.get_mut(id) {
                            buf.completed = true;
                        }
                    }
                }

                // Buffer WebSocket message (don't send immediately)
                if session_key.is_some() {
                    let ws_msg = match &event {
                        SubagentEvent::Thinking { id, content } => {
                            // Only add prefix at line start to avoid repeating for every char
                            let at_start = subagent_at_line_start.get(id).copied().unwrap_or(true);
                            let msg = if at_start || content.starts_with('\n') {
                                format!("{} {}", task_label, content.trim_start())
                            } else {
                                content.clone()
                            };
                            // Update line start state based on content
                            subagent_at_line_start.insert(id.clone(), content.ends_with('\n'));
                            Some(WebSocketMessage::thinking(msg))
                        }
                        SubagentEvent::Content { id, content } => {
                            // Only add prefix at line start
                            let at_start = subagent_at_line_start.get(id).copied().unwrap_or(true);
                            let msg = if at_start || content.starts_with('\n') {
                                format!("{} {}", task_label, content.trim_start())
                            } else {
                                content.clone()
                            };
                            // Update line start state based on content
                            subagent_at_line_start.insert(id.clone(), content.ends_with('\n'));
                            Some(WebSocketMessage::content(msg))
                        }
                        SubagentEvent::Iteration { iteration, .. } => Some(WebSocketMessage::text(
                            format!("{} Iteration {} completed", task_label, iteration),
                        )),
                        SubagentEvent::ToolStart {
                            tool_name,
                            arguments,
                            ..
                        } => Some(WebSocketMessage::tool_start(
                            format!("{} {}", task_label, tool_name),
                            arguments.clone(),
                        )),
                        SubagentEvent::ToolEnd {
                            tool_name, output, ..
                        } => Some(WebSocketMessage::tool_end(
                            format!("{} {}", task_label, tool_name),
                            Some(output.clone()),
                        )),
                        SubagentEvent::Error { error, .. } => Some(WebSocketMessage::text(
                            format!("{} Error: {}", task_label, error),
                        )),
                        _ => None, // Started, Completed - don't send to WS
                    };

                    // Add message to the subagent's buffer
                    if let Some(msg) = ws_msg {
                        if let Some(buf) = subagent_buffers.get_mut(subagent_id) {
                            buf.messages.push(msg);
                        }
                    }
                }

                // Check if this subagent has completed and flush its buffer
                if let Some(ref key) = session_key {
                    if let Some(buf) = subagent_buffers.get_mut(subagent_id) {
                        if buf.completed {
                            // Flush all buffered messages for this subagent in ordered format
                            // Use flush_ordered to ensure Thinking messages come before Content
                            for msg in buf.flush_ordered() {
                                let outbound = OutboundMessage::with_ws_message(
                                    key.channel.clone(),
                                    &key.chat_id,
                                    msg,
                                );
                                // Use timeout + send to apply backpressure without indefinite blocking
                                // This gives the channel time to drain while avoiding blocking forever
                                match timeout(
                                    Duration::from_millis(100),
                                    outbound_tx.send(outbound),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => { /* sent successfully */ }
                                    Ok(Err(e)) => warn!("Outbound channel closed: {}", e),
                                    Err(_) => warn!(
                                        "Send timeout after 100ms, outbound channel congested"
                                    ),
                                }
                            }
                            buf.completed = false;
                        }
                    }
                }
            }
        });

        // Wait for spawn results
        let spawn_results = join_all(spawn_futures).await;

        info!(
            "All {} subagent spawn requests submitted (results pending)",
            spawn_results.len()
        );

        // Check for spawn failures
        let mut spawn_failures = 0;
        for (idx, result) in spawn_results.into_iter().enumerate() {
            if let Err(e) = result {
                warn!("Task {} failed to spawn: {}", idx + 1, e);
                spawn_failures += 1;
            }
        }

        if spawn_failures > 0 {
            warn!(
                "{} subagent(s) failed to spawn, expecting {} results",
                spawn_failures,
                task_count - spawn_failures
            );
        }

        // Wait for all results
        info!("Waiting for {} subagent results...", task_count);
        let results = tracker.wait_for_all(task_count).await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to wait for subagent results: {}", e))
        })?;

        if results.len() < task_count {
            warn!(
                "Only received {}/{} subagent results. Missing results may be due to: \
                 1) Subagent task crashed before sending result, \
                 2) Channel closed unexpectedly, \
                 3) Timeout waiting for results",
                results.len(),
                task_count
            );
        } else {
            info!("All {} subagents completed successfully", results.len());
        }

        // Aggregate results
        let mut output = format!("Completed {} parallel tasks:\n\n", task_count);
        for (idx, result) in results.iter().enumerate() {
            // Include thinking content if available
            if let Some(ref reasoning) = result.response.reasoning_content {
                if !reasoning.is_empty() {
                    output.push_str(&format!("**Thinking:**\n{}\n\n", reasoning));
                }
            }

            output.push_str(&format!(
                "## Task {} (ID: {})\n**Model:** {}\n**Prompt:** {}\n**Response:**\n{}\n\n",
                idx + 1,
                &result.id,
                result.model.as_deref().unwrap_or("unknown"),
                &result.task,
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

        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_too_many_tasks_validation() {
        let tool = SpawnParallelTool::new();
        let tasks: Vec<String> = (0..15).map(|i| format!("Task {}", i)).collect();
        let args = serde_json::json!({
            "tasks": tasks
        });

        let result = tool.execute(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_no_manager_error() {
        let tool = SpawnParallelTool::new();
        let args = serde_json::json!({
            "tasks": ["Task 1"]
        });

        let result = tool.execute(args).await;
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
