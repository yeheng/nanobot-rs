//! SQLite-backed store for machine-state persistence.
//!
//! The `SqliteStore` is split across several files for clarity:
//! - `mod.rs` — core struct, construction, schema migration
//! - `kv.rs` — key-value store API
//! - `session.rs` — session metadata, messages, and summaries API
//! - `cron.rs` — cron job persistence API
//!
//! **Note:** Explicit long-term memory (facts, preferences, decisions) lives
//! exclusively in `~/.nanobot/memory/*.md` files. SQLite only stores
//! machine-state (sessions, summaries, cron jobs, kv).

mod cron;
mod kv;
mod session;

use std::path::PathBuf;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use tracing::debug;

pub use cron::CronJobRow;
#[allow(unused_imports)]
pub use session::{MessageRow, SessionMeta};

/// SQLite-backed store for machine-state persistence.
///
/// Stores sessions, summaries, cron jobs, and key-value pairs in a
/// single SQLite database file. Uses `sqlx::SqlitePool` for native async
/// I/O without blocking the tokio runtime.
///
/// **Not** used for explicit long-term memory — that lives in Markdown files.
#[derive(Clone)]
pub struct SqliteStore {
    pub(crate) pool: SqlitePool,
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
    ///
    /// Explicit long-term memory lives exclusively in `~/.nanobot/memory/*.md` files
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

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    pub(crate) async fn temp_store() -> SqliteStore {
        let path =
            std::env::temp_dir().join(format!("nanobot_sqlite_test_{}.db", uuid::Uuid::new_v4()));
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

    // ── Session tests ──

    #[tokio::test]
    async fn test_sqlite_session_meta_and_messages() {
        let store = temp_store().await;

        store.save_session_meta("test:123", 0).await.unwrap();
        let meta = store.load_session_meta("test:123").await.unwrap();
        assert!(meta.is_some());
        assert_eq!(meta.unwrap().key, "test:123");

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
        assert_eq!(messages[1].tools_used, Some("[\"tool1\"]".to_string()));
    }

    #[tokio::test]
    async fn test_sqlite_session_upsert_meta() {
        let store = temp_store().await;

        store.save_session_meta("key1", 5).await.unwrap();
        let meta1 = store.load_session_meta("key1").await.unwrap();
        assert_eq!(meta1.unwrap().last_consolidated, 5);

        store.save_session_meta("key1", 10).await.unwrap();
        let meta2 = store.load_session_meta("key1").await.unwrap();
        assert_eq!(meta2.unwrap().last_consolidated, 10);
    }

    #[tokio::test]
    async fn test_sqlite_session_delete() {
        let store = temp_store().await;

        store.save_session_meta("key1", 0).await.unwrap();
        let ts = Utc::now();
        store
            .append_session_message("key1", "user", "test", &ts, None)
            .await
            .unwrap();

        assert!(store.load_session_meta("key1").await.unwrap().is_some());
        assert!(!store
            .load_session_messages("key1")
            .await
            .unwrap()
            .is_empty());

        assert!(store.delete_session("key1").await.unwrap());
        assert!(!store.delete_session("key1").await.unwrap());
        assert!(store.load_session_meta("key1").await.unwrap().is_none());
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

    #[tokio::test]
    async fn test_sqlite_clear_session_messages_clears_summary() {
        let store = temp_store().await;

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

        store.clear_session_messages("key1").await.unwrap();
        assert!(store
            .load_session_messages("key1")
            .await
            .unwrap()
            .is_empty());
        assert!(store.load_session_summary("key1").await.unwrap().is_none());
    }
}
