//! SQLite-backed memory store with FTS5 full-text search.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use tokio::sync::Mutex;
use tracing::debug;

use super::store::{MemoryEntry, MemoryMetadata, MemoryQuery, MemoryStore};

/// SQLite-backed memory store with FTS5 full-text search.
///
/// Persists memory entries, history, long-term memory, and sessions in a
/// single SQLite database file. Uses a single `Connection` behind a
/// `tokio::sync::Mutex` for async safety.
#[derive(Clone)]
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
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
    /// (`~/.nanobot/memory.db`).
    pub fn new() -> anyhow::Result<Self> {
        let path = crate::config::config_dir().join("memory.db");
        Self::with_path(path)
    }

    /// Create a new `SqliteStore` with a custom database path.
    pub fn with_path(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        Self::init_db(&conn)?;
        debug!("Opened SqliteStore at {:?}", path);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn init_db(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS memories (
                id          TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                metadata    TEXT NOT NULL DEFAULT '{}',
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS memory_tags (
                memory_id   TEXT NOT NULL,
                tag         TEXT NOT NULL,
                PRIMARY KEY (memory_id, tag),
                FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_memory_tags_tag ON memory_tags(tag);

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                id,
                content,
                content='memories',
                content_rowid='rowid'
            );

            -- Triggers to keep FTS5 index in sync
            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, id, content)
                VALUES (new.rowid, new.id, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, content)
                VALUES ('delete', old.rowid, old.id, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, id, content)
                VALUES ('delete', old.rowid, old.id, old.content);
                INSERT INTO memories_fts(rowid, id, content)
                VALUES (new.rowid, new.id, new.content);
            END;

            -- History table for conversation history
            CREATE TABLE IF NOT EXISTS history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                content     TEXT NOT NULL,
                created_at  TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_history_created_at ON history(created_at);

            -- Key-value store for long-term memory and other raw data
            CREATE TABLE IF NOT EXISTS kv_store (
                key         TEXT PRIMARY KEY,
                value       TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            );

            -- Sessions table (metadata only)
            CREATE TABLE IF NOT EXISTS sessions (
                key         TEXT PRIMARY KEY,
                last_consolidated INTEGER NOT NULL DEFAULT 0,
                updated_at  TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at);

            -- Session messages table (one row per message)
            CREATE TABLE IF NOT EXISTS session_messages (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key TEXT NOT NULL,
                role        TEXT NOT NULL,
                content     TEXT NOT NULL,
                timestamp   TEXT NOT NULL,
                tools_used  TEXT,
                FOREIGN KEY (session_key) REFERENCES sessions(key) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_session_messages_session_key ON session_messages(session_key);
            CREATE INDEX IF NOT EXISTS idx_session_messages_timestamp ON session_messages(timestamp);

            PRAGMA foreign_keys = ON;
            ",
        )?;
        Ok(())
    }

    // ── History API ──

    /// Read all history entries, ordered by creation time (oldest first).
    pub async fn read_history(&self) -> anyhow::Result<String> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare("SELECT content FROM history ORDER BY id ASC")?;

        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        let mut result = String::new();
        for row in rows {
            result.push_str(&row?);
        }

        Ok(result)
    }

    /// Append a new history entry.
    pub async fn append_history(&self, content: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let created_at = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO history (content, created_at) VALUES (?1, ?2)",
            rusqlite::params![content, created_at],
        )?;

        debug!("Appended history entry");
        Ok(())
    }

    /// Write (replace) the entire history with new content.
    pub async fn write_history(&self, content: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;

        // Clear existing history
        conn.execute("DELETE FROM history", [])?;

        // Insert new content as a single entry
        let created_at = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO history (content, created_at) VALUES (?1, ?2)",
            rusqlite::params![content, created_at],
        )?;

        debug!("Wrote history");
        Ok(())
    }

    /// Clear all history entries.
    pub async fn clear_history(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM history", [])?;
        debug!("Cleared history");
        Ok(())
    }

    // ── Key-value store API (replaces file-based MEMORY.md etc.) ──

    /// Read a raw value by key.
    pub async fn read_raw(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare("SELECT value FROM kv_store WHERE key = ?1")?;
        let mut rows = stmt.query(rusqlite::params![key])?;

        if let Some(row) = rows.next()? {
            let value: String = row.get(0)?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    /// Write a raw value by key (upsert).
    pub async fn write_raw(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let updated_at = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO kv_store (key, value, updated_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, value, updated_at],
        )?;
        debug!("Wrote kv_store key: {}", key);
        Ok(())
    }

    /// Delete a raw key. Returns `true` if the key existed.
    pub async fn delete_raw(&self, key: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().await;
        let changed = conn.execute(
            "DELETE FROM kv_store WHERE key = ?1",
            rusqlite::params![key],
        )?;
        Ok(changed > 0)
    }

    // ── Session API (Legacy Blob - for migration only) ──

    /// Load a session by key (legacy JSON blob format).
    /// Used for backward compatibility during migration.
    #[deprecated(note = "Use load_session_messages instead for per-message storage")]
    pub async fn load_session(&self, key: &str) -> anyhow::Result<Option<String>> {
        let conn = self.conn.lock().await;
        // Check if this is legacy format (has 'data' column) or new format
        let has_data_column: bool = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='data'",
            [],
            |row| row.get::<_, i32>(0),
        )? > 0;

        if has_data_column {
            let mut stmt = conn.prepare("SELECT data FROM sessions WHERE key = ?1")?;
            let mut rows = stmt.query(rusqlite::params![key])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                return Ok(Some(data));
            }
        }
        Ok(None)
    }

    /// Save a session (legacy JSON blob format).
    #[deprecated(note = "Use append_session_message instead for per-message storage")]
    pub async fn save_session(&self, key: &str, data: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let updated_at = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO sessions (key, data, updated_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, data, updated_at],
        )?;
        debug!("Saved session (legacy): {}", key);
        Ok(())
    }

    /// Delete a session by key.
    pub async fn delete_session(&self, key: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().await;
        // CASCADE will delete messages automatically
        let changed = conn.execute(
            "DELETE FROM sessions WHERE key = ?1",
            rusqlite::params![key],
        )?;
        Ok(changed > 0)
    }

    // ── Session API (New Per-Message Storage) ──

    /// Create or update session metadata.
    pub async fn save_session_meta(
        &self,
        key: &str,
        last_consolidated: usize,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let updated_at = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR REPLACE INTO sessions (key, last_consolidated, updated_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, last_consolidated as i64, updated_at],
        )?;
        debug!("Saved session meta: {}", key);
        Ok(())
    }

    /// Load session metadata.
    pub async fn load_session_meta(&self, key: &str) -> anyhow::Result<Option<SessionMeta>> {
        let conn = self.conn.lock().await;
        let mut stmt =
            conn.prepare("SELECT key, last_consolidated FROM sessions WHERE key = ?1")?;
        let mut rows = stmt.query(rusqlite::params![key])?;

        if let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            let last_consolidated: i64 = row.get(1)?;
            Ok(Some(SessionMeta {
                key,
                last_consolidated: last_consolidated as usize,
            }))
        } else {
            Ok(None)
        }
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
        let conn = self.conn.lock().await;
        let timestamp_str = timestamp.to_rfc3339();
        let tools_json =
            tools_used.map(|t| serde_json::to_string(t).unwrap_or_else(|_| "[]".to_string()));

        // Ensure session exists
        let updated_at = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO sessions (key, last_consolidated, updated_at) VALUES (?1, 0, ?2)",
            rusqlite::params![session_key, updated_at],
        )?;

        // Insert message
        conn.execute(
            "INSERT INTO session_messages (session_key, role, content, timestamp, tools_used) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_key, role, content, timestamp_str, tools_json],
        )?;

        // Update session updated_at
        conn.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE key = ?2",
            rusqlite::params![updated_at, session_key],
        )?;

        debug!("Appended message to session: {}", session_key);
        Ok(())
    }

    /// Load all messages for a session.
    pub async fn load_session_messages(
        &self,
        session_key: &str,
    ) -> anyhow::Result<Vec<MessageRow>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT role, content, timestamp, tools_used FROM session_messages WHERE session_key = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_key], |row| {
            let role: String = row.get(0)?;
            let content: String = row.get(1)?;
            let timestamp_str: String = row.get(2)?;
            let tools_json: Option<String> = row.get(3)?;

            let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            Ok(MessageRow {
                role,
                content,
                timestamp,
                tools_used: tools_json,
            })
        })?;

        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    }

    /// Clear all messages for a session (keep metadata).
    pub async fn clear_session_messages(&self, session_key: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM session_messages WHERE session_key = ?1",
            rusqlite::params![session_key],
        )?;
        conn.execute(
            "UPDATE sessions SET last_consolidated = 0, updated_at = ?1 WHERE key = ?2",
            rusqlite::params![Utc::now().to_rfc3339(), session_key],
        )?;
        debug!("Cleared session messages: {}", session_key);
        Ok(())
    }

    /// Update last_consolidated for a session.
    pub async fn update_session_consolidated(
        &self,
        session_key: &str,
        last_consolidated: usize,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE sessions SET last_consolidated = ?1, updated_at = ?2 WHERE key = ?3",
            rusqlite::params![
                last_consolidated as i64,
                Utc::now().to_rfc3339(),
                session_key
            ],
        )?;
        Ok(())
    }
}

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn save(&self, entry: &MemoryEntry) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let metadata_json = serde_json::to_string(&entry.metadata)?;
        let created = entry.created_at.to_rfc3339();
        let updated = entry.updated_at.to_rfc3339();

        conn.execute(
            "INSERT OR REPLACE INTO memories (id, content, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![entry.id, entry.content, metadata_json, created, updated],
        )?;

        // Sync tags: delete old, insert new
        conn.execute(
            "DELETE FROM memory_tags WHERE memory_id = ?1",
            rusqlite::params![entry.id],
        )?;
        for tag in &entry.metadata.tags {
            conn.execute(
                "INSERT INTO memory_tags (memory_id, tag) VALUES (?1, ?2)",
                rusqlite::params![entry.id, tag],
            )?;
        }

        debug!("Saved memory entry: {}", entry.id);
        Ok(())
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<MemoryEntry>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, content, metadata, created_at, updated_at FROM memories WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;

        if let Some(row) = rows.next()? {
            let entry = row_to_entry(row)?;
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().await;
        let changed = conn.execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])?;
        Ok(changed > 0)
    }

    async fn search(&self, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().await;
        search_impl(&conn, query)
    }
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> anyhow::Result<MemoryEntry> {
    let id: String = row.get(0)?;
    let content: String = row.get(1)?;
    let metadata_json: String = row.get(2)?;
    let created_str: String = row.get(3)?;
    let updated_str: String = row.get(4)?;

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
fn search_impl(conn: &Connection, query: &MemoryQuery) -> anyhow::Result<Vec<MemoryEntry>> {
    let mut sql = String::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1u32;

    if query.text.is_some() {
        sql.push_str(
            "SELECT m.id, m.content, m.metadata, m.created_at, m.updated_at \
             FROM memories m \
             JOIN memories_fts f ON m.id = f.id \
             WHERE f.content MATCH ?",
        );
        sql.push_str(&param_idx.to_string());
        params.push(Box::new(query.text.clone().unwrap()));
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
            " AND json_extract(m.metadata, '$.source') = ?{}",
            param_idx
        ));
        params.push(Box::new(source.clone()));
        param_idx += 1;
    }

    // Filter by tags (AND semantics: entry must have ALL tags)
    for tag in &query.tags {
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM memory_tags t WHERE t.memory_id = m.id AND t.tag = ?{})",
            param_idx
        ));
        params.push(Box::new(tag.clone()));
        param_idx += 1;
    }

    // Order by updated_at descending for deterministic results
    sql.push_str(" ORDER BY m.updated_at DESC");

    // Limit / offset
    if let Some(limit) = query.limit {
        sql.push_str(&format!(" LIMIT ?{}", param_idx));
        params.push(Box::new(limit as i64));
        param_idx += 1;
    }
    if let Some(offset) = query.offset {
        if query.limit.is_none() {
            sql.push_str(&format!(" LIMIT -1 OFFSET ?{}", param_idx));
        } else {
            sql.push_str(&format!(" OFFSET ?{}", param_idx));
        }
        params.push(Box::new(offset as i64));
    }

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;

    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        entries.push(row_to_entry(row)?);
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> SqliteStore {
        let path =
            std::env::temp_dir().join(format!("nanobot_sqlite_test_{}.db", uuid::Uuid::new_v4()));
        SqliteStore::with_path(path).unwrap()
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
        let store = temp_store();
        let entry = make_entry("e1", "hello world");
        store.save(&entry).await.unwrap();

        let got = store.get("e1").await.unwrap().unwrap();
        assert_eq!(got.id, "e1");
        assert_eq!(got.content, "hello world");
    }

    #[tokio::test]
    async fn test_sqlite_save_overwrites() {
        let store = temp_store();
        store.save(&make_entry("e1", "v1")).await.unwrap();
        store.save(&make_entry("e1", "v2")).await.unwrap();

        let got = store.get("e1").await.unwrap().unwrap();
        assert_eq!(got.content, "v2");
    }

    #[tokio::test]
    async fn test_sqlite_get_nonexistent() {
        let store = temp_store();
        assert!(store.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sqlite_delete() {
        let store = temp_store();
        store.save(&make_entry("e1", "data")).await.unwrap();
        assert!(store.delete("e1").await.unwrap());
        assert!(!store.delete("e1").await.unwrap());
        assert!(store.get("e1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sqlite_fts5_search() {
        let store = temp_store();
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
        let store = temp_store();
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
        let store = temp_store();
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
        let store = temp_store();
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
        let store = temp_store();
        store.save(&make_entry("e1", "a")).await.unwrap();
        store.save(&make_entry("e2", "b")).await.unwrap();

        let results = store.search(&MemoryQuery::default()).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_sqlite_metadata_extra_preserved() {
        let store = temp_store();
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
            let store = SqliteStore::with_path(path.clone()).unwrap();
            store.save(&make_entry("e1", "persisted")).await.unwrap();
        }

        // Read with second store instance
        {
            let store = SqliteStore::with_path(path.clone()).unwrap();
            let got = store.get("e1").await.unwrap().unwrap();
            assert_eq!(got.content, "persisted");
        }

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn test_sqlite_concurrent_access() {
        let store = Arc::new(temp_store());

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
        let store = temp_store();

        store.append_history("First entry\n").await.unwrap();
        store.append_history("Second entry\n").await.unwrap();

        let history = store.read_history().await.unwrap();
        assert!(history.contains("First entry"));
        assert!(history.contains("Second entry"));
    }

    #[tokio::test]
    async fn test_sqlite_history_read_empty() {
        let store = temp_store();
        let history = store.read_history().await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_history_write() {
        let store = temp_store();

        store.append_history("Old entry\n").await.unwrap();
        store.write_history("New content\n").await.unwrap();

        let history = store.read_history().await.unwrap();
        assert!(!history.contains("Old entry"));
        assert!(history.contains("New content"));
    }

    #[tokio::test]
    async fn test_sqlite_history_clear() {
        let store = temp_store();

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
            let store = SqliteStore::with_path(path.clone()).unwrap();
            store.append_history("Persisted history\n").await.unwrap();
        }

        // Read with second store instance
        {
            let store = SqliteStore::with_path(path.clone()).unwrap();
            let history = store.read_history().await.unwrap();
            assert!(history.contains("Persisted history"));
        }

        let _ = std::fs::remove_file(path);
    }

    // ── Key-value store tests ──

    #[tokio::test]
    async fn test_sqlite_kv_read_write() {
        let store = temp_store();

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
        let store = temp_store();

        store.write_raw("key1", "v1").await.unwrap();
        store.write_raw("key1", "v2").await.unwrap();

        assert_eq!(
            store.read_raw("key1").await.unwrap(),
            Some("v2".to_string())
        );
    }

    #[tokio::test]
    async fn test_sqlite_kv_nonexistent() {
        let store = temp_store();
        assert_eq!(store.read_raw("nope").await.unwrap(), None);
    }

    // ── Session tests ──

    #[tokio::test]
    async fn test_sqlite_session_save_and_load() {
        let store = temp_store();

        let data = r#"{"key":"test:123","messages":[],"last_consolidated":0}"#;
        store.save_session("test:123", data).await.unwrap();

        let loaded = store.load_session("test:123").await.unwrap();
        assert_eq!(loaded, Some(data.to_string()));
    }

    #[tokio::test]
    async fn test_sqlite_session_upsert() {
        let store = temp_store();

        store.save_session("key1", "v1").await.unwrap();
        store.save_session("key1", "v2").await.unwrap();

        assert_eq!(
            store.load_session("key1").await.unwrap(),
            Some("v2".to_string())
        );
    }

    #[tokio::test]
    async fn test_sqlite_session_delete() {
        let store = temp_store();

        store.save_session("key1", "data").await.unwrap();
        assert!(store.delete_session("key1").await.unwrap());
        assert!(!store.delete_session("key1").await.unwrap());
        assert!(store.load_session("key1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sqlite_session_nonexistent() {
        let store = temp_store();
        assert!(store.load_session("nope").await.unwrap().is_none());
    }
}
