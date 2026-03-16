//! Session metadata, messages, and summaries API for SqliteStore.

use chrono::{DateTime, Utc};
use sqlx::Row;
use tracing::debug;

use super::SqliteStore;

/// Session metadata for per-message storage.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub key: String,
    pub last_consolidated: usize,
}

/// Message row for session messages.
#[derive(Debug, Clone)]
pub struct MessageRow {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub tools_used: Option<String>,
}

impl SqliteStore {
    // ── Session API (Legacy Blob - for migration only) ──

    /// Load a session by key (legacy JSON blob format).
    /// Used for backward compatibility during migration.
    #[deprecated(note = "Use load_session_messages instead for per-message storage")]
    pub async fn load_session(&self, key: &str) -> anyhow::Result<Option<String>> {
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
        let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
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
}
