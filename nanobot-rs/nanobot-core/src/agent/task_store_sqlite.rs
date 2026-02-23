//! SQLite-backed task store for SubagentManager.
//!
//! Provides O(1) single-task persistence via `INSERT OR REPLACE`.
//! Gated behind the `sqlite` feature flag.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::subagent::{SubagentTask, TaskPriority, TaskStatus};
use super::task_store::TaskStore;

/// SQLite-backed task persistence.
///
/// Each task is stored as a single row, enabling O(1) upserts.
pub struct SqliteTaskStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteTaskStore {
    /// Open (or create) the SQLite database at `db_path` and initialise the schema.
    pub fn new(db_path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&db_path)?;
        Self::init_db(&conn)?;
        debug!("Opened SqliteTaskStore at {:?}", db_path);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn init_db(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS tasks (
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
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
            ",
        )?;
        Ok(())
    }

    /// Migrate tasks from a legacy `tasks.json` file into SQLite.
    ///
    /// After a successful import the JSON file is renamed to `*.migrated`
    /// so migration only runs once.
    pub fn migrate_from_json(&self, json_path: &PathBuf) -> anyhow::Result<()> {
        if !json_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(json_path)?;
        let tasks: Vec<SubagentTask> = match serde_json::from_str(&content) {
            Ok(t) => t,
            Err(e) => {
                warn!("Could not parse legacy tasks.json for migration: {}", e);
                return Ok(());
            }
        };

        // We can't async-lock here (called from sync context), so open a
        // second connection for migration. SQLite WAL mode allows this.
        // Alternatively, the caller can lock before invoking.
        // For simplicity, use the blocking approach since this runs at init.
        if !tasks.is_empty() {
            // Use the path from the existing connection
            let conn_path = json_path.parent().unwrap_or(json_path).join("tasks.db");
            let conn = Connection::open(&conn_path)?;
            Self::init_db(&conn)?;
            let tx = conn.unchecked_transaction()?;
            for task in &tasks {
                Self::upsert_task_sync(&tx, task)?;
            }
            tx.commit()?;
            info!(
                "Migrated {} tasks from {:?} to SQLite",
                tasks.len(),
                json_path
            );
        }

        let backup = json_path.with_extension("json.migrated");
        std::fs::rename(json_path, &backup)?;
        info!("Renamed {:?} → {:?}", json_path, backup);

        Ok(())
    }

    /// Synchronous upsert for a single task (used in migration and save).
    fn upsert_task_sync(conn: &Connection, task: &SubagentTask) -> anyhow::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO tasks
             (id, prompt, channel, chat_id, session_key, status, priority,
              created_at, started_at, completed_at, result, error,
              timeout_secs, progress, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                task.id,
                task.prompt,
                task.channel,
                task.chat_id,
                task.session_key,
                status_to_str(&task.status),
                priority_to_str(&task.priority),
                task.created_at.to_rfc3339(),
                task.started_at.map(|t| t.to_rfc3339()),
                task.completed_at.map(|t| t.to_rfc3339()),
                task.result,
                task.error,
                task.timeout_secs as i64,
                task.progress as i32,
                serde_json::to_string(&task.metadata)?,
            ],
        )?;
        Ok(())
    }

    fn row_to_task(row: &rusqlite::Row<'_>) -> anyhow::Result<SubagentTask> {
        let id: String = row.get(0)?;
        let prompt: String = row.get(1)?;
        let channel: String = row.get(2)?;
        let chat_id: String = row.get(3)?;
        let session_key: String = row.get(4)?;
        let status_str: String = row.get(5)?;
        let priority_str: String = row.get(6)?;
        let created_str: String = row.get(7)?;
        let started_str: Option<String> = row.get(8)?;
        let completed_str: Option<String> = row.get(9)?;
        let result: Option<String> = row.get(10)?;
        let error: Option<String> = row.get(11)?;
        let timeout_secs: i64 = row.get(12)?;
        let progress: i32 = row.get(13)?;
        let metadata_json: String = row.get(14)?;

        let status = str_to_status(&status_str)?;
        let priority = str_to_priority(&priority_str)?;
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

#[async_trait]
impl TaskStore for SqliteTaskStore {
    async fn load_all(&self) -> anyhow::Result<HashMap<String, SubagentTask>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, prompt, channel, chat_id, session_key, status, priority,
                    created_at, started_at, completed_at, result, error,
                    timeout_secs, progress, metadata
             FROM tasks",
        )?;
        let mut rows = stmt.query([])?;
        let mut map = HashMap::new();
        while let Some(row) = rows.next()? {
            let task = Self::row_to_task(row)?;
            map.insert(task.id.clone(), task);
        }
        info!("Loaded {} tasks from SQLite", map.len());
        Ok(map)
    }

    async fn save_task(&self, task: &SubagentTask) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        Self::upsert_task_sync(&conn, task)?;
        Ok(())
    }

    async fn save_all(&self, _tasks: &[SubagentTask]) -> anyhow::Result<()> {
        // No-op: individual saves via save_task are sufficient for SQLite.
        Ok(())
    }

    async fn remove_tasks(&self, ids: &[String]) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        for id in ids {
            conn.execute("DELETE FROM tasks WHERE id = ?1", rusqlite::params![id])?;
        }
        debug!("Removed {} tasks from SQLite", ids.len());
        Ok(())
    }
}

// ── Enum ↔ string helpers ──────────────────────────────────

fn status_to_str(s: &TaskStatus) -> &'static str {
    match s {
        TaskStatus::Pending => "Pending",
        TaskStatus::Running => "Running",
        TaskStatus::Completed => "Completed",
        TaskStatus::Failed => "Failed",
        TaskStatus::Cancelled => "Cancelled",
        TaskStatus::Timeout => "Timeout",
    }
}

fn str_to_status(s: &str) -> anyhow::Result<TaskStatus> {
    match s {
        "Pending" => Ok(TaskStatus::Pending),
        "Running" => Ok(TaskStatus::Running),
        "Completed" => Ok(TaskStatus::Completed),
        "Failed" => Ok(TaskStatus::Failed),
        "Cancelled" => Ok(TaskStatus::Cancelled),
        "Timeout" => Ok(TaskStatus::Timeout),
        _ => anyhow::bail!("Unknown TaskStatus: {}", s),
    }
}

fn priority_to_str(p: &TaskPriority) -> &'static str {
    match p {
        TaskPriority::Low => "Low",
        TaskPriority::Normal => "Normal",
        TaskPriority::High => "High",
        TaskPriority::Urgent => "Urgent",
    }
}

fn str_to_priority(s: &str) -> anyhow::Result<TaskPriority> {
    match s {
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
        let store = SqliteTaskStore::new(dir.path().join("tasks.db")).unwrap();

        let t = make_task("hello");
        store.save_task(&t).await.unwrap();

        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[&t.id].prompt, "hello");
    }

    #[tokio::test]
    async fn test_sqlite_store_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteTaskStore::new(dir.path().join("tasks.db")).unwrap();

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
        let store = SqliteTaskStore::new(dir.path().join("tasks.db")).unwrap();

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
        let store = SqliteTaskStore::new(dir.path().join("tasks.db")).unwrap();

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

    #[tokio::test]
    async fn test_sqlite_store_migration() {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("tasks.json");

        // Write a legacy JSON file
        let t1 = make_task("migrated_task_1");
        let t2 = make_task("migrated_task_2");
        let json = serde_json::to_string_pretty(&vec![t1.clone(), t2.clone()]).unwrap();
        std::fs::write(&json_path, &json).unwrap();

        // Create SQLite store and migrate
        let store = SqliteTaskStore::new(dir.path().join("tasks.db")).unwrap();
        store.migrate_from_json(&json_path).unwrap();

        // JSON file should be renamed
        assert!(!json_path.exists());
        assert!(json_path.with_extension("json.migrated").exists());

        // Tasks should be in SQLite
        let loaded = store.load_all().await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[&t1.id].prompt, "migrated_task_1");
        assert_eq!(loaded[&t2.id].prompt, "migrated_task_2");
    }
}
