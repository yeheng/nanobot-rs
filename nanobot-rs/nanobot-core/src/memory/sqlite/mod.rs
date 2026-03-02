//! SQLite-backed memory store with FTS5 full-text search.
//!
//! The `SqliteStore` is split across several files for clarity:
//! - `mod.rs` — core struct, construction, schema migration
//! - `memories.rs` — `MemoryStore` trait implementation (FTS5 search)
//! - `kv.rs` — key-value store API
//! - `session.rs` — session metadata, messages, and summaries API
//! - `cron.rs` — cron job persistence API

mod cron;
mod kv;
mod memories;
mod session;

use std::path::PathBuf;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;
use tracing::debug;

pub use cron::CronJobRow;
#[allow(unused_imports)]
pub use session::{MessageRow, SessionMeta};

/// SQLite-backed memory store with FTS5 full-text search.
///
/// Persists memory entries, long-term memory, and sessions in a
/// single SQLite database file. Uses `sqlx::SqlitePool` for native async
/// I/O without blocking the tokio runtime.
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
    async fn init_db(&self) -> anyhow::Result<()> {
        // ── Memories tables ──

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
    use crate::memory::store::{MemoryEntry, MemoryMetadata, MemoryQuery, MemoryStore};
    use chrono::Utc;
    use std::sync::Arc;

    pub(crate) async fn temp_store() -> SqliteStore {
        let path =
            std::env::temp_dir().join(format!("nanobot_sqlite_test_{}.db", uuid::Uuid::new_v4()));
        SqliteStore::with_path(path).await.unwrap()
    }

    pub(crate) fn make_entry(id: &str, content: &str) -> MemoryEntry {
        let now = Utc::now();
        MemoryEntry {
            id: id.to_string(),
            content: content.to_string(),
            metadata: MemoryMetadata::default(),
            created_at: now,
            updated_at: now,
        }
    }

    pub(crate) fn make_entry_with_meta(
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

    // ── Memory (FTS5) tests ──

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

        {
            let store = SqliteStore::with_path(path.clone()).await.unwrap();
            store.save(&make_entry("e1", "persisted")).await.unwrap();
        }

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
