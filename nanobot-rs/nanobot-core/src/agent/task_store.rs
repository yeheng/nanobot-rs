//! SQLite-backed task store for SubagentManager.
//!
//! Provides O(1) single-task persistence via `INSERT OR REPLACE`.
//! This is the only task storage backend - JSON is only used for one-time migration.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use tokio::fs;
use tracing::{debug, info};

use super::subagent::{SubagentTask, TaskPriority, TaskStatus};

/// SQLite-backed task persistence.
///
/// Each task is stored as a single row, enabling O(1) upserts.
pub struct SqliteTaskStore {
    pool: SqlitePool,
}

impl SqliteTaskStore {
    /// Open (or create) the SQLite database at `db_path` and initialise the schema.
    pub async fn new(db_path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(3)
            .connect_with(options)
            .await?;

        let store = Self { pool };
        store.init_db().await?;
        debug!("Opened SqliteTaskStore at {:?}", db_path);
        Ok(store)
    }

    async fn init_db(&self) -> anyhow::Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tasks (
                id            TEXT PRIMARY KEY,
                prompt        TEXT NOT NULL,
                channel       TEXT NOT NULL,
                chat_id       TEXT NOT NULL,
                session_key   TEXT NOT NULL,
                status        TEXT NOT NULL DEFAULT 'Pending',
                priority      TEXT NOT NULL DEFAULT 'Normal',
                created_at    TEXT NOT NULL,
                started_at    TEXT,
                completed_at  TEXT,
                result        TEXT,
                error         TEXT,
                timeout_secs  INTEGER NOT NULL DEFAULT 300,
                progress      INTEGER NOT NULL DEFAULT 0,
                metadata      TEXT NOT NULL DEFAULT '{}'
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status)")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Load all tasks from SQLite.
    pub async fn load_all(&self) -> anyhow::Result<HashMap<String, SubagentTask>> {
        let rows: Vec<SqliteRow> = sqlx::query(
            "SELECT id, prompt, channel, chat_id, session_key, status, priority,
                    created_at, started_at, completed_at, result, error,
                    timeout_secs, progress, metadata
             FROM tasks",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut map = HashMap::new();
        for row in &rows {
            let task = Self::row_to_task(row)?;
            map.insert(task.id.clone(), task);
        }
        info!("Loaded {} tasks from SQLite", map.len());
        Ok(map)
    }

    /// Persist a single task (insert or update).
    pub async fn save_task(&self, task: &SubagentTask) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO tasks
             (id, prompt, channel, chat_id, session_key, status, priority,
              created_at, started_at, completed_at, result, error,
              timeout_secs, progress, metadata)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(&task.id)
        .bind(&task.prompt)
        .bind(&task.channel)
        .bind(&task.chat_id)
        .bind(&task.session_key)
        .bind(status_to_int(&task.status))
        .bind(priority_to_int(&task.priority))
        .bind(task.created_at.to_rfc3339())
        .bind(task.started_at.map(|t| t.to_rfc3339()))
        .bind(task.completed_at.map(|t| t.to_rfc3339()))
        .bind(&task.result)
        .bind(&task.error)
        .bind(task.timeout_secs as i64)
        .bind(task.progress as i32)
        .bind(serde_json::to_string(&task.metadata)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove tasks by IDs.
    pub async fn remove_tasks(&self, ids: &[String]) -> anyhow::Result<()> {
        for id in ids {
            sqlx::query("DELETE FROM tasks WHERE id = $1")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        debug!("Removed {} tasks from SQLite", ids.len());
        Ok(())
    }

    fn row_to_task(row: &SqliteRow) -> anyhow::Result<SubagentTask> {
        let id: String = row.get("id");
        let prompt: String = row.get("prompt");
        let channel: String = row.get("channel");
        let chat_id: String = row.get("chat_id");
        let session_key: String = row.get("session_key");
        let status = parse_status_from_row(row)?;
        let priority = parse_priority_from_row(row)?;
        let created_str: String = row.get("created_at");
        let started_str: Option<String> = row.get("started_at");
        let completed_str: Option<String> = row.get("completed_at");
        let result: Option<String> = row.get("result");
        let error: Option<String> = row.get("error");
        let timeout_secs: i64 = row.get("timeout_secs");
        let progress: i32 = row.get("progress");
        let metadata_json: String = row.get("metadata");

        let created_at = DateTime::parse_from_rfc3339(&created_str)?.with_timezone(&Utc);
        let started_at = started_str
            .as_deref()
            .map(|s| DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc)))
            .transpose()?;
        let completed_at = completed_str
            .as_deref()
            .map(|s| DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&Utc)))
            .transpose()?;
        let metadata: HashMap<String, String> = serde_json::from_str(&metadata_json)?;

        Ok(SubagentTask {
            id,
            prompt,
            channel,
            chat_id,
            session_key,
            status,
            priority,
            created_at,
            started_at,
            completed_at,
            result,
            error,
            timeout_secs: timeout_secs as u64,
            progress: progress as u8,
            metadata,
        })
    }
}

// ── Enum ↔ integer helpers ──────────────────────────────────
//
// Store enums as integers in SQLite for efficiency.
// Read path is lenient: accepts both integers and legacy strings for backward compatibility.

fn status_to_int(s: &TaskStatus) -> i32 {
    match s {
        TaskStatus::Pending => 0,
        TaskStatus::Running => 1,
        TaskStatus::Completed => 2,
        TaskStatus::Failed => 3,
        TaskStatus::Cancelled => 4,
        TaskStatus::Timeout => 5,
    }
}

fn int_to_status(v: i32) -> anyhow::Result<TaskStatus> {
    match v {
        0 => Ok(TaskStatus::Pending),
        1 => Ok(TaskStatus::Running),
        2 => Ok(TaskStatus::Completed),
        3 => Ok(TaskStatus::Failed),
        4 => Ok(TaskStatus::Cancelled),
        5 => Ok(TaskStatus::Timeout),
        _ => anyhow::bail!("Unknown TaskStatus int: {}", v),
    }
}

fn priority_to_int(p: &TaskPriority) -> i32 {
    match p {
        TaskPriority::Low => 0,
        TaskPriority::Normal => 1,
        TaskPriority::High => 2,
        TaskPriority::Urgent => 3,
    }
}

fn int_to_priority(v: i32) -> anyhow::Result<TaskPriority> {
    match v {
        0 => Ok(TaskPriority::Low),
        1 => Ok(TaskPriority::Normal),
        2 => Ok(TaskPriority::High),
        3 => Ok(TaskPriority::Urgent),
        _ => anyhow::bail!("Unknown TaskPriority int: {}", v),
    }
}

/// Parse a status column value that may be an integer or legacy name string.
fn parse_status_from_row(row: &SqliteRow) -> anyhow::Result<TaskStatus> {
    // Try integer first
    if let Ok(v) = row.try_get::<i32, _>("status") {
        return int_to_status(v);
    }
    // Fall back to string
    let s: String = row.get("status");
    // Try numeric string (e.g. "0", "1")
    if let Ok(v) = s.parse::<i32>() {
        return int_to_status(v);
    }
    // Legacy named string
    match s.as_str() {
        "Pending" => Ok(TaskStatus::Pending),
        "Running" => Ok(TaskStatus::Running),
        "Completed" => Ok(TaskStatus::Completed),
        "Failed" => Ok(TaskStatus::Failed),
        "Cancelled" => Ok(TaskStatus::Cancelled),
        "Timeout" => Ok(TaskStatus::Timeout),
        _ => anyhow::bail!("Unknown TaskStatus: {}", s),
    }
}

/// Parse a priority column value that may be an integer or legacy name string.
fn parse_priority_from_row(row: &SqliteRow) -> anyhow::Result<TaskPriority> {
    // Try integer first
    if let Ok(v) = row.try_get::<i32, _>("priority") {
        return int_to_priority(v);
    }
    // Fall back to string
    let s: String = row.get("priority");
    // Try numeric string (e.g. "0", "1")
    if let Ok(v) = s.parse::<i32>() {
        return int_to_priority(v);
    }
    // Legacy named string
    match s.as_str() {
        "Low" => Ok(TaskPriority::Low),
        "Normal" => Ok(TaskPriority::Normal),
        "High" => Ok(TaskPriority::High),
        "Urgent" => Ok(TaskPriority::Urgent),
        _ => anyhow::bail!("Unknown TaskPriority: {}", s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(prompt: &str) -> SubagentTask {
        SubagentTask::new(prompt, "test_ch", "chat1", "sess1")
    }

    #[tokio::test]
    async fn test_sqlite_store_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteTaskStore::new(dir.path().join("tasks.db"))
            .await
            .unwrap();

        let t = make_task("hello");
        store.save_task(&t).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[&t.id].prompt, "hello");
    }

    #[tokio::test]
    async fn test_sqlite_store_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteTaskStore::new(dir.path().join("tasks.db"))
            .await
            .unwrap();

        let mut t = make_task("v1");
        store.save_task(&t).await.unwrap();

        t.prompt = "v2".to_string();
        t.status = TaskStatus::Running;
        t.started_at = Some(Utc::now());
        store.save_task(&t).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[&t.id].prompt, "v2");
        assert_eq!(loaded[&t.id].status, TaskStatus::Running);
        assert!(loaded[&t.id].started_at.is_some());
    }

    #[tokio::test]
    async fn test_sqlite_store_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteTaskStore::new(dir.path().join("tasks.db"))
            .await
            .unwrap();

        let t1 = make_task("a");
        let t2 = make_task("b");
        let t3 = make_task("c");
        store.save_task(&t1).await.unwrap();
        store.save_task(&t2).await.unwrap();
        store.save_task(&t3).await.unwrap();

        store
            .remove_tasks(&[t1.id.clone(), t3.id.clone()])
            .await
            .unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key(&t2.id));
    }

    #[tokio::test]
    async fn test_sqlite_store_all_fields_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteTaskStore::new(dir.path().join("tasks.db"))
            .await
            .unwrap();

        let mut t = SubagentTask::new("full test", "telegram", "chat42", "session:x")
            .with_priority(TaskPriority::Urgent)
            .with_timeout(999)
            .with_metadata("env", "prod")
            .with_metadata("region", "us-east");
        t.status = TaskStatus::Completed;
        t.started_at = Some(Utc::now());
        t.completed_at = Some(Utc::now());
        t.result = Some("42".to_string());
        t.error = None;
        t.progress = 100;

        store.save_task(&t).await.unwrap();
        let loaded = store.load_all().await.unwrap();
        let lt = &loaded[&t.id];

        assert_eq!(lt.prompt, "full test");
        assert_eq!(lt.channel, "telegram");
        assert_eq!(lt.chat_id, "chat42");
        assert_eq!(lt.session_key, "session:x");
        assert_eq!(lt.status, TaskStatus::Completed);
        assert_eq!(lt.priority, TaskPriority::Urgent);
        assert_eq!(lt.timeout_secs, 999);
        assert_eq!(lt.progress, 100);
        assert_eq!(lt.result, Some("42".to_string()));
        assert!(lt.error.is_none());
        assert!(lt.started_at.is_some());
        assert!(lt.completed_at.is_some());
        assert_eq!(lt.metadata.get("env").unwrap(), "prod");
        assert_eq!(lt.metadata.get("region").unwrap(), "us-east");
    }

}
