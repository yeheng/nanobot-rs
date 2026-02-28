//! Task execution logic for subagents.
//!
//! Provides the core execution logic for running subagent tasks.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::agent::task_store::SqliteTaskStore;
use crate::providers::LlmProvider;
use crate::tools::ToolRegistry;

use super::context::ContextBuilder;
use super::loop_::{AgentConfig, AgentLoop};
use super::subagent::{SubagentTask, TaskStatus};

/// Type alias for the tasks map shared state.
pub type TasksMap = Arc<RwLock<std::collections::HashMap<String, SubagentTask>>>;

/// Result of task execution.
#[derive(Debug)]
pub enum TaskResult {
    /// Task completed successfully
    Completed { result: String },
    /// Task failed with error
    Failed { error: String },
    /// Task timed out
    Timeout,
}

/// Execute a subagent task.
///
/// This function creates a lightweight agent loop and executes the task
/// with the specified timeout. It does NOT manage task state - the caller
/// is responsible for updating the task status before and after execution.
pub async fn execute_task(
    task: &SubagentTask,
    provider: Arc<dyn LlmProvider>,
    workspace: PathBuf,
    context: ContextBuilder,
    tool_factory: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
    timeout: Duration,
) -> TaskResult {
    let prompt = task.prompt.clone();
    let tid = task.id.clone();

    // Create a lightweight agent config for subagents
    let agent_config = AgentConfig {
        model: provider.default_model().to_string(),
        max_iterations: 10,
        ..Default::default()
    };

    let tools = tool_factory();

    // Create the agent loop
    let agent = match AgentLoop::with_cached_context(
        provider,
        workspace,
        agent_config,
        tools,
        context,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            return TaskResult::Failed {
                error: format!("Failed to initialise subagent: {}", e),
            };
        }
    };

    // Execute with timeout
    let session_key = format!("subagent:{}", tid);
    let result =
        tokio::time::timeout(timeout, agent.process_direct(&prompt, &session_key)).await;

    match result {
        Ok(Ok(response)) => {
            info!("Subagent task {} completed", tid);
            TaskResult::Completed {
                result: response.content,
            }
        }
        Ok(Err(e)) => {
            warn!("Subagent task {} failed: {}", tid, e);
            TaskResult::Failed {
                error: e.to_string(),
            }
        }
        Err(_) => {
            warn!("Subagent task {} timed out", tid);
            TaskResult::Timeout
        }
    }
}

/// Update task state based on execution result.
///
/// Returns the updated task snapshot for persistence.
pub fn update_task_from_result(
    task: &mut SubagentTask,
    result: TaskResult,
) {
    let now = Utc::now();

    match result {
        TaskResult::Completed { result: content } => {
            task.status = TaskStatus::Completed;
            task.result = Some(content);
            task.completed_at = Some(now);
            task.progress = 100;
        }
        TaskResult::Failed { error } => {
            task.status = TaskStatus::Failed;
            task.error = Some(error);
            task.completed_at = Some(now);
        }
        TaskResult::Timeout => {
            task.status = TaskStatus::Timeout;
            task.error = Some("Task timed out".to_string());
            task.completed_at = Some(now);
        }
    }
}

/// Persist a task to the SQLite store.
pub async fn persist_task(store: &SqliteTaskStore, task: &SubagentTask) {
    if let Err(e) = store.save_task(task).await {
        warn!("Failed to persist task {}: {}", task.id, e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_result_debug() {
        let result = TaskResult::Completed {
            result: "test".to_string(),
        };
        assert!(format!("{:?}", result).contains("Completed"));
    }

    #[test]
    fn test_update_task_completed() {
        let mut task = SubagentTask::new("test", "test", "test", "test");
        task.status = TaskStatus::Running;

        update_task_from_result(
            &mut task,
            TaskResult::Completed {
                result: "done".to_string(),
            },
        );

        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.result, Some("done".to_string()));
        assert_eq!(task.progress, 100);
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn test_update_task_failed() {
        let mut task = SubagentTask::new("test", "test", "test", "test");
        task.status = TaskStatus::Running;

        update_task_from_result(
            &mut task,
            TaskResult::Failed {
                error: "something went wrong".to_string(),
            },
        );

        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.error, Some("something went wrong".to_string()));
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn test_update_task_timeout() {
        let mut task = SubagentTask::new("test", "test", "test", "test");
        task.status = TaskStatus::Running;

        update_task_from_result(&mut task, TaskResult::Timeout);

        assert_eq!(task.status, TaskStatus::Timeout);
        assert_eq!(task.error, Some("Task timed out".to_string()));
        assert!(task.completed_at.is_some());
    }
}
