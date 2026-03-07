//! SQLite persistence layer for the pipeline subsystem.
//!
//! Provides CRUD operations for pipeline tasks, flow audit logs, and
//! progress entries. Shares the same `SqlitePool` as the rest of the
//! system but operates on its own set of tables.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tracing::debug;

use super::models::{FlowLogEntry, PipelineTask, ProgressEntry, TaskPriority};

/// Persistence layer for pipeline entities.
#[derive(Clone)]
pub struct PipelineStore {
    pub(crate) pool: SqlitePool,
}

impl PipelineStore {
    /// Wrap an existing pool (the same one used by `SqliteStore`).
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Create the pipeline-specific tables.
    ///
    /// Safe to call multiple times (`CREATE TABLE IF NOT EXISTS`).
    pub async fn init_tables(&self) -> anyhow::Result<()> {
        // ── pipeline_tasks ──
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pipeline_tasks (
                id              TEXT PRIMARY KEY,
                title           TEXT NOT NULL,
                description     TEXT NOT NULL DEFAULT '',
                state           TEXT NOT NULL DEFAULT 'pending',
                priority        TEXT NOT NULL DEFAULT 'normal',
                assigned_role   TEXT,
                review_count    INTEGER NOT NULL DEFAULT 0,
                retry_count     INTEGER NOT NULL DEFAULT 0,
                last_heartbeat  TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL,
                result          TEXT,
                origin_channel  TEXT,
                origin_chat_id  TEXT
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_pipeline_tasks_state ON pipeline_tasks(state)")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_pipeline_tasks_role ON pipeline_tasks(assigned_role)",
        )
        .execute(&self.pool)
        .await?;

        // ── pipeline_flow_log ──
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pipeline_flow_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id     TEXT NOT NULL,
                from_state  TEXT NOT NULL,
                to_state    TEXT NOT NULL,
                agent_role  TEXT NOT NULL,
                reason      TEXT,
                timestamp   TEXT NOT NULL,
                FOREIGN KEY (task_id) REFERENCES pipeline_tasks(id) ON DELETE CASCADE
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_pipeline_flow_log_task ON pipeline_flow_log(task_id)",
        )
        .execute(&self.pool)
        .await?;

        // ── pipeline_progress_log ──
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pipeline_progress_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id     TEXT NOT NULL,
                agent_role  TEXT NOT NULL,
                content     TEXT NOT NULL,
                percentage  REAL,
                timestamp   TEXT NOT NULL,
                FOREIGN KEY (task_id) REFERENCES pipeline_tasks(id) ON DELETE CASCADE
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_pipeline_progress_task ON pipeline_progress_log(task_id)",
        )
        .execute(&self.pool)
        .await?;

        debug!("Pipeline tables initialised");
        Ok(())
    }

    // ── Task CRUD ───────────────────────────────────────────────────

    /// Create a new pipeline task.
    pub async fn create_task(&self, task: &PipelineTask) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO pipeline_tasks
                (id, title, description, state, priority, assigned_role,
                 review_count, retry_count, last_heartbeat, created_at, updated_at,
                 result, origin_channel, origin_chat_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task.id)
        .bind(&task.title)
        .bind(&task.description)
        .bind(&task.state)
        .bind(task.priority.to_string())
        .bind(&task.assigned_role)
        .bind(task.review_count)
        .bind(task.retry_count)
        .bind(task.last_heartbeat.to_rfc3339())
        .bind(task.created_at.to_rfc3339())
        .bind(task.updated_at.to_rfc3339())
        .bind(&task.result)
        .bind(&task.origin_channel)
        .bind(&task.origin_chat_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Fetch a task by ID.
    pub async fn get_task(&self, id: &str) -> anyhow::Result<Option<PipelineTask>> {
        let row = sqlx::query_as::<_, TaskRow>(
            "SELECT id, title, description, state, priority, assigned_role,
                    review_count, retry_count, last_heartbeat, created_at, updated_at,
                    result, origin_channel, origin_chat_id
             FROM pipeline_tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into))
    }

    /// Transition a task's state using optimistic locking.
    ///
    /// Returns `Ok(true)` if the transition succeeded, `Ok(false)` if the
    /// expected state did not match (concurrent modification).
    pub async fn update_task_state(
        &self,
        id: &str,
        expected: &str,
        new_state: &str,
        assigned_role: Option<&str>,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE pipeline_tasks
             SET state = ?, assigned_role = COALESCE(?, assigned_role),
                 updated_at = ?, last_heartbeat = ?
             WHERE id = ? AND state = ?",
        )
        .bind(new_state)
        .bind(assigned_role)
        .bind(&now)
        .bind(&now)
        .bind(id)
        .bind(expected)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Increment the review counter for a task.
    pub async fn increment_review_count(&self, id: &str) -> anyhow::Result<u32> {
        sqlx::query("UPDATE pipeline_tasks SET review_count = review_count + 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        let count: (i32,) = sqlx::query_as("SELECT review_count FROM pipeline_tasks WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;

        Ok(count.0 as u32)
    }

    /// Set the final result text and mark the task as Done.
    pub async fn set_result(&self, id: &str, result: &str) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE pipeline_tasks SET result = ?, updated_at = ? WHERE id = ?")
            .bind(result)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// List tasks filtered by state.
    pub async fn list_tasks_by_state(&self, state: &str) -> anyhow::Result<Vec<PipelineTask>> {
        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT id, title, description, state, priority, assigned_role,
                    review_count, retry_count, last_heartbeat, created_at, updated_at,
                    result, origin_channel, origin_chat_id
             FROM pipeline_tasks WHERE state = ? ORDER BY created_at ASC",
        )
        .bind(state)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// List tasks assigned to a specific role.
    pub async fn list_tasks_by_role(&self, role: &str) -> anyhow::Result<Vec<PipelineTask>> {
        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT id, title, description, state, priority, assigned_role,
                    review_count, retry_count, last_heartbeat, created_at, updated_at,
                    result, origin_channel, origin_chat_id
             FROM pipeline_tasks WHERE assigned_role = ? ORDER BY created_at ASC",
        )
        .bind(role)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    // ── Heartbeat / Stall detection ─────────────────────────────────

    /// Update the heartbeat timestamp for a task.
    pub async fn update_heartbeat(&self, id: &str) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE pipeline_tasks SET last_heartbeat = ? WHERE id = ?")
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Find tasks whose heartbeat is older than `timeout_secs` and that are
    /// in one of the given active states.
    pub async fn find_stalled_tasks(
        &self,
        timeout_secs: u64,
        active_states: &HashSet<String>,
    ) -> anyhow::Result<Vec<PipelineTask>> {
        if active_states.is_empty() {
            return Ok(vec![]);
        }

        let cutoff = Utc::now() - chrono::Duration::seconds(timeout_secs as i64);

        // Build dynamic IN clause: IN (?, ?, ...)
        let placeholders: Vec<&str> = active_states.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT id, title, description, state, priority, assigned_role,
                    review_count, retry_count, last_heartbeat, created_at, updated_at,
                    result, origin_channel, origin_chat_id
             FROM pipeline_tasks
             WHERE last_heartbeat < ?
               AND state IN ({})
             ORDER BY last_heartbeat ASC",
            placeholders.join(", ")
        );

        let mut query = sqlx::query_as::<_, TaskRow>(&sql).bind(cutoff.to_rfc3339());

        for state in active_states {
            query = query.bind(state.clone());
        }

        let rows = query.fetch_all(&self.pool).await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    // ── Flow log ────────────────────────────────────────────────────

    /// Append a flow-transition record.
    pub async fn append_flow_log(
        &self,
        task_id: &str,
        from_state: &str,
        to_state: &str,
        agent_role: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO pipeline_flow_log (task_id, from_state, to_state, agent_role, reason, timestamp)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(task_id)
        .bind(from_state)
        .bind(to_state)
        .bind(agent_role)
        .bind(reason)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Retrieve the flow log for a task, ordered chronologically.
    pub async fn get_flow_log(&self, task_id: &str) -> anyhow::Result<Vec<FlowLogEntry>> {
        let rows = sqlx::query_as::<_, FlowLogRow>(
            "SELECT id, task_id, from_state, to_state, agent_role, reason, timestamp
             FROM pipeline_flow_log WHERE task_id = ? ORDER BY id ASC",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    // ── Progress log ────────────────────────────────────────────────

    /// Append a progress entry and update the task heartbeat.
    pub async fn append_progress(
        &self,
        task_id: &str,
        agent_role: &str,
        content: &str,
        percentage: Option<f32>,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO pipeline_progress_log (task_id, agent_role, content, percentage, timestamp)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(task_id)
        .bind(agent_role)
        .bind(content)
        .bind(percentage)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        // Piggyback heartbeat update
        sqlx::query("UPDATE pipeline_tasks SET last_heartbeat = ? WHERE id = ?")
            .bind(&now)
            .bind(task_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Retrieve progress entries for a task.
    pub async fn get_progress(&self, task_id: &str) -> anyhow::Result<Vec<ProgressEntry>> {
        let rows = sqlx::query_as::<_, ProgressRow>(
            "SELECT id, task_id, agent_role, content, percentage, timestamp
             FROM pipeline_progress_log WHERE task_id = ? ORDER BY id ASC",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }
}

// ── Internal row types for sqlx ─────────────────────────────────────

#[derive(sqlx::FromRow)]
struct TaskRow {
    id: String,
    title: String,
    description: String,
    state: String,
    priority: String,
    assigned_role: Option<String>,
    review_count: i32,
    retry_count: i32,
    last_heartbeat: String,
    created_at: String,
    updated_at: String,
    result: Option<String>,
    origin_channel: Option<String>,
    origin_chat_id: Option<String>,
}

impl From<TaskRow> for PipelineTask {
    fn from(r: TaskRow) -> Self {
        Self {
            id: r.id,
            title: r.title,
            description: r.description,
            state: r.state,
            priority: TaskPriority::from_str_lossy(&r.priority),
            assigned_role: r.assigned_role,
            review_count: r.review_count as u32,
            retry_count: r.retry_count as u32,
            last_heartbeat: DateTime::parse_from_rfc3339(&r.last_heartbeat)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            created_at: DateTime::parse_from_rfc3339(&r.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: DateTime::parse_from_rfc3339(&r.updated_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            result: r.result,
            origin_channel: r.origin_channel,
            origin_chat_id: r.origin_chat_id,
        }
    }
}

#[derive(sqlx::FromRow)]
struct FlowLogRow {
    id: i64,
    task_id: String,
    from_state: String,
    to_state: String,
    agent_role: String,
    reason: Option<String>,
    timestamp: String,
}

impl From<FlowLogRow> for FlowLogEntry {
    fn from(r: FlowLogRow) -> Self {
        Self {
            id: r.id,
            task_id: r.task_id,
            from_state: r.from_state,
            to_state: r.to_state,
            agent_role: r.agent_role,
            reason: r.reason,
            timestamp: DateTime::parse_from_rfc3339(&r.timestamp)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        }
    }
}

#[derive(sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    task_id: String,
    agent_role: String,
    content: String,
    percentage: Option<f64>,
    timestamp: String,
}

impl From<ProgressRow> for ProgressEntry {
    fn from(r: ProgressRow) -> Self {
        Self {
            id: r.id,
            task_id: r.task_id,
            agent_role: r.agent_role,
            content: r.content,
            percentage: r.percentage.map(|p| p as f32),
            timestamp: DateTime::parse_from_rfc3339(&r.timestamp)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_store() -> PipelineStore {
        let path =
            std::env::temp_dir().join(format!("nanobot_pipeline_test_{}.db", uuid::Uuid::new_v4()));

        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();

        let store = PipelineStore::new(pool);
        store.init_tables().await.unwrap();
        store
    }

    fn make_task(id: &str, title: &str) -> PipelineTask {
        let now = Utc::now();
        PipelineTask {
            id: id.to_string(),
            title: title.to_string(),
            description: String::new(),
            state: "pending".to_string(),
            priority: TaskPriority::Normal,
            assigned_role: None,
            review_count: 0,
            retry_count: 0,
            last_heartbeat: now,
            created_at: now,
            updated_at: now,
            result: None,
            origin_channel: None,
            origin_chat_id: None,
        }
    }

    #[tokio::test]
    async fn test_create_and_get_task() {
        let store = temp_store().await;
        let task = make_task("t1", "Test Task");
        store.create_task(&task).await.unwrap();

        let fetched = store.get_task("t1").await.unwrap().unwrap();
        assert_eq!(fetched.title, "Test Task");
        assert_eq!(fetched.state, "pending");
    }

    #[tokio::test]
    async fn test_optimistic_lock_transition() {
        let store = temp_store().await;
        store.create_task(&make_task("t2", "Lock")).await.unwrap();

        // Correct expected state
        let ok = store
            .update_task_state("t2", "pending", "triage", Some("taizi"))
            .await
            .unwrap();
        assert!(ok);

        // Wrong expected state → no rows affected
        let fail = store
            .update_task_state("t2", "pending", "planning", None)
            .await
            .unwrap();
        assert!(!fail);
    }

    #[tokio::test]
    async fn test_list_by_state() {
        let store = temp_store().await;
        store.create_task(&make_task("a", "A")).await.unwrap();
        store.create_task(&make_task("b", "B")).await.unwrap();
        store
            .update_task_state("b", "pending", "triage", Some("taizi"))
            .await
            .unwrap();

        let pending = store.list_tasks_by_state("pending").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "a");

        let triage = store.list_tasks_by_state("triage").await.unwrap();
        assert_eq!(triage.len(), 1);
        assert_eq!(triage[0].id, "b");
    }

    #[tokio::test]
    async fn test_flow_log() {
        let store = temp_store().await;
        store.create_task(&make_task("f1", "Flow")).await.unwrap();

        store
            .append_flow_log("f1", "pending", "triage", "taizi", Some("initial triage"))
            .await
            .unwrap();
        store
            .append_flow_log("f1", "triage", "planning", "zhongshu", None)
            .await
            .unwrap();

        let log = store.get_flow_log("f1").await.unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].from_state, "pending");
        assert_eq!(log[1].to_state, "planning");
    }

    #[tokio::test]
    async fn test_progress_and_heartbeat() {
        let store = temp_store().await;
        store
            .create_task(&make_task("p1", "Progress"))
            .await
            .unwrap();

        store
            .append_progress("p1", "gong", "50% done", Some(50.0))
            .await
            .unwrap();

        let entries = store.get_progress("p1").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "50% done");
        assert!((entries[0].percentage.unwrap() - 50.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_stalled_tasks() {
        let store = temp_store().await;
        let mut task = make_task("s1", "Stall");
        // Set heartbeat to 2 minutes ago
        task.last_heartbeat = Utc::now() - chrono::Duration::seconds(120);
        task.state = "executing".to_string();
        store.create_task(&task).await.unwrap();

        let active_states: HashSet<String> =
            ["executing", "triage", "planning", "reviewing", "assigned"]
                .iter()
                .map(|s| s.to_string())
                .collect();

        let stalled = store.find_stalled_tasks(60, &active_states).await.unwrap();
        assert_eq!(stalled.len(), 1);
        assert_eq!(stalled[0].id, "s1");

        // With a longer timeout, the task is not stalled
        let not_stalled = store.find_stalled_tasks(300, &active_states).await.unwrap();
        assert_eq!(not_stalled.len(), 0);
    }

    #[tokio::test]
    async fn test_stalled_tasks_empty_active_states() {
        let store = temp_store().await;
        let mut task = make_task("s2", "Stall2");
        task.last_heartbeat = Utc::now() - chrono::Duration::seconds(120);
        task.state = "executing".to_string();
        store.create_task(&task).await.unwrap();

        let empty: HashSet<String> = HashSet::new();
        let stalled = store.find_stalled_tasks(60, &empty).await.unwrap();
        assert!(stalled.is_empty());
    }
}
