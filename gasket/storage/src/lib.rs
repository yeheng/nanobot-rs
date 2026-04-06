//! SQLite-backed storage, history processing, and semantic embedding for gasket.
//!
//! This crate provides:
//! - **Persistence:** Sessions, conversation messages, summaries, cron jobs, key-value store
//! - **History:** Token-budget-aware history truncation and multi-dimensional retrieval
//! - **Search:** Full-text search types and semantic embedding
//! - **Vector math:** Cosine similarity and top-K retrieval
//!
//! **Note:** Explicit long-term memory (facts, preferences, decisions) lives
//! exclusively in `~/.gasket/memory/*.md` files. SQLite only stores
//! machine-state.

mod cron;
mod event_store;
mod kv;
pub mod memory;

// ── Merged from gasket-history ──
pub mod processor;
pub mod query;
pub mod search;

// ── Merged from gasket-semantic ──
#[cfg(feature = "local-embedding")]
mod embedder;
mod vector_math;

use std::path::PathBuf;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tracing::debug;

pub use cron::CronJobRow;
pub use event_store::{EventFilter, EventStore, EventStoreTrait, StoreError};

// ── History re-exports ──
pub use processor::{count_tokens, process_history, HistoryConfig, ProcessedHistory};
pub use query::{
    HistoryQuery, HistoryQueryBuilder, HistoryResult, HistoryRetriever, QueryOrder, ResultMeta,
    SemanticQuery, TimeRange,
};

// ── Semantic re-exports (always available) ──
pub use vector_math::{cosine_similarity, top_k_similar};

// ── Semantic re-exports (feature-gated) ──
#[cfg(feature = "local-embedding")]
pub use embedder::{EmbeddingConfig, TextEmbedder, DEFAULT_CACHE_DIR, DEFAULT_MODEL};
#[cfg(feature = "local-embedding")]
pub use vector_math::{bytes_to_embedding, embedding_to_bytes};

// Re-export sqlx types for consumers that need direct pool access
pub use sqlx::sqlite::SqliteRow;
pub use sqlx::{query as sql_query, query_as, Row, SqlitePool};

/// Get the default configuration directory (`~/.gasket`).
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gasket")
}

/// SQLite-backed store for machine-state persistence.
///
/// Stores sessions, summaries, cron jobs, and key-value pairs in a
/// single SQLite database file. Uses `sqlx::SqlitePool` for native async
/// I/O without blocking the tokio runtime.
///
/// **Not** used for explicit long-term memory — that lives in Markdown files.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Create a new `SqliteStore` with the default database path
    /// (`~/.gasket/gasket.db`).
    pub async fn new() -> anyhow::Result<Self> {
        let path = config_dir().join("gasket.db");
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

        // Pool size rationale: the gateway uses per-session Actor serialization
        // (each session has a dedicated actor that processes messages one at a time),
        // so typical concurrent SQLite access equals the number of *active sessions*
        // (not total requests). 5 connections comfortably handles most workloads;
        // WAL mode further reduces contention by allowing concurrent readers.
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

    /// Get a clone of the underlying SQLite pool.
    ///
    /// Useful for sharing the pool with other subsystems (e.g., pipeline).
    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    /// Create a `SqliteStore` from an existing pool (no migrations).
    ///
    /// Intended for test setups and internal use where the caller already
    /// has a configured pool. Does NOT run migrations — the caller is
    /// responsible for ensuring the schema exists.
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    // ── Session Summary API ──

    /// Save or replace a session summary (upsert).
    pub async fn save_session_summary(
        &self,
        session_key: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        let created_at = chrono::Utc::now().to_rfc3339();
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

    // ── Session Embeddings API (Semantic History Recall) ──

    /// Save an embedding for a message.
    ///
    /// The embedding is stored as a BLOB using bytemuck for zero-copy
    /// conversion between `[f32]` and `[u8]`.
    pub async fn save_embedding(
        &self,
        message_id: &str,
        session_key: &str,
        embedding: &[f32],
    ) -> anyhow::Result<()> {
        let embedding_bytes = bytemuck::cast_slice(embedding);
        sqlx::query(
            "INSERT OR REPLACE INTO session_embeddings (message_id, session_key, embedding) VALUES ($1, $2, $3)",
        )
        .bind(message_id)
        .bind(session_key)
        .bind(embedding_bytes)
        .execute(&self.pool)
        .await?;
        debug!("Saved embedding for message: {}", message_id);
        Ok(())
    }

    /// Load all embeddings for a session.
    ///
    /// Returns a vector of `(message_id, content, embedding)` tuples.
    /// The embedding is converted back from BLOB to `Vec<f32>` using bytemuck.
    pub async fn load_session_embeddings(
        &self,
        session_key: &str,
    ) -> anyhow::Result<Vec<(String, String, Vec<f32>)>> {
        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
            r#"
            SELECT e.message_id, m.content, e.embedding
            FROM session_embeddings e
            LEFT JOIN session_messages m ON e.message_id = CAST(m.id AS TEXT)
            WHERE e.session_key = $1
            ORDER BY m.id ASC
            "#,
        )
        .bind(session_key)
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let message_id: String = row.get("message_id");
            let content: String = row.get("content");
            let embedding_blob: Vec<u8> = row.get("embedding");

            // Convert BLOB back to Vec<f32>
            let embedding = bytemuck::cast_slice(&embedding_blob).to_vec();
            result.push((message_id, content, embedding));
        }
        Ok(result)
    }

    /// Check whether an embedding already exists for a given message.
    ///
    /// Used by the summarization layer to skip redundant embedding
    /// computation when the same event is evicted more than once
    /// (e.g. during repeated context compression).
    pub async fn has_embedding(&self, message_id: &str) -> anyhow::Result<bool> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(*) FROM session_embeddings WHERE message_id = $1")
                .bind(message_id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|(count,)| count > 0).unwrap_or(false))
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

    /// Create all tables, indexes, triggers, and virtual tables.
    ///
    /// Only machine-state tables are created here:
    /// - `kv_store` — generic key-value persistence
    /// - `sessions` / `session_messages` / `session_summaries` — conversation history
    /// - `cron_jobs` — scheduled tasks
    /// - `session_embeddings` — semantic history recall
    ///
    /// Explicit long-term memory lives exclusively in `~/.gasket/memory/*.md` files
    /// (Single Source of Truth — no SQLite `memories` table).
    async fn init_db(&self) -> anyhow::Result<()> {
        // ── Key-value store ──

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS kv_store (
                key         TEXT PRIMARY KEY,
                value       TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Sessions tables ──

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

        // ── Session summaries ──

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_summaries (
                session_key TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                created_at  TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Cron jobs ──

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

        // ── Session embeddings (for semantic history recall) ──

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_embeddings (
                message_id  TEXT PRIMARY KEY,
                session_key TEXT NOT NULL,
                embedding   BLOB NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (session_key) REFERENCES sessions(key) ON DELETE CASCADE
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_session_embeddings_session_key
             ON session_embeddings(session_key)",
        )
        .execute(&self.pool)
        .await?;

        // ── Memory system tables ──

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memory_embeddings (
                memory_path   TEXT PRIMARY KEY,
                scenario      TEXT NOT NULL,
                tags          TEXT,
                frequency     TEXT NOT NULL DEFAULT 'warm',
                embedding     BLOB NOT NULL,
                token_count   INTEGER NOT NULL,
                created_at    TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_mem_emb_scenario
             ON memory_embeddings(scenario)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_mem_emb_frequency
             ON memory_embeddings(frequency)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memory_metadata (
                id          TEXT NOT NULL,
                path        TEXT NOT NULL,
                scenario    TEXT NOT NULL,
                title       TEXT NOT NULL DEFAULT '',
                memory_type TEXT NOT NULL DEFAULT 'note',
                frequency   TEXT NOT NULL DEFAULT 'warm',
                tags        TEXT NOT NULL DEFAULT '[]',
                tokens      INTEGER NOT NULL DEFAULT 0,
                updated     TEXT NOT NULL DEFAULT '',
                last_accessed TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (scenario, path)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_meta_scenario_freq
             ON memory_metadata(scenario, frequency)",
        )
        .execute(&self.pool)
        .await?;

        // === Event sourcing new tables ===

        // Session metadata table (v2 with branch support)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions_v2 (
                key             TEXT PRIMARY KEY,
                current_branch  TEXT NOT NULL DEFAULT 'main',
                branches        TEXT NOT NULL DEFAULT '{}',
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL,
                last_consolidated_event TEXT,
                total_events    INTEGER NOT NULL DEFAULT 0,
                total_tokens    INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Event table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS session_events (
                id              TEXT PRIMARY KEY,
                session_key     TEXT NOT NULL,
                event_type      TEXT NOT NULL,
                content         TEXT NOT NULL,
                embedding       BLOB,
                branch          TEXT DEFAULT 'main',
                tools_used      TEXT DEFAULT '[]',
                token_usage     TEXT,
                token_len       INTEGER NOT NULL DEFAULT 0,
                event_data      TEXT,
                extra           TEXT DEFAULT '{}',
                created_at      TEXT NOT NULL,
                FOREIGN KEY (session_key) REFERENCES sessions_v2(key) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Indexes for session_events
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_events_session_branch ON session_events(session_key, branch)",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_created ON session_events(created_at)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_type ON session_events(event_type)")
            .execute(&self.pool)
            .await?;

        // Covering index for get_latest_summary: single seek, no table scan
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_events_session_type_created \
             ON session_events(session_key, branch, event_type, created_at DESC)",
        )
        .execute(&self.pool)
        .await?;

        // Summary index table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS summary_index (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key     TEXT NOT NULL,
                event_id        TEXT NOT NULL,
                summary_type    TEXT NOT NULL,
                topic           TEXT,
                covered_events  TEXT NOT NULL,
                created_at      TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_summary_session ON summary_index(session_key)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_summary_type ON summary_index(summary_type)")
            .execute(&self.pool)
            .await?;

        // ── Materialization engine tables ──

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS failed_events (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id      TEXT NOT NULL,
                handler_name  TEXT NOT NULL,
                error_text    TEXT NOT NULL,
                retry_count   INTEGER DEFAULT 0,
                max_retries   INTEGER DEFAULT 5,
                next_retry_at TEXT NOT NULL,
                dead_letter   INTEGER DEFAULT 0,
                created_at    TEXT DEFAULT (datetime('now'))
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_failed_events_dedup
             ON failed_events(event_id, handler_name)",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) async fn temp_store() -> SqliteStore {
        let path =
            std::env::temp_dir().join(format!("gasket_sqlite_test_{}.db", uuid::Uuid::new_v4()));
        SqliteStore::with_path(path).await.unwrap()
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

    // ── Session Summary tests ──

    #[tokio::test]
    async fn test_sqlite_session_summary_save_and_load() {
        let store = temp_store().await;

        assert!(store
            .load_session_summary("test:123")
            .await
            .unwrap()
            .is_none());

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
}
