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

mod event_store;
pub mod fs;
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

use gasket_types::SessionKey;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tracing::debug;

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

    /// Save or replace a session summary with its sequence watermark (upsert).
    ///
    /// The `covered_upto_sequence` is the high-water mark: all events with
    /// `sequence <= covered_upto_sequence` are covered by this summary and
    /// can be safely garbage-collected.
    pub async fn save_session_summary(
        &self,
        session_key: &SessionKey,
        content: &str,
        covered_upto_sequence: i64,
    ) -> anyhow::Result<()> {
        let key_str = session_key.to_string();
        let created_at = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR REPLACE INTO session_summaries (session_key, content, covered_upto_sequence, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&key_str)
        .bind(content)
        .bind(covered_upto_sequence)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;
        debug!(
            "Saved session summary for {}: covering up to sequence {}",
            session_key, covered_upto_sequence
        );
        Ok(())
    }

    /// Load a session summary and its sequence watermark.
    ///
    /// Returns `Some((content, covered_upto_sequence))` if a summary exists,
    /// or `None` if no summary has been generated for this session yet.
    pub async fn load_session_summary(
        &self,
        session_key: &SessionKey,
    ) -> anyhow::Result<Option<(String, i64)>> {
        let key_str = session_key.to_string();
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT content, covered_upto_sequence FROM session_summaries WHERE session_key = $1",
        )
        .bind(&key_str)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Delete a session summary.
    pub async fn delete_session_summary(&self, session_key: &SessionKey) -> anyhow::Result<bool> {
        let key_str = session_key.to_string();
        let result = sqlx::query("DELETE FROM session_summaries WHERE session_key = $1")
            .bind(&key_str)
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
            LEFT JOIN session_events m ON e.message_id = m.id
            WHERE e.session_key = $1
            ORDER BY m.sequence ASC
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

    // ── Cron State API (Execution state for cron jobs) ──

    /// Get cron state for a job.
    ///
    /// Returns `(last_run_at, next_run_at)` if state exists, or `None` if not found.
    pub async fn get_cron_state(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<(Option<String>, Option<String>)>> {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT last_run_at, next_run_at FROM cron_state WHERE job_id = $1")
                .bind(job_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    /// Upsert cron state for a job.
    ///
    /// Persists execution state (last_run/next_run) to survive restarts.
    pub async fn upsert_cron_state(
        &self,
        job_id: &str,
        last_run: Option<&str>,
        next_run: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO cron_state (job_id, last_run_at, next_run_at) VALUES ($1, $2, $3)",
        )
        .bind(job_id)
        .bind(last_run)
        .bind(next_run)
        .execute(&self.pool)
        .await?;
        debug!(
            "Updated cron state for job {}: last_run={:?}, next_run={:?}",
            job_id, last_run, next_run
        );
        Ok(())
    }

    /// Delete cron state for a job.
    ///
    /// Call when a job is removed to keep database clean.
    pub async fn delete_cron_state(&self, job_id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM cron_state WHERE job_id = $1")
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() > 0 {
            debug!("Deleted cron state for job {}", job_id);
        }
        Ok(result.rows_affected() > 0)
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
    /// - `sessions_v2` / `session_events` / `session_summaries` — conversation history
    /// - `cron_state` — scheduled tasks
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

        // ── Session summaries ──
        // Stores the rolling summary with a sequence watermark (high-water mark)
        // indicating which events are already covered by the summary.
        // Events with sequence <= covered_upto_sequence can be garbage-collected.

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_summaries (
                session_key            TEXT PRIMARY KEY,
                content                TEXT NOT NULL,
                covered_upto_sequence  INTEGER NOT NULL DEFAULT 0,
                created_at             TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Cron state (execution state, separate from config) ──
        // Config lives in ~/.gasket/cron/*.md files (SSOT)
        // State (last_run/next_run) lives here (high-frequency writes)

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS cron_state (
                job_id      TEXT PRIMARY KEY,
                last_run_at TEXT,
                next_run_at TEXT
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cron_state_next_run ON cron_state(next_run_at)",
        )
        .execute(&self.pool)
        .await?;

        // ── Session embeddings (for semantic history recall) ──

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_embeddings (
                message_id  TEXT PRIMARY KEY,
                session_key TEXT NOT NULL,
                embedding   BLOB NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (session_key) REFERENCES sessions_v2(key) ON DELETE CASCADE
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
                access_count BIGINT NOT NULL DEFAULT 0,
                file_mtime  BIGINT NOT NULL DEFAULT 0,
                file_size   BIGINT NOT NULL DEFAULT 0,
                needs_embedding INTEGER NOT NULL DEFAULT 1,
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

        // Session metadata table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions_v2 (
                key             TEXT PRIMARY KEY,
                channel         TEXT NOT NULL DEFAULT '',
                chat_id         TEXT NOT NULL DEFAULT '',
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
                channel         TEXT NOT NULL DEFAULT '',
                chat_id         TEXT NOT NULL DEFAULT '',
                event_type      TEXT NOT NULL,
                content         TEXT NOT NULL,
                embedding       BLOB,
                tools_used      TEXT DEFAULT '[]',
                token_usage     TEXT,
                token_len       INTEGER NOT NULL DEFAULT 0,
                event_data      TEXT,
                extra           TEXT DEFAULT '{}',
                created_at      TEXT NOT NULL,
                sequence        INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (session_key) REFERENCES sessions_v2(key) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Indexes for session_events

        // Indexes for channel/chat_id queries
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions_v2(channel)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_sessions_chat_id ON sessions_v2(chat_id)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_channel ON session_events(channel)")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_events_chat_id ON session_events(chat_id)")
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
             ON session_events(session_key, event_type, created_at DESC)",
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

        // ── Migrations for existing databases ──

        // Add covered_upto_sequence column to session_summaries if it doesn't exist.
        // SQLite ALTER TABLE ADD COLUMN is safe — it's a no-op if the column already exists
        // in modern SQLite versions, but we guard with a pragma check for older versions.
        let has_watermark: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('session_summaries') WHERE name = 'covered_upto_sequence'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if !has_watermark {
            sqlx::query(
                "ALTER TABLE session_summaries ADD COLUMN covered_upto_sequence INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await?;
        }

        // Add sequence column to session_events if it doesn't exist (migration for older DBs).
        let has_sequence: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('session_events') WHERE name = 'sequence'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if !has_sequence {
            sqlx::query(
                "ALTER TABLE session_events ADD COLUMN sequence INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await?;
        }

        // Add needs_embedding column to memory_metadata if it doesn't exist.
        let has_needs_embedding: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('memory_metadata') WHERE name = 'needs_embedding'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if !has_needs_embedding {
            sqlx::query(
                "ALTER TABLE memory_metadata ADD COLUMN needs_embedding INTEGER NOT NULL DEFAULT 1",
            )
            .execute(&self.pool)
            .await?;
        }

        // Add access_count column to memory_metadata if it doesn't exist.
        // This column stores the machine runtime state (access tracking) that was
        // previously written to Markdown frontmatter. Now SQLite is the sole source of truth.
        let has_access_count: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('memory_metadata') WHERE name = 'access_count'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        if !has_access_count {
            sqlx::query(
                "ALTER TABLE memory_metadata ADD COLUMN access_count BIGINT NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await?;
        }

        // Index for efficient watermark-based queries on session_events
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_events_session_sequence ON session_events(session_key, sequence)",
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
        let key = SessionKey::new(gasket_types::ChannelType::Cli, "test:123");

        assert!(store.load_session_summary(&key).await.unwrap().is_none());

        store
            .save_session_summary(&key, "This is a summary of the conversation.", 42)
            .await
            .unwrap();

        let summary = store.load_session_summary(&key).await.unwrap();
        assert_eq!(
            summary,
            Some(("This is a summary of the conversation.".to_string(), 42))
        );
    }

    #[tokio::test]
    async fn test_sqlite_session_summary_upsert() {
        let store = temp_store().await;
        let key = SessionKey::new(gasket_types::ChannelType::Cli, "key1");

        store
            .save_session_summary(&key, "Summary v1", 10)
            .await
            .unwrap();
        store
            .save_session_summary(&key, "Summary v2", 20)
            .await
            .unwrap();

        let summary = store.load_session_summary(&key).await.unwrap();
        assert_eq!(summary, Some(("Summary v2".to_string(), 20)));
    }

    #[tokio::test]
    async fn test_sqlite_session_summary_delete() {
        let store = temp_store().await;
        let key = SessionKey::new(gasket_types::ChannelType::Cli, "key1");

        store
            .save_session_summary(&key, "Summary", 5)
            .await
            .unwrap();
        assert!(store.delete_session_summary(&key).await.unwrap());
        assert!(!store.delete_session_summary(&key).await.unwrap());
        assert!(store.load_session_summary(&key).await.unwrap().is_none());
    }
}
