//! Domain models for the multi-agent pipeline.
//!
//! These structs map 1:1 to the SQLite tables defined in `store.rs`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Priority levels for pipeline tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Critical,
}

impl std::fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Normal => write!(f, "normal"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

impl TaskPriority {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "low" => Self::Low,
            "high" => Self::High,
            "critical" => Self::Critical,
            _ => Self::Normal,
        }
    }
}

/// A pipeline task — the central entity that flows through the pipeline graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineTask {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Detailed description / prompt.
    pub description: String,
    /// Current state in the pipeline (validated against PipelineGraph at boundaries).
    pub state: String,
    /// Priority level.
    pub priority: TaskPriority,
    /// Role currently responsible for this task.
    pub assigned_role: Option<String>,
    /// Number of review round-trips so far.
    pub review_count: u32,
    /// Retry counter for stall recovery.
    pub retry_count: u32,
    /// Last heartbeat timestamp (for stall detection).
    pub last_heartbeat: DateTime<Utc>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
    /// Optional result content when task reaches Done.
    pub result: Option<String>,
    /// Origin channel (e.g. "telegram", "cli").
    pub origin_channel: Option<String>,
    /// Origin chat ID for routing the result back.
    pub origin_chat_id: Option<String>,
}

/// A single entry in the task flow audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowLogEntry {
    pub id: i64,
    pub task_id: String,
    pub from_state: String,
    pub to_state: String,
    pub agent_role: String,
    pub reason: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Progress report from an executing agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEntry {
    pub id: i64,
    pub task_id: String,
    pub agent_role: String,
    pub content: String,
    pub percentage: Option<f32>,
    pub timestamp: DateTime<Utc>,
}
