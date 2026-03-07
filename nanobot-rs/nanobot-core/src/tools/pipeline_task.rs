//! Pipeline task board tool.
//!
//! Allows agents to interact with the shared task board:
//! create, get, list, transition, and query flow logs.

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::instrument;

use super::base::{Tool, ToolError, ToolResult};
use crate::pipeline::orchestrator::PipelineEvent;
use crate::pipeline::state_machine::TaskState;
use crate::pipeline::store::PipelineStore;
use crate::pipeline::models::{PipelineTask, TaskPriority};

pub struct PipelineTaskTool {
    store: PipelineStore,
    event_tx: mpsc::Sender<PipelineEvent>,
}

impl PipelineTaskTool {
    pub fn new(store: PipelineStore, event_tx: mpsc::Sender<PipelineEvent>) -> Self {
        Self { store, event_tx }
    }
}

#[derive(Deserialize)]
struct TaskArgs {
    action: String,
    // create fields
    title: Option<String>,
    description: Option<String>,
    priority: Option<String>,
    origin_channel: Option<String>,
    origin_chat_id: Option<String>,
    // get / transition / flow_log fields
    task_id: Option<String>,
    // list fields
    state: Option<String>,
    role: Option<String>,
    // transition fields
    to_state: Option<String>,
    agent_role: Option<String>,
    reason: Option<String>,
}

#[async_trait]
impl Tool for PipelineTaskTool {
    fn name(&self) -> &str {
        "pipeline_task"
    }

    fn description(&self) -> &str {
        "Interact with the pipeline task board: create, get, list, transition state, or query flow logs."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "get", "list", "transition", "flow_log"],
                    "description": "The action to perform"
                },
                "title": {
                    "type": "string",
                    "description": "Task title (for create)"
                },
                "description": {
                    "type": "string",
                    "description": "Task description (for create)"
                },
                "priority": {
                    "type": "string",
                    "enum": ["low", "normal", "high", "critical"],
                    "description": "Task priority (for create, default: normal)"
                },
                "origin_channel": {
                    "type": "string",
                    "description": "Origin channel for routing results back (for create)"
                },
                "origin_chat_id": {
                    "type": "string",
                    "description": "Origin chat ID (for create)"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID (for get, transition, flow_log)"
                },
                "state": {
                    "type": "string",
                    "description": "Filter by state (for list)"
                },
                "role": {
                    "type": "string",
                    "description": "Filter by assigned role (for list)"
                },
                "to_state": {
                    "type": "string",
                    "description": "Target state (for transition)"
                },
                "agent_role": {
                    "type": "string",
                    "description": "Role performing the transition (for transition)"
                },
                "reason": {
                    "type": "string",
                    "description": "Reason for the transition (for transition)"
                }
            },
            "required": ["action"]
        })
    }

    #[instrument(name = "tool.pipeline_task", skip_all)]
    async fn execute(&self, args: Value) -> ToolResult {
        let args: TaskArgs = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        match args.action.as_str() {
            "create" => self.do_create(args).await,
            "get" => self.do_get(args).await,
            "list" => self.do_list(args).await,
            "transition" => self.do_transition(args).await,
            "flow_log" => self.do_flow_log(args).await,
            other => Err(ToolError::InvalidArguments(format!(
                "Unknown action: {other}"
            ))),
        }
    }
}

impl PipelineTaskTool {
    async fn do_create(&self, args: TaskArgs) -> ToolResult {
        let title = args
            .title
            .ok_or_else(|| ToolError::InvalidArguments("title is required".into()))?;
        let now = Utc::now();
        let id = uuid::Uuid::new_v4().to_string();

        let task = PipelineTask {
            id: id.clone(),
            title,
            description: args.description.unwrap_or_default(),
            state: TaskState::Pending,
            priority: args
                .priority
                .as_deref()
                .map(TaskPriority::from_str_lossy)
                .unwrap_or_default(),
            assigned_role: None,
            review_count: 0,
            retry_count: 0,
            last_heartbeat: now,
            created_at: now,
            updated_at: now,
            result: None,
            origin_channel: args.origin_channel,
            origin_chat_id: args.origin_chat_id,
        };

        self.store
            .create_task(&task)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        // Notify the orchestrator
        let _ = self.event_tx.send(PipelineEvent::TaskCreated { task_id: id.clone() }).await;

        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "created",
            "task_id": id,
        }))
        .unwrap())
    }

    async fn do_get(&self, args: TaskArgs) -> ToolResult {
        let id = args
            .task_id
            .ok_or_else(|| ToolError::InvalidArguments("task_id is required".into()))?;

        let task = self
            .store
            .get_task(&id)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?
            .ok_or_else(|| ToolError::NotFound(format!("Task {id} not found")))?;

        Ok(serde_json::to_string_pretty(&task).unwrap())
    }

    async fn do_list(&self, args: TaskArgs) -> ToolResult {
        let tasks = if let Some(state_str) = &args.state {
            let state = TaskState::from_str_lossy(state_str).ok_or_else(|| {
                ToolError::InvalidArguments(format!("Unknown state: {state_str}"))
            })?;
            self.store
                .list_tasks_by_state(state)
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?
        } else if let Some(role) = &args.role {
            self.store
                .list_tasks_by_role(role)
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?
        } else {
            // Default: list all pending
            self.store
                .list_tasks_by_state(TaskState::Pending)
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?
        };

        Ok(serde_json::to_string_pretty(&tasks).unwrap())
    }

    async fn do_transition(&self, args: TaskArgs) -> ToolResult {
        let id = args
            .task_id
            .ok_or_else(|| ToolError::InvalidArguments("task_id is required".into()))?;
        let to_str = args
            .to_state
            .ok_or_else(|| ToolError::InvalidArguments("to_state is required".into()))?;
        let agent_role = args
            .agent_role
            .ok_or_else(|| ToolError::InvalidArguments("agent_role is required".into()))?;

        let to_state = TaskState::from_str_lossy(&to_str)
            .ok_or_else(|| ToolError::InvalidArguments(format!("Unknown state: {to_str}")))?;

        // Fetch current task to validate transition
        let task = self
            .store
            .get_task(&id)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?
            .ok_or_else(|| ToolError::NotFound(format!("Task {id} not found")))?;

        if !task.state.can_transition_to(to_state) {
            return Err(ToolError::ExecutionError(format!(
                "Invalid transition: {} → {}",
                task.state, to_state
            )));
        }

        let ok = self
            .store
            .update_task_state(&id, task.state, to_state, Some(&agent_role))
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        if !ok {
            return Err(ToolError::ExecutionError(
                "Concurrent modification: task state changed".into(),
            ));
        }

        // Write audit log
        self.store
            .append_flow_log(
                &id,
                &task.state.to_string(),
                &to_state.to_string(),
                &agent_role,
                args.reason.as_deref(),
            )
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        // Notify orchestrator
        let _ = self
            .event_tx
            .send(PipelineEvent::TaskTransitioned {
                task_id: id.clone(),
                new_state: to_state,
                agent_role: agent_role.clone(),
            })
            .await;

        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "status": "transitioned",
            "task_id": id,
            "from": task.state.to_string(),
            "to": to_state.to_string(),
        }))
        .unwrap())
    }

    async fn do_flow_log(&self, args: TaskArgs) -> ToolResult {
        let id = args
            .task_id
            .ok_or_else(|| ToolError::InvalidArguments("task_id is required".into()))?;

        let log = self
            .store
            .get_flow_log(&id)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        Ok(serde_json::to_string_pretty(&log).unwrap())
    }
}
