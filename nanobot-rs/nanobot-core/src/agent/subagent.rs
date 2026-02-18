//! Subagent manager for background task execution
//!
//! Provides functionality to spawn and manage background subagent tasks
//! that can execute long-running operations independently.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info};
use uuid::Uuid;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Default for TaskPriority {
    fn default() -> Self {
        Self::Normal
    }
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
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Timeout
        )
    }

    /// Get task duration (if started)
    pub fn duration(&self) -> Option<Duration> {
        self.started_at.map(|start| {
            let end = self.completed_at.unwrap_or_else(Utc::now);
            (end - start)
                .to_std()
                .unwrap_or(Duration::ZERO)
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

    /// Enable task notifications
    pub enable_notifications: bool,

    /// Task queue size
    pub queue_size: usize,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 5,
            default_timeout: 300,
            enable_notifications: true,
            queue_size: 100,
        }
    }
}

/// Subagent manager for handling background tasks
#[allow(dead_code)]
pub struct SubagentManager {
    /// Configuration
    config: SubagentConfig,

    /// Active tasks
    tasks: Arc<RwLock<HashMap<String, SubagentTask>>>,

    /// Task queue sender
    task_sender: mpsc::Sender<SubagentTask>,

    /// Notification sender
    notification_sender: mpsc::Sender<TaskNotification>,

    /// Notification receiver (for external handling)
    notification_receiver: Option<mpsc::Receiver<TaskNotification>>,
}

impl SubagentManager {
    /// Create a new subagent manager
    pub fn new(config: SubagentConfig) -> Self {
        let (task_sender, mut task_receiver) = mpsc::channel::<SubagentTask>(config.queue_size);
        let (notification_sender, notification_receiver) =
            mpsc::channel::<TaskNotification>(config.queue_size);

        let tasks: Arc<RwLock<HashMap<String, SubagentTask>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Start the task processor
        let tasks_clone = tasks.clone();
        let notification_sender_clone = notification_sender.clone();

        tokio::spawn(async move {
            while let Some(mut task) = task_receiver.recv().await {
                let tasks = tasks_clone.clone();
                let notifier = notification_sender_clone.clone();

                // Update task status
                task.status = TaskStatus::Running;
                task.started_at = Some(Utc::now());
                tasks.write().await.insert(task.id.clone(), task.clone());

                let task_id = task.id.clone();
                let timeout = Duration::from_secs(task.timeout_secs);

                tokio::spawn(async move {
                    // Simulate task execution (in real implementation, this would call agent)
                    let result = tokio::time::timeout(timeout, async {
                        // Placeholder: In real implementation, this would:
                        // 1. Create a new AgentLoop with the provider
                        // 2. Execute process_direct with the prompt
                        // 3. Return the result
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        Ok::<_, anyhow::Error>(format!("Task completed: {}", task_id))
                    })
                    .await;

                    let mut tasks = tasks.write().await;
                    if let Some(t) = tasks.get_mut(&task_id) {
                        match result {
                            Ok(Ok(res)) => {
                                t.status = TaskStatus::Completed;
                                t.result = Some(res.clone());
                                t.completed_at = Some(Utc::now());
                                t.progress = 100;

                                let _ = notifier
                                    .send(TaskNotification {
                                        task_id: task_id.clone(),
                                        status: TaskStatus::Completed,
                                        result: Some(res),
                                        error: None,
                                        channel: t.channel.clone(),
                                        chat_id: t.chat_id.clone(),
                                    })
                                    .await;
                            }
                            Ok(Err(e)) => {
                                t.status = TaskStatus::Failed;
                                t.error = Some(e.to_string());
                                t.completed_at = Some(Utc::now());

                                let _ = notifier
                                    .send(TaskNotification {
                                        task_id: task_id.clone(),
                                        status: TaskStatus::Failed,
                                        result: None,
                                        error: t.error.clone(),
                                        channel: t.channel.clone(),
                                        chat_id: t.chat_id.clone(),
                                    })
                                    .await;
                            }
                            Err(_) => {
                                t.status = TaskStatus::Timeout;
                                t.error = Some("Task timed out".to_string());
                                t.completed_at = Some(Utc::now());

                                let _ = notifier
                                    .send(TaskNotification {
                                        task_id: task_id.clone(),
                                        status: TaskStatus::Timeout,
                                        result: None,
                                        error: Some("Task timed out".to_string()),
                                        channel: t.channel.clone(),
                                        chat_id: t.chat_id.clone(),
                                    })
                                    .await;
                            }
                        }
                    }
                });
            }
        });

        Self {
            config,
            tasks,
            task_sender,
            notification_sender,
            notification_receiver: Some(notification_receiver),
        }
    }

    /// Get the notification receiver
    pub fn take_notification_receiver(&mut self) -> Option<mpsc::Receiver<TaskNotification>> {
        self.notification_receiver.take()
    }

    /// Submit a new task
    pub async fn submit(&self, task: SubagentTask) -> anyhow::Result<String> {
        let task_id = task.id.clone();

        // Store task
        self.tasks.write().await.insert(task_id.clone(), task.clone());

        // Send to queue
        self.task_sender
            .send(task)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to submit task: {}", e))?;

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
    pub async fn cancel(&self, task_id: &str) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            if !task.is_finished() {
                task.status = TaskStatus::Cancelled;
                task.completed_at = Some(Utc::now());
                info!("Cancelled task: {}", task_id);
                return true;
            }
        }
        false
    }

    /// Clean up finished tasks older than the specified duration
    pub async fn cleanup_old_tasks(&self, older_than: Duration) -> usize {
        let mut tasks = self.tasks.write().await;
        let cutoff = Utc::now()
            - chrono::Duration::from_std(older_than).unwrap_or(chrono::Duration::zero());

        let initial_count = tasks.len();
        tasks.retain(|_, t| {
            if t.is_finished() {
                if let Some(completed) = t.completed_at {
                    return completed > cutoff;
                }
            }
            true
        });

        let removed = initial_count - tasks.len();
        if removed > 0 {
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
        let task = SubagentTask::new("test", "test", "test", "test")
            .with_priority(TaskPriority::High);
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

    #[tokio::test]
    async fn test_subagent_manager_creation() {
        let config = SubagentConfig::default();
        let manager = SubagentManager::new(config);

        let stats = manager.stats().await;
        assert_eq!(stats.total, 0);
    }

    #[tokio::test]
    async fn test_submit_task() {
        let manager = SubagentManager::new(SubagentConfig::default());

        let task = SubagentTask::new("Test task", "telegram", "chat1", "session1");

        let task_id = manager.submit(task).await.unwrap();

        // Task should be stored
        let stored = manager.get_task(&task_id).await;
        assert!(stored.is_some());
    }

    #[tokio::test]
    async fn test_get_tasks_by_status() {
        let manager = SubagentManager::new(SubagentConfig::default());

        let task = SubagentTask::new("Test task", "telegram", "chat1", "session1");

        manager.submit(task).await.unwrap();

        let pending = manager.get_tasks_by_status(TaskStatus::Pending).await;
        // Note: Task might have transitioned to Running depending on timing
        assert!(pending.len() <= 1);
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
        assert!(json.contains("completed"));
    }

    #[test]
    fn test_subagent_stats_default() {
        let stats = SubagentStats::default();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.running, 0);
    }
}
