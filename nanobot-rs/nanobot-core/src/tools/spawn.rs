//! Spawn tool for background task execution (subagent)

use async_trait::async_trait;
use serde_json::Value;

use super::base::{simple_schema, Tool, ToolError};

/// Spawn tool for running background tasks
pub struct SpawnTool {
    channel: Option<String>,
    chat_id: Option<String>,
}

impl SpawnTool {
    /// Create a new spawn tool
    pub fn new() -> Self {
        Self {
            channel: None,
            chat_id: None,
        }
    }

    /// Set the current channel and chat_id context
    pub fn set_context(&mut self, channel: String, chat_id: String) {
        self.channel = Some(channel);
        self.chat_id = Some(chat_id);
    }
}

impl Default for SpawnTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a background task to perform a long-running operation. The task will run independently and report results when complete."
    }

    fn parameters(&self) -> Value {
        simple_schema(&[("task", "string", true), ("timeout", "number", false)])
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let task = args["task"].as_str().unwrap_or_default();
        let timeout = args["timeout"].as_u64().unwrap_or(300);

        if task.is_empty() {
            return Err(ToolError::InvalidArguments(
                "Task description is required".to_string(),
            ));
        }

        // In a real implementation, this would spawn a background task
        // that runs the agent loop with the given task
        tracing::info!("Spawning background task: {} (timeout: {}s)", task, timeout);

        // For now, return a placeholder response
        Ok(format!(
            "Background task started. I will notify you when complete.\nTask: {}",
            task
        ))
    }
}

/// Spawn request for background tasks
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub task: String,
    pub timeout: Option<u64>,
    pub channel: String,
    pub chat_id: String,
}

/// Background task manager
pub struct TaskManager {
    tasks: Vec<SpawnRequest>,
}

impl TaskManager {
    /// Create a new task manager
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Add a task
    pub fn add_task(&mut self, request: SpawnRequest) -> String {
        let task_id = uuid::Uuid::new_v4().to_string();
        self.tasks.push(request);
        task_id
    }

    /// Get active tasks
    pub fn active_tasks(&self) -> &[SpawnRequest] {
        &self.tasks
    }

    /// Remove a task
    pub fn remove_task(&mut self, _task_id: &str) -> bool {
        // In production, would track by ID
        true
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
