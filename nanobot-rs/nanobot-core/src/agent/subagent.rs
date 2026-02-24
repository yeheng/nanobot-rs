//! Subagent manager for background task execution
//!
//! Provides functionality to spawn and manage background subagent tasks
//! that can execute long-running operations independently.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::providers::LlmProvider;
use crate::tools::ToolRegistry;

use super::context::ContextBuilder;
use super::loop_::{AgentConfig, AgentLoop};
use super::task_store_sqlite::SqliteTaskStore;

/// Status of a subagent task
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task is pending execution
    Pending,
    /// Task is currently running
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed with an error
    Failed,
    /// Task was cancelled
    Cancelled,
    /// Task timed out
    Timeout,
}

/// Priority level for tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

/// A subagent task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTask {
    /// Unique task ID
    pub id: String,

    /// Task description/prompt
    pub prompt: String,

    /// Channel where the task was spawned
    pub channel: String,

    /// Chat ID where the task was spawned
    pub chat_id: String,

    /// Session key for context
    pub session_key: String,

    /// Task status
    pub status: TaskStatus,

    /// Task priority
    pub priority: TaskPriority,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Start timestamp
    pub started_at: Option<DateTime<Utc>>,

    /// Completion timestamp
    pub completed_at: Option<DateTime<Utc>>,

    /// Task result (if completed)
    pub result: Option<String>,

    /// Error message (if failed)
    pub error: Option<String>,

    /// Timeout in seconds
    pub timeout_secs: u64,

    /// Progress percentage (0-100)
    pub progress: u8,

    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl SubagentTask {
    /// Create a new subagent task
    pub fn new(
        prompt: impl Into<String>,
        channel: impl Into<String>,
        chat_id: impl Into<String>,
        session_key: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            prompt: prompt.into(),
            channel: channel.into(),
            chat_id: chat_id.into(),
            session_key: session_key.into(),
            status: TaskStatus::Pending,
            priority: TaskPriority::Normal,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
            timeout_secs: 300, // 5 minutes default
            progress: 0,
            metadata: HashMap::new(),
        }
    }

    /// Set the task priority
    pub fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Set the timeout
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Check if the task is finished
    pub fn is_finished(&self) -> bool {
        matches!(
            self.status,
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::Cancelled
                | TaskStatus::Timeout
        )
    }

    /// Get task duration (if started)
    pub fn duration(&self) -> Option<Duration> {
        self.started_at.map(|start| {
            let end = self.completed_at.unwrap_or_else(Utc::now);
            (end - start).to_std().unwrap_or(Duration::ZERO)
        })
    }
}

/// Notification for task completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNotification {
    /// Task ID
    pub task_id: String,

    /// Task status
    pub status: TaskStatus,

    /// Result (if completed)
    pub result: Option<String>,

    /// Error (if failed)
    pub error: Option<String>,

    /// Channel to notify
    pub channel: String,

    /// Chat ID to notify
    pub chat_id: String,
}

/// Configuration for the subagent manager
#[derive(Debug, Clone)]
pub struct SubagentConfig {
    /// Maximum concurrent tasks
    pub max_concurrent: usize,

    /// Default task timeout in seconds
    pub default_timeout: u64,

    /// Task queue size
    pub queue_size: usize,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 5,
            default_timeout: 300,
            queue_size: 100,
        }
    }
}

/// Subagent manager for handling background tasks.
///
/// Each spawned task creates an independent `AgentLoop` that shares the same
/// LLM provider and cached context builder but operates in its own session.
///
/// Task state is persisted via SQLite.
pub struct SubagentManager {
    /// Active tasks
    tasks: Arc<RwLock<HashMap<String, SubagentTask>>>,

    /// Task handles
    handles: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,

    /// LLM provider (shared across subagents)
    provider: Arc<dyn LlmProvider>,

    /// Workspace path
    workspace: PathBuf,

    /// Configuration
    config: SubagentConfig,

    /// Factory that produces a fresh ToolRegistry for each subagent
    tool_factory: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,

    /// Persistence backend (SQLite)
    store: Arc<SqliteTaskStore>,

    /// Cached context builder (shared across subagents to avoid repeated I/O)
    context: ContextBuilder,
}

impl SubagentManager {
    /// Create a new subagent manager
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        workspace: PathBuf,
        config: SubagentConfig,
        tool_factory: Arc<dyn Fn() -> ToolRegistry + Send + Sync>,
    ) -> Self {
        let data_dir = workspace.join("data");
        let json_path = data_dir.join("tasks.json");
        let db_path = data_dir.join("tasks.db");

        // SQLite persistence backend
        let store = match SqliteTaskStore::new(db_path) {
            Ok(s) => {
                // One-time migration from legacy JSON
                if json_path.exists() {
                    if let Err(e) = s.migrate_from_json(&json_path) {
                        warn!("JSON→SQLite migration failed: {}", e);
                    }
                }
                Arc::new(s)
            }
            Err(e) => {
                panic!("Failed to open SqliteTaskStore: {}", e);
            }
        };

        // Load existing tasks from the store (sync at init is acceptable)
        let tasks = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(store.load_all())
        })
        .unwrap_or_else(|e| {
            warn!("Failed to load tasks: {}, starting fresh", e);
            HashMap::new()
        });

        let tasks = Arc::new(RwLock::new(tasks));

        // Build context once at startup (synchronous I/O is acceptable here)
        let context = match ContextBuilder::new(workspace.clone()) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to build context: {}, using default", e);
                ContextBuilder::new(PathBuf::from(".")).unwrap_or_else(|_| {
                    // Absolute fallback with empty prompt
                    ContextBuilder::new(PathBuf::from("/nonexistent")).unwrap()
                })
            }
        };

        Self {
            tasks,
            handles: Arc::new(RwLock::new(HashMap::new())),
            provider,
            workspace,
            config,
            tool_factory,
            store,
            context,
        }
    }

    /// Persist a single task to SQLite.
    async fn persist_task(store: &SqliteTaskStore, task: &SubagentTask) {
        if let Err(e) = store.save_task(task).await {
            warn!("Failed to persist task {}: {}", task.id, e);
        }
    }

    /// Force-flush all tasks to disk (for shutdown or batch operations).
    pub async fn flush(&self) {
        // SQLite saves tasks individually, so this is a no-op.
        // Kept for API compatibility.
    }

    /// Recover tasks that were interrupted (mark running tasks as failed)
    #[instrument(name = "subagent.recover", skip_all)]
    pub async fn recover_interrupted_tasks(&self) {
        let mut tasks = self.tasks.write().await;
        let mut recovered = Vec::new();

        for task in tasks.values_mut() {
            if task.status == TaskStatus::Running || task.status == TaskStatus::Pending {
                task.status = TaskStatus::Failed;
                task.error = Some("Task interrupted by process restart".to_string());
                task.completed_at = Some(Utc::now());
                recovered.push(task.clone());
            }
        }

        drop(tasks); // Release lock before I/O

        if !recovered.is_empty() {
            info!("Recovered {} interrupted tasks", recovered.len());
            for task in &recovered {
                Self::persist_task(&self.store, task).await;
            }
        }
    }

    /// Submit a new task for background execution
    #[instrument(name = "subagent.submit", skip_all, fields(task_id = %task.id))]
    pub async fn submit(&self, task: SubagentTask) -> anyhow::Result<String> {
        // Check concurrency limit and insert as pending
        {
            let mut tasks = self.tasks.write().await;
            let running = tasks
                .values()
                .filter(|t| t.status == TaskStatus::Running)
                .count();
            if running >= self.config.max_concurrent {
                anyhow::bail!(
                    "Maximum concurrent tasks ({}) reached. Wait for tasks to complete.",
                    self.config.max_concurrent
                );
            }
            tasks.insert(task.id.clone(), task.clone());
        }

        let task_id = task.id.clone();
        let prompt = task.prompt.clone();
        let timeout = Duration::from_secs(task.timeout_secs);

        // Persist the newly inserted task
        Self::persist_task(&self.store, &task).await;

        // Spawn the actual execution
        let tasks_ref = self.tasks.clone();
        let store = self.store.clone();
        let provider = self.provider.clone();
        let workspace = self.workspace.clone();
        let tid = task_id.clone();
        let tool_factory = self.tool_factory.clone();
        // Clone the cached context (Arc<String> internally, so this is cheap)
        let context = self.context.clone();

        let handle = tokio::spawn(async move {
            // Mark as running
            let task_snapshot = {
                let mut tasks = tasks_ref.write().await;
                if let Some(t) = tasks.get_mut(&tid) {
                    t.status = TaskStatus::Running;
                    t.started_at = Some(Utc::now());
                }
                tasks.get(&tid).cloned()
            };
            if let Some(snap) = &task_snapshot {
                Self::persist_task(&store, snap).await;
            }

            info!(
                "Subagent task {} started: {}",
                tid,
                &prompt[..prompt.len().min(80)]
            );

            // Create a lightweight agent loop for this task using cached context
            // This avoids synchronous file I/O in async context
            let agent_config = AgentConfig {
                model: provider.default_model().to_string(),
                max_iterations: 10,
                ..Default::default()
            };
            let tools = tool_factory();
            let agent = match AgentLoop::with_cached_context(
                provider,
                workspace,
                agent_config,
                tools,
                context,
            ) {
                Ok(a) => a,
                Err(e) => {
                    let snap = {
                        let mut tasks = tasks_ref.write().await;
                        if let Some(t) = tasks.get_mut(&tid) {
                            t.status = TaskStatus::Failed;
                            t.error = Some(format!("Failed to initialise subagent: {}", e));
                        }
                        tasks.get(&tid).cloned()
                    };
                    if let Some(snap) = &snap {
                        Self::persist_task(&store, snap).await;
                    }
                    return;
                }
            };

            // Execute with timeout
            let session_key = format!("subagent:{}", tid);
            let result =
                tokio::time::timeout(timeout, agent.process_direct(&prompt, &session_key)).await;

            // Update task state
            let snap = {
                let mut tasks = tasks_ref.write().await;
                if let Some(t) = tasks.get_mut(&tid) {
                    match result {
                        Ok(Ok(response)) => {
                            t.status = TaskStatus::Completed;
                            t.result = Some(response.content);
                            t.completed_at = Some(Utc::now());
                            t.progress = 100;
                            info!("Subagent task {} completed", tid);
                        }
                        Ok(Err(e)) => {
                            t.status = TaskStatus::Failed;
                            t.error = Some(e.to_string());
                            t.completed_at = Some(Utc::now());
                            warn!("Subagent task {} failed: {}", tid, e);
                        }
                        Err(_) => {
                            t.status = TaskStatus::Timeout;
                            t.error = Some("Task timed out".to_string());
                            t.completed_at = Some(Utc::now());
                            warn!("Subagent task {} timed out", tid);
                        }
                    }
                }
                tasks.get(&tid).cloned()
            };
            if let Some(snap) = &snap {
                Self::persist_task(&store, snap).await;
            }
        });

        self.handles.write().await.insert(task_id.clone(), handle);

        info!("Submitted subagent task: {}", task_id);
        Ok(task_id)
    }

    /// Get a task by ID
    pub async fn get_task(&self, task_id: &str) -> Option<SubagentTask> {
        self.tasks.read().await.get(task_id).cloned()
    }

    /// Get all tasks
    pub async fn get_all_tasks(&self) -> Vec<SubagentTask> {
        self.tasks.read().await.values().cloned().collect()
    }

    /// Get tasks by status
    pub async fn get_tasks_by_status(&self, status: TaskStatus) -> Vec<SubagentTask> {
        self.tasks
            .read()
            .await
            .values()
            .filter(|t| t.status == status)
            .cloned()
            .collect()
    }

    /// Cancel a task
    #[instrument(name = "subagent.cancel", skip(self), fields(task_id = %task_id))]
    pub async fn cancel(&self, task_id: &str) -> bool {
        if let Some(handle) = self.handles.write().await.remove(task_id) {
            handle.abort();
        }
        let snap = {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.get_mut(task_id) {
                if !task.is_finished() {
                    task.status = TaskStatus::Cancelled;
                    task.completed_at = Some(Utc::now());
                    info!("Cancelled task: {}", task_id);
                    Some(task.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(snap) = &snap {
            Self::persist_task(&self.store, snap).await;
            return true;
        }
        false
    }

    /// Clean up finished tasks older than the specified duration
    #[instrument(name = "subagent.cleanup", skip_all)]
    pub async fn cleanup_old_tasks(&self, older_than: Duration) -> usize {
        let mut tasks = self.tasks.write().await;
        let cutoff =
            Utc::now() - chrono::Duration::from_std(older_than).unwrap_or(chrono::Duration::zero());

        let mut removed_ids = Vec::new();
        tasks.retain(|id, t| {
            if t.is_finished() {
                if let Some(completed) = t.completed_at {
                    if completed <= cutoff {
                        removed_ids.push(id.clone());
                        return false;
                    }
                }
            }
            true
        });

        let removed = removed_ids.len();
        if removed > 0 {
            drop(tasks);
            if let Err(e) = self.store.remove_tasks(&removed_ids).await {
                warn!("Failed to remove tasks from store: {}", e);
            }
            debug!("Cleaned up {} old tasks", removed);
        }
        removed
    }

    /// Get statistics
    pub async fn stats(&self) -> SubagentStats {
        let tasks = self.tasks.read().await;
        let mut stats = SubagentStats::default();

        for task in tasks.values() {
            stats.total += 1;
            match task.status {
                TaskStatus::Pending => stats.pending += 1,
                TaskStatus::Running => stats.running += 1,
                TaskStatus::Completed => stats.completed += 1,
                TaskStatus::Failed => stats.failed += 1,
                TaskStatus::Cancelled => stats.cancelled += 1,
                TaskStatus::Timeout => stats.timeout += 1,
            }
        }

        stats
    }
}

/// Statistics for the subagent manager
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubagentStats {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub timeout: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subagent_task_creation() {
        let task = SubagentTask::new(
            "Test task prompt",
            "telegram",
            "chat123",
            "session:telegram:chat123",
        );

        assert!(task.id.starts_with(|c: char| c.is_ascii_alphanumeric()));
        assert_eq!(task.prompt, "Test task prompt");
        assert_eq!(task.channel, "telegram");
        assert_eq!(task.chat_id, "chat123");
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.started_at.is_none());
        assert!(task.completed_at.is_none());
    }

    #[test]
    fn test_task_priority() {
        let task =
            SubagentTask::new("test", "test", "test", "test").with_priority(TaskPriority::High);
        assert_eq!(task.priority, TaskPriority::High);
    }

    #[test]
    fn test_task_timeout() {
        let task = SubagentTask::new("test", "test", "test", "test").with_timeout(600);
        assert_eq!(task.timeout_secs, 600);
    }

    #[test]
    fn test_task_metadata() {
        let task = SubagentTask::new("test", "test", "test", "test")
            .with_metadata("key1", "value1")
            .with_metadata("key2", "value2");

        assert_eq!(task.metadata.get("key1"), Some(&"value1".to_string()));
        assert_eq!(task.metadata.get("key2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_task_is_finished() {
        let mut task = SubagentTask::new("test", "test", "test", "test");

        assert!(!task.is_finished());

        task.status = TaskStatus::Running;
        assert!(!task.is_finished());

        task.status = TaskStatus::Completed;
        assert!(task.is_finished());

        task.status = TaskStatus::Failed;
        assert!(task.is_finished());

        task.status = TaskStatus::Cancelled;
        assert!(task.is_finished());
    }

    #[test]
    fn test_task_notification_serialization() {
        let notification = TaskNotification {
            task_id: "task123".to_string(),
            status: TaskStatus::Completed,
            result: Some("Task completed successfully".to_string()),
            error: None,
            channel: "telegram".to_string(),
            chat_id: "chat123".to_string(),
        };

        let json = serde_json::to_string(&notification).unwrap();
        assert!(json.contains("task123"));
        assert!(json.contains("Completed"));
    }

    #[test]
    fn test_subagent_stats_default() {
        let stats = SubagentStats::default();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.running, 0);
    }
}
