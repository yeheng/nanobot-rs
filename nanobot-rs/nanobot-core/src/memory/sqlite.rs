//! SQLite-backed memory store with FTS5 full-text search.

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};
use tracing::debug;

use super::store::{MemoryEntry, MemoryMetadata, MemoryQuery, MemoryStore};

/// SQLite-backed memory store with FTS5 full-text search.
///
/// Persists memory entries, history, long-term memory, and sessions in a
/// single SQLite database file. Uses `sqlx::SqlitePool` for native async
/// I/O without blocking the tokio runtime.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

/// Session metadata for per-message storage
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub key: String,
    pub last_consolidated: usize,
}

/// Message row for session messages
#[derive(Debug, Clone)]
pub struct MessageRow {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub tools_used: Option<String>,
}

impl SqliteStore {
    /// Create a new `SqliteStore` with the default database path
    /// (`~/.nanobot/nanobot.db`).
    pub async fn new() -> anyhow::Result<Self> {
        let path = crate::config::config_dir().join("nanobot.db");
        Self::with_path(path).await
    }

    /// Create a new `SqliteStore` with a custom database path.
    pub async fn with_path(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        let store = Self { pool };
        store.init_db().await?;
        store.health_check().await?;
        debug!("Opened SqliteStore at {:?}", path);
        Ok(store)
    }

    /// Verify that the database is usable (integrity + read/write).
    async fn health_check(&self) -> anyhow::Result<()> {
        // Integrity check
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&self.pool)
            .await?;
        if integrity != "ok" {
            anyhow::bail!("SQLite integrity check failed: {}", integrity);
        }

        // Write check — try inserting and deleting a sentinel row in kv_store
        sqlx::query(
            "INSERT OR REPLACE INTO kv_store (key, value, updated_at) VALUES ('__health_check__', '1', datetime('now'))",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("DELETE FROM kv_store WHERE key = '__health_check__'")
            .execute(&self.pool)
            .await?;

        debug!("SQLite health check passed");
        Ok(())
    }

    async fn init_db(&self) -> anyhow::Result<()> {
        // sqlx doesn't have execute_batch, so we run each statement separately.
        // FTS5 virtual table + triggers must also be individual statements.

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memories (
                id          TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                metadata    TEXT NOT NULL DEFAULT '{}',
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memory_tags (
                memory_id   TEXT NOT NULL,
                tag         TEXT NOT NULL,
                PRIMARY KEY (memory_id, tag),
                FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_tags_tag ON memory_tags(tag)")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                id,
                content,
                content='memories',
                content_rowid='rowid'
            )",
        )
        .execute(&self.pool)
        .await?;

        // Triggers to keep FTS5 index in sync
        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, id, content)
                VALUES (new.rowid, new.id, new.content);
            END",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, content)
                VALUES ('delete', old.rowid, old.id, old.content);
            END",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, content)
                VALUES ('delete', old.rowid, old.id, old.content);
                INSERT INTO memories_fts(rowid, id, content)
                VALUES (new.rowid, new.id, new.content);
            END",
        )
        .execute(&self.pool)
        .await?;

        // History table for conversation history
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                content     TEXT NOT NULL,
                created_at  TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_history_created_at ON history(created_at)")
            .execute(&self.pool)
            .await?;

        // Key-value store for long-term memory and other raw data
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS kv_store (
                key         TEXT PRIMARY KEY,
                value       TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // Sessions table (metadata only)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
                key         TEXT PRIMARY KEY,
                last_consolidated INTEGER NOT NULL DEFAULT 0,
                updated_at  TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at)")
            .execute(&self.pool)
            .await?;

        // Session messages table (one row per message)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_messages (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key TEXT NOT NULL,
                role        TEXT NOT NULL,
                content     TEXT NOT NULL,
                timestamp   TEXT NOT NULL,
                tools_used  TEXT,
                FOREIGN KEY (session_key) REFERENCES sessions(key) ON DELETE CASCADE
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_session_messages_session_key ON session_messages(session_key)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_session_messages_timestamp ON session_messages(timestamp)",
        )
        .execute(&self.pool)
        .await?;

        // Cron jobs table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS cron_jobs (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                cron        TEXT NOT NULL,
                message     TEXT NOT NULL,
                channel     TEXT,
                chat_id     TEXT,
                last_run    TEXT,
                next_run    TEXT,
                enabled     INTEGER NOT NULL DEFAULT 1
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_cron_jobs_next_run ON cron_jobs(next_run)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_cron_jobs_enabled ON cron_jobs(enabled)")
            .execute(&self.pool)
            .await?;

        // Session summaries table (one row per session)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_summaries (
                session_key TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                created_at  TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ── History API ──

    /// Read all history entries, ordered by creation time (oldest first).
    pub async fn read_history(&self) -> anyhow::Result<String> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT content FROM history ORDER BY id ASC")
            .fetch_all(&self.pool)
            .await?;

        let mut result = String::new();
        for (content,) in rows {
            result.push_str(&content);
        }
        Ok(result)
    }

    /// Append a new history entry.
    pub async fn append_history(&self, content: &str) -> anyhow::Result<()> {
        let created_at = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO history (content, created_at) VALUES ($1, $2)")
            .bind(content)
            .bind(&created_at)
            .execute(&self.pool)
            .await?;
        debug!("Appended history entry");
        Ok(())
    }

    /// Write (replace) the entire history with new content.
    pub async fn write_history(&self, content: &str) -> anyhow::Result<()> {
        // Clear existing history
        sqlx::query("DELETE FROM history")
            .execute(&self.pool)
            .await?;

        // Insert new content as a single entry
        let created_at = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO history (content, created_at) VALUES ($1, $2)")
            .bind(content)
            .bind(&created_at)
            .execute(&self.pool)
            .await?;
        debug!("Wrote history");
        Ok(())
    }

    /// Clear all history entries.
    pub async fn clear_history(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM history")
            .execute(&self.pool)
            .await?;
        debug!("Cleared history");
        Ok(())
    }

    // ── Key-value store API (replaces file-based MEMORY.md etc.) ──

    /// Read a raw value by key.
    pub async fn read_raw(&self, key: &str) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM kv_store WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(v,)| v))
    }

    /// Write a raw value by key (upsert).
    pub async fn write_raw(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let updated_at = Utc::now().to_rfc3339();
        sqlx::query("INSERT OR REPLACE INTO kv_store (key, value, updated_at) VALUES ($1, $2, $3)")
            .bind(key)
            .bind(value)
            .bind(&updated_at)
            .execute(&self.pool)
            .await?;
        debug!("Wrote kv_store key: {}", key);
        Ok(())
    }

    /// Delete a raw key. Returns `true` if the key existed.
    pub async fn delete_raw(&self, key: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM kv_store WHERE key = $1")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ── Session API (Legacy Blob - for migration only) ──

    /// Load a session by key (legacy JSON blob format).
    /// Used for backward compatibility during migration.
    #[deprecated(note = "Use load_session_messages instead for per-message storage")]
    pub async fn load_session(&self, key: &str) -> anyhow::Result<Option<String>> {
        // Check if this is legacy format (has 'data' column) or new format
        let has_data_column: bool = sqlx::query_scalar::<_, i32>(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='data'",
        )
        .fetch_one(&self.pool)
        .await?
            > 0;

        if has_data_column {
            let row: Option<(String,)> = sqlx::query_as("SELECT data FROM sessions WHERE key = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
            return Ok(row.map(|(d,)| d));
        }
        Ok(None)
    }

    /// Save a session (legacy JSON blob format).
    #[deprecated(note = "Use append_session_message instead for per-message storage")]
    pub async fn save_session(&self, key: &str, data: &str) -> anyhow::Result<()> {
        let updated_at = Utc::now().to_rfc3339();
        sqlx::query("INSERT OR REPLACE INTO sessions (key, data, updated_at) VALUES ($1, $2, $3)")
            .bind(key)
            .bind(data)
            .bind(&updated_at)
            .execute(&self.pool)
            .await?;
        debug!("Saved session (legacy): {}", key);
        Ok(())
    }

    /// Delete a session by key.
    pub async fn delete_session(&self, key: &str) -> anyhow::Result<bool> {
        // CASCADE will delete messages automatically
        let result = sqlx::query("DELETE FROM sessions WHERE key = $1")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ── Session API (New Per-Message Storage) ──

    /// Create or update session metadata.
    pub async fn save_session_meta(
        &self,
        key: &str,
        last_consolidated: usize,
    ) -> anyhow::Result<()> {
        let updated_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR REPLACE INTO sessions (key, last_consolidated, updated_at) VALUES ($1, $2, $3)",
        )
        .bind(key)
        .bind(last_consolidated as i64)
        .bind(&updated_at)
        .execute(&self.pool)
        .await?;
        debug!("Saved session meta: {}", key);
        Ok(())
    }

    /// Load session metadata.
    pub async fn load_session_meta(&self, key: &str) -> anyhow::Result<Option<SessionMeta>> {
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT key, last_consolidated FROM sessions WHERE key = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|(key, lc)| SessionMeta {
            key,
            last_consolidated: lc as usize,
        }))
    }

    /// Append a single message to a session (O(1) operation).
    pub async fn append_session_message(
        &self,
        session_key: &str,
        role: &str,
        content: &str,
        timestamp: &DateTime<Utc>,
        tools_used: Option<&[String]>,
    ) -> anyhow::Result<()> {
        let timestamp_str = timestamp.to_rfc3339();
        let tools_json =
            tools_used.map(|t| serde_json::to_string(t).unwrap_or_else(|_| "[]".to_string()));
        let updated_at = Utc::now().to_rfc3339();

        // Ensure session exists
        sqlx::query(
            "INSERT OR IGNORE INTO sessions (key, last_consolidated, updated_at) VALUES ($1, 0, $2)",
        )
        .bind(session_key)
        .bind(&updated_at)
        .execute(&self.pool)
        .await?;

        // Insert message
        sqlx::query(
            "INSERT INTO session_messages (session_key, role, content, timestamp, tools_used) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(session_key)
        .bind(role)
        .bind(content)
        .bind(&timestamp_str)
        .bind(&tools_json)
        .execute(&self.pool)
        .await?;

        // Update session updated_at
        sqlx::query("UPDATE sessions SET updated_at = $1 WHERE key = $2")
            .bind(&updated_at)
            .bind(session_key)
            .execute(&self.pool)
            .await?;

        debug!("Appended message to session: {}", session_key);
        Ok(())
    }

    /// Load all messages for a session.
    pub async fn load_session_messages(
        &self,
        session_key: &str,
    ) -> anyhow::Result<Vec<MessageRow>> {
        let rows: Vec<SqliteRow> = sqlx::query(
            "SELECT role, content, timestamp, tools_used FROM session_messages WHERE session_key = $1 ORDER BY id ASC",
        )
        .bind(session_key)
        .fetch_all(&self.pool)
        .await?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in &rows {
            let role: String = row.get("role");
            let content: String = row.get("content");
            let timestamp_str: String = row.get("timestamp");
            let tools_json: Option<String> = row.get("tools_used");

            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            messages.push(MessageRow {
                role,
                content,
                timestamp,
                tools_used: tools_json,
            });
        }
        Ok(messages)
    }

    /// Clear all messages for a session (keep metadata). Also clears summary.
    pub async fn clear_session_messages(&self, session_key: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM session_messages WHERE session_key = $1")
            .bind(session_key)
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE sessions SET last_consolidated = 0, updated_at = $1 WHERE key = $2")
            .bind(Utc::now().to_rfc3339())
            .bind(session_key)
            .execute(&self.pool)
            .await?;
        // Also clear any associated summary
        self.delete_session_summary(session_key).await?;
        debug!("Cleared session messages: {}", session_key);
        Ok(())
    }

    /// Update last_consolidated for a session.
    pub async fn update_session_consolidated(
        &self,
        session_key: &str,
        last_consolidated: usize,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET last_consolidated = $1, updated_at = $2 WHERE key = $3")
            .bind(last_consolidated as i64)
            .bind(Utc::now().to_rfc3339())
            .bind(session_key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── Session Summary API ──

    /// Save or replace a session summary (upsert).
    pub async fn save_session_summary(
        &self,
        session_key: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        let created_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR REPLACE INTO session_summaries (session_key, content, created_at) VALUES ($1, $2, $3)",
        )
        .bind(session_key)
        .bind(content)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        debug!("Saved session summary: {}", session_key);
        Ok(())
    }

    /// Load a session summary.
    pub async fn load_session_summary(&self, session_key: &str) -> anyhow::Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT content FROM session_summaries WHERE session_key = $1")
                .bind(session_key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(c,)| c))
    }

    /// Delete a session summary.
    pub async fn delete_session_summary(&self, session_key: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM session_summaries WHERE session_key = $1")
            .bind(session_key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ── Cron Jobs API ──

    /// Save or update a cron job (O(1) operation).
    pub async fn save_cron_job(&self, job: &CronJobRow) -> anyhow::Result<()> {
        let last_run = job.last_run.map(|dt| dt.to_rfc3339());
        let next_run = job.next_run.map(|dt| dt.to_rfc3339());
        let enabled_int: i64 = if job.enabled { 1 } else { 0 };

        sqlx::query(
            "INSERT OR REPLACE INTO cron_jobs (id, name, cron, message, channel, chat_id, last_run, next_run, enabled)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&job.id)
        .bind(&job.name)
        .bind(&job.cron)
        .bind(&job.message)
        .bind(&job.channel)
        .bind(&job.chat_id)
        .bind(&last_run)
        .bind(&next_run)
        .bind(enabled_int)
        .execute(&self.pool)
        .await?;
        debug!("Saved cron job: {}", job.id);
        Ok(())
    }

    /// Load all cron jobs.
    pub async fn load_cron_jobs(&self) -> anyhow::Result<Vec<CronJobRow>> {
        let rows: Vec<SqliteRow> = sqlx::query(
            "SELECT id, name, cron, message, channel, chat_id, last_run, next_run, enabled FROM cron_jobs",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut jobs = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.get("id");
            let name: String = row.get("name");
            let cron: String = row.get("cron");
            let message: String = row.get("message");
            let channel: Option<String> = row.get("channel");
            let chat_id: Option<String> = row.get("chat_id");
            let last_run_str: Option<String> = row.get("last_run");
            let next_run_str: Option<String> = row.get("next_run");
            let enabled_int: i64 = row.get("enabled");

            let last_run = last_run_str
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            let next_run = next_run_str
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            jobs.push(CronJobRow {
                id,
                name,
                cron,
                message,
                channel,
                chat_id,
                last_run,
                next_run,
                enabled: enabled_int != 0,
            });
        }
        Ok(jobs)
    }

    /// Delete a cron job by ID. Returns true if the job existed.
    pub async fn delete_cron_job(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM cron_jobs WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

/// Cron job row for database storage
#[derive(Debug, Clone)]
pub struct CronJobRow {
    pub id: String,
    pub name: String,
    pub cron: String,
    pub message: String,
    pub channel: Option<String>,
    pub chat_id: Option<String>,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub enabled: bool,
}

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn save(&self, entry: &MemoryEntry) -> anyhow::Result<()> {
        let metadata_json = serde_json::to_string(&entry.metadata)?;
        let created = entry.created_at.to_rfc3339();
        let updated = entry.updated_at.to_rfc3339();

        sqlx::query(
            "INSERT OR REPLACE INTO memories (id, content, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&entry.id)
        .bind(&entry.content)
        .bind(&metadata_json)
        .bind(&created)
        .bind(&updated)
        .execute(&self.pool)
        .await?;

        // Sync tags: delete old, insert new
        sqlx::query("DELETE FROM memory_tags WHERE memory_id = $1")
            .bind(&entry.id)
            .execute(&self.pool)
            .await?;
        for tag in &entry.metadata.tags {
            sqlx::query("INSERT INTO memory_tags (memory_id, tag) VALUES ($1, $2)")
                .bind(&entry.id)
                .bind(tag)
                .execute(&self.pool)
                .await?;
        }

        debug!("Saved memory entry: {}", entry.id);
        Ok(())
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<MemoryEntry>> {
        let row: Option<SqliteRow> = sqlx::query(
            "SELECT id, content, metadata, created_at, updated_at FROM memories WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_entry(&row)?)),
            None => Ok(None),
        }
    }

    async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM memories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn search(&self, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>> {
        search_impl(&self.pool, query).await
    }
}

fn row_to_entry(row: &SqliteRow) -> anyhow::Result<MemoryEntry> {
    let id: String = row.get("id");
    let content: String = row.get("content");
    let metadata_json: String = row.get("metadata");
    let created_str: String = row.get("created_at");
    let updated_str: String = row.get("updated_at");

    let metadata: MemoryMetadata = serde_json::from_str(&metadata_json)?;
    let created_at = DateTime::parse_from_rfc3339(&created_str)?.with_timezone(&Utc);
    let updated_at = DateTime::parse_from_rfc3339(&updated_str)?.with_timezone(&Utc);

    Ok(MemoryEntry {
        id,
        content,
        metadata,
        created_at,
        updated_at,
    })
}

/// Build and execute the search query dynamically.
///
/// sqlx doesn't support dynamic parameter indexing like rusqlite's `?N`,
/// so we build the SQL with `$N` positional parameters and collect bind
/// values into typed vectors, then use `query_with` + `SqliteArguments`.
async fn search_impl(pool: &SqlitePool, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>> {
    // We use sqlx::query and bind dynamically via a helper approach.
    // Since sqlx's `Query::bind` is chained, we build SQL with $1..$N
    // and collect bind values in order.

    let mut sql = String::new();
    let mut bind_values: Vec<String> = Vec::new();
    let mut param_idx = 1u32;

    if let Some(text) = &query.text {
        sql.push_str(&format!(
            "SELECT m.id, m.content, m.metadata, m.created_at, m.updated_at \
             FROM memories m \
             JOIN memories_fts f ON m.id = f.id \
             WHERE f.content MATCH ${}",
            param_idx
        ));
        bind_values.push(text.clone());
        param_idx += 1;
    } else {
        sql.push_str(
            "SELECT m.id, m.content, m.metadata, m.created_at, m.updated_at \
             FROM memories m WHERE 1=1",
        );
    }

    // Filter by source
    if let Some(source) = &query.source {
        sql.push_str(&format!(
            " AND json_extract(m.metadata, '$.source') = ${}",
            param_idx
        ));
        bind_values.push(source.clone());
        param_idx += 1;
    }

    // Filter by tags (AND semantics: entry must have ALL tags)
    for tag in &query.tags {
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM memory_tags t WHERE t.memory_id = m.id AND t.tag = ${})",
            param_idx
        ));
        bind_values.push(tag.clone());
        param_idx += 1;
    }

    // Order by updated_at descending for deterministic results
    sql.push_str(" ORDER BY m.updated_at DESC");

    // Limit / offset
    if let Some(limit) = query.limit {
        sql.push_str(&format!(" LIMIT ${}", param_idx));
        bind_values.push(limit.to_string());
        param_idx += 1;
    }
    if let Some(offset) = query.offset {
        if query.limit.is_none() {
            sql.push_str(&format!(" LIMIT -1 OFFSET ${}", param_idx));
        } else {
            sql.push_str(&format!(" OFFSET ${}", param_idx));
        }
        bind_values.push(offset.to_string());
    }

    // Build the query with dynamic binds
    let mut q = sqlx::query(&sql);
    for val in &bind_values {
        q = q.bind(val);
    }

    let rows = q.fetch_all(pool).await?;

    let mut entries = Vec::with_capacity(rows.len());
    for row in &rows {
        entries.push(row_to_entry(row)?);
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn temp_store() -> SqliteStore {
        let path =
            std::env::temp_dir().join(format!("nanobot_sqlite_test_{}.db", uuid::Uuid::new_v4()));
        SqliteStore::with_path(path).await.unwrap()
    }

    fn make_entry(id: &str, content: &str) -> MemoryEntry {
        let now = Utc::now();
        MemoryEntry {
            id: id.to_string(),
            content: content.to_string(),
            metadata: MemoryMetadata::default(),
            created_at: now,
            updated_at: now,
        }
    }

    fn make_entry_with_meta(
        id: &str,
        content: &str,
        source: Option<&str>,
        tags: &[&str],
    ) -> MemoryEntry {
        let now = Utc::now();
        MemoryEntry {
            id: id.to_string(),
            content: content.to_string(),
            metadata: MemoryMetadata {
                source: source.map(|s| s.to_string()),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                extra: serde_json::Value::Null,
            },
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_sqlite_save_and_get() {
        let store = temp_store().await;
        let entry = make_entry("e1", "hello world");
        store.save(&entry).await.unwrap();

        let got = store.get("e1").await.unwrap().unwrap();
        assert_eq!(got.id, "e1");
        assert_eq!(got.content, "hello world");
    }

    #[tokio::test]
    async fn test_sqlite_save_overwrites() {
        let store = temp_store().await;
        store.save(&make_entry("e1", "v1")).await.unwrap();
        store.save(&make_entry("e1", "v2")).await.unwrap();

        let got = store.get("e1").await.unwrap().unwrap();
        assert_eq!(got.content, "v2");
    }

    #[tokio::test]
    async fn test_sqlite_get_nonexistent() {
        let store = temp_store().await;
        assert!(store.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sqlite_delete() {
        let store = temp_store().await;
        store.save(&make_entry("e1", "data")).await.unwrap();
        assert!(store.delete("e1").await.unwrap());
        assert!(!store.delete("e1").await.unwrap());
        assert!(store.get("e1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sqlite_fts5_search() {
        let store = temp_store().await;
        store
            .save(&make_entry("e1", "rust is a systems programming language"))
            .await
            .unwrap();
        store
            .save(&make_entry("e2", "python is great for data science"))
            .await
            .unwrap();
        store
            .save(&make_entry("e3", "rust and python are both popular"))
            .await
            .unwrap();

        let results = store
            .search(&MemoryQuery {
                text: Some("rust".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        let ids: Vec<&str> = results.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"e1"));
        assert!(ids.contains(&"e3"));
    }

    #[tokio::test]
    async fn test_sqlite_search_by_tags() {
        let store = temp_store().await;
        store
            .save(&make_entry_with_meta("e1", "a", None, &["rust", "lang"]))
            .await
            .unwrap();
        store
            .save(&make_entry_with_meta("e2", "b", None, &["rust"]))
            .await
            .unwrap();
        store
            .save(&make_entry_with_meta("e3", "c", None, &["python"]))
            .await
            .unwrap();

        let results = store
            .search(&MemoryQuery {
                tags: vec!["rust".to_string()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        // AND semantics
        let results = store
            .search(&MemoryQuery {
                tags: vec!["rust".to_string(), "lang".to_string()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "e1");
    }

    #[tokio::test]
    async fn test_sqlite_search_by_source() {
        let store = temp_store().await;
        store
            .save(&make_entry_with_meta("e1", "a", Some("user"), &[]))
            .await
            .unwrap();
        store
            .save(&make_entry_with_meta("e2", "b", Some("agent"), &[]))
            .await
            .unwrap();

        let results = store
            .search(&MemoryQuery {
                source: Some("user".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "e1");
    }

    #[tokio::test]
    async fn test_sqlite_search_limit_offset() {
        let store = temp_store().await;
        for i in 0..5 {
            store
                .save(&make_entry(&format!("e{}", i), &format!("content {}", i)))
                .await
                .unwrap();
        }

        let results = store
            .search(&MemoryQuery {
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        let all = store.search(&MemoryQuery::default()).await.unwrap();
        assert_eq!(all.len(), 5);
    }

    #[tokio::test]
    async fn test_sqlite_search_empty_returns_all() {
        let store = temp_store().await;
        store.save(&make_entry("e1", "a")).await.unwrap();
        store.save(&make_entry("e2", "b")).await.unwrap();

        let results = store.search(&MemoryQuery::default()).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_sqlite_metadata_extra_preserved() {
        let store = temp_store().await;
        let now = Utc::now();
        let entry = MemoryEntry {
            id: "e1".to_string(),
            content: "test".to_string(),
            metadata: MemoryMetadata {
                source: Some("user".to_string()),
                tags: vec!["a".to_string()],
                extra: serde_json::json!({"key": "value", "num": 42}),
            },
            created_at: now,
            updated_at: now,
        };

        store.save(&entry).await.unwrap();
        let got = store.get("e1").await.unwrap().unwrap();
        assert_eq!(got.metadata.extra["key"], "value");
        assert_eq!(got.metadata.extra["num"], 42);
        assert_eq!(got.metadata.source.as_deref(), Some("user"));
        assert_eq!(got.metadata.tags, vec!["a".to_string()]);
    }

    #[tokio::test]
    async fn test_sqlite_persistence() {
        let path = std::env::temp_dir().join(format!(
            "nanobot_sqlite_persist_{}.db",
            uuid::Uuid::new_v4()
        ));

        // Write with first store instance
        {
            let store = SqliteStore::with_path(path.clone()).await.unwrap();
            store.save(&make_entry("e1", "persisted")).await.unwrap();
        }

        // Read with second store instance
        {
            let store = SqliteStore::with_path(path.clone()).await.unwrap();
            let got = store.get("e1").await.unwrap().unwrap();
            assert_eq!(got.content, "persisted");
        }

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn test_sqlite_concurrent_access() {
        let store = Arc::new(temp_store().await);

        let mut handles = vec![];
        for i in 0..10 {
            let store = store.clone();
            let handle = tokio::spawn(async move {
                let entry = make_entry(&format!("e{}", i), &format!("content {}", i));
                store.save(&entry).await.unwrap();
                let got = store.get(&format!("e{}", i)).await.unwrap();
                assert!(got.is_some());
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let all = store.search(&MemoryQuery::default()).await.unwrap();
        assert_eq!(all.len(), 10);
    }

    // ── History tests ──

    #[tokio::test]
    async fn test_sqlite_history_append_and_read() {
        let store = temp_store().await;

        store.append_history("First entry\n").await.unwrap();
        store.append_history("Second entry\n").await.unwrap();

        let history = store.read_history().await.unwrap();
        assert!(history.contains("First entry"));
        assert!(history.contains("Second entry"));
    }

    #[tokio::test]
    async fn test_sqlite_history_read_empty() {
        let store = temp_store().await;
        let history = store.read_history().await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_history_write() {
        let store = temp_store().await;

        store.append_history("Old entry\n").await.unwrap();
        store.write_history("New content\n").await.unwrap();

        let history = store.read_history().await.unwrap();
        assert!(!history.contains("Old entry"));
        assert!(history.contains("New content"));
    }

    #[tokio::test]
    async fn test_sqlite_history_clear() {
        let store = temp_store().await;

        store.append_history("Entry 1\n").await.unwrap();
        store.append_history("Entry 2\n").await.unwrap();

        store.clear_history().await.unwrap();

        let history = store.read_history().await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_history_persistence() {
        let path = std::env::temp_dir().join(format!(
            "nanobot_sqlite_history_{}.db",
            uuid::Uuid::new_v4()
        ));

        // Write with first store instance
        {
            let store = SqliteStore::with_path(path.clone()).await.unwrap();
            store.append_history("Persisted history\n").await.unwrap();
        }

        // Read with second store instance
        {
            let store = SqliteStore::with_path(path.clone()).await.unwrap();
            let history = store.read_history().await.unwrap();
            assert!(history.contains("Persisted history"));
        }

        let _ = std::fs::remove_file(path);
    }

    // ── Key-value store tests ──

    #[tokio::test]
    async fn test_sqlite_kv_read_write() {
        let store = temp_store().await;

        store.write_raw("MEMORY.md", "# Memory").await.unwrap();
        assert_eq!(
            store.read_raw("MEMORY.md").await.unwrap(),
            Some("# Memory".to_string())
        );

        assert!(store.delete_raw("MEMORY.md").await.unwrap());
        assert_eq!(store.read_raw("MEMORY.md").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_sqlite_kv_upsert() {
        let store = temp_store().await;

        store.write_raw("key1", "v1").await.unwrap();
        store.write_raw("key1", "v2").await.unwrap();

        assert_eq!(
            store.read_raw("key1").await.unwrap(),
            Some("v2".to_string())
        );
    }

    #[tokio::test]
    async fn test_sqlite_kv_nonexistent() {
        let store = temp_store().await;
        assert_eq!(store.read_raw("nope").await.unwrap(), None);
    }

    // ── Session tests ──

    #[tokio::test]
    async fn test_sqlite_session_meta_and_messages() {
        let store = temp_store().await;

        // Test session metadata
        store.save_session_meta("test:123", 0).await.unwrap();
        let meta = store.load_session_meta("test:123").await.unwrap();
        assert!(meta.is_some());
        assert_eq!(meta.unwrap().key, "test:123");

        // Test appending messages
        let ts1 = Utc::now();
        store
            .append_session_message("test:123", "user", "Hello", &ts1, None)
            .await
            .unwrap();
        let ts2 = Utc::now();
        store
            .append_session_message(
                "test:123",
                "assistant",
                "Hi!",
                &ts2,
                Some(&["tool1".to_string()]),
            )
            .await
            .unwrap();

        let messages = store.load_session_messages("test:123").await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Hello");
        assert_eq!(messages[1].role, "assistant");
        // tools_used is stored as JSON array string
        assert_eq!(messages[1].tools_used, Some("[\"tool1\"]".to_string()));
    }

    #[tokio::test]
    async fn test_sqlite_session_upsert_meta() {
        let store = temp_store().await;

        // First insert
        store.save_session_meta("key1", 5).await.unwrap();
        let meta1 = store.load_session_meta("key1").await.unwrap();
        assert_eq!(meta1.unwrap().last_consolidated, 5);

        // Update (upsert)
        store.save_session_meta("key1", 10).await.unwrap();
        let meta2 = store.load_session_meta("key1").await.unwrap();
        assert_eq!(meta2.unwrap().last_consolidated, 10);
    }

    #[tokio::test]
    async fn test_sqlite_session_delete() {
        let store = temp_store().await;

        // Create session with metadata and messages
        store.save_session_meta("key1", 0).await.unwrap();
        let ts = Utc::now();
        store
            .append_session_message("key1", "user", "test", &ts, None)
            .await
            .unwrap();

        // Verify session exists
        assert!(store.load_session_meta("key1").await.unwrap().is_some());
        assert!(!store
            .load_session_messages("key1")
            .await
            .unwrap()
            .is_empty());

        // Delete and verify
        assert!(store.delete_session("key1").await.unwrap());
        assert!(!store.delete_session("key1").await.unwrap());
        assert!(store.load_session_meta("key1").await.unwrap().is_none());
        // Messages should be cascade deleted
        assert!(store
            .load_session_messages("key1")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_session_nonexistent() {
        let store = temp_store().await;
        assert!(store.load_session_meta("nope").await.unwrap().is_none());
        assert!(store
            .load_session_messages("nope")
            .await
            .unwrap()
            .is_empty());
    }

    // ── Session Summary tests ──

    #[tokio::test]
    async fn test_sqlite_session_summary_save_and_load() {
        let store = temp_store().await;

        // No summary initially
        assert!(store
            .load_session_summary("test:123")
            .await
            .unwrap()
            .is_none());

        // Save summary
        store
            .save_session_summary("test:123", "This is a summary of the conversation.")
            .await
            .unwrap();

        let summary = store.load_session_summary("test:123").await.unwrap();
        assert_eq!(
            summary,
            Some("This is a summary of the conversation.".to_string())
        );
    }

    #[tokio::test]
    async fn test_sqlite_session_summary_upsert() {
        let store = temp_store().await;

        store
            .save_session_summary("key1", "Summary v1")
            .await
            .unwrap();
        store
            .save_session_summary("key1", "Summary v2")
            .await
            .unwrap();

        let summary = store.load_session_summary("key1").await.unwrap();
        assert_eq!(summary, Some("Summary v2".to_string()));
    }

    #[tokio::test]
    async fn test_sqlite_session_summary_delete() {
        let store = temp_store().await;

        store.save_session_summary("key1", "Summary").await.unwrap();
        assert!(store.delete_session_summary("key1").await.unwrap());
        assert!(!store.delete_session_summary("key1").await.unwrap());
        assert!(store.load_session_summary("key1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sqlite_clear_session_messages_clears_summary() {
        let store = temp_store().await;

        // Create session with metadata, messages, and summary
        store.save_session_meta("key1", 0).await.unwrap();
        let ts = Utc::now();
        store
            .append_session_message("key1", "user", "test", &ts, None)
            .await
            .unwrap();
        store
            .save_session_summary("key1", "Old summary")
            .await
            .unwrap();

        // Clear messages should also clear summary
        store.clear_session_messages("key1").await.unwrap();
        assert!(store
            .load_session_messages("key1")
            .await
            .unwrap()
            .is_empty());
        assert!(store.load_session_summary("key1").await.unwrap().is_none());
    }
}
